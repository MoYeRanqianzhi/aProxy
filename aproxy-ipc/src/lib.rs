//! aproxy 与同它打交道的程序共用的契约：aproxy 本体、桌面界面，以及第三方写的客户端，
//! 都从这里取同一份定义，而不是各抄一份。
//!
//! 现在有的：
//! - [`home`]：aProxy 主目录的解析（永久兼容契约 C1）。
//! - [`ledger`]：进程台账 `<home>/ledger/`（永久兼容契约 C3）——条目的格式、写者、读者
//!   与它的义务、启动标记的推导。强制安装靠它找到并结束一个 home 下的全部 aproxy 进程。
//!
//! 本包不打印、不结束进程：这两件事属于调用它的程序。

pub mod home;
pub mod ledger;

/// 本包的版本。与 aproxy 同号发版，台账条目的 `version` 字段写的就是它。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
