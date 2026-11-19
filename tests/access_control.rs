//! 访问控制，从外面看：home 只让属主访问，别的用户连不上实例的控制 socket。
//!
//! - home：aproxy 建出来的 home 只让属主访问；属于本用户而权限过宽的 home，第一次往里写
//!   配置时被收紧；只读的命令不改权限。
//! - 控制 socket（unix）：别的用户连不上，root 连得上。这一项要一个「别的用户」与免密的
//!   `sudo`——开发机上通常没有，所以那个测试标了 `#[ignore]`，由 CI 的 ubuntu 任务建好
//!   用户、把名字放进 `APROXY_TEST_OTHER_USER` 之后用 `--ignored` 运行。它的客户端用
//!   python3 写：要以另一个用户的身份运行，而那个用户读不到本测试的构建目录；况且要核对
//!   的正是「任何程序连上来会怎样」，不该经过 aproxy 自己的客户端代码。Windows 的管道权限
//!   由 `src/daemon.rs` 的单测读回 DACL 核对。

mod common;
use common::TestHome;
#[cfg(unix)]
use common::free_port;

use std::path::Path;
use std::process::Command;

/// 在 `home` 里运行一条 aproxy 命令，要求它成功，返回 stdout。
fn aproxy_ok(home: &TestHome, args: &[&str]) -> String {
    let out = home.cmd().args(args).output().expect("运行 aproxy 失败");
    assert!(
        out.status.success(),
        "{args:?} 失败：{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// home 是否只让属主访问。
fn is_private(home: &Path) -> bool {
    aproxy::fsx::access_of(home).unwrap() == aproxy::fsx::Access::Private
}

/// 让别的用户也能读 `home`：unix 改权限位，Windows 加一项给 Everyone 的可继承读权限。
fn loosen(home: &Path) {
    #[cfg(unix)]
    chmod(home, 0o755);
    #[cfg(windows)]
    {
        // *S-1-1-0 = Everyone；(OI)(CI) = 文件与子目录都继承；R = 读
        let out = Command::new("icacls")
            .arg(home)
            .args(["/grant", "*S-1-1-0:(OI)(CI)R"])
            .output()
            .expect("运行 icacls 失败");
        assert!(
            out.status.success(),
            "icacls 失败：{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// aproxy 建出来的 home 只让属主访问：unix 是 0700，Windows 是不继承上级的显式 DACL。
#[test]
fn a_new_home_is_private_to_its_owner() {
    let home = TestHome::unborn();
    assert!(!home.path().exists());
    aproxy_ok(&home, &["settings", "set", "watchdog.enabled", "false"]);
    assert!(home.path().join("settings.json").exists());
    assert!(is_private(home.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    #[cfg(windows)]
    {
        let sddl = aproxy::fsx::dacl_sddl(home.path()).unwrap();
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert_eq!(sddl.matches("(A;OICI;FA;;;").count(), 3, "{sddl}");
    }
}

/// 权限过宽的既有 home（别的版本或安装脚本建的、放在公共目录里的）：只读的命令照实报告、
/// 不改它；第一次往里写配置时被收紧。
#[test]
fn a_loose_home_is_tightened_by_the_first_config_write() {
    let home = TestHome::new();
    loosen(home.path());
    assert!(!is_private(home.path()), "前提：home 此刻别人读得到");

    // 只读的命令不改权限；doctor 指出来
    aproxy_ok(&home, &["settings", "get", "language"]);
    let report = aproxy_ok(&home, &["doctor"]);
    assert!(report.contains("[file_permissions_too_open]"), "{report}");
    assert!(!is_private(home.path()), "只读的命令不该改权限");

    aproxy_ok(&home, &["settings", "set", "language", "en"]);
    assert!(is_private(home.path()), "写过配置之后 home 只让属主访问");
    let report = aproxy_ok(&home, &["doctor"]);
    assert!(!report.contains("[file_permissions_too_open]"), "{report}");
}

/// 连上 `argv[1]` 的 unix socket，发一个 ping，打印结果：
/// - `connect:<异常名>`：连不上；
/// - `recv:<异常名>`：连上了，读应答时出错；
/// - `reply:<应答>`：读到的应答（对端不答就断开时为空）。
#[cfg(unix)]
const CLIENT: &str = r#"
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect(sys.argv[1])
except OSError as e:
    print("connect:" + type(e).__name__)
    sys.exit(0)
s.settimeout(5)
data = b""
try:
    s.sendall(b'{"v":1,"op":"ping"}\n')
    while not data.endswith(b"\n"):
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
except OSError as e:
    print("recv:" + type(e).__name__)
    sys.exit(0)
print("reply:" + data.decode().strip())
"#;

/// 以 `sudo_args` 给出的身份（空 = 本用户，不经 sudo）运行客户端，返回它打印的那一行。
#[cfg(unix)]
fn connect_as(sudo_args: &[&str], socket: &Path) -> String {
    let mut cmd = if sudo_args.is_empty() {
        Command::new("python3")
    } else {
        let mut cmd = Command::new("sudo");
        // -n：要密码时直接失败，不挂在提示上
        cmd.arg("-n").args(sudo_args).arg("python3");
        cmd
    };
    let out = cmd
        .arg("-c")
        .arg(CLIENT)
        .arg(socket)
        .output()
        .expect("运行 python3 客户端失败");
    assert!(
        out.status.success(),
        "客户端没有正常结束（{sudo_args:?}）：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "要免密 sudo 与 APROXY_TEST_OTHER_USER 指定的第二个用户，由 CI 的 ubuntu 任务运行"]
fn control_socket_refuses_other_users_and_admits_root() {
    let other = std::env::var("APROXY_TEST_OTHER_USER")
        .expect("APROXY_TEST_OTHER_USER 应是一个已存在的、不是本用户的用户名");
    let home = TestHome::new();
    let port = free_port();
    let cfg = home.path().join("access.toml");
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://access-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    home.stop_on_drop(port);
    let out = home
        .cmd()
        .args(["start", "--config", cfg.to_str().unwrap()])
        .output()
        .expect("运行 aproxy 失败");
    assert!(
        out.status.success(),
        "start 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let run = home.path().join("run");
    let socket = run.join(format!("{port}.sock"));
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600,
            "控制 socket 只有属主连得上"
        );
    }

    // 本用户照常连得上：先确认这个客户端本身是好的
    let own = connect_as(&[], &socket);
    assert!(
        own.starts_with("reply:") && own.contains("\"ok\":true"),
        "{own}"
    );

    // 目录的权限本来就挡着别的用户（home 是 0700）。这里故意把一路上的目录都放开，单独
    // 核对 socket 自己的两层保护
    for dir in [home.path(), run.as_path()] {
        chmod(dir, 0o755);
    }

    // 第一层：socket 文件是 0600，别的用户连 connect 都过不去
    assert_eq!(
        connect_as(&["-u", &other], &socket),
        "connect:PermissionError",
        "别的用户不该连得上 0600 的 socket"
    );

    // 第二层：就算 socket 文件的权限被放开，实例核对对端的 uid，不是本用户也不是 root 的
    // 连接直接断开——不读请求、不回任何内容
    chmod(&socket, 0o666);
    let refused = connect_as(&["-u", &other], &socket);
    assert!(
        refused == "reply:" || refused.starts_with("recv:"),
        "别的用户的连接应被断开而不是得到应答：{refused}"
    );
    chmod(&socket, 0o600);

    // root 连得上，并得到正常的应答：`sudo aproxy stop` 靠它
    let root = connect_as(&["-u", "root"], &socket);
    assert!(
        root.starts_with("reply:") && root.contains("\"ok\":true"),
        "root 应连得上并得到应答：{root}"
    );

    // 被拒的连接没有任何效果：实例照常应答本用户
    let own = connect_as(&[], &socket);
    assert!(own.contains("\"state\":\"serving\""), "{own}");
}
