//! 读写全局设置时的命令诊断；机器读的错误原因明确使用英文。
use aproxy_ipc::message;
message!(read(e: &std::io::Error, path: std::path::Display<'_>), en: "Cannot read settings.json: {e}\nFile: {path}", zh: "读取 settings.json 失败: {e}\n文件: {path}");
message!(invalid_object(), en: "settings.json is not a valid JSON object. Repair it manually or delete it to return to built-in defaults:", zh: "settings.json 不是合法的 JSON 对象，只能手工修复（或删除它，回到内置默认）:");
message!(not_json(key: &str, e: &serde_json::Error), en: "Write the value of {key} as JSON, for example true, 30, \"text\", [\"a\"], or {{\"k\":1}}. Parse error: {e}", zh: "{key} 的值要写成 JSON（例如 true、30、\"text\"、[\"a\"]、{{\"k\":1}}），解析失败: {e}");
