//! 控制端点的访问控制，跨用户实测：别的用户连不上实例的控制 socket，root 连得上。
//!
//! 只在 unix 上有意义（Windows 的管道权限由 `src/daemon.rs` 的单测读回 DACL 核对），并且
//! 要一个「别的用户」与免密的 `sudo`——开发机上通常没有，所以测试标了 `#[ignore]`，由 CI 的
//! ubuntu 任务建好用户、把名字放进 `APROXY_TEST_OTHER_USER` 之后用 `--ignored` 运行。
//!
//! 客户端用 python3 写：它要以另一个用户的身份运行，而那个用户读不到本测试的构建目录；
//! 况且要核对的正是「任何程序连上来会怎样」，不该经过 aproxy 自己的客户端代码。

#![cfg(unix)]

mod common;
use common::{TestHome, free_port};

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// 连上 `argv[1]` 的 unix socket，发一个 ping，打印结果：
/// - `connect:<异常名>`：连不上；
/// - `recv:<异常名>`：连上了，读应答时出错；
/// - `reply:<应答>`：读到的应答（对端不答就断开时为空）。
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

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

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
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600,
        "控制 socket 只有属主连得上"
    );

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
