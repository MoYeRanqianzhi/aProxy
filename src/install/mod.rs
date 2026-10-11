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

/// 可安装版本的下限。0.1.x 与 0.2.0 起的版本互不兼容（控制协议、安装状态的位置、
/// `--version` 的格式都变了）：0.2.0 起的安装器驱动不了 0.1.x 的二进制，也联系不上
/// 0.1.x 的实例，装回 0.1.x 只会在交换二进制之后半途坏掉。所以在线安装与 `--from`
/// 都拒绝低于它的目标，`--allow-downgrade` 也越不过它。
///
/// 按「主.次.修订」比较：0.2.0 的预发布（alpha/beta/rc，以及开发构建自报的
/// 0.2.0-alpha.0）属于 0.2.0 这条线。按 semver 它们低于 0.2.0，直接比较 Version
/// 会把它们一起拒掉。
pub const INSTALL_FLOOR: semver::Version = semver::Version::new(0, 2, 0);

/// 目标版本不低于 [`INSTALL_FLOOR`]；低于时的错误说明为什么、怎样回到 0.1.x。
pub fn check_floor(target: &semver::Version) -> Result<(), String> {
    let floor = &INSTALL_FLOOR;
    if (target.major, target.minor, target.patch) >= (floor.major, floor.minor, floor.patch) {
        return Ok(());
    }
    Err(format!(
        "目标版本 {target} 低于 {floor}，不能安装：0.1.x 与 0.2.0 起的版本互不兼容（控制协议与磁盘布局都变了），\
         --allow-downgrade 也越不过这条下限。要回到 0.1.x：先用当前版本执行 `aproxy stop all` 停掉全部实例，\
         删掉 bin 目录里的 aproxy 二进制，再用 0.1.x 的安装脚本重新安装"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rejects_0_1_x_and_admits_the_whole_0_2_0_line() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        for below in ["0.1.0", "0.1.99", "0.1.0-alpha.17", "0.0.1"] {
            let err = check_floor(&v(below)).unwrap_err();
            assert!(
                err.contains("0.1.x") && err.contains("--allow-downgrade"),
                "{err}"
            );
        }
        // 0.2.0 的预发布（含开发构建的 alpha.0 与最小的 0.2.0-0）都在线内
        for ok in [
            "0.2.0-0",
            "0.2.0-alpha.0",
            "0.2.0-rc.1",
            "0.2.0",
            "0.3.0",
            "1.0.0",
        ] {
            assert!(check_floor(&v(ok)).is_ok(), "{ok}");
        }
        assert!(check_floor(&v(env!("CARGO_PKG_VERSION"))).is_ok());
    }
}
