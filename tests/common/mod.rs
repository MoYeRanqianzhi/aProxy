//! 集成测试的公共设施。每个 `tests/*.rs` 用 `mod common;` 引入（每个测试程序各编译一份）。
//!
//! 核心是 [`TestHome`]：测试拉起的每一个 aproxy 进程都经它的 [`TestHome::cmd`] 或
//! [`TestHome::cmd_with`] 拉起——隔离的 home、固定的语言、清掉的代理变量都只在这一处注入；
//! 测试结束时，它按端口停掉登记过的实例，再结束这个 home 的台账里仍在运行的进程。
//! `tests/test_home_check.rs` 扫描测试源码，拦下绕过它直接拉起 aproxy 的写法。
//!
//! 安全：开发机上可能跑着用户真正在用的 aproxy 实例，跑测试的会话本身可能就经它连 API。
//! 测试只在自己新建的临时 home 里运行 aproxy；停实例只按端口执行 `stop <端口>`，绝不
//! `stop all`；兜底结束进程只按这个临时 home 的台账（pid 加启动标记核验），绝不按进程名。

// 每个测试程序都编译一份本模块，各自只用到其中一部分；没用到的项在那个测试程序里是死代码，
// 编译器会逐项告警。整个模块放行这一类告警，而不是在每个测试文件里挑着引入。
#![allow(dead_code)]

use std::ffi::OsStr;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError};

/// 保留子进程的原始结果，同时给测试的捕获日志留一份状态；子进程崩溃时两个流可能都为空。
pub trait ReportExit {
    fn report_exit(self) -> Self;
}

/// Windows 的异常退出保留状态字的全部位，不把负的十进制数误认为 CLI 退出码。
pub fn status_text(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) if cfg!(windows) && code < 0 => {
            format!("abnormal termination: 0x{:08X}", code as u32)
        }
        Some(code) => format!("exit status: 0x{:08X}", code as u32),
        None => format!("abnormal termination: {status}"),
    }
}

impl ReportExit for std::io::Result<std::process::Output> {
    fn report_exit(self) -> Self {
        self.inspect(|out| eprintln!("{}", status_text(out.status)))
    }
}

impl ReportExit for std::io::Result<std::process::ExitStatus> {
    fn report_exit(self) -> Self {
        self.inspect(|status| eprintln!("{}", status_text(*status)))
    }
}

impl ReportExit for std::io::Result<Option<std::process::ExitStatus>> {
    fn report_exit(self) -> Self {
        self.inspect(|status| {
            if let Some(status) = status {
                eprintln!("{}", status_text(*status));
            }
        })
    }
}

/// 结构化命令的 stdout 只能有一个完整文档，不能混入给终端的说明。
pub fn output_document(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("invalid command document: {e}; {out:?}"))
}

/// 另一个线程 fork 的子进程在 exec 之前可能短暂持有刚复制的镜像的写句柄。
/// 只重试内核报的执行冲突；路径、权限和格式错误不等待，不重跑已执行的命令。
pub fn retry_exec<T>(mut exec: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut waits = [25, 50, 100, 200, 400, 800].into_iter();
    loop {
        match exec() {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && let Some(ms) = waits.next() =>
            {
                std::thread::sleep(std::time::Duration::from_millis(ms))
            }
            result => return result,
        }
    }
}

/// 执行安装、恢复测试写入临时目录的镜像，并保留原有的标准流设置。
pub trait CopiedExecutable {
    fn spawn_ready(&mut self) -> std::io::Result<Child>;
    fn output_ready(&mut self) -> std::io::Result<std::process::Output>;
}
impl CopiedExecutable for Command {
    fn spawn_ready(&mut self) -> std::io::Result<Child> {
        retry_exec(|| self.spawn())
    }
    fn output_ready(&mut self) -> std::io::Result<std::process::Output> {
        retry_exec(|| self.output().report_exit())
    }
}

/// 当前构建出的 aproxy（cargo 为集成测试提供的绝对路径）
pub const APROXY: &str = env!("CARGO_BIN_EXE_aproxy");

/// 从子进程环境里删掉的代理变量（大小写两种写法都有程序读）。开发机的环境里常带着指向
/// 外部代理的这几个变量，aproxy 子进程继承之后，发往 127.0.0.1 上 mock 的请求也会被送去
/// 代理，造成随机的假失败。
const PROXY_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// 子进程的 NO_PROXY / no_proxy：环回地址一律直连。这里设值而不是删掉：除了环境变量，
/// 代理客户端在一些平台上还会读系统的代理设置（Windows 上是注册表里的那一份），显式的
/// NO_PROXY 让环回上的 mock 无论如何都不经过代理。
const NO_PROXY_LOOPBACK: &str = "127.0.0.1,localhost";

/// 一个隔离的 aproxy home（自己新建的临时目录），也是测试拉起 aproxy 的唯一入口。
///
/// - 新建时核对目录确实在系统临时目录里、不是用户的 `~/.aproxy`；会结束进程的命令
///   （`stop`、`restart`、`install --force`）只能在这样的 home 里运行。
/// - [`new`](Self::new) 写入 `settings.json` = `{"watchdog": {"enabled": false}}`：`start` 会顺带拉起全局
///   看护者，它带着 aproxy 二进制的镜像一直活到闲置自灭（默认 300 秒）——测试早已结束，它还
///   锁着构建产物，让随后的 cargo 重新链接失败，还会把测试里模拟崩溃的实例重新拉起来。要测
///   看护者的测试显式开启（[`settings`](Self::settings)），或者自己拉起
///   看护者（先 [`enable_watchdog`](Self::enable_watchdog)，再 [`watchdog_cmd`](Self::watchdog_cmd)）。
/// - [`cmd`](Self::cmd) / [`cmd_with`](Self::cmd_with) 返回已注入环境的 `Command`，见
///   [`cmd_with`](Self::cmd_with)。
/// - 析构时先对 [`stop_on_drop`](Self::stop_on_drop) 登记过的每个端口执行一次
///   `aproxy stop <端口>`（优雅停止），再兜底结束这个 home 的台账（永久兼容契约 C3）里仍在
///   运行的进程——没登记端口的实例、看护者、被 restore 或看护者拉回来的实例，之后才删除临时
///   目录。守护以分离进程运行，不随测试进程退出；析构在断言失败的 unwind 路径上同样执行，
///   进程不会泄漏到测试之外。只按端口停，绝不 `stop all`。
pub struct TestHome {
    /// 临时目录；在析构函数停完实例之后才随字段一起删除
    dir: tempfile::TempDir,
    /// 注入给 aproxy 的 APROXY_HOME：一般就是临时目录本身，[`unborn`](Self::unborn) 时是它
    /// 下面一个还不存在的子目录
    home: PathBuf,
    /// 析构时要停的实例：(执行 `stop` 的 aproxy, 端口)。用 Mutex 而不是 RefCell：
    /// `&TestHome` 要能交给别的线程（例如并发执行 CLI 的测试）
    stops: Mutex<Vec<(PathBuf, u16)>>,
}

impl TestHome {
    /// 新建一个隔离的 home，并关掉其中的看护者（理由见类型的文档）。
    pub fn new() -> Self {
        let home = Self::in_temp_dir(Path::to_path_buf);
        home.settings(r#"{"watchdog": {"enabled": false}}"#);
        home
    }

    /// 一个还不存在的 home：APROXY_HOME 指向临时目录下尚未创建的子目录，用来验证某个命令
    /// 不创建、不写 home。它没有 settings.json，也不能调用 [`settings`](Self::settings)。
    pub fn unborn() -> Self {
        Self::in_temp_dir(|dir| dir.join("home"))
    }

    /// 非 Unicode 的绝对路径仍位于受控临时目录内，用于核对 JSON 显示与实际读写的区别。
    #[cfg(unix)]
    pub fn non_unicode() -> Self {
        use std::os::unix::ffi::OsStrExt;
        Self::in_temp_dir(|dir| {
            let home = dir.join(OsStr::from_bytes(b"home-\xff"));
            std::fs::create_dir(&home).unwrap();
            home
        })
    }

    /// 新建临时目录并核对它的位置；`home_of` 由临时目录给出注入的 APROXY_HOME。
    fn in_temp_dir(home_of: impl FnOnce(&Path) -> PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // 比较前都规范化：Windows 的临时目录可能经 8.3 短名给出，macOS 的 /var 是 /private/var
        // 的符号链接，不规范化就会把同一个目录判成两个
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let canonical = real(dir.path());
        assert!(
            canonical.starts_with(real(&std::env::temp_dir())),
            "测试 home 必须在临时目录里: {}",
            canonical.display()
        );
        if let Some(user_dir) = aproxy_ipc::home::user_dir() {
            assert!(
                !canonical.starts_with(real(&user_dir.join(".aproxy"))),
                "测试 home 绝不能是用户的 ~/.aproxy: {}",
                canonical.display()
            );
        }
        let home = home_of(dir.path());
        // APROXY_HOME 必须是绝对路径（永久兼容契约 C1），相对路径会被 aproxy 拒绝
        assert!(
            home.is_absolute(),
            "测试 home 不是绝对路径: {}",
            home.display()
        );
        // 已经存在的 home 起步就只让属主访问，与 aproxy 自己建出来的一样：临时目录在 unix 上
        // 按 umask 建（多半是 0755），在有的 Windows 机器上还给了别的账户权限，那样的 home
        // 会让 doctor 多出一条告警
        if home.exists() {
            aproxy::fsx::secure_home_in(&home).expect("收紧测试 home 的权限失败");
        }
        Self {
            dir,
            home,
            stops: Mutex::new(Vec::new()),
        }
    }

    /// home 的路径（注入给 aproxy 的 APROXY_HOME）
    pub fn path(&self) -> &Path {
        &self.home
    }

    /// 用 `json` 整个改写 `settings.json`（例如 `{"watchdog": {"enabled": true}}` 开启看护者）
    pub fn settings(&self, json: &str) {
        std::fs::write(self.home.join("settings.json"), json).unwrap();
    }

    /// 在这个 home 里运行当前构建的 aproxy，环境见 [`cmd_with`](Self::cmd_with)。
    pub fn cmd(&self) -> Command {
        self.cmd_with(APROXY)
    }

    /// 故意给出相对 home，让预检失败。即使被测程序误把它当有效路径，也只能写进临时目录。
    pub fn cmd_with_relative_home(&self) -> Command {
        let mut cmd = self.cmd();
        cmd.current_dir(self.path())
            .env(aproxy_ipc::home::HOME_ENV, "relative");
        cmd
    }

    /// 在这个 home 里运行 `exe`：aproxy 的副本（放进 `<home>/bin/` 的已安装形态、改成发布
    /// 资产名的副本、别的版本），或者会转而运行 aproxy 的程序（例如经 `cmd /c` 运行 bin 里的
    /// `aproxy.bat`）。返回的 `Command` 已经：
    /// - 注入 APROXY_HOME（绝对路径）：aproxy 与它分离拉起的守护、看护者都继承它，注册表、
    ///   日志、控制端点全落在这个 home 里，`stop` 也只找得到这个 home 的实例；
    /// - 注入 `APROXY_LANG=en`：让输出的语言不随开发机的语言环境变化；
    /// - 删掉 [`PROXY_VARS`]，并把 NO_PROXY 设为环回地址（理由见这两个常量）。
    ///
    /// 调用方可以再追加别的环境变量与参数，但不要改写或删除 APROXY_HOME：删掉它，aproxy
    /// 就回到用户真实的 home（`tests/test_home_check.rs` 会拦下这种写法）。
    pub fn cmd_with(&self, exe: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(exe);
        cmd.env(aproxy_ipc::home::HOME_ENV, &self.home)
            .env("APROXY_LANG", "en");
        for var in PROXY_VARS {
            cmd.env_remove(var);
        }
        cmd.env("NO_PROXY", NO_PROXY_LOOPBACK)
            .env("no_proxy", NO_PROXY_LOOPBACK);
        cmd
    }

    /// 直接以后台实例的入口（`aproxy __serve`）在这个 home 里运行一个实例的命令，不经
    /// `aproxy start`：测试因此握着实例进程的句柄，能等它退出、读它的退出码、直接结束它
    /// 来模拟崩溃。`cfg` 是它的配置文件。
    pub fn serve_cmd(&self, cfg: impl AsRef<Path>) -> Command {
        self.serve_cmd_with(APROXY, cfg)
    }

    /// 同 [`serve_cmd`](Self::serve_cmd)，但运行 `exe`。
    ///
    /// 拉起规格写成 `<home>/run/` 下的交接文件（真实的拉起方在实例就绪后删掉它，这里留给
    /// 临时目录一起删）。规格里记的环境与 [`cmd_with`](Self::cmd_with) 注入的一致：代理
    /// 变量记为没设、NO_PROXY 记为环回——实例把规格存进期望状态记录，之后 restart、看护者
    /// 重拉会照它设置子进程的环境，不这样记，开发机环境里的代理变量就会在那时漏进去。
    pub fn serve_cmd_with(&self, exe: impl AsRef<OsStr>, cfg: impl AsRef<Path>) -> Command {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let log_path = self
            .home
            .join("logs")
            .join(format!("test-{}-{seq}.log", std::process::id()));
        let mut spec = aproxy::spec::capture(
            Some(cfg.as_ref()),
            aproxy::config::Overrides::default(),
            &log_path,
        );
        for (name, value) in spec.env.iter_mut() {
            if PROXY_VARS.iter().any(|var| var.eq_ignore_ascii_case(name)) {
                *value = None;
            } else if name.eq_ignore_ascii_case("NO_PROXY") {
                *value = Some(NO_PROXY_LOOPBACK.to_string());
            }
        }
        let spec_path = aproxy::spec::write_spec_file(&self.home.join("run"), &spec).unwrap();
        let mut cmd = self.cmd_with(exe);
        cmd.arg("__serve")
            .arg("--home")
            .arg(&self.home)
            .arg("--spec")
            .arg(spec_path);
        cmd
    }

    /// 打开这个 home 的看护者开关（整个改写 settings.json）。要自己拉起看护者的测试先调用
    /// 它：看护者每个周期都读这个开关，关着就退出。
    pub fn enable_watchdog(&self) {
        self.settings(r#"{"watchdog": {"enabled": true}}"#);
    }

    /// 直接以看护者的入口（`aproxy __watchdog`）在这个 home 里运行看护者的命令。
    pub fn watchdog_cmd(&self) -> Command {
        self.watchdog_cmd_with(APROXY)
    }

    /// 同 [`watchdog_cmd`](Self::watchdog_cmd)，但运行 `exe`。
    pub fn watchdog_cmd_with(&self, exe: impl AsRef<OsStr>) -> Command {
        let mut cmd = self.cmd_with(exe);
        cmd.arg("__watchdog").arg("--home").arg(&self.home);
        cmd
    }

    /// 登记一个端口：析构时用当前构建执行 `aproxy stop <端口>`。在拉起实例的地方紧接着
    /// 登记；那个端口上到时没有实例也无妨（stop 报「没有运行中的实例」，忽略）。
    pub fn stop_on_drop(&self, port: u16) {
        self.stop_on_drop_with(APROXY, port);
    }

    /// 同 [`stop_on_drop`](Self::stop_on_drop)，但用 `exe` 执行 stop：当前构建联系不上的实例
    /// （例如别的大版本启动的）只能用启动它的那个版本来停。
    pub fn stop_on_drop_with(&self, exe: impl AsRef<Path>, port: u16) {
        self.stops
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((exe.as_ref().to_path_buf(), port));
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        // 析构可能正处在断言失败的 unwind 里，这里再 panic 会直接中止整个测试程序：
        // 锁中毒照样取出数据，stop 与结束进程的失败一律忽略
        let stops = std::mem::take(&mut *self.stops.lock().unwrap_or_else(PoisonError::into_inner));
        for (exe, port) in stops {
            // 标准流全部接到空设备、只取退出状态：不留任何管道，stop 期间被拉起的进程
            // （例如残留安装状态触发的续作）也就不会让这里等它的输出结束
            let _ = self
                .cmd_with(&exe)
                .args(["stop", &port.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn_ready()
                .and_then(|mut child| child.wait().report_exit());
        }
        // 兜底：台账里仍在运行的进程。条目只可能由带着这个 APROXY_HOME 运行的进程（或测试
        // 自己）写进来；ledger::terminate 只认 pid 加启动标记，并核对属主是当前用户，核验
        // 不过就什么都不做，pid 被复用也误伤不了别人。跳过测试进程自己：库层测试可能在进程
        // 内登记过它（例如替测试进程写的条目），结束它就等于中止整个测试程序
        let me = std::process::id();
        for found in aproxy_ipc::ledger::read_all_in(&self.home) {
            if found.entry.pid != me {
                let _ =
                    aproxy_ipc::ledger::terminate(&found.entry, std::time::Duration::from_secs(5));
            }
        }
    }
}

/// 测试自己拉起、持有句柄的子进程（哑进程、直接拉起的守护与看护者）：析构时终止并回收，
/// 断言失败的 unwind 路径上同样执行。句柄就是身份，结束的一定是这个子进程本身，不会误伤
/// 别的进程；已经退出的子进程 kill 失败无害，wait 负责回收 unix 上的僵尸。
pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait().report_exit();
    }
}

/// 向系统要一个当前空闲的环回端口（绑定 127.0.0.1:0、读出端口号、随即放开），给要写进
/// 配置的固定端口用。
///
/// 不再从测试进程的 pid 派生端口：Windows 上 Hyper-V/WinNAT 会保留成片的排除区间
/// （`netsh interface ipv4 show excludedportrange protocol=tcp`），派生出的端口落进去时守护
/// 以「无法绑定」启动失败，只能逐个试探着跳过。系统分配的端口在分配的那一刻必定可绑定，
/// 也不会是用户实例占着的端口或默认端口 12345。
///
/// 同一个测试程序里发出去的端口互不相同（记在进程内的表里）：并行的测试各用各的端口，
/// 同一个测试连续要几个也不会拿到重复的。剩下的窗口是放开之后、守护绑定之前，别的进程
/// 可能恰好拿走这个端口（概率很低）；要做到零竞争，就让守护监听端口 0，再从注册表读回
/// 实际端口（见 proxy_integration 的 port_zero_restore_record_uses_actual_port）。
pub fn free_port() -> u16 {
    static HANDED_OUT: Mutex<Vec<u16>> = Mutex::new(Vec::new());
    let mut handed_out = HANDED_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if !handed_out.contains(&port) {
            handed_out.push(port);
            return port;
        }
    }
}

/// examples/format-echo(.exe) 的路径：转换器测试用它当跨平台的假 format 程序（不依赖系统
/// 里的 python）。example 与 aproxy 二进制在同一个构建目录里（`target/<profile>/` 下的
/// `examples/`），所以从 cargo 给出的 aproxy 路径直接推出来，不靠猜。
///
/// 它是 example，不是这些测试的依赖：不带过滤的 `cargo test` 会顺带编译全部 example，只跑
/// 某个测试文件（`cargo test --test transform_integration`）时不会——干净的构建目录下要先
/// `cargo build --example format-echo`，否则这里 panic 并给出这条命令。
pub fn format_echo_path() -> PathBuf {
    let path = Path::new(APROXY)
        .with_file_name("examples")
        .join(format!("format-echo{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "找不到 {}：只跑部分测试时 cargo 不编译 example，先执行 `cargo build --example format-echo`",
        path.display()
    );
    path
}

/// 实际启动路径的可控时钟副本，只替换服务子进程的运行时，不修改产品的 hello 时限。
pub fn start_clock_path() -> PathBuf {
    Path::new(APROXY).with_file_name("examples").join(format!(
        "format-start-clock{}",
        std::env::consts::EXE_SUFFIX
    ))
}
