//! 终端上显示的时长与读取标准输入时的诊断。

use aproxy_ipc::message;

message!(seconds(secs: u64), en: "{secs}s", zh: "{secs} 秒");

message!(minutes(mins: u64, secs: u64), en: "{mins}m {secs}s", zh: "{mins} 分 {secs} 秒");

message!(hours(hours: u64, mins: u64), en: "{hours}h {mins}m", zh: "{hours} 小时 {mins} 分");

message!(days(days: u64, hours: u64), en: "{days}d {hours}h", zh: "{days} 天 {hours} 小时");

message!(stdin(e: &std::io::Error), en: "Cannot read the value from stdin: {e}", zh: "从 stdin 读取值失败: {e}");
