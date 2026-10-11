//! install/upgrade：二进制安装与升级。
//!
//! 设计全文见 `.agents/plan/install-v1.md`（四条铁律：先标记、先下载后
//! rename、ACK 齐了才交换、逐个重启最后删除）。本模块当前承载状态文件
//! 与状态机；下载链条、交换原语、IPC 广播等随计划分步落位。

pub mod announce;
pub mod broadcast;
pub mod download;
pub mod flow;
pub mod restart;
pub mod skills;
pub mod staging;
pub mod state;
pub mod swap;

use std::path::{Path, PathBuf};

/// 安装者的全部状态都放在这里：`<home>/install/`。状态文件（state.rs）、备料与
/// 下载暂存（staging.rs）、一键安装脚本的下载暂存都在它下面，此外不写任何地方；
/// skill 目录自己的 `.dl`、`.staging` 例外，留在 `<home>/skills/` 下——它们要与
/// 目标目录同卷才能原子替换。
///
/// 收在一处是永久兼容契约 C4 的路径部分：强制安装（不走交接、不依赖任何版本读懂
/// 别的版本的状态）可以随时整目录清空它，而不必认识里面的内容。
///
/// 0.1.0 把状态写在 `run/install.state`、备料放在 `<home>/staging/`，新位置读不到
/// 它们是有意的：0.1.0 留下的安装现场与 0.2.0 起的安装器互不相认，自然隔离。
pub fn install_dir_in(home: &Path) -> PathBuf {
    home.join("install")
}
