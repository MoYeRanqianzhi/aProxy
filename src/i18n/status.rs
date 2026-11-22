//! 实例清点的终端文字；运行记录与 IPC 快照在此处只读。

use aproxy_ipc::message;

message!(idle(), en: "idle", zh: "闲置");

message!(active(), en: "active", zh: "活跃");

message!(filtered(which: &str, idle_threshold_secs: u64), en: "No {which} aProxy instances (threshold: {idle_threshold_secs} s).", zh: "没有{which}的 aProxy 实例（阈值 {idle_threshold_secs} 秒）。");

message!(incompatible(n: usize), en: "Instances with unverified replies ({n}): the protocol or identity differs. Use the same aProxy version as the instance.", zh: "无法采信应答的 aProxy 实例 ({n})——协议版本不同或身份对不上，用与实例同版本的 aproxy 操作它:");

message!(incompatible_row(port: u16, pid: &str, version: &str, code: &str), en: "  Port {port}  pid {pid}  v{version}  reason {code}", zh: "  端口 {port}  pid {pid}  v{version}  原因 {code}");

message!(pending(n: usize), en: "Instances awaiting restore ({n}): not running. aproxy restore or the watchdog can start them from their records.", zh: "等待恢复的实例 ({n})——没在运行，留着期望状态记录，aproxy restore 或看护者会拉起它:");

message!(missing(port: u16), en: "  Port {port} (configuration file not found)", zh: "  端口 {port}（配置文件找不到）");

message!(down_port(port: u16), en: "  Port {port}", zh: "  端口 {port}");

message!(forget(port: u16), en: "    Run aproxy stop {port} if you no longer need this instance.", zh: "    不再需要它时执行 aproxy stop {port}");

message!(none(), en: "No running aProxy instances.", zh: "没有运行中的 aProxy 实例。");

message!(unreadable(n: usize), en: "Unreadable records ({n}), left in place. If a newer aProxy wrote them, use that version to manage them:", zh: "读不懂的记录 ({n})，原样保留（多半由更新版本的 aproxy 写下，请用那个版本处理）:");

message!(gc_paused(), en: "Log cleanup is paused: run/ contains unreadable records.", zh: "日志清理已暂停：run/ 里有读不懂的记录。");

message!(watchdog(pid: u32, version: &str, since_scan: &str), en: "Watchdog: pid {pid}  v{version}  last scan {since_scan} ago", zh: "看护者: pid {pid}  v{version}  最近扫描 {since_scan} 前");

message!(watchdog_stale(pid: u32, version: &str, since_scan: &str), en: "Watchdog: pid {pid}  v{version}  no scan for {since_scan} (it may be unresponsive)", zh: "看护者: pid {pid}  v{version}  已 {since_scan} 没有扫描（可能卡住了）");

message!(processes(n: usize), en: "Other aProxy processes in the ledger ({n}):", zh: "台账里的其他 aProxy 进程 ({n}):");

message!(running(n: usize), en: "Running aProxy instances ({n}):", zh: "运行中的 aProxy 实例 ({n}):");

message!(live(port: &str, pid: u32, version: &str, uptime: &str, idle: &str), en: "  Port {port}  pid {pid}  v{version}  uptime {uptime}  idle {idle}", zh: "  端口 {port}  pid {pid}  v{version}  已运行 {uptime}  闲置 {idle}");

message!(endpoints(addr: &str, upstream: &str, config: &str), en: "    Listen http://{addr}   upstream {upstream}   config {config}", zh: "    监听 http://{addr}   上游 {upstream}   配置 {config}");

message!(recent(code: &str, message: &str, age: &str), en: "\"{code}{message}\" ({age} ago)", zh: "\"{code}{message}\"（{age} 前）");

message!(no_error(), en: "none", zh: "无");

message!(requests(requests: u64, active: u64, retries: u64, rescued: u64, error: &str), en: "    Requests {requests}  in flight {active}  retries {retries}  rescued {rescued}  last error: {error}", zh: "    请求 {requests}  在途 {active}  重试 {retries}  救回 {rescued}  最近错误: {error}");

message!(starting(), en: "    Starting (not accepting requests yet)", zh: "    正在启动（还没开始接收请求）");

message!(swapping(), en: "    Replacing the binary during a rolling install. Do not stop or restart this instance.", zh: "    二进制更换中（install 滚动重启阶段，请勿手动干预此实例）");

message!(stopping(), en: "    Stopping", zh: "    正在退出");

message!(unknown(), en: "    Unknown state. Use the same aProxy version as the instance.", zh: "    状态未知（实例比当前 CLI 新，用同版本的 aproxy 查看）");

message!(version(version: &str, cli_version: &str, port: &str), en: "    Instance v{version} differs from CLI v{cli_version}. After replacing the binary, run aproxy restart {port} to finish the upgrade.", zh: "    注意: 实例版本 v{version} 与当前 CLI v{cli_version} 不同，替换二进制后执行 aproxy restart {port} 可完成升级");

message!(unresponsive(n: usize), en: "Unresponsive aProxy instances ({n}): the processes are running but do not answer on the control channel:", zh: "无响应的 aProxy 实例 ({n})——进程仍在，但不应答控制通道:");

message!(unresponsive_row(port: &str, pid: u32, version: &str, config: &str), en: "  Port {port}  pid {pid}  v{version}  config {config}\n    Run aproxy stop {port} --force to stop it, or aproxy restart {port} --force to restart it.", zh: "  端口 {port}  pid {pid}  v{version}  配置 {config}\n    可用 aproxy stop {port} --force 结束（或 aproxy restart {port} --force 重启）");

message!(no_config(), en: "(no configuration file)", zh: "（没有配置文件）");
