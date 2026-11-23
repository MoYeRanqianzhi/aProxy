//! 代理读取文件与离线检查使用同一份严格解析器；分层默认值仍由代理负责。
pub use aproxy_ipc::config_file::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aproxy_ipc::diag::{Diagnostic, Level};
    use std::path::Path;

    fn parse_text(text: &str) -> (FileConfig, Vec<Diagnostic>) {
        parse(text, Path::new("config.toml"))
    }

    /// Windows 的批处理不用隐式 cmd 启动；显式解释器作为真正的子进程不受这个限制。
    #[cfg(windows)]
    #[test]
    fn windows_format_batch_targets_are_rejected_at_validation() {
        for command in [
            "tool.cmd",
            "TOOL.BaT",
            "tool.CMD.",
            r"scripts\.cmd",
            r"scripts\.BAT",
        ] {
            let (_, d) = parse_text(&format!(
                "[[format]]\ncommand='{command}'\nstages=['request']"
            ));
            assert!(d.iter().any(|d| {
                d.code == "config_invalid_value"
                    && d.message
                        .to_string()
                        .contains("interpreter in command and the script in args")
            }));
        }
        assert!(
            parse_text(
                "[[format]]\ncommand='interpreter.exe'\nargs=['script']\nstages=['request']"
            )
            .1
            .is_empty()
        );
    }
    /// 解析出的诊断，压成 `码 键路径 行号` 方便比对
    fn brief(diagnostics: &[Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{} {} {}",
                    d.code,
                    d.key.as_deref().unwrap_or("-"),
                    d.line.map_or("-".to_string(), |l| l.to_string())
                )
            })
            .collect()
    }

    /// 设计稿 §7.2 的样例
    const SAMPLE: &str = r#"base_url = "https://relay.example.com"
# delivery 用默认的 "retry"

[timeouts]
first_byte_secs = 600          # 长时间的非流式生成

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false
pool_max = 1
worker_idle_secs = 0

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
"#;

    #[test]
    fn the_documented_sample_parses() {
        let (config, diagnostics) = parse_text(SAMPLE);
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(config.delivery, None, "没写的键是 None，不补默认值");
        assert_eq!(config.timeouts.first_byte_secs, Some(600));
        assert_eq!(config.timeouts.connect_secs, None);
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: Some(r"/v1/messages/count_tokens(\?.*)?".into()),
                status: None,
                max_attempts: 3,
            }])
        );
        let formats = config.format.unwrap();
        assert_eq!(
            formats.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["adaptive", "convert", "keys"]
        );
        let adaptive = &formats[0];
        assert_eq!(adaptive.process, Process::Persistent);
        assert_eq!(
            adaptive.stages,
            [Stage::Request, Stage::Decide, Stage::Notify]
        );
        assert!(!adaptive.required);
        assert_eq!((adaptive.pool_max, adaptive.worker_idle_secs), (1, 0));
        // 没写的键取默认值
        let convert = &formats[1];
        assert!(convert.required);
        assert_eq!(convert.pool_max, DEFAULT_FORMAT_POOL_MAX);
        assert_eq!(convert.timeout_secs, DEFAULT_FORMAT_TIMEOUT_SECS);
        assert_eq!(convert.breaker_failures, DEFAULT_FORMAT_BREAKER_FAILURES);
    }

    #[test]
    fn every_key_of_the_table_is_read() {
        let (config, diagnostics) = parse_text(
            r#"
base_url = "https://api.example.com"
listen_addr = "127.0.0.1:12346"
api_key = "sk-x"
proxy = "http://127.0.0.1:7890"
proxy_username = "u"
proxy_password = "p"
max_request_mb = 64
max_response_mb = 0
disk_cache = false
delivery = "forward"
allowed_hosts = ["my-host"]
allowed_origins = ["http://localhost:5173"]
log_file = "logs/a.log"
keepalive = { interval_secs = 10, trigger = "accept", heartbeat = ": ping\n\n" }

[extra_headers]
anthropic-version = "2023-06-01"

[override_headers]
"x-api-key" = "k"

[timeouts]
connect_secs = 5
read_idle_secs = 120

[retry]
max_backoff_secs = 60
rules = [{ status = [404, 400], max_attempts = 0 }]

[history]
body_budget_mb = 8
"#,
        );
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(config.listen_addr.as_deref(), Some("127.0.0.1:12346"));
        assert_eq!(config.max_response_mb, Some(0));
        assert_eq!(config.disk_cache, Some(false));
        assert_eq!(config.delivery, Some(Delivery::Forward));
        assert_eq!(config.allowed_hosts, Some(vec!["my-host".to_string()]));
        assert_eq!(
            config.extra_headers.unwrap()["anthropic-version"],
            "2023-06-01"
        );
        assert_eq!(config.override_headers.unwrap()["x-api-key"], "k");
        assert_eq!(config.timeouts.connect_secs, Some(5));
        assert_eq!(config.timeouts.read_idle_secs, Some(120));
        // 内联表
        assert_eq!(config.keepalive.interval_secs, Some(10));
        assert_eq!(config.keepalive.trigger, Some(KeepaliveTrigger::Accept));
        assert_eq!(config.keepalive.heartbeat.as_deref(), Some(": ping\n\n"));
        assert_eq!(config.retry.max_backoff_secs, Some(60));
        // 内联表的数组也是规则；0 = 不限
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: None,
                status: Some(vec![404, 400]),
                max_attempts: 0,
            }])
        );
        assert_eq!(config.history.body_budget_mb, Some(8));
    }

    /// 不认识的键一律是 error，带键路径与行号——顶层、子表、规则与 format 条目里都一样。
    /// 0.1.x 的旧键也只是「不认识」。
    #[test]
    fn unknown_keys_are_errors_everywhere() {
        let (_, diagnostics) = parse_text(
            r#"base_url = "https://api.example.com"
forward_only = true
max_body_mb = 1

[timeouts]
total_secs = 1

[keepalive]
enabled = true

[[retry.rules]]
max_attempts = 3
header = "x"

[[format]]
command = "f"
stages = ["request"]
mode = "persistent"

[request_transform]
command = "f"
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_unknown_key timeouts.total_secs 6",
                "config_unknown_key keepalive.enabled 9",
                "config_unknown_key retry.rules[0].header 13",
                "config_unknown_key format[0].mode 18",
                "config_unknown_key forward_only 2",
                "config_unknown_key max_body_mb 3",
                "config_unknown_key request_transform 20",
            ]
        );
        assert!(
            diagnostics.iter().all(|d| d.severity == Level::Error
                && d.file == "config.toml"
                && d.column.is_some())
        );
        // 普通键从行首起；表头里的键在方括号之后
        assert_eq!(diagnostics[4].column, Some(1));
        assert_eq!(diagnostics[6].column, Some(2));
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let (config, diagnostics) = parse_text("base_url = \"x\"\nlisten_addr = \n");
        assert_eq!(config, FileConfig::default());
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!((d.code, d.key.as_deref()), ("config_syntax", None));
        assert_eq!(d.line, Some(2));
        assert!(d.column.is_some());
    }

    #[test]
    fn wrong_types_and_values() {
        let (config, diagnostics) = parse_text(
            r#"base_url = 1
max_request_mb = "big"
max_response_mb = -1
disk_cache = "yes"
delivery = "fast"
allowed_hosts = "host"
extra_headers = { a = 1 }
timeouts = 5

[keepalive]
trigger = "always"
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_type base_url 1",
                "config_type extra_headers 7",
                "config_type max_request_mb 2",
                "config_invalid_value max_response_mb 3",
                "config_type disk_cache 4",
                "config_invalid_value delivery 5",
                "config_type allowed_hosts 6",
                "config_type timeouts 8",
                "config_invalid_value keepalive.trigger 11",
            ]
        );
        // 读不出来的键都是 None，不带进半对半错的值
        assert_eq!(config, FileConfig::default());
        let always = diagnostics.last().unwrap();
        assert_eq!(always.details["reason"], "per_request_only");
    }

    #[test]
    fn retry_rules_are_all_or_nothing() {
        let (config, diagnostics) = parse_text(
            r#"
[[retry.rules]]
path = "/ok"
max_attempts = 2

[[retry.rules]]
path = "/missing-max"

[[retry.rules]]
path = "("
max_attempts = 1

[[retry.rules]]
status = [99, 200, 600]
max_attempts = 1

[[retry.rules]]
body = "overloaded"
max_attempts = 1
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_missing_key retry.rules[1].max_attempts 7",
                "config_invalid_value retry.rules[2].path 10",
                "config_invalid_value retry.rules[3].status 14",
                "config_invalid_value retry.rules[3].status 14",
                "config_not_supported retry.rules[4].body 18",
            ]
        );
        // 有问题的规则整条不要：少一个匹配器的规则比用户写的更宽
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: Some("/ok".into()),
                status: None,
                max_attempts: 2,
            }])
        );
    }

    #[test]
    fn format_entries_are_checked() {
        let (config, diagnostics) = parse_text(
            r#"
[[format]]
stages = ["request"]

[[format]]
command = "/opt/tools/keys.exe"
stages = ["request", "admit", "bogus"]

[[format]]
command = "both"
stages = ["response", "stream"]
process = "persistent"

[[format]]
command = "streamer"
stages = ["stream"]

[[format]]
command = "cool"
stages = ["request"]
breaker_cooldown_secs = 0
pool_max = 0
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_missing_key format[0].command 3",
                "config_not_supported format[1].stages 7",
                "config_invalid_value format[1].stages 7",
                "config_conflict format[2].stages 11",
                "config_conflict format[3].process 16",
                "config_invalid_value format[4].pool_max 22",
                "config_invalid_value format[4].breaker_cooldown_secs 21",
            ]
        );
        assert_eq!(config.format, Some(vec![]), "有问题的条目都不要");
    }

    /// 名字会进入日志和每次尝试的决定；长度按 UTF-8 字节而不是字符计，控制符不能被 trim 掩盖。
    #[test]
    fn format_names_are_bounded_at_their_source() {
        let mixed = format!("{}界", "a".repeat(61));
        for (name, valid) in [
            ("a".into(), true),
            ("a".repeat(64), true),
            (mixed.clone(), true),
            ("a".repeat(65), false),
            (format!("{mixed}a"), false),
            (String::new(), false),
            ("x\ny".into(), false),
            ("\tname".into(), false),
            ("x\u{85}y".into(), false),
        ] {
            let value = serde_json::to_string(&name).unwrap();
            let (cfg, diagnostics) = parse_text(&format!(
                "[[format]]\nname={value}\ncommand='tool'\nstages=['request']"
            ));
            let entries = cfg.format.unwrap();
            if valid {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
                assert_eq!(entries[0].name, name);
            } else {
                assert!(entries.is_empty(), "{value}");
                assert_eq!(
                    brief(&diagnostics),
                    ["config_invalid_value format[0].name 2"]
                );
                assert!(diagnostics[0].message.to_string().contains("set name"));
            }
        }
        // 命令本身不受名字长度限制：显式取短名时，不检查没有被采用的文件名主干。
        let command = format!("{}.exe", "x".repeat(65));
        let (cfg, diagnostics) = parse_text(&format!(
            "[[format]]\ncommand='{command}'\nname='short'\nstages=['request']"
        ));
        assert!(diagnostics.is_empty());
        assert_eq!(cfg.format.unwrap()[0].name, "short");
    }

    /// 选择全部条目的关键字不能通过省略名字绕过，普通名字仍区分大小写。
    #[test]
    fn format_selection_reserves_all() {
        for (table, reserved) in [
            ("command='all.exe'", true),
            ("command='tool'\nname=' all '", true),
            ("command='All.exe'", false),
        ] {
            let (_, ds) = parse_text(&format!("[[format]]\n{table}\nstages=['request']"));
            if reserved {
                assert_eq!(ds.len(), 1);
                assert_eq!(ds[0].code, "format_name_reserved");
                assert_eq!(ds[0].details["name"], "all");
            } else {
                assert!(ds.is_empty());
            }
        }
    }

    #[test]
    fn format_names_default_to_the_command_stem_and_must_be_unique() {
        let (config, diagnostics) = parse_text(
            r#"
[[format]]
command = "/opt/tools/aproxy-format.exe"
stages = ["request"]

[[format]]
name = "aproxy-format"
command = "other"
stages = ["response"]
"#,
        );
        let formats = config.format.unwrap();
        assert_eq!(formats[0].name, "aproxy-format");
        assert_eq!(brief(&diagnostics), ["config_conflict format[1].name 7"]);
    }

    /// 请求方向上，声明了 retry 的程序之后的程序也要声明 retry；只参与响应方向的不受限。
    #[test]
    fn formats_after_a_retry_format_must_declare_retry() {
        let (_, diagnostics) = parse_text(
            r#"
[[format]]
name = "route"
command = "f"
stages = ["request", "retry"]

[[format]]
name = "convert"
command = "f"
stages = ["request"]

[[format]]
name = "observe"
command = "f"
process = "persistent"
stages = ["notify"]

[[format]]
name = "keys"
command = "f"
stages = ["request", "retry"]
"#,
        );
        assert_eq!(brief(&diagnostics), ["config_conflict format[1].stages 8"]);
        assert!(diagnostics[0].message.to_string().contains("\"route\""));
    }

    #[test]
    fn locate_finds_keys_in_every_table_form() {
        let text = r#"base_url = "x"
keepalive = { interval_secs = 1 }
timeouts.connect_secs = 5

[extra_headers]
"x.y" = "v"

[[format]]
command = "a"

[[format]]
# 注释
command = "b"
stages = []

[retry]
rules = [{ max_attempts = 1 }, { path = "/x", max_attempts = 2 }]
"#;
        assert_eq!(locate(text, "base_url"), Some((1, 1)));
        assert_eq!(locate(text, "keepalive.interval_secs"), Some((2, 15)));
        assert_eq!(locate(text, "timeouts.connect_secs"), Some((3, 10)));
        assert_eq!(locate(text, "extra_headers.\"x.y\""), Some((6, 1)));
        assert_eq!(locate(text, "format[1].stages"), Some((14, 1)));
        // 没写的键落在条目上；不存在的条目落在数组的键上
        assert_eq!(locate(text, "format[1].process"), Some((13, 1)));
        assert_eq!(locate(text, "format[5].stages"), Some((8, 3)));
        assert_eq!(locate(text, "retry.rules[1].path"), Some((17, 34)));
        assert_eq!(locate(text, "nope"), None);
        assert_eq!(locate("x = ", "x"), None);
    }

    /// 每条诊断的码都在码表的诊断族里。
    #[test]
    fn every_code_is_a_registered_diagnostic_code() {
        let texts = [
            "x = ",
            "unknown = 1",
            "base_url = 1",
            "max_request_mb = -1",
            "[[retry.rules]]\npath = \"/x\"",
            "[[retry.rules]]\nbody = \"x\"\nmax_attempts = 1",
            "[[format]]\ncommand = \"f\"\nstages = [\"response\", \"stream\"]\nprocess = \"persistent\"",
        ];
        for text in texts {
            let (_, diagnostics) = parse_text(text);
            assert!(!diagnostics.is_empty(), "{text}");
            for d in diagnostics {
                let entry =
                    aproxy_ipc::code::lookup(d.code).unwrap_or_else(|| panic!("{}", d.code));
                assert!(
                    entry
                        .families
                        .contains(&aproxy_ipc::code::Family::Diagnostic),
                    "{}",
                    d.code
                );
            }
        }
    }
}
