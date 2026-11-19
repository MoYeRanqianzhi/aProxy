//! 操作层：管理实例的每个动作（选目标、启动、停止、重启、恢复、别名、列举）各有一个函数，
//! 做完把**结果交回调用方**，自己不打印、不结束进程。
//!
//! 调用方有两个：命令行（`src/commands/`）把结果渲染成给人看的文字、决定退出码；桌面界面
//! 在自己的进程里直接调用同一批函数——所以这里的函数不能替调用方做这两件事，打印出去的
//! 文字界面拿不到，`process::exit` 会把界面一起带走。`tests/lib_is_quiet.rs` 守着这条线：
//! 库的非测试代码里不出现 `println!`、`eprintln!` 与 `process::exit`。
//!
//! 结果的形态：
//! - 成功与失败都是带数据的枚举（哪个端口、哪个 pid、卡在哪一步），不是拼好的句子，
//!   调用方按自己的语言与场合去说；
//! - 从更底层原样带上来的原因（配置校验、绑定端口、IPC 的错误文字）仍是字符串，放在
//!   名为 `reason` 或 `error` 的字段里。
//!
//! 逐个处理多个实例时（`stop all`、`restart all`、`restore`），这里只给「处理一个」的
//! 函数，循环留给调用方：每个实例可能要等好几秒，调用方要在每一个做完时就能报告进度。

pub mod alias;
pub mod enumerate;
pub mod restart;
pub mod restore;
pub mod start;
pub mod stop;
pub mod target;
