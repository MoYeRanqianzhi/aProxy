//! 设置编辑和别名校验的原因，同一项校验可供终端与固定英文的诊断使用。

use aproxy_ipc::message;

message!(bad_key(key: &str), en: "Invalid key path: {key}. Separate keys with dots, for example watchdog.enabled, aliases.work or defaults.extra_headers.\"x-foo\".", zh: "键路径写法不对: {key}（点号分隔，例如 watchdog.enabled、aliases.work、defaults.extra_headers.\"x-foo\"）");
message!(syntax(path: std::path::Display<'_>, diagnostics: &str), en: "Cannot read settings.json. Nothing was changed. Repair or remove {path} first:{diagnostics}", zh: "settings.json 现在读不懂，没有修改（请先手工修复或删除 {path}）:{diagnostics}");
message!(invalid(diagnostics: &str), en: "This change would invalidate settings.json. Nothing was saved:{diagnostics}", zh: "这次修改会让 settings.json 无效，没有保存:{diagnostics}");
message!(conflict(path: std::path::Display<'_>), en: "Another program is modifying settings.json. Nothing was saved. Close the editor and try again: {path}", zh: "settings.json 一直在被别的程序修改，没有保存（关掉正在编辑它的程序后重试）: {path}");
message!(locked(), en: "Another aProxy process is writing the configuration. The lock was still held after 10 seconds. Nothing was saved; try again later.", zh: "另一个 aproxy 正在写配置，等了 10 秒仍未结束，没有保存（稍后重试）");
message!(save(e: &std::io::Error), en: "Cannot save settings.json: {e}", zh: "保存 settings.json 失败: {e}");
