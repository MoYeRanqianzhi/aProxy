//! 停止与重启共用的目标选择、单实例结果与恢复记录。

use aproxy_ipc::message;

message!(unknown(target: &str), en: "Unknown alias or port: {target}\nRun aproxy alias list to see the aliases, or aproxy alias add {target} <path> to add one.", zh: "未知的别名或端口号: {target}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {target} <路径> 添加");

message!(ambiguous(n: usize), en: "There are {n} running instances. Specify a port, alias or all:", zh: "有 {n} 个实例在运行，必须指定端口号、别名或 all：");

message!(alias_pid(alias: &str, port: &str, pid: u32), en: "Instance for alias {alias} (port {port}, pid {pid})", zh: "别名 {alias} 的实例（端口 {port}，pid {pid}）");

message!(port_pid(port: &str, pid: u32), en: "Instance on port {port} (pid {pid})", zh: "端口 {port} 的实例（pid {pid}）");

message!(unresponsive(who: &str), en: "{who} is running but does not respond on the control channel. Use --force to terminate it.", zh: "{who}进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束");

message!(idle_only(cmd: &str), en: "--idle-threshold-secs requires the idle target. Example: aproxy {cmd} idle --idle-threshold-secs 600", zh: "--idle-threshold-secs 只能与目标 idle 一起用，例如: aproxy {cmd} idle --idle-threshold-secs 600");

message!(forgotten(port: &str), en: "The instance on port {port} is no longer running. Its desired-state record was removed. Restore and the watchdog will not start it again.", zh: "端口 {port} 的实例已不在运行；已删除它的期望状态记录，restore 与看护者不会再拉起它。");

message!(no_port(port: &str), en: "No running aProxy instance on port {port}.\nIf another program is using this port, aProxy does not manage it.", zh: "端口 {port} 上没有运行中的 aProxy 实例。\n（若该端口被其他程序占用，与本工具无关）");

message!(no_alias(target: &str, path: std::path::Display<'_>), en: "Alias {target} (configuration {path}) is not running.", zh: "别名 {target}（配置 {path}）当前未在运行。");

message!(forgot_config(path: std::path::Display<'_>, ports: &str), en: "The instance for configuration {path} is no longer running. Its desired-state record was removed (ports {ports}). Restore and the watchdog will not start it again.", zh: "配置 {path} 的实例已不在运行；已删除它的期望状态记录（端口 {ports}），restore 与看护者不会再拉起它。");

message!(none_idle(idle_secs: u64), en: "No instances have been idle for more than {idle_secs} seconds.", zh: "没有闲置超过 {idle_secs} 秒的实例。");

message!(not_idle(pid: u32, port: &str, idle_secs: u64), en: "Pid {pid} on port {port} received a request in the meantime and has been idle for only {idle_secs} seconds, so it was not stopped.", zh: "pid {pid}（端口 {port}）刚才又收到了请求（已闲置 {idle_secs} 秒），不再算闲置，未停止");

message!(cleaned(ports: &str), en: "Removed desired-state records left by stopped instances (ports {ports}).", zh: "已删除不在运行的实例留下的期望状态记录（端口 {ports}）。");

message!(retain(path: std::path::Display<'_>, reason: &str), en: "Retained unreadable record {path} ({reason}). It may have been written by a later aProxy version. Use that version to manage it.", zh: "保留了读不懂的记录 {path}（{reason}）：多半由更新版本的 aproxy 写下，请用那个版本处理。");

message!(stopped(pid: u32, port: &str), en: "Stopped pid {pid} (port {port})", zh: "已停止 pid {pid}（端口 {port}）");

message!(gone(pid: u32, port: &str), en: "Pid {pid} (port {port}) is no longer running", zh: "pid {pid}（端口 {port}）已不在运行");

message!(killed(pid: u32, port: &str), en: "Terminated pid {pid} by force (port {port})", zh: "已强制终止 pid {pid}（端口 {port}）");

message!(generation(port: &str), en: "The selected instance on port {port} was replaced; the current instance was not stopped.", zh: "选中的端口 {port} 上的实例已被替换，当前实例没有停止。");

message!(still_running(pid: u32, port: &str), en: "Pid {pid} received the stop request but has not exited. Check with aproxy status, or run aproxy stop {port} --force.", zh: "pid {pid} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop {port} --force 强制结束");

message!(no_reply(pid: u32, port: &str, error: &str), en: "Pid {pid} did not respond to the stop request ({error}). To terminate it, run aproxy stop {port} --force.", zh: "pid {pid} 不应答停止请求（{error}），可用 aproxy stop {port} --force 强制结束");

message!(restore_error(port: &str, werr: &str), en: "Could not put back the desired-state record for port {port}: {werr}\n", zh: "放回端口 {port} 的期望状态记录失败: {werr}\n");

message!(kill_error(pid: u32, error: &str), en: "Could not terminate pid {pid}: {error}", zh: "强制终止 pid {pid} 失败: {error}");
