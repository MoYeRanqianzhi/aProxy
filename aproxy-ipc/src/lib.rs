//! aproxy 与同它打交道的程序共用的契约：aproxy 本体、桌面界面，以及第三方写的客户端，
//! 都从这里取同一份定义，而不是各抄一份。
//!
//! 现在有的：
//! - [`home`]：aProxy 主目录的解析（永久兼容契约 C1）。
//! - [`code`]：错误码表——名字、族、`details` 的键与类型、HTTP 状态与退出码，并导出 JSON。
//! - [`diag`]：配置诊断——config.toml 与 settings.json 里一个问题的形状（码、级别、文件、键路径、行列）。
//! - [`endpoint`]：实例控制端点、心跳节、安装宣告节的名字，以及区分 home 的 `home_id`。
//! - [`redact`]：打码的唯一出口——URL、密钥头、占位对象、已知密钥替换。
//! - [`ledger`]：进程台账 `<home>/ledger/`（永久兼容契约 C3）——条目的格式、写者、读者
//!   与它的义务、启动标记的推导。强制安装靠它找到并结束一个 home 下的全部 aproxy 进程。
//!
//! 本包不打印、不结束进程：这两件事属于调用它的程序。

#[cfg(feature = "client")]
pub mod client;
pub mod code;
#[cfg(feature = "config-file")]
pub mod config_file;
#[cfg(feature = "body-inspection")]
pub mod decode;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod diag;
pub mod endpoint;
#[cfg(feature = "client")]
pub mod enumerate;
pub mod home;
pub mod i18n;
#[cfg(feature = "body-inspection")]
pub mod inspection;
pub mod ledger;
pub mod program;
pub mod redact;
#[cfg(feature = "client")]
pub mod registry;
#[cfg(feature = "client")]
pub mod view;
#[cfg(feature = "client")]
pub mod wire;

/// 本包的版本。与 aproxy 同号发版，台账条目的 `version` 字段写的就是它。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod dependency_tests {
    /// Check the normal dependency closure on *all* targets, not the feature-unified
    /// workspace graph (which intentionally includes the asynchronous client).
    #[test]
    fn featureless_tree_stays_small() {
        use std::collections::BTreeSet;
        let tree = std::process::Command::new(env!("CARGO"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args([
                "tree",
                "--offline",
                "--locked",
                "-p",
                "aproxy-ipc",
                "--no-default-features",
                "--target",
                "all",
                "-e",
                "normal",
                "--prefix",
                "none",
            ])
            .output()
            .unwrap();
        assert!(
            tree.status.success(),
            "{}",
            String::from_utf8_lossy(&tree.stderr)
        );
        let text = String::from_utf8(tree.stdout).unwrap();
        let names: BTreeSet<_> = text
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .collect();
        let expected = "aproxy-ipc block-buffer cfg-if const-oid cpufeatures crypto-common digest hybrid-array itoa libc memchr proc-macro2 quote serde serde_core serde_derive serde_json sha2 syn typenum unicode-ident windows-sys windows-targets windows_aarch64_gnullvm windows_aarch64_msvc windows_i686_gnu windows_i686_gnullvm windows_i686_msvc windows_x86_64_gnu windows_x86_64_gnullvm windows_x86_64_msvc zmij";
        assert_eq!(names, expected.split_whitespace().collect());
    }
}
