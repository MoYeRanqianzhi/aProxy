//! aProxy — 本地 API 代理，无限重试保障 agent 工作流不中断。

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use aproxy::config;

#[derive(Parser, Debug)]
#[command(name = "aproxy", version, about = "Local API proxy with infinite retries", long_about = None)]
struct Cli {
    /// 配置文件路径（默认 ~/.aproxy/config.toml）。
    /// 多开不同配置的进程时各自指定，例如：
    /// aproxy --config ~/.aproxy/work.toml 与 aproxy --config ~/.aproxy/personal.toml
    /// （listen_addr 须互不相同）
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,

    /// 上游 API base URL，覆盖配置文件中的 base_url
    #[arg(long, value_name = "URL")]
    baseurl: Option<String>,

    /// 监听地址，覆盖配置文件中的 listen_addr
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    /// 上游代理 URL（仅本次运行生效，不写入配置），覆盖配置文件中的 proxy
    #[arg(long, value_name = "URL")]
    proxy: Option<String>,

    /// 快捷 api_key（等效覆盖 Authorization: Bearer <key>，仅本次运行生效），覆盖配置文件中的 api_key
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 查看或修改配置（配置文件位于 ~/.aproxy/config.toml）
    Config {
        /// 设置上游 base URL，例如 https://api.anthropic.com
        #[arg(long, value_name = "URL")]
        baseurl: Option<String>,
        /// 设置监听地址，例如 127.0.0.1:12345
        #[arg(long, value_name = "ADDR")]
        listen: Option<String>,
        /// 快捷设置 api_key（等效覆盖 Authorization: Bearer <key>）
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        /// 额外请求头（仅当未携带时追加），格式 key=value，可重复
        #[arg(long = "extra-header", value_name = "KEY=VALUE")]
        extra_headers: Vec<String>,
        /// 覆盖请求头（无条件覆盖），格式 key=value，可重复
        #[arg(long = "override-header", value_name = "KEY=VALUE")]
        override_headers: Vec<String>,
        /// 保活心跳间隔秒数，0 表示关闭
        #[arg(long, value_name = "SECS")]
        keepalive_secs: Option<u64>,
        /// 设置上游代理 URL，例如 http://127.0.0.1:7890 或 socks5://user:pass@127.0.0.1:7890
        #[arg(long, value_name = "URL")]
        proxy: Option<String>,
        /// 设置代理用户名（可选，优先于 URL 内嵌的用户名）
        #[arg(long, value_name = "USER")]
        proxy_username: Option<String>,
        /// 设置代理密码（可选，优先于 URL 内嵌的密码）
        #[arg(long, value_name = "PASS")]
        proxy_password: Option<String>,
        /// 清空已配置的 api_key
        #[arg(long)]
        clear_api_key: bool,
        /// 清空 extra/override 头
        #[arg(long)]
        clear_headers: bool,
        /// 清空代理配置（URL、用户名、密码）
        #[arg(long)]
        clear_proxy: bool,
        /// 打印当前配置及文件路径
        #[arg(long)]
        show: bool,
    },
}

#[tokio::main]
async fn main() {
    // 日志：默认 info，可通过 RUST_LOG 覆盖
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // 实际生效的配置文件路径：--config 显式指定，否则默认 ~/.aproxy/config.toml
    let cfg_path = cli.config.clone().unwrap_or_else(config::config_path);

    // 子命令：config
    if let Some(Commands::Config {
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
        show,
    }) = cli.command
    {
        handle_config_cmd(
            cfg_path,
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
            show,
        );
        return;
    }

    // 主流程：启动代理服务。显式指定的配置文件必须存在——打错路径时给出明确
    // 报错，而不是静默回退默认配置后报「base_url 为空」误导排查
    if cli.config.is_some() && !cfg_path.exists() {
        eprintln!("指定的配置文件不存在: {}", cfg_path.display());
        std::process::exit(1);
    }
    let mut cfg = config::load_from(&cfg_path);

    // 命令行参数覆盖配置文件
    if let Some(u) = cli.baseurl {
        cfg.base_url = u;
    }
    if let Some(l) = cli.listen {
        cfg.listen_addr = l;
    }
    if let Some(p) = cli.proxy {
        cfg.proxy = Some(p);
    }
    if let Some(k) = cli.api_key {
        cfg.api_key = Some(k);
    }
    cfg = cfg.normalized();

    if let Err(msg) = cfg.validate() {
        eprintln!("配置错误: {msg}");
        eprintln!("位置: {}", cfg_path.display());
        eprintln!();
        eprintln!("请执行以下任一操作后重试:");
        eprintln!("  aproxy config --baseurl https://api.anthropic.com");
        eprintln!("  或手动编辑 {}", cfg_path.display());
        std::process::exit(1);
    }

    let listen_addr = cfg.listen_addr.clone();
    let base_url = cfg.base_url.clone();
    // 日志与控制台都可能被粘贴分享，内嵌凭据的 base_url 一律打码后输出
    tracing::info!(listen = %listen_addr, base_url = %mask_base_url(&base_url), config = %cfg_path.display(), "启动 aProxy");

    let state = aproxy::proxy::AppState::new(cfg);
    let app = aproxy::proxy::router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("无法监听 {listen_addr}: {e}");
            std::process::exit(1);
        });

    println!("aProxy 已启动");
    println!("  监听: http://{listen_addr}");
    println!("  Base URL: {}", mask_base_url(&base_url));
    if !base_url.is_empty() {
        let base = base_url.trim_end_matches('/');
        println!("  提示: 将你的 API base URL 指向 http://{listen_addr}");
        println!("        上游路径与查询参数将完整透传到 {base}/<path>?<query>");
    }
    println!("按 Ctrl+C 退出。");

    // 优雅关闭 + 宽限强退：keepalive 后台重试任务不会主动结束，axum::serve 等待
    // 在途连接完成时可能被其无限期挂起，收到退出信号后给 10 秒宽限窗口即强制退出。
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()) => {
            result.unwrap();
        }
        _ = async {
            shutdown_signal().await;
            tracing::info!("10 秒后强制退出（在途请求将中断）");
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            std::process::exit(0);
        } => {}
    }
}

/// 密钥类值的展示打码：保留前 6 个字符 + "***"（按字符截断，避免多字节字符在字节边界 panic）。
fn mask_secret(s: &str) -> String {
    let prefix: String = s.chars().take(6).collect();
    format!("{prefix}***")
}

/// 对代理 URL 中的密码打码（`http://user:***@host:port`），仅用于展示，绝不输出真实密码。
fn mask_proxy_url(raw: &str) -> String {
    // 解析失败时原样返回会泄露内嵌密码，只回显已隐去的安全占位
    let Ok(url) = url::Url::parse(raw) else {
        return "<无法解析的代理配置，已隐去>".to_string();
    };
    let user = url.username();
    if user.is_empty() && url.password().is_none() {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    // 仅用户名无密码时不渲染 ":***@"，避免让人误以为配置了密码
    let auth = if url.password().is_some() {
        format!("{user}:***@")
    } else {
        format!("{user}@")
    };
    let mut masked = format!("{}://{}{}{}", url.scheme(), auth, host, port);
    if let Some(q) = url.query() {
        masked.push('?');
        masked.push_str(q);
    }
    masked
}

fn parse_kv(s: &str) -> Option<(String, String)> {
    let (k, v) = s.split_once('=')?;
    let k = k.trim();
    let v = v.trim();
    if k.is_empty() {
        return None;
    }
    Some((k.to_string(), v.to_string()))
}

/// base_url 展示打码：内嵌 userinfo（`https://user:pass@host`）时隐去密码段。
/// 无凭据（绝大多数情况）或解析失败时原样返回。
fn mask_base_url(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    if url.password().is_none() {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path();
    format!("{}://{}:***@{}{}{}", url.scheme(), url.username(), host, port, path)
}

/// 展示前守卫：配置文件存在但解析失败时 load() 会静默回退默认值，`--show` 打印的
/// 「当前配置」并非用户文件的真实内容——至少要警告，避免误导排障。
fn warn_if_config_broken(path: &std::path::Path) {
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Err(e) = toml::from_str::<aproxy::config::Config>(&content) {
            eprintln!("警告: 配置文件解析失败，以下展示的是回退默认值而非文件内容");
            eprintln!("解析错误: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_keeps_prefix_and_masks_rest() {
        // 保留前 6 个字符
        assert_eq!(mask_secret("sk-ant-api03-abcdef"), "sk-ant***");
        assert_eq!(mask_secret("abc"), "abc***");
        assert_eq!(mask_secret(""), "***");
    }

    #[test]
    fn mask_secret_multibyte_no_panic() {
        // 按 char 截断，多字节字符不会在字节边界 panic
        assert_eq!(mask_secret("你好世界，测试"), "你好世界，测***");
    }

    #[test]
    fn mask_proxy_url_variants() {
        // 无凭据原样返回
        assert_eq!(
            mask_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        // 有密码打码
        assert_eq!(
            mask_proxy_url("http://alice:secret@127.0.0.1:7890"),
            "http://alice:***@127.0.0.1:7890"
        );
        // 仅用户名不加 ":***@"
        assert_eq!(
            mask_proxy_url("socks5://alice@127.0.0.1:1080"),
            "socks5://alice@127.0.0.1:1080"
        );
        // 解析失败回安全占位而非原文（原文可能含密码）
        assert_eq!(mask_proxy_url("not a url"), "<无法解析的代理配置，已隐去>");
        // query 保留
        assert_eq!(
            mask_proxy_url("http://alice:pw@h:1?p=x"),
            "http://alice:***@h:1?p=x"
        );
    }

    #[test]
    fn mask_base_url_variants() {
        // 无凭据原样返回（绝大多数情况）
        assert_eq!(
            mask_base_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1"
        );
        // 内嵌密码打码，路径保留
        assert_eq!(
            mask_base_url("https://alice:secret@example.com/anthropic"),
            "https://alice:***@example.com/anthropic"
        );
        // 解析失败原样返回（base_url 已过 validate，此处不会发生）
        assert_eq!(mask_base_url("::bad::"), "::bad::");
    }

    #[test]
    fn parse_kv_variants() {
        assert_eq!(
            parse_kv("x-key = some value"),
            Some(("x-key".into(), "some value".into()))
        );
        // 值中允许 '='：只按第一个 '=' 切分
        assert_eq!(
            parse_kv("authorization=Bearer a=b"),
            Some(("authorization".into(), "Bearer a=b".into()))
        );
        assert_eq!(parse_kv("no-equals"), None);
        assert_eq!(parse_kv("  =value"), None);
    }
}

fn handle_config_cmd(
    path: PathBuf,
    baseurl: Option<String>,
    listen: Option<String>,
    api_key: Option<String>,
    extra_headers: Vec<String>,
    override_headers: Vec<String>,
    keepalive_secs: Option<u64>,
    proxy: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    clear_api_key: bool,
    clear_headers: bool,
    clear_proxy: bool,
    show: bool,
) {
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
                        eprintln!("现有配置文件解析失败，拒绝覆盖（请先修复或删除 {}）", path.display());
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
        if !cfg.base_url.is_empty() {
            if let Err(msg) = cfg.validate_base_url() {
                eprintln!("base_url 无效，未保存: {msg}");
                std::process::exit(1);
            }
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
        println!(
            "proxy          = {}",
            cfg.proxy
                .as_deref()
                .map(mask_proxy_url)
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!(
            "proxy_username = {}",
            cfg.proxy_username
                .as_deref()
                .unwrap_or("(未设置)")
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

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("安装 Ctrl+C 监听失败");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("安装 SIGTERM 监听失败")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("收到退出信号，正在关闭...");
}
