//! `aproxy config [选项]`：查看或修改配置文件与默认配置指向。

use std::path::PathBuf;

use aproxy::config::edit::{Change, EditError};
use aproxy::config::{self, Source};
use aproxy::settings;

use crate::util::{mask_secret, parse_kv};
use aproxy::cli::ConfigArgs;

/// 转换器 args 里「后一个参数是密钥」的旗标（比较时大小写不敏感）。format
/// 程序常见的传 key 写法：`--api-key sk-...` 或 `--api-key=sk-...`。
const SECRET_ARG_FLAGS: &[&str] = &[
    "--api-key",
    "--api_key",
    "--apikey",
    "--key",
    "--token",
    "--secret",
    "--password",
];

/// 转换器 args 的展示打码：密钥旗标之后的值（或 `--flag=值` 的值）走
/// mask_secret；其余参数走 URL 脱敏出口（参数里出现的 `user:pass@host`、
/// `?key=` 一并遮掉，普通参数原样）。
fn mask_transform_args(args: &[String]) -> Vec<String> {
    let is_secret_flag = |s: &str| SECRET_ARG_FLAGS.iter().any(|f| f.eq_ignore_ascii_case(s));
    let mut out = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for arg in args {
        if mask_next {
            out.push(mask_secret(arg));
            mask_next = false;
        } else if is_secret_flag(arg) {
            out.push(arg.clone());
            mask_next = true;
        } else if let Some((flag, value)) = arg.split_once('=')
            && is_secret_flag(flag)
        {
            out.push(format!("{flag}={}", mask_secret(value)));
        } else {
            out.push(aproxy_ipc::redact::url(arg));
        }
    }
    out
}

pub(crate) fn handle_config_cmd(
    path: Result<PathBuf, String>,
    args: ConfigArgs,
    explicit_config: bool,
) {
    let ConfigArgs {
        baseurl,
        listen,
        api_key,
        extra_headers,
        override_headers,
        keepalive_secs,
        proxy,
        proxy_username,
        proxy_password,
        clear_api_key,
        clear_headers,
        clear_proxy,
        unset,
        set_default,
        clear_default,
        show,
    } = args;

    // 默认配置文件指向存于 settings.json（内部配置），与 config.toml 内容无关，
    // 独立处理后再进入 toml 内容编辑流程
    if set_default.is_some() || clear_default {
        if set_default.is_some() && clear_default {
            eprintln!("--set-default 与 --clear-default 不能同时使用");
            std::process::exit(1);
        }
        let mut s = settings::load_for_update().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
        if let Some(p) = set_default {
            let abs = settings::expand_path(&p);
            if !abs.is_file() {
                eprintln!("配置文件不存在: {}", abs.display());
                std::process::exit(1);
            }
            s.default_config = Some(abs.display().to_string());
            match settings::save(&s) {
                Ok(()) => println!("已把 {} 设为默认配置文件", abs.display()),
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            s.default_config = None;
            match settings::save(&s) {
                Ok(()) => println!("已取消默认配置文件设置（恢复用 ~/.aproxy/config.toml）"),
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    // 以下编辑/查看 toml 内容，才真正需要配置文件路径
    let path = super::require_cfg_path(path);
    let fail = |msg: String| -> ! {
        eprintln!("{msg}");
        crate::util::exit(1)
    };
    let non_empty = |value: String, name: &str, clear: &str| -> String {
        let value = value.trim().to_string();
        if value.is_empty() {
            fail(format!("{name} 不能为空（用 {clear} 清空）"));
        }
        value
    };

    let mut changes: Vec<Change> = Vec::new();
    if let Some(u) = baseurl {
        changes.push(Change::set("base_url", u.trim().trim_end_matches('/')));
    }
    if let Some(l) = listen {
        changes.push(Change::set("listen_addr", l.trim()));
    }
    if clear_api_key {
        changes.push(Change::unset("api_key"));
    } else if let Some(k) = api_key {
        changes.push(Change::set(
            "api_key",
            non_empty(k, "api_key", "--clear-api-key"),
        ));
    }
    if clear_headers {
        changes.push(Change::unset("extra_headers"));
        changes.push(Change::unset("override_headers"));
    }
    for (table, flag, list) in [
        ("extra_headers", "extra-header", extra_headers),
        ("override_headers", "override-header", override_headers),
    ] {
        for kv in list {
            let Some((k, v)) = parse_kv(&kv) else {
                fail(format!("{flag} 格式错误，应为 key=value，得到: {kv}"));
            };
            changes.push(Change::set(
                &format!("{table}.{}", config::quote_key(&k)),
                v,
            ));
        }
    }
    if let Some(s) = keepalive_secs {
        changes.push(Change::set("keepalive.interval_secs", s as i64));
    }
    if clear_proxy {
        for key in ["proxy", "proxy_username", "proxy_password"] {
            changes.push(Change::unset(key));
        }
    } else if let Some(p) = proxy {
        changes.push(Change::set("proxy", non_empty(p, "proxy", "--clear-proxy")));
    }
    if let Some(u) = proxy_username {
        changes.push(Change::set(
            "proxy_username",
            non_empty(u, "proxy-username", "--clear-proxy"),
        ));
    }
    if let Some(p) = proxy_password {
        changes.push(Change::set(
            "proxy_password",
            non_empty(p, "proxy-password", "--clear-proxy"),
        ));
    }
    for key in unset {
        changes.push(Change::unset(&key));
    }

    let defaults =
        config::resolve::Defaults::from_settings(&settings::load(), &settings::settings_path());
    let changed = !changes.is_empty();
    if changed {
        // 只改这几个键：文件里的注释、键的顺序与其余内容原样保留。会引入新错误的改动
        // 不写入；文件里原有的、与这次改动无关的错误不挡路，可以逐项修
        match config::edit::apply(&path, &aproxy_ipc::home::home(), &defaults, &changes) {
            Ok(applied) if applied.changed => println!("已保存配置到 {}", applied.file.display()),
            Ok(applied) => println!("配置没有变化: {}", applied.file.display()),
            Err(EditError::BadKey(key)) => {
                eprintln!(
                    "键路径写法不对: {key}（点号分隔，例如 timeouts.connect_secs、extra_headers.\"x-foo\"；数组表只能整体删除）"
                );
                crate::util::exit(aproxy_ipc::code::EXIT_USAGE as i32)
            }
            Err(EditError::Syntax(diagnostics)) => {
                eprintln!(
                    "现有配置文件有语法错误，没有修改（请先修复或删除 {}）:",
                    path.display()
                );
                print_diagnostics(&diagnostics);
                crate::util::exit(aproxy_ipc::code::EXIT_CONFIG as i32)
            }
            Err(EditError::Invalid(diagnostics)) => {
                eprintln!("这次修改会让配置无效，没有保存:");
                print_diagnostics(&diagnostics);
                crate::util::exit(aproxy_ipc::code::EXIT_CONFIG as i32)
            }
            Err(EditError::Conflict) => fail(format!(
                "配置文件一直在被别的程序修改，没有保存（关掉正在编辑它的程序后重试）: {}",
                path.display()
            )),
            Err(EditError::LockBusy) => fail(
                "另一个 aproxy 正在写配置，等了 10 秒仍未结束，没有保存（稍后重试）".to_string(),
            ),
            Err(EditError::Io(e)) => fail(format!("保存配置失败: {e}")),
        }
    }
    if show || !changed {
        // 显式 --config 指向的文件不存在时，展示的只是默认值（抬头却是用户给的路径）——
        // 纯粹的排障误导，报错而非静默回退。写操作不受此限：--config 指向新路径 + 修改
        // 参数是「创建新实例配置」的合法入口（多开工作流）；默认路径豁免：首配场景就是
        // 在尚不存在的 config.toml 上创建。
        if explicit_config && !path.is_file() {
            fail(format!("指定的配置文件不存在: {}", path.display()));
        }
        let resolved = match config::resolve(&path, &config::Overrides::default(), &defaults) {
            Ok(resolved) => resolved,
            Err(e) => fail(format!("读取配置文件失败: {e}\n文件: {}", path.display())),
        };
        print_effective(&path, &resolved);
        if !resolved.is_valid() {
            // 有 error 时照样显示，附上诊断；这份配置现在启动不了
            eprintln!();
            eprintln!("这份配置有错误，现在不能用来启动:");
            let errors: Vec<_> = resolved.errors().cloned().collect();
            print_diagnostics(&errors);
            if resolved.config.base_url.is_empty() {
                eprintln!();
                eprintln!("提示: base_url 为空，请设置:");
                eprintln!("  aproxy config --baseurl https://api.anthropic.com");
            }
            let unknown: Vec<&str> = errors
                .iter()
                .filter(|d| d.code == "config_unknown_key")
                .filter_map(|d| d.key.as_deref())
                .collect();
            if !unknown.is_empty() {
                eprintln!();
                eprintln!("不认识的键可以这样删除:");
                for key in unknown {
                    eprintln!("  aproxy config --unset '{key}'");
                }
            }
            crate::util::exit(aproxy_ipc::code::EXIT_CONFIG as i32)
        }
    }
}

/// 逐条打印诊断（stderr）：`文件:行:列: 键: 说明 [码]`。
fn print_diagnostics(diagnostics: &[aproxy_ipc::diag::Diagnostic]) {
    for d in diagnostics {
        eprintln!("  {}", aproxy::ops::start::render_diagnostic(d));
    }
}

/// 来源的中文说法（展示在每个值后面）。
fn source_label(source: Option<Source>) -> &'static str {
    match source {
        Some(Source::Override) => "命令行",
        Some(Source::Toml) => "config.toml",
        Some(Source::Settings) => "settings.json 全局默认",
        Some(Source::Env) => "环境变量",
        Some(Source::Default) => "内置默认",
        None => "未设置",
    }
}

/// 打印一份配置的生效值：每个键一行，后面注明它来自哪一层。密钥只显示打码后的样子。
fn print_effective(path: &std::path::Path, resolved: &config::Resolved) {
    let cfg = &resolved.config;
    let line = |key: &str, value: String| {
        println!(
            "{key:<26} = {value}  （{}）",
            source_label(resolved.sources.get(key).copied())
        );
    };
    let quoted = |s: &str| format!("\"{s}\"");
    let optional = |v: Option<String>| v.unwrap_or_else(|| "(未设置)".to_string());
    // 0 在这几个键上有特别的含义，写在值后面
    let with_zero = |n: u64, zero: &str| {
        if n == 0 {
            format!("0（{zero}）")
        } else {
            n.to_string()
        }
    };
    let list = |items: &[String], empty: &str| {
        if items.is_empty() {
            format!("[]（{empty}）")
        } else {
            // 原样列出（不用 {:?}：Debug 会把正则里的 \? 写成 \\?，对照 toml 会被误导）
            format!("[{}]", items.join(", "))
        }
    };

    println!("配置文件: {}", path.display());
    line("base_url", quoted(&aproxy_ipc::redact::url(&cfg.base_url)));
    line("listen_addr", quoted(&cfg.listen_addr));
    line(
        "api_key",
        optional(cfg.api_key.as_deref().map(|k| quoted(&mask_secret(k)))),
    );
    line(
        "delivery",
        if cfg.forward_only_enabled() {
            "\"forward\"（边收边转：不缓冲、不重试）".to_string()
        } else {
            "\"retry\"".to_string()
        },
    );
    line(
        "max_request_mb",
        with_zero(
            cfg.max_body_mb.unwrap_or(config::DEFAULT_MAX_BODY_MB),
            "不设限",
        ),
    );
    line("max_response_mb", with_zero(cfg.spool_limit_mb, "不设限"));
    line("disk_cache", cfg.disk_cache_enabled().to_string());
    line(
        "allowed_hosts",
        list(
            cfg.allowed_hosts(),
            "内置默认：回环监听只放行 localhost / 127.0.0.1 / [::1]",
        ),
    );
    line(
        "allowed_origins",
        list(
            cfg.allowed_origins(),
            "内置默认：拒绝一切携带 Origin 的浏览器请求",
        ),
    );
    line(
        "log_file",
        optional(cfg.log_file.clone())
            + if cfg.log_file.is_none() {
                "（~/.aproxy/logs/ 下按启动随机命名，实际路径看 aproxy status）"
            } else {
                ""
            },
    );
    line(
        "timeouts.connect_secs",
        with_zero(cfg.connect_timeout_secs, "不设限"),
    );
    line(
        "timeouts.first_byte_secs",
        with_zero(cfg.first_byte_timeout_secs, "不设限"),
    );
    line(
        "timeouts.read_idle_secs",
        with_zero(cfg.read_timeout_secs, "不设限"),
    );
    line(
        "keepalive.interval_secs",
        with_zero(cfg.keepalive_interval_secs, "保活关闭"),
    );
    line(
        "keepalive.trigger",
        format!(
            "\"{}\"（accept=看 Accept 头 / body_stream=看请求体 stream:true / any=任一）",
            cfg.keepalive_trigger().as_str()
        ),
    );
    // 心跳字节通常含换行：按字符串字面量形式（{:?}）打印，换行显示为 `\n`，与 toml 里的
    // 写法一一对应
    line(
        "keepalive.heartbeat",
        format!("{:?}", String::from_utf8_lossy(cfg.keepalive_heartbeat())),
    );
    line(
        "retry.max_backoff_secs",
        with_zero(cfg.max_retry_backoff_secs, "所有重试零延迟"),
    );
    line(
        "retry.rules",
        if cfg.retry_rules.is_empty() {
            "[]（一切请求无限重试）".to_string()
        } else {
            format!("{} 条", cfg.retry_rules.len())
        },
    );
    for (index, rule) in cfg.retry_rules.iter().enumerate() {
        let path = rule.path.as_deref().unwrap_or("(任何路径)");
        let status = match &rule.status {
            Some(codes) => codes
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
            None => "(任何状态码)".to_string(),
        };
        let limit = match rule.max_attempts {
            0 => "不限（豁免）".to_string(),
            n => format!("第 {n} 次失败后透传"),
        };
        println!("  {}. path = {path}  status = {status}  {limit}", index + 1);
    }

    // format 程序：请求、响应、心跳各至多一个
    let formats = [
        ("request", &cfg.request_transform),
        ("response", &cfg.response_transform),
        ("heartbeat", &cfg.heartbeat_transform),
    ];
    if formats.iter().all(|(_, t)| t.is_none()) {
        line("format", "[]".to_string());
    }
    for (stage, t) in formats {
        let Some(t) = t else { continue };
        println!("format \"{}\"（{stage} 阶段）:", t.name);
        println!("  command          = \"{}\"", t.command);
        if !t.args.is_empty() {
            // 多 key 轮换类 format 常把 key 直接写在 args 里
            println!(
                "  args             = [{}]",
                mask_transform_args(&t.args).join(", ")
            );
        }
        if !t.env.is_empty() {
            // 环境变量的值只写不读：只列名字
            let names: Vec<&str> = t.env.keys().map(String::as_str).collect();
            println!("  env              = {}（值不显示）", names.join(", "));
        }
        match t.mode {
            config::TransformMode::Spawn => {
                println!("  process          = spawn（每条消息起一个进程）")
            }
            config::TransformMode::Persistent => println!(
                "  process          = persistent（进程池 pool_max={}，空闲 {}）",
                t.effective_pool_max(),
                match t.effective_idle_timeout_secs() {
                    0 => "永不回收".to_string(),
                    secs => format!("{secs}s 后回收"),
                }
            ),
        }
        println!(
            "  timeout_secs     = {}",
            with_zero(t.effective_timeout_secs(), "不限")
        );
        if t.every_attempt {
            println!("  每次重试前用原始请求重新转换（声明了 retry 阶段）");
        }
    }

    line(
        "proxy",
        optional(cfg.proxy.as_deref().map(aproxy_ipc::redact::url)),
    );
    line("proxy_username", optional(cfg.proxy_username.clone()));
    line(
        "proxy_password",
        optional(cfg.proxy_password.as_ref().map(|_| "***".to_string())),
    );
    for (table, headers) in [
        ("extra_headers", &cfg.extra_headers),
        ("override_headers", &cfg.override_headers),
    ] {
        if headers.is_empty() {
            println!("{table:<26} = (空)");
            continue;
        }
        println!("{table}:");
        let mut names: Vec<&String> = headers.keys().collect();
        names.sort();
        for name in names {
            // 头值常含完整鉴权令牌，与 api_key 一致做截断打码
            let source = resolved
                .sources
                .get(&format!("{table}.{}", config::quote_key(name)))
                .copied();
            println!(
                "  {name} = \"{}\"  （{}）",
                mask_secret(&headers[name]),
                source_label(source)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_args_mask_values_after_secret_flags() {
        let args: Vec<String> = [
            "run",
            "--api-key",
            "SUPERSECRET2",
            "--TOKEN=tok-abcdefgh",
            "--config",
            "C:/agg.toml",
            "https://user:pw@h/x?key=Q",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let masked = mask_transform_args(&args);
        assert_eq!(
            masked,
            [
                "run",
                "--api-key",
                "SUPERS***",
                "--TOKEN=tok-ab***",
                "--config",
                "C:/agg.toml",
                "https://***@h/x?key=***",
            ]
        );
        // 旗标在末尾（缺值）不越界，原样保留
        assert_eq!(mask_transform_args(&["--key".to_string()]), ["--key"]);
    }
}
