//! `aproxy config [选项]`：查看或修改配置文件与默认配置指向。

use std::path::PathBuf;

use aproxy::config;
use aproxy::config::mask_base_url;
use aproxy::settings;

use crate::cli::ConfigArgs;
use crate::util::{mask_proxy_url, mask_secret, parse_kv};

/// 展示前守卫：配置文件存在但解析失败时 load() 会静默回退默认值，`--show` 打印的
/// 「当前配置」并非用户文件的真实内容——至少要警告，避免误导排障。
fn warn_if_config_broken(path: &std::path::Path) {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Err(e) = toml::from_str::<aproxy::config::Config>(&content)
    {
        eprintln!("警告: 配置文件解析失败，以下展示的是回退默认值而非文件内容");
        eprintln!("解析错误: {e}");
    }
}

pub(crate) fn handle_config_cmd(path: PathBuf, args: ConfigArgs) {
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
        let mut s = settings::load();
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

    let mut cfg = config::load_from(&path);

    let mut changed = false;
    if let Some(u) = baseurl {
        cfg.base_url = u.trim_end_matches('/').to_string();
        changed = true;
    }
    if let Some(l) = listen {
        cfg.listen_addr = l;
        changed = true;
    }
    if clear_api_key {
        cfg.api_key = None;
        changed = true;
    } else if let Some(k) = api_key {
        if k.trim().is_empty() {
            eprintln!("api_key 不能为空（用 --clear-api-key 清空）");
            std::process::exit(1);
        }
        cfg.api_key = Some(k.trim().to_string());
        changed = true;
    }
    if clear_headers {
        cfg.extra_headers.clear();
        cfg.override_headers.clear();
        changed = true;
    }
    for kv in extra_headers {
        let Some((k, v)) = parse_kv(&kv) else {
            eprintln!("extra-header 格式错误，应为 key=value，得到: {kv}");
            std::process::exit(1);
        };
        cfg.extra_headers.insert(k, v);
        changed = true;
    }
    for kv in override_headers {
        let Some((k, v)) = parse_kv(&kv) else {
            eprintln!("override-header 格式错误，应为 key=value，得到: {kv}");
            std::process::exit(1);
        };
        cfg.override_headers.insert(k, v);
        changed = true;
    }
    if let Some(s) = keepalive_secs {
        cfg.keepalive_interval_secs = s;
        changed = true;
    }
    if clear_proxy {
        cfg.proxy = None;
        cfg.proxy_username = None;
        cfg.proxy_password = None;
        changed = true;
    } else if let Some(p) = proxy {
        if p.trim().is_empty() {
            eprintln!("proxy 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy = Some(p.trim().to_string());
        changed = true;
    }
    if let Some(u) = proxy_username {
        if u.trim().is_empty() {
            eprintln!("proxy-username 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy_username = Some(u.trim().to_string());
        changed = true;
    }
    if let Some(p) = proxy_password {
        if p.trim().is_empty() {
            eprintln!("proxy-password 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy_password = Some(p.trim().to_string());
        changed = true;
    }

    if changed {
        // 保存前守卫：配置文件存在但解析失败时 load() 会静默回退默认值，
        // 直接保存会把用户手写（或损坏）的配置整个覆盖掉——拒绝修改并报告解析错误。
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Err(e) = toml::from_str::<aproxy::config::Config>(&content) {
                        eprintln!(
                            "现有配置文件解析失败，拒绝覆盖（请先修复或删除 {}）",
                            path.display()
                        );
                        eprintln!("解析错误: {e}");
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("读取配置文件失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        cfg = cfg.normalized();
        // 保存前校验已设置的 base_url 格式（允许为空——首次配置可分多次完成，中间态
        // 合法）；否则 query 形式的 base_url 会被存盘，直到下次启动才报错
        if !cfg.base_url.is_empty()
            && let Err(msg) = cfg.validate_base_url()
        {
            eprintln!("base_url 无效，未保存: {msg}");
            std::process::exit(1);
        }
        match config::save_to(&path, &cfg) {
            Ok(()) => println!("已保存配置到 {}", path.display()),
            Err(e) => {
                eprintln!("保存配置失败: {e}");
                std::process::exit(1);
            }
        }
    }

    if show || !changed {
        // 重新加载以展示最终值
        warn_if_config_broken(&path);
        let cfg = config::load_from(&path).normalized();
        println!("配置文件: {}", path.display());
        println!("base_url     = \"{}\"", mask_base_url(&cfg.base_url));
        println!("listen_addr  = \"{}\"", cfg.listen_addr);
        println!(
            "api_key      = {}",
            cfg.api_key
                .as_deref()
                .map(mask_secret)
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!("keepalive_interval_secs = {}", cfg.keepalive_interval_secs);
        // 0 表示所有重试零延迟，语义特殊，提示出来
        if cfg.max_retry_backoff_secs == 0 {
            println!("max_retry_backoff_secs = 0（所有重试零延迟）");
        } else {
            println!("max_retry_backoff_secs = {}", cfg.max_retry_backoff_secs);
        }
        println!("spool_limit_mb          = {}", cfg.spool_limit_mb);
        // max_body_mb / disk_cache：None = toml 未显式配置（运行时由 settings
        // 注入），展示生效值更利于排障——但此处 cfg 未经启动注入，注明来源
        match cfg.max_body_mb {
            Some(0) => println!("max_body_mb             = 0（不设限）"),
            Some(mb) => println!("max_body_mb             = {mb}"),
            None => println!(
                "max_body_mb             = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
        match cfg.disk_cache {
            Some(dc) => println!("disk_cache              = {dc}"),
            None => println!(
                "disk_cache              = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
        // 0 表示不设限，语义特殊，提示出来
        if cfg.connect_timeout_secs == 0 {
            println!("connect_timeout_secs    = 0（不设限）");
        } else {
            println!("connect_timeout_secs    = {}", cfg.connect_timeout_secs);
        }
        if cfg.read_timeout_secs == 0 {
            println!("read_timeout_secs       = 0（不设限）");
        } else {
            println!("read_timeout_secs       = {}", cfg.read_timeout_secs);
        }
        println!(
            "proxy          = {}",
            cfg.proxy
                .as_deref()
                .map(mask_proxy_url)
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!(
            "proxy_username = {}",
            cfg.proxy_username.as_deref().unwrap_or("(未设置)")
        );
        println!(
            "proxy_password = {}",
            cfg.proxy_password
                .as_deref()
                .map(|_| "***".to_string())
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        if cfg.extra_headers.is_empty() {
            println!("extra_headers    = (空)");
        } else {
            println!("extra_headers:");
            for (k, v) in &cfg.extra_headers {
                // 头值常含完整鉴权令牌，与 api_key 一致做截断打码
                println!("  {k} = \"{}\"", mask_secret(v));
            }
        }
        if cfg.override_headers.is_empty() {
            println!("override_headers = (空)");
        } else {
            println!("override_headers:");
            for (k, v) in &cfg.override_headers {
                println!("  {k} = \"{}\"", mask_secret(v));
            }
        }
        if cfg.base_url.is_empty() {
            println!();
            println!("提示: base_url 为空，请设置:");
            println!("  aproxy config --baseurl https://api.anthropic.com");
        }
    }
}
