//! 配置值、来源、校验和写回的终端文案。错误文档显式取同一文案的英文。

use aproxy_ipc::message;

message!(read(e: &std::io::Error, path: std::path::Display<'_>), en: "Cannot read the configuration file: {e}\nFile: {path}", zh: "读取配置文件失败: {e}\n文件: {path}");

message!(specified_missing(path: std::path::Display<'_>), en: "The specified configuration file does not exist: {path}", zh: "指定的配置文件不存在: {path}");

message!(withheld(), en: "These credentials were not inherited from global defaults because they belong to another target:", zh: "全局默认里的这些凭据没有继承（它们属于另一个目标）:");

message!(targets(key: &str, default: &str, target: &str), en: "  {key}  default target: {default}; configuration target: {target}", zh: "  {key}  默认值属于 {default}，这份配置指向 {target}");

message!(create(), en: "No configuration file yet. Run aproxy config set <key> <value> to create it.", zh: "配置文件还不存在；用 aproxy config set <键> <值> 写入第一个键时创建。");

message!(warning(issue: &str), en: "Warning: {issue}", zh: "警告: {issue}");

message!(invalid(), en: "This configuration has errors and cannot start an instance:", zh: "这份配置有错误，现在不能用来启动:");

message!(base_url(), en: "base_url is empty. Set it with:", zh: "提示: base_url 为空，请设置:");

message!(unknown_keys(), en: "To remove unrecognized keys:", zh: "不认识的键可以这样删除:");

message!(bad_key(key: &str), en: "Invalid key path: {key}. Separate keys with dots, for example timeouts.connect_secs or extra_headers.\"x-foo\". Read or write an array of tables as a whole.", zh: "键路径写法不对: {key}（点号分隔，例如 timeouts.connect_secs、extra_headers.\"x-foo\"；数组表只能整体读写）");

message!(unknown(key: &str), en: "Unknown key: {key}. Run aproxy config show --effective to list available keys.", zh: "不认识的键: {key}（aproxy config show --effective 列出全部的键）");

message!(syntax(), en: "The configuration file has syntax errors:", zh: "配置文件有语法错误:");

message!(cannot_get(key: &str, path: std::path::Display<'_>), en: "Cannot read {key}. Repair {path} first.", zh: "读不出 {key}：先修复 {path}");

message!(origin(source: &str), en: "({source})", zh: "（{source}）");

message!(unset(), en: "(not set)", zh: "(未设置)");

message!(not_toml(key: &str, e: &str), en: "Write the value of {key} as TOML, for example true, 30, \"text\", [\"a\"], or {{ k = 1 }}. Parse error: {e}", zh: "{key} 的值要写成 TOML（例如 true、30、\"text\"、[\"a\"]、{{ k = 1 }}），解析失败: {e}");

message!(not_modified(), en: "The existing configuration has syntax errors. Nothing was changed:", zh: "现有配置文件有语法错误，没有修改:");

message!(repair(path: std::path::Display<'_>), en: "Repair or remove {path} first.", zh: "先修复或删除 {path}");

message!(refused(), en: "This change would invalidate the configuration. Nothing was saved.", zh: "这次修改会让配置无效，没有保存");

message!(conflict(path: std::path::Display<'_>), en: "Another program is modifying the configuration file. Nothing was saved. Close the editor and try again: {path}", zh: "配置文件一直在被别的程序修改，没有保存（关掉正在编辑它的程序后重试）: {path}");

message!(save_error(e: &std::io::Error), en: "Cannot save the configuration: {e}", zh: "保存配置失败: {e}");

message!(saved(path: std::path::Display<'_>), en: "Saved to {path}", zh: "已保存到 {path}");

message!(unchanged(path: std::path::Display<'_>), en: "No changes: {path}", zh: "没有变化: {path}");

message!(restart(), en: "Restart these running instances to apply the change:", zh: "运行中的实例要重启才用上这次改动:");

message!(command_line(), en: "command line", zh: "命令行");

message!(global(), en: "settings.json global default", zh: "settings.json 全局默认");

message!(environment(), en: "environment", zh: "环境变量");

message!(builtin(), en: "built-in default", zh: "内置默认");

message!(not_set(), en: "not set", zh: "未设置");

message!(forward(), en: "\"forward\" (stream without buffering or retrying)", zh: "\"forward\"（边收边转：不缓冲、不重试）");

message!(unlimited(), en: "unlimited", zh: "不设限");

message!(local_hosts(), en: "built-in default: loopback listeners allow only localhost, 127.0.0.1 and [::1]", zh: "内置默认：回环监听只放行 localhost / 127.0.0.1 / [::1]");

message!(no_origins(), en: "built-in default: reject all browser requests with an Origin header", zh: "内置默认：拒绝一切携带 Origin 的浏览器请求");

message!(log_path(), en: " (random name under ~/.aproxy/logs/ at startup; run aproxy logs <port> to see the path)", zh: "（~/.aproxy/logs/ 下按启动随机命名，实际路径用 aproxy logs <端口> 查看）");

message!(no_bodies(), en: "do not retain request or response bodies", zh: "不保留请求体与响应体");

message!(no_keepalive(), en: "keepalive off", zh: "保活关闭");

message!(no_delay(), en: "no delay between retries", zh: "所有重试零延迟");

message!(no_rules(), en: "[] (retry all requests indefinitely)", zh: "[]（一切请求无限重试）");

message!(any_path(), en: "(any path)", zh: "(任何路径)");

message!(any_status(), en: "(any status code)", zh: "(任何状态码)");

message!(exempt(), en: "unlimited (exempt)", zh: "不限（豁免）");

message!(no_limit(), en: "no limit", zh: "不限");

message!(value(key: &str, value: &str, source: &str), en: "{key:<26} = {value}  ({source})", zh: "{key:<26} = {value}  （{source}）");

message!(zero(zero: &str), en: "0 ({zero})", zh: "0（{zero}）");

message!(empty_list(empty: &str), en: "[] ({empty})", zh: "[]（{empty}）");

message!(file(path: std::path::Display<'_>), en: "Configuration file: {path}", zh: "配置文件: {path}");

message!(keepalive(trigger: &str), en: "\"{trigger}\" (accept: Accept header; body_stream: stream:true in request body; any: either)", zh: "\"{trigger}\"（accept=看 Accept 头 / body_stream=看请求体 stream:true / any=任一）");

message!(rules(n: usize), en_one: "{n} rule", en_other: "{n} rules", zh: "{n} 条");

message!(after(n: u32), en: "pass through after failure {n}", zh: "第 {n} 次失败后透传");

message!(stage(name: &str, stage: &str), en: "format \"{name}\" (stages: {stage}):", zh: "format \"{name}\"（{stage} 阶段）:");

message!(env_names(keys: &str), en: "  env              = {keys} (values hidden)", zh: "  env              = {keys}（值不显示）");

message!(spawn(), en: "  process          = spawn (one process per message)", zh: "  process          = spawn（每条消息起一个进程）");

message!(pool(n: u32, idle: &str), en: "  process          = persistent (pool_max={n}, idle workers: {idle})", zh: "  process          = persistent（进程池 pool_max={n}，空闲 {idle}）");

message!(never(), en: "kept running", zh: "永不回收");

message!(reap(secs: u64), en: "stopped after {secs}s", zh: "{secs}s 后回收");

message!(empty_table(table: &str), en: "{table:<26} = (empty)", zh: "{table:<26} = (空)");

message!(header(name: &str, value: &str, source: &str), en: "  {name} = {value}  ({source})", zh: "  {name} = {value}  （{source}）");

message!(default_missing(path: std::path::Display<'_>), en: "Default configuration file in settings.json does not exist: {path}\nSet a new path with aproxy settings set default_config <path>, or remove the setting with aproxy settings unset default_config.", zh: "settings.json 指定的默认配置文件不存在: {path}\n用 aproxy settings set default_config <路径> 重新指定，或 aproxy settings unset default_config 取消");

message!(chain_precedence(), en: "The effective value comes first. A running instance's start options, kept in its launch spec, take precedence over these layers and are not shown here.", zh: "每个键先列生效值。运行中实例的 start 选项保存在启动规格里，优先于以下各层，这里不列出。");

message!(instance_only(), en: " — per instance, not inherited", zh: " —— 属于单个实例，不继承");

message!(lower_layer(source: &str, value: &str), en: "    {source} set {value} (shadowed)", zh: "    {source} 设为 {value}（被上层覆盖）");

message!(no_formats(), en: "This configuration has no format programs.", zh: "这份配置没有 format 程序。");

message!(format_entry(name: &str), en: "format {name}:", zh: "format {name}：");

message!(stage_order(), en: "Programs by stage:", zh: "各阶段的程序：");

message!(observers(names: &str), en: "notify: {names} (observe only)", zh: "notify: {names}（只观察）");

message!(no_stage_program(stages: &str), en: "{stages}: no program", zh: "{stages}: 没有程序");

message!(first_answer(field: &str, names: &str), en: "{field} — first program to give a value: {names}", zh: "{field} —— 取第一个给出值的程序：{names}");

message!(last_upstream(name: &str), en: "request: changes accumulate; {name} is the last layer before upstream.", zh: "request：内容逐层传递，最后由 {name} 处理，再发往上游。");

message!(last_client(stage: &str, name: &str), en: "{stage}: changes accumulate; {name} is the last layer before the client.", zh: "{stage}：内容逐层传递，最后由 {name} 处理，再回给客户端。");

message!(retry_entries(retry: &str, first: &str), en: "Request programs called again when the retry segment runs: {retry}\nRequest programs on the first attempt only: {first}", zh: "重试段执行时再次调用的 request 程序：{retry}\n只在首次尝试调用的 request 程序：{first}");

message!(optional_entries(names: &str), en: "Skipped on failure or while their breaker is open (required = false): {names}", zh: "执行失败或熔断期间跳过的程序（required = false）：{names}");

message!(format_limits(), en: "Programs declare stages, not what they do to the content. This view cannot tell which protocol a request ends up in. For an actual request, see Protocol path in the desktop request detail and aproxy logs.", zh: "程序声明参与哪些阶段，不声明怎样修改内容。这里看不出请求最终使用哪种协议。要看一条请求实际走了哪条路，请看桌面请求详情的「协议路径」和 aproxy logs。");
