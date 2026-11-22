//! 发现配置时显示的摘要、来源和列表行，不参与 JSON 文档的生成。

use aproxy_ipc::message;

message!(empty(), en: "No configuration files found. See config_dirs in settings.json (defaults: ~/.aproxy/ and ~/.aproxy/configs/).", zh: "没有找到配置文件。配置目录列表见 settings.json 的 config_dirs（默认 ~/.aproxy/ 与 ~/.aproxy/configs/）。");

message!(found(n: usize), en_one: "Found {n} configuration:", en_other: "Found {n} configurations:", zh: "找到 {n} 个配置:");

message!(aliases(names: &str), en: "  Aliases: {names}", zh: "  别名: {names}");

message!(default(), en: "  [default]", zh: "  [默认]");

message!(valid(path: std::path::Display<'_>, port: &str, url: &str, aliases: &str, default: &str), en: "  {path}  port {port}  upstream {url}{aliases}{default}", zh: "  {path}  端口 {port}  上游 {url}{aliases}{default}");

message!(invalid(path: std::path::Display<'_>, alias_part: &str, default_part: &str), en: "  {path}  (not a valid aProxy configuration){alias_part}{default_part}", zh: "  {path}  （无法解析为有效 aProxy 配置）{alias_part}{default_part}");
