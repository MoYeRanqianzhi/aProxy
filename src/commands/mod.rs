//! 子命令模块：每个 `aproxy <cmd>` 一个文件，与 CLI 定义（crate::cli）分离。
//!
//! 这里的每个文件只做两件事：调用操作层（`aproxy::ops`）拿到结果，再把结果渲染成给人看的
//! 文字、决定退出码。target 解析、停止、拉起这些动作本身在操作层，界面与命令行共用。

pub(crate) mod alias;
pub(crate) mod config;
#[path = "../i18n/config.rs"]
mod config_text;
pub(crate) mod desktop;
pub(crate) mod doctor;
pub(crate) mod find;
pub(crate) mod install;
pub(crate) mod logs;
pub(crate) mod migrate;
pub(crate) mod restart;
pub(crate) mod restore;
pub(crate) mod settings;
pub(crate) mod skill;
pub(crate) mod start;
pub(crate) mod status;
pub(crate) mod stop;

use std::path::PathBuf;

use aproxy::settings as app_settings;
use aproxy_ipc::i18n::{Lang, current};
use serde_json::{Value, json};

/// 由 clap 已解析的命令选择输出协议，不从原始参数里猜测（值本身也可能是 `--json`）。
#[derive(Clone, Copy)]
pub(crate) enum JsonOutput {
    /// 普通命令和只读安装检查输出一个文档。
    Document,
    /// 普通安装的进度按行追加，最后一行才表示完成。
    InstallEvents,
}

/// 一个 CLI 进程只执行一个面向用户的命令；异步子任务也应看到同一种输出协议。
static JSON_OUTPUT: std::sync::OnceLock<JsonOutput> = std::sync::OnceLock::new();
/// 某些有诊断或逐项失败的完整报告也用非零码退出，退出兜底不能再输出第二份文档。
static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl JsonOutput {
    /// 只在命令声明并实际解析到 JSON 旗标时启用，不改变无旗标与内部子命令的 stdout。
    pub(crate) fn for_command(command: Option<&aproxy::cli::Commands>) -> Option<Self> {
        use aproxy::cli::{AliasCmd, Commands, ConfigCmd, SettingsCmd};
        let json = match command? {
            Commands::Install(a) if a.json && !a.check => return Some(Self::InstallEvents),
            Commands::Install(a) => a.json,
            Commands::Start(a) => a.json,
            Commands::Skill {
                cmd:
                    aproxy::cli::SkillCmd::Install { json, .. } | aproxy::cli::SkillCmd::List { json },
            } => *json,
            Commands::Skill {
                cmd:
                    aproxy::cli::SkillCmd::Show { .. } | aproxy::cli::SkillCmd::VerifyPackage { .. },
            } => false,
            Commands::Stop { json, .. }
            | Commands::Restart { json, .. }
            | Commands::Restore { json, .. }
            | Commands::Migrate { json, .. }
            | Commands::Doctor { json }
            | Commands::Status { json, .. }
            | Commands::Find { json, .. }
            | Commands::Alias {
                cmd: AliasCmd::List { json },
            } => *json,
            Commands::Config(a) => match &a.cmd {
                ConfigCmd::Show { json, .. }
                | ConfigCmd::Get { json, .. }
                | ConfigCmd::Set { json, .. }
                | ConfigCmd::Unset { json, .. } => *json,
            },
            Commands::Settings { cmd } => match cmd {
                SettingsCmd::Show { json }
                | SettingsCmd::Get { json, .. }
                | SettingsCmd::Set { json, .. }
                | SettingsCmd::Unset { json, .. } => *json,
            },
            Commands::Alias { .. }
            | Commands::Logs { .. }
            | Commands::Desktop { .. }
            | Commands::RestoreAtLogin { .. }
            | Commands::Serve { .. }
            | Commands::Watchdog { .. } => false,
        };
        json.then_some(Self::Document)
    }

    /// 在读 home 和设置之前选定输出，入口拒绝也能带回与子命令相同的结果格式。
    pub(crate) fn bind(self) {
        let _ = JSON_OUTPUT.set(self);
    }

    /// 安装在尚未选定目标时失败，版本为 null；普通错误没有事件外壳。
    pub(crate) fn error_document(self, error: Value) -> Value {
        match self {
            Self::Document => json!({"error": error}),
            Self::InstallEvents => {
                json!({"event":"result","outcome":"failed","version":null,"error":error})
            }
        }
    }
}

/// 没有更具体的结构化结果时才补整条命令的错误。不要从任意 stderr 文本反推结构化字段或泄露输入。
pub(crate) fn finish_json(code: i32) {
    if code != 0
        && let Some(mode) = JSON_OUTPUT.get()
        && !REPORTED.load(std::sync::atomic::Ordering::Acquire)
    {
        let name = match mode {
            JsonOutput::InstallEvents => "install_failed",
            JsonOutput::Document => "command_failed",
        };
        let message = aproxy_ipc::code::lookup(name)
            .expect("registered error")
            .summary;
        print_doc(mode.error_document(json!({"code":name,"message":message,"details":{}})));
    }
}

/// 一条命令整体失败的原因：码表里的码、它的 `details`、给人看的一段话。
///
/// 退出码取自码表（`aproxy_ipc::code`）；诊断码（例如 `config_unknown_key`）作为整条命令的
/// 失败原因时，是「配置无效」，退出码 3。
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// 中英文同时有读者时才保存第二种语言的文字。
    terminal: Option<String>,
    pub(crate) details: Box<Value>,
    /// 同一个握手诊断的退出码取决于故障能否自行恢复。
    exit_override: Option<u8>,
}

impl Failure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Failure {
            code,
            message: message.into(),
            terminal: None,
            details: Box::new(details),
            exit_override: None,
        }
    }

    /// 一条失败可以同时写入 stderr 与 JSON；两次显示从同一组参数和同一条定义取词。
    pub(crate) fn rendered(
        code: &'static str,
        render: impl Fn(Lang) -> String,
        details: Value,
    ) -> Self {
        Self::for_readers(
            code,
            render,
            details,
            current(),
            JSON_OUTPUT.get().is_some(),
        )
    }

    fn for_readers(
        code: &'static str,
        render: impl Fn(Lang) -> String,
        details: Value,
        terminal: Lang,
        machine: bool,
    ) -> Self {
        let mut failure = Self::new(
            code,
            render(if machine { Lang::En } else { terminal }),
            details,
        );
        if machine && terminal != Lang::En {
            failure.terminal = Some(render(terminal));
        }
        failure
    }

    /// 给实际存在的输出补上同一操作阶段，纯终端命令不构造英文副本。
    fn context(mut self, render: impl Fn(Lang, &str) -> String) -> Self {
        let machine = JSON_OUTPUT.get().is_some();
        if machine && current() == Lang::Zh {
            self.terminal = Some(render(Lang::Zh, self.terminal()));
        }
        self.message = render(if machine { Lang::En } else { current() }, &self.message);
        self
    }

    /// 终端没有独立的译文时，沿用原始故障证据。
    fn terminal(&self) -> &str {
        self.terminal.as_deref().unwrap_or(&self.message)
    }

    /// 前台与后台用同一份结构化诊断；静态协议不匹配和程序暂时不可用的退出码不同。
    pub(crate) fn handshake(e: &aproxy::transform::StartupError) -> Self {
        let mut failure = Self::rendered(e.code(), |lang| e.render(lang), e.details());
        failure.exit_override = Some(e.exit_code());
        failure
    }

    /// 这个失败的退出码。
    pub(crate) fn exit_code(&self) -> u8 {
        if let Some(code) = self.exit_override {
            return code;
        }
        use aproxy_ipc::code::ExitCode;
        match aproxy_ipc::code::lookup(self.code).and_then(|code| code.exit) {
            Some(ExitCode::Fixed(code)) => code,
            Some(ExitCode::ChildOr(fallback)) => self.details["code"]
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .filter(|&n| n != 0)
                .unwrap_or(fallback),
            None => aproxy_ipc::code::EXIT_CONFIG,
        }
    }

    /// 报告并退出：原因写到 stderr；`json` 为真时 stdout 上是错误文档
    /// `{"schema":1,"error":{"code","message","details"}}`。
    pub(crate) fn exit(self, json: bool) -> ! {
        crate::util::exit(i32::from(self.report(json)))
    }

    /// 写出同一份错误文档并返回退出码，让需要收回运行时的命令走正常返回路径。
    pub(crate) fn report(self, json: bool) -> u8 {
        let exit = self.exit_code();
        eprintln!("{}", self.terminal());
        if json {
            print_doc(json!({
                "error": {"code": self.code, "message": self.message, "details": self.details},
            }));
        }
        exit
    }
}

/// 给人看的一行：平时写到 stdout；`json` 为真时 stdout 只留给那一个 JSON 文档，这行改写到
/// stderr。
pub(crate) fn say(json: bool, text: impl std::fmt::Display) {
    if json {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

/// 逐个实例处理的命令（stop、restart、restore）的结果清单：每个实例一项，最后汇总成退出码
/// 与 `--json` 的文档 `{"results":[{"target","port","outcome","error"?,"reason"?}],
/// "summary":{"ok","failed"}}`。
///
/// `target` 是命令行上写的目标（没写时为 null），`port` 是这一项说的那个实例的端口。
pub(crate) struct Results {
    target: Option<String>,
    items: Vec<Value>,
    ok: usize,
    /// 失败的各项的退出码
    failed: Vec<u8>,
    /// 有一项的结局虽然不算错误，却要让命令以「部分成功」（5）结束
    partial: bool,
}

impl Results {
    pub(crate) fn new(target: Option<&str>) -> Self {
        Results {
            target: target.map(str::to_string),
            items: Vec::new(),
            ok: 0,
            failed: Vec::new(),
            partial: false,
        }
    }

    fn item(&self, port: &str, outcome: &str) -> serde_json::Map<String, Value> {
        let mut item = serde_json::Map::new();
        item.insert("target".to_string(), json!(self.target));
        item.insert("port".to_string(), port_value(port));
        item.insert("outcome".to_string(), json!(outcome));
        item
    }

    /// 这个实例达到了命令要的状态。`reason` 说明为什么是这个结局（例如 `not_idle`）。
    pub(crate) fn ok(&mut self, port: &str, outcome: &str, reason: Option<&str>) {
        let mut item = self.item(port, outcome);
        if let Some(reason) = reason {
            item.insert("reason".to_string(), json!(reason));
        }
        self.items.push(Value::Object(item));
        self.ok += 1;
    }

    /// 这个实例没有达到命令要的状态。`outcome` 一般是 `failed`。
    pub(crate) fn failed(&mut self, port: &str, outcome: &str, failure: &Failure) {
        let mut item = self.item(port, outcome);
        item.insert(
            "error".to_string(),
            json!({"code": failure.code, "message": failure.message, "details": failure.details}),
        );
        self.items.push(Value::Object(item));
        self.failed.push(failure.exit_code());
    }

    /// 这个实例没有达到命令要的状态，却也不算出错（例如 restore 时配置文件暂时不在，记录
    /// 保留着等它回来）：不计入 `ok` 与 `failed`，命令至少以「部分成功」（5）结束。
    pub(crate) fn deferred(&mut self, port: &str, outcome: &str) {
        let item = self.item(port, outcome);
        self.items.push(Value::Object(item));
        self.partial = true;
    }

    /// 退出码：没有失败是 0；有成有败是 5；全部失败时，各项的退出码相同就用它，否则 1。
    pub(crate) fn exit_code(&self) -> u8 {
        use aproxy_ipc::code::{EXIT_FAILED, EXIT_OK, EXIT_PARTIAL};
        match self.failed.as_slice() {
            [] if self.partial => EXIT_PARTIAL,
            [] => EXIT_OK,
            _ if self.ok > 0 || self.partial => EXIT_PARTIAL,
            [first, rest @ ..] if rest.iter().all(|code| code == first) => *first,
            _ => EXIT_FAILED,
        }
    }

    /// 结束命令：`json` 为真时把文档写到 stdout，返回 [`Self::exit_code`]，由入口回收运行时后再退出。
    pub(crate) fn finish(self, json: bool) -> u8 {
        let exit = self.exit_code();
        if json {
            print_doc(json!({
                "results": self.items,
                "summary": {"ok": self.ok, "failed": self.failed.len()},
            }));
        }
        exit
    }
}

/// 端口写成 JSON：数字；空串（这一项说的不是某个端口，例如一个没在运行的别名）是 null。
pub(crate) fn port_value(port: &str) -> Value {
    port.parse::<u16>().map_or(Value::Null, |port| json!(port))
}

/// 配置、别名和启动命令用同一个原因报告找不到配置文件，JSON 也不另造一份措辞。
pub(crate) fn missing_config(lang: Lang, path: &std::path::Path) -> String {
    aproxy::spawn::startup::LaunchReason::Missing {
        path: path.display().to_string(),
    }
    .render(lang)
}

/// 实例没能拉起来：时限内没就绪是 `ready_timeout`，其余（拉不起进程、进程起来就退出）是
/// `start_failed`，`details` 里带 startup.log 这次新增的几行。`message` 是给人看的说明。
pub(crate) fn spawn_failure(
    error: &aproxy::spawn::SpawnError,
    message: impl Fn(Lang) -> String,
) -> Failure {
    use aproxy::spawn::SpawnError;
    match error {
        SpawnError::Exited {
            format: Some(e), ..
        } => Failure::handshake(e),
        SpawnError::TimedOut { .. } => Failure::rendered(
            "ready_timeout",
            message,
            json!({"timeout_ms": aproxy::spawn::READY_TIMEOUT.as_millis() as u64}),
        ),
        SpawnError::Spawn(_) | SpawnError::Exited { .. } => {
            let lines = error
                .startup_log()
                .map(|log| log.lines.clone())
                .unwrap_or_default();
            let mut details = json!({"startup_log": lines});
            if let SpawnError::Exited {
                code: Some(code), ..
            } = error
            {
                details["code"] = json!(code);
            }
            Failure::rendered("start_failed", message, details)
        }
    }
}

/// 监听地址绑定不了：被占用是 `port_in_use`；无权限、地址不可用是这个地址本来就起不来，
/// 归 `start_failed`。
pub(crate) fn bind_failure(listen_addr: &str, error: &std::io::Error) -> Failure {
    let reason = aproxy::spawn::startup::LaunchReason::from_bind(listen_addr, error);
    let message = |lang| reason.render(lang);
    if error.kind() == std::io::ErrorKind::AddrInUse {
        Failure::rendered(
            "port_in_use",
            message,
            json!({
                "port": port_value(aproxy::daemon::port_of(listen_addr)),
                "owner": "unknown",
            }),
        )
    } else {
        Failure::rendered("start_failed", message, json!({"startup_log": []}))
    }
}

/// 把 `--json` 的文档写到 stdout：一行，`schema` 在最前面，其余的键照 `body` 的顺序。
pub(crate) fn print_doc(body: Value) {
    if body.get("event").is_none() || body["event"] == "result" {
        REPORTED.store(true, std::sync::atomic::Ordering::Release);
    }
    let mut doc = serde_json::Map::new();
    doc.insert("schema".to_string(), 1.into());
    if let Value::Object(body) = body {
        doc.extend(body);
    }
    println!("{}", Value::Object(doc));
}

/// 实际生效的配置文件路径：命令行上显式给的（`explicit`） > settings.json 的 `default_config`
/// （用户可把日常主力配置换成任意文件）> 默认 `~/.aproxy/config.toml`。
///
/// default_config 失效（文件被删/移动）返回 Err（给用户的报错）：静默回退默认
/// 配置会让用户在错误的配置文件上排障。但只有真正要读配置的命令（启动、
/// `config` 查看/修改内容）才兑现这个 Err（见 `require_cfg_path`）——status、
/// stop、doctor、自动拉起的 `install --continue` 等与配置文件无关的命令不能被
/// 一个失效的 default_config 拦住，报错里推荐的 `aproxy settings unset default_config`
/// 自己更必须能执行。
///
/// 显式给的路径在此展开 `~` 并按当前工作目录绝对化：这个路径会被转发给
/// 守护子进程、写进注册表与 .restore，日后可能在别的工作目录被读取（restore
/// 会把找不到配置文件的记录当失效清理掉）。
pub(crate) fn resolve_cfg_path(explicit: Option<&std::path::Path>) -> Result<PathBuf, Failure> {
    if let Some(p) = explicit {
        return Ok(match p.to_str() {
            Some(s) => app_settings::expand_path(s),
            None => std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()),
        });
    }
    match app_settings::load().default_config {
        Some(p) => {
            let path = app_settings::expand_path(&p);
            if path.is_file() {
                Ok(path)
            } else {
                Err(Failure::rendered(
                    "target_not_found",
                    |lang| config_text::default_missing(lang, path.display()),
                    json!({"target": path.display().to_string()}),
                ))
            }
        }
        None => Ok(aproxy::config::config_path()),
    }
}

/// 兑现 `resolve_cfg_path` 的结果：默认配置文件失效时报告并以 4 退出。
pub(crate) fn require_cfg_path(cfg_path: Result<PathBuf, Failure>, json: bool) -> PathBuf {
    cfg_path.unwrap_or_else(|failure| failure.exit(json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    // 参数预检不读 home。若处理函数直接退出进程，运行时的所有者连第二个调用都到不了。
    #[tokio::test]
    async fn lifecycle_results_return_to_the_runtime_owner() {
        assert_eq!(stop::handle_stop_cmd(None, Some(1), false, false).await, 2);
        assert_eq!(
            restart::handle_restart_cmd(
                None,
                Some(1),
                false,
                Vec::new(),
                aproxy::spec::Launch::new("restart"),
                false
            )
            .await,
            2
        );
        assert_eq!(Results::new(None).finish(false), 0);
    }

    #[test]
    fn start_failed_preserves_child_status_but_only_exits_with_portable_failure_codes() {
        use aproxy::spawn::{SpawnError, StartupLog};
        for (code, exit) in [
            (Some(3), 3),
            (Some(255), 255),
            (Some(0), 1),
            (Some(256), 1),
            (Some(0xc0000374), 1),
            (None, 1),
        ] {
            let error = SpawnError::Exited {
                pid: 7,
                code,
                format: None,
                report: None,
                startup_log: StartupLog {
                    path: "startup.log".into(),
                    lines: Vec::new(),
                },
            };
            let failure = spawn_failure(&error, |lang| error.render(lang));
            assert_eq!(failure.code, "start_failed");
            assert_eq!(
                failure.details.get("code").map(|v| v.as_u64().unwrap()),
                code.map(u64::from)
            );
            assert_eq!(failure.exit_code(), exit);
        }
    }

    #[test]
    fn failure_formats_only_for_existing_readers() {
        for (machine, expected) in [(false, vec![Lang::Zh]), (true, vec![Lang::En, Lang::Zh])] {
            let calls = std::cell::RefCell::new(Vec::new());
            let f = Failure::for_readers(
                "command_failed",
                |l| {
                    calls.borrow_mut().push(l);
                    format!("{l:?}")
                },
                json!({}),
                Lang::Zh,
                machine,
            );
            assert_eq!(f.terminal(), "Zh");
            assert_eq!(calls.into_inner(), expected);
            if machine {
                assert_eq!(f.message, "En");
            }
        }
    }

    #[test]
    fn explicit_config_path_is_made_absolute() {
        // 相对 --config 必须在 CLI 边界绝对化：它会被写进 .restore，restore 若在
        // 别的工作目录运行，相对路径会被当成「配置已不存在」而清掉记录
        let path = resolve_cfg_path(Some(std::path::Path::new("rel.toml"))).unwrap();
        assert!(path.is_absolute(), "{}", path.display());
        assert_eq!(path, std::env::current_dir().unwrap().join("rel.toml"));

        let path = resolve_cfg_path(Some(std::path::Path::new("~/x.toml"))).unwrap();
        assert_eq!(path, aproxy_ipc::home::user_dir().unwrap().join("x.toml"));
    }

    #[test]
    fn skills_only_accepts_a_version() {
        // 旧安装器可能显式传版本；解析时接受，由命令检查它是否就是当前版本，
        // 解析层必须接受这种写法
        assert!(
            aproxy::cli::Cli::try_parse_from(["aproxy", "install", "0.1.0", "--skills-only"])
                .is_ok()
        );
        assert!(
            aproxy::cli::Cli::try_parse_from(["aproxy", "install", "--skills-only", "--abort"])
                .is_err(),
            "与 --abort 等仍互斥"
        );
    }

    #[test]
    fn force_install_flags_parse() {
        let parse = |args: &[&str]| {
            aproxy::cli::Cli::try_parse_from(std::iter::once("aproxy").chain(args.iter().copied()))
        };
        let install = |args: &[&str]| match parse(args).unwrap().command {
            Some(aproxy::cli::Commands::Install(a)) => a,
            other => panic!("{args:?} 解析成了 {other:?}"),
        };
        let a = install(&[
            "upgrade",
            "--force",
            "latest",
            "--yes",
            "--variant",
            "headless",
        ]);
        assert!(a.force && a.yes);
        assert_eq!(a.version.as_deref(), Some("latest"));
        assert_eq!(a.variant.as_deref(), Some("headless"));
        let a = install(&["install", "--force", "--from", "x", "--yes"]);
        assert_eq!(a.from.as_deref(), Some("x"));
        let a = install(&["install", "--cpu", "x86-64-v3"]);
        assert!(!a.force);
        assert_eq!(a.cpu.as_deref(), Some("x86-64-v3"));
        // 普通安装也能选变体：在线安装装它，与 --from 同用时是核对
        let a = install(&["install", "--variant", "headless"]);
        assert!(!a.force);
        assert_eq!(a.variant.as_deref(), Some("headless"));
        let a = install(&["install", "--from", "x", "--variant", "desktop"]);
        assert_eq!(a.variant.as_deref(), Some("desktop"));
        // 只属于强制安装的参数离不开 --force；--force 与普通安装的特殊模式、与对它不起
        // 作用的选项互斥；变体与收编、中止、只更新 skill 互斥；VERSION 与 --from 互斥；
        // 变体与指令集档只认列出的取值
        for args in [
            &["install", "--yes"][..],
            &["install", "--variant", "headless", "--adopt"],
            &["install", "--variant", "headless", "--abort"],
            &["install", "--variant", "desktop", "--skills-only"],
            &["install", "--variant", "gui"],
            &["install", "--force", "--skills-only"],
            &["install", "--force", "--adopt"],
            &["install", "--force", "--abort"],
            &["install", "--force", "--no-skills"],
            &["upgrade", "--force", "--allow-downgrade"],
            &["install", "--force", "0.2.0", "--from", "x"],
            &["install", "--force", "--variant", "gui"],
            &["install", "--force", "--cpu", "x86-64-v4"],
            &["install", "--force", "--from", "x", "--cpu", "baseline"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }
}
