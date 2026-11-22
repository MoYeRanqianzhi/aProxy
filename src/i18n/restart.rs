//! 重启失败的阶段、旧实例状态及恢复方法。
use aproxy_ipc::message;
message!(failed(port: &str, error: &str, excerpt: &str), en: "Restart failed on port {port}. The old instance has stopped and service is interrupted: {error}{excerpt}\nCorrect the problem and run aproxy restore. If enabled, the watchdog also retries automatically.", zh: "端口 {port} 重启失败（旧实例已停止，该端口服务已中断）：{error}{excerpt}\n修正上述问题后执行 aproxy restore 恢复服务（看护者开着的话也会自动重试）。");
message!(config(path: std::path::Display<'_>), en: "\n  Config: {path}", zh: "\n  配置: {path}");
