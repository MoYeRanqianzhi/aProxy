//! 重启失败的阶段、旧实例状态及恢复方法。
use aproxy_ipc::message;
message!(failed(port: &str, error: &str, excerpt: &str), en: "Restart failed on port {port}. The old instance has stopped and service is interrupted: {error}{excerpt}\nCorrect the problem and run aproxy restore. If enabled, the watchdog also retries automatically.", zh: "端口 {port} 重启失败（旧实例已停止，该端口服务已中断）：{error}{excerpt}\n修正上述问题后执行 aproxy restore 恢复服务（看护者开着的话也会自动重试）。");
message!(config(path: std::path::Display<'_>), en: "\n  Config: {path}", zh: "\n  配置: {path}");

message!(not_running(message: &str), en: "{message}\nRestart only restarts running instances. To start an instance, use aproxy start.", zh: "{message}\nrestart 只重启运行中的实例，不启动新实例；要启动用 aproxy start");

message!(no_port(port: &str), en: "No running aProxy instance on port {port}.", zh: "端口 {port} 上没有运行中的 aProxy 实例。");

message!(done(new_port: &str, pid: u32, old: u32, addr: &str), en: "Restarted (port {new_port})\n  pid: {pid} (was {old})\n  Listening: http://{addr}", zh: "已重启（端口 {new_port}）\n  pid: {pid}（旧 pid {old}）\n  监听: http://{addr}");

message!(starting_state(), en: "Instance is still starting; no restart was attempted.", zh: "实例仍在启动，未尝试重启。");

message!(unsupported_state(), en: "no launch record; the old instance is still running", zh: "没有可用来重新拉起它的记录，旧实例保持运行");

message!(precheck_state(), en: "preflight failed; the old instance is still running", zh: "预检未通过，旧实例保持运行");

message!(changed_state(), en: "The current instance was not stopped.", zh: "当前实例未停止。");

message!(stop_state(), en: "stop not confirmed; no new instance was started", zh: "停止未确认，未启动新实例");

message!(start_state(), en: "the old instance stopped and its replacement is not ready; service is interrupted", zh: "旧实例已停止、新实例未就绪，服务已中断");

message!(failed_count(total: usize, failed: usize), en: "\nRestart failed for {failed} of {total} instances. The remaining instances were restarted:", zh: "\n{total} 个实例中有 {failed} 个重启失败（已跳过，其余照常重启）：");

message!(state(port: &str, state: &str), en: "  Port {port}: {state}", zh: "  端口 {port}：{state}");

message!(starting(port: &str), en: "Instance on port {port} is still starting; retry after it begins serving.", zh: "端口 {port} 的实例仍在启动；开始提供服务后再试。");

message!(unsupported(port: &str), en: "Port {port} was not restarted: this instance has no launch record. Restart foreground instances in their own terminal. The old instance is still running.", zh: "端口 {port} 未重启：这个实例没有可用来重新拉起它的记录（前台运行的实例要在它自己的终端里重启），旧实例保持运行。");

message!(precheck(port: &str, cause: &str), en: "Port {port} was not restarted: preflight failed. The old instance is still running.\n{cause}", zh: "端口 {port} 未重启：预检未通过，旧实例保持运行。\n{cause}");

message!(stop_failed(port: &str, cause: &str), en: "{cause}\nStop was not confirmed on port {port}. Restart was aborted. No new instance was started to avoid competing for the port.", zh: "{cause}\n端口 {port} 停止未确认，重启中止（未启动新实例，以免与旧实例争抢端口）。");

message!(unreadable(reason: &str), en: "Cannot read the desired-state record for this instance: {reason}", zh: "这个实例的期望状态记录读不懂：{reason}");

message!(missing(path: std::path::Display<'_>), en: "Configuration file no longer exists ({path})", zh: "配置文件已不存在（{path}）");

message!(taken(pid: u32, port: &str), en: "An aProxy instance (pid {pid}) is already running on the new configuration’s listening port {port}.", zh: "新配置的监听端口 {port} 上已有 aProxy 实例（pid {pid}）在运行");

message!(format_selection(), en: "Use --format all alone, or specify one or more format names.", zh: "请单独使用 --format all，或指定一个或多个 format 条目名。");
message!(format_done(port:&str,name:&str,program:&str,version:&str), en: "Port {port}: restarted format {name} ({program} {version}).", zh: "端口 {port}：已重启 format {name}（{program} {version}）。");
message!(format_failed(port:&str,name:&str,reason:&str), en: "Port {port}: format {name} handshake failed: {reason}.", zh: "端口 {port}：format {name} 握手失败：{reason}。");
message!(format_config(), en: "config.toml was not reloaded; restart the instance to change a [[format]] entry.", zh: "未重读 config.toml；修改 [[format]] 条目须重启实例。");
message!(format_refused(port:&str,message:&str), en: "Port {port}: could not restart formats: {message}", zh: "端口 {port}：无法重启 format：{message}");
message!(format_unsupported(port:&str), en: "Port {port}: this instance does not support restarting formats.", zh: "端口 {port}：此实例不支持单独重启 format。");
