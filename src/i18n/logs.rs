//! 日志阅读器自己的提示；读取到的日志行不在这里翻译。

use aproxy_ipc::message;

message!(foreground(port: &str), en: "The instance on port {port} runs with --foreground. Its logs go to its console, not to a file.", zh: "端口 {port} 的实例以前台模式运行（--foreground），日志输出在它的控制台，没有日志文件。");

message!(record_missing(port: &str, log_path: &str), en: "The instance on port {port} is not running, and the log file in its record does not exist: {log_path}", zh: "端口 {port} 的实例没在运行；它的期望状态记录指向的日志文件不存在: {log_path}");

message!(no_all(), en: "aproxy logs does not support all. Specify a port or a configuration alias to read one instance log at a time.", zh: "aproxy logs 不支持 all：一次只读一个实例的日志，请指定端口号或配置别名。");

message!(no_port(port: &str), en: "No running aProxy instance on port {port} and no record from a previous run.", zh: "端口 {port} 上没有运行中的 aProxy 实例，也没有它上次运行留下的记录。");

message!(no_alias(target: &str, path: std::path::Display<'_>), en: "Alias {target} (config {path}) has no running instance and no record from a previous run.", zh: "别名 {target}（配置 {path}）当前未在运行，也没有它上次运行留下的记录。");

message!(empty(), en: "No running aProxy instances and no records from a previous run.", zh: "没有运行中的 aProxy 实例，也没有实例上次运行留下的记录。");

message!(following(pid: u32, port: &str), en: "Following pid {pid} on port {port}. Press Ctrl+C to exit.", zh: "正在连接 pid {pid}（端口 {port}）——Ctrl+C 退出。");

message!(no_follow(), en: " (not following a stopped instance)", zh: "（没在运行，不跟随）");

message!(stopped(port: &str, note: &str), en: "The instance on port {port} is not running. Showing the log from its last run{note}.", zh: "端口 {port} 的实例没在运行，以下是它上次运行的日志{note}。");

message!(file(path: std::path::Display<'_>), en: "Log file: {path}", zh: "日志文件: {path}");

message!(multi_live(n: usize), en: "There are {n} running instances. Specify a port or configuration alias to read one log:", zh: "有 {n} 个实例在运行，必须指定端口号或配置别名（一次只读一个实例的日志）：");

message!(multi_stopped(n: usize), en: "There are no running instances and {n} saved records. Specify a port to read its last log:", zh: "没有运行中的实例；有 {n} 个实例留下了期望状态记录，指定端口查看它上次运行的日志：");

message!(no_file(), en: "The instance exited without creating a log file.", zh: "实例已退出，日志文件未生成。");

message!(file_timeout(path: std::path::Display<'_>), en: "Log file not created: {path}. The instance may be running with --foreground and logging to its console.", zh: "日志文件迟迟未生成: {path}（该实例可能以 --foreground 运行，日志输出在它的控制台）");

message!(open(e: &std::io::Error), en: "Cannot open the log file: {e}", zh: "无法打开日志文件: {e}");

message!(end(), en: "The instance stopped. Log following has ended.", zh: "实例已停止，日志跟踪结束。");

message!(truncated(), en: "── Log file truncated. Following from the beginning. ──", zh: "── 日志文件已被截断，重新从头跟踪 ──");
