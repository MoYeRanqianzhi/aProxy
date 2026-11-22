//! 启动终端上的说明；前台服务只在直接面对用户时使用。

use aproxy_ipc::message;

message!(unknown(t: &str), en: "Unknown alias or configuration file: {t}\nRun aproxy alias list to see the aliases, or aproxy alias add {t} <path> to add one.", zh: "未知的别名或配置文件: {t}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {t} <路径> 添加");

message!(ready(pid: u32, addr: &str), en: "Instance ready: pid {pid} at {addr}", zh: "实例已就绪：pid {pid}，地址 {addr}");

message!(already(), en: "aProxy is already running on this port:", zh: "此端口已有 aProxy 在运行，无需重复启动：");

message!(existing(pid: u32, addr: &str, version: &str), en: "  pid {pid}  listening on http://{addr}  v{version}", zh: "  pid {pid}  监听 http://{addr}  v{version}");

message!(manage(port: &str), en: "Show instances: aproxy status    Stop: aproxy stop {port}", zh: "查看实例: aproxy status    停止: aproxy stop {port}");

message!(other_address(port: &str, other_listen_addr: &str, listen_addr: &str), en: "Port {port} is in use by an aProxy instance listening on {other_listen_addr}. This instance would listen on {listen_addr}. Instances are identified by port. Change the listening port of one of them.", zh: "端口 {port} 已被监听地址 {other_listen_addr} 的 aProxy 实例使用（本进程将监听 {listen_addr}），两者不能并存。\n实例按端口号区分，请为其中一个更换监听地址/端口。");

message!(started(), en: "aProxy started in the background", zh: "aProxy 已在后台启动");

message!(listen(addr: &str), en: "  Listening: http://{addr}", zh: "  监听: http://{addr}");

message!(client(addr: &str), en: "  Set the API base URL to http://{addr}", zh: "  提示: 将你的 API base URL 指向 http://{addr}");

message!(upstream(base: &str), en: "        Paths and query parameters are forwarded unchanged to {base}/<path>?<query>", zh: "        上游路径与查询参数将完整透传到 {base}/<path>?<query>");

message!(forward(), en: "  Forward-only mode: no buffering or retries", zh: "  仅转发模式：不缓冲、不重试");

message!(formats(n: usize), en: "  Format programs: {n}", zh: "  格式程序数: {n}");

message!(config(path: std::path::Display<'_>), en: "  Configuration: {path}", zh: "  配置: {path}");

message!(no_config(), en: "  Configuration: no configuration file; using command-line arguments and global defaults", zh: "  配置: （没有配置文件，使用命令行参数与全局默认）");

message!(log(log_path: &str), en: "  Log: {log_path}", zh: "  日志: {log_path}");

message!(base(url: &str), en: "  Base URL: {url}", zh: "  Base URL: {url}");

message!(foreground(), en: "aProxy started in the foreground", zh: "aProxy 已启动（前台）");

message!(quit(), en: "Press Ctrl+C to stop.", zh: "按 Ctrl+C 退出。");
