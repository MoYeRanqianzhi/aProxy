//! 配置别名的终端输出。文件路径和别名由操作层提供，不在翻译中改写。

use aproxy_ipc::message;

message!(updated(name: &str, path: std::path::Display<'_>), en: "Updated alias {name} -> {path}", zh: "已更新别名 {name} -> {path}");

message!(added(name: &str, path: std::path::Display<'_>), en: "Added alias {name} -> {path}", zh: "已添加别名 {name} -> {path}");

message!(usage(name: &str), en: "Start: aproxy start {name}    Stop: aproxy stop {name}", zh: "启动: aproxy start {name}    停止: aproxy stop {name}");

message!(removed(name: &str, path: &str), en: "Removed alias {name} (was {path})", zh: "已删除别名 {name}（原指向 {path}）");

message!(empty(), en: "No aliases. To add one, run aproxy alias add <name> <config.toml path>.", zh: "暂无别名。添加: aproxy alias add <名称> <config.toml 路径>");

message!(count(n: usize), en: "Aliases ({n}):", zh: "别名 ({n}):");

message!(invalid(reason: &str), en: "Invalid alias: {reason}", zh: "别名无效: {reason}");

message!(not_found(name: &str), en: "Alias {name} does not exist. Run aproxy alias list to see available aliases.", zh: "别名 {name} 不存在。用 aproxy alias list 查看。");

message!(invalid_json(), en: "not a valid JSON object", zh: "不是合法的 JSON 对象");
message!(cannot_edit(reason: &str, path: std::path::Display<'_>), en: "Cannot read settings.json ({reason}). Nothing was changed. Repair or remove the file first: {path}", zh: "settings.json 读不懂（{reason}），本次不做修改。请先修复或删除该文件: {path}");
