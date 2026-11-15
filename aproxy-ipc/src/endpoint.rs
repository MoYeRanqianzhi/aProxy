//! 端点与内核对象的名字：实例的控制端点、心跳节、安装宣告节。
//!
//! 这些名字是实例与找它的程序（命令行、看护者、桌面界面、第三方客户端）之间的约定，两边
//! 必须算出同一个字符串，所以只在这里算一次。
//!
//! 名字都属于某个 run 目录（`<home>/run`）：unix 的 socket 本来就在 run 目录里；Windows 的
//! 命名管道与 `Local\` 内核对象是全机共享的名字空间，所以名字里带上 run 目录的标识
//! （[`home_id`]）。否则隔离 home（APROXY_HOME）里的 `aproxy stop <端口>` 会停掉另一个 home
//! 在同一端口上的实例——例如用临时 home 做实验时停掉用户正在用的生产实例。

use std::path::Path;

/// run 目录的标识：规范化路径的 SHA-256 前 16 个十六进制字符。Windows 的
/// 全机名字（控制管道、心跳节、安装宣告节）靠它区分 home。规范化让同一目录的不同写法
/// （大小写、分隔符、相对路径、subst 盘符、目录联接）得到同一个标识。
///
/// 目录还不存在时规范化「最近的已存在祖先」再接上其余部分：守护可能在 run
/// 目录建出来之前就算端点名（注册表写失败时），客户端在目录存在后再算——
/// 两边若一个走规范化、一个走字面绝对路径，经联接或 subst 访问的 home 会
/// 得到两个不同的标识，客户端就再也找不到这个实例。
pub fn home_id(run_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let absolute = std::path::absolute(run_dir).unwrap_or_else(|_| run_dir.to_path_buf());
    let mut resolved = None;
    for ancestor in absolute.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            // ancestors() 产出的都是 absolute 的前缀，strip_prefix 必然成功
            let rest = absolute.strip_prefix(ancestor).unwrap_or(Path::new(""));
            resolved = Some(if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            });
            break;
        }
    }
    let text = resolved.unwrap_or(absolute).display().to_string();
    #[cfg(windows)]
    let text = {
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        text.replace('/', r"\").to_lowercase()
    };
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// 实例的控制端点：Windows 为命名管道名 `\\.\pipe\aproxy-<home_id>-<port>`，unix 为
/// socket 路径 `<run_dir>/<port>.sock`。端口号在一个 run 目录内唯一区分实例。
pub fn instance(run_dir: &Path, port: &str) -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\aproxy-{}-{port}", home_id(run_dir))
    }
    #[cfg(unix)]
    {
        run_dir.join(format!("{port}.sock")).display().to_string()
    }
}

/// 实例心跳节的名字：Windows 为 `Local\aproxy-<home_id>-heart-<port>`，unix 为
/// `aproxy-<home_id>-heart-<port>`（调用方把它放进自己的心跳目录）。
pub fn heartbeat(run_dir: &Path, port: &str) -> String {
    let home_id = home_id(run_dir);
    #[cfg(windows)]
    {
        format!(r"Local\aproxy-{home_id}-heart-{port}")
    }
    #[cfg(unix)]
    {
        format!("aproxy-{home_id}-heart-{port}")
    }
}

/// 安装宣告节的名字 `Local\aproxy-<home_id>-install`。只有 Windows 用内核对象；unix 的
/// 宣告是 `<home>/install/announce` 文件。
#[cfg(windows)]
pub fn install_announce(run_dir: &Path) -> String {
    format!(r"Local\aproxy-{}-install", home_id(run_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_id_separates_homes_and_is_stable_across_spellings() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        // 两个 home 同一端口的端点必须不同：Windows 管道名全机共享，相同就
        // 意味着一个 home 的命令能控制另一个 home 的实例
        assert_ne!(
            instance(&a.path().join("run"), "12345"),
            instance(&b.path().join("run"), "12345")
        );
        // run 目录建出前后是同一个标识：守护可能先于目录创建算出端点名
        let run = a.path().join("run");
        let before = home_id(&run);
        std::fs::create_dir_all(&run).unwrap();
        assert_eq!(before, home_id(&run));
        // 同一目录的另一种写法得到同一个标识
        assert_eq!(home_id(&run), home_id(&a.path().join(".").join("run")));
        #[cfg(windows)]
        {
            let spelled = run.display().to_string().to_uppercase().replace('\\', "/");
            assert_eq!(home_id(&run), home_id(Path::new(&spelled)));
        }
    }

    #[test]
    fn names_have_the_documented_shape() {
        let run = tempfile::tempdir().unwrap();
        let id = home_id(run.path());
        assert_eq!(id.len(), 16);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        #[cfg(windows)]
        {
            assert_eq!(
                instance(run.path(), "12345"),
                format!(r"\\.\pipe\aproxy-{id}-12345")
            );
            assert_eq!(
                heartbeat(run.path(), "12345"),
                format!(r"Local\aproxy-{id}-heart-12345")
            );
            assert_eq!(
                install_announce(run.path()),
                format!(r"Local\aproxy-{id}-install")
            );
        }
        #[cfg(unix)]
        {
            assert_eq!(
                instance(run.path(), "12345"),
                run.path().join("12345.sock").display().to_string()
            );
            assert_eq!(
                heartbeat(run.path(), "12345"),
                format!("aproxy-{id}-heart-12345")
            );
        }
    }
}
