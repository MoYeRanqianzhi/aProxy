//! 按期望状态记录恢复实例的命令输出。
use aproxy_ipc::message;
message!(failed(port:&str,error:&str,excerpt:&str), en:"Restore failed on port {port}: {error}{excerpt}", zh:"端口 {port} 恢复失败：{error}{excerpt}");

message!(skip(path: std::path::Display<'_>, reason: &str), en: "Skipping unreadable record {path}: {reason}", zh: "跳过读不懂的记录 {path}: {reason}");

message!(none(), en: "No instances to restore.", zh: "没有需要恢复的实例。");

message!(running(port: &str), en: "Port {port} is already running. Skipping.", zh: "端口 {port} 已在运行，跳过。");

message!(missing(port: &str, config: &str), en: "Configuration file not found for port {port} ({config}). The instance was not restored. Its record is retained. Run aproxy restore when the file is back, or aproxy stop {port} if this instance is no longer needed.", zh: "端口 {port} 的配置文件找不到（{config}），这次没有恢复；记录已保留，文件回来之后再执行 aproxy restore。不再需要这个实例时执行 aproxy stop {port}");

message!(moved(pid: u32, port: &str, old: &str), en: "Restored pid {pid} (port {port}; the configured port changed from {old}, and the record now belongs to the new port)", zh: "已恢复 pid {pid}（端口 {port}；配置的端口已从 {old} 变更，原端口的记录已由新端口接替）");

message!(restored(pid: u32, port: &str), en: "Restored pid {pid} (port {port})", zh: "已恢复 pid {pid}（端口 {port}）");
