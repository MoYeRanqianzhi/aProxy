//! `aproxy config [选项]`：查看或修改配置文件与默认配置指向。

use std::path::PathBuf;

use aproxy::config;
use aproxy::config::mask_base_url;
use aproxy::settings;

use crate::cli::ConfigArgs;
use crate::util::{mask_secret, parse_kv};

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
            out.push(mask_base_url(arg));
        }
    }
    out
}

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
            // 错误消息内嵌 base_url 原文（可能带 user:pass），同一脱敏出口
            let msg = msg.replace(&cfg.base_url, &mask_base_url(&cfg.base_url));
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
        // 显式 --config 指向的文件不存在时，展示的只是内置默认值（抬头却是
        // 用户给的路径）——纯粹的排障误导，报错而非静默回退。写操作不受此限：
        // --config 指向新路径 + 修改参数是「创建新实例配置」的合法入口（多开
        // 工作流）；默认路径豁免：首配场景就是在尚不存在的 config.toml 上创建。
        if explicit_config && !path.is_file() {
            eprintln!("指定的配置文件不存在: {}", path.display());
            std::process::exit(1);
        }
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
        // keepalive_trigger：同 max_body_mb 等的三态来源展示；原值原样打印（含
        // 非法写法——此处不校验，start/doctor 会点名报错，展示真实内容便于对照）
        match &cfg.keepalive_trigger {
            Some(t) => println!(
                "keepalive_trigger       = {t}（accept=看 Accept 头 / body_stream=看请求体 stream:true / any=任一）"
            ),
            None => println!(
                "keepalive_trigger       = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
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
        // forward_only：None 与 Some 一起展示来源；true 必须把「放弃重试」
        // 写在脸上——它不是一个无害的性能开关
        match cfg.forward_only {
            Some(true) => {
                println!("forward_only            = true（仅转发：不缓冲、不重试）")
            }
            Some(false) => println!("forward_only            = false"),
            None => println!(
                "forward_only            = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
        // bounded_retry_paths：与上面三个 Option 字段同款的三态展示；空列表
        // 显式标注「功能关闭」，有值时**原样**列出模式（不能用 {:?}——Debug
        // 会把 \? 转义成 \\?，用户对照 toml 会被误导）
        match &cfg.bounded_retry_paths {
            Some(list) if list.is_empty() => {
                println!("bounded_retry_paths     = []（功能关闭）")
            }
            Some(list) => {
                println!(
                    "bounded_retry_paths     = [{}]（命中者失败 3 次即透传）",
                    list.join(", ")
                )
            }
            None => println!(
                "bounded_retry_paths     = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
        // 入站白名单：同款三态展示；空列表注明它等于内置默认策略（不是「全拒」
        // 也不是「全放」，两种误读都会把排障带偏）
        match &cfg.allowed_hosts {
            Some(list) if list.is_empty() => println!(
                "allowed_hosts           = []（内置默认：回环监听只放行 localhost / 127.0.0.1 / [::1]）"
            ),
            Some(list) => println!("allowed_hosts           = [{}]", list.join(", ")),
            None => println!(
                "allowed_hosts           = （未在 toml 设置，运行时取 settings.json 全局默认）"
            ),
        }
        match &cfg.allowed_origins {
            Some(list) if list.is_empty() => println!(
                "allowed_origins         = []（内置默认：拒绝一切携带 Origin 的浏览器请求）"
            ),
            Some(list) => println!("allowed_origins         = [{}]", list.join(", ")),
            None => println!(
                "allowed_origins         = （未在 toml 设置，运行时取 settings.json 全局默认）"
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
        match &cfg.log_file {
            Some(f) => println!(
                "log_file                = {f}（自定义守护日志文件；CLI --log-file 可覆盖）"
            ),
            None => println!(
                "log_file                = (未设)（内置: ~/.aproxy/logs/ 下按启动随机命名，实际路径以 aproxy status/logs 经 IPC 获取为准）"
            ),
        }
        // 外部转换器（request_transform/response_transform）：逐字段展示便于
        // 排障对照 toml；两字段无 settings 全局默认层，未设置即「(未设置)」
        for (name, t) in [
            ("request_transform", &cfg.request_transform),
            ("response_transform", &cfg.response_transform),
        ] {
            let Some(t) = t else {
                println!("{name} = (未设置)");
                continue;
            };
            println!("{name}:");
            println!("  command           = \"{}\"", t.command);
            if !t.args.is_empty() {
                // 多 key 轮换类 format 常把 key 直接写在 args 里
                println!(
                    "  args              = [{}]",
                    mask_transform_args(&t.args).join(", ")
                );
            }
            match t.mode {
                aproxy::config::TransformMode::Spawn => {
                    println!("  mode              = spawn（每请求一次性进程）")
                }
                aproxy::config::TransformMode::Persistent => {
                    println!(
                        "  mode              = persistent（进程池 pool_max={}，空闲 {}s 后回收{}）",
                        t.effective_pool_max(),
                        t.effective_idle_timeout_secs(),
                        if t.effective_idle_timeout_secs() == 0 {
                            "（永不回收）"
                        } else {
                            ""
                        }
                    );
                }
            }
            println!(
                "  timeout_secs      = {}{}",
                t.effective_timeout_secs(),
                if t.effective_timeout_secs() == 0 {
                    "（不限）"
                } else {
                    ""
                }
            );
            // extra 格式由 format 自定，官方示例就用它内联多个 key（JSON 列表）
            // ——无法逐字段识别，整体按密钥打码，只留前缀与长度供对照 toml
            match &t.extra {
                Some(e) => println!(
                    "  extra             = {}（已打码，共 {} 字符；原样透传进信封）",
                    mask_secret(e),
                    e.chars().count()
                ),
                None => println!("  extra             = (未设置)"),
            }
        }
        println!(
            "proxy          = {}",
            cfg.proxy
                .as_deref()
                .map(mask_base_url)
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
