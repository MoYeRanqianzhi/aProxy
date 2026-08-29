//! aProxy — 本地 API 代理，无限重试保障 agent 工作流不中断。

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use aproxy::config;

#[derive(Parser, Debug)]
#[command(name = "aproxy", version, about = "Local API proxy with infinite retries", long_about = None)]
struct Cli {
    /// 上游 API base URL，覆盖配置文件中的 upstream_url
    #[arg(long, value_name = "URL")]
    upstream: Option<String>,

    /// 监听地址，覆盖配置文件中的 listen_addr
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 查看或修改配置（配置文件位于 ~/.aproxy/config.toml）
    Config {
        /// 设置上游 URL，例如 https://api.anthropic.com
        #[arg(long, value_name = "URL")]
        upstream: Option<String>,
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
        /// 清空已配置的 api_key
        #[arg(long)]
        clear_api_key: bool,
        /// 清空 extra/override 头
        #[arg(long)]
        clear_headers: bool,
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

    // 子命令：config
    if let Some(Commands::Config {
        upstream,
        listen,
        api_key,
        extra_headers,
        override_headers,
        keepalive_secs,
        clear_api_key,
        clear_headers,
        show,
    }) = cli.command
    {
        handle_config_cmd(
            upstream,
            listen,
            api_key,
            extra_headers,
            override_headers,
            keepalive_secs,
            clear_api_key,
            clear_headers,
            show,
        );
        return;
    }

    // 主流程：启动代理服务
    let mut cfg = config::load();

    // 命令行参数覆盖配置文件
    if let Some(u) = cli.upstream {
        cfg.upstream_url = u;
    }
    if let Some(l) = cli.listen {
        cfg.listen_addr = l;
    }
    cfg = cfg.normalized();

    if let Err(msg) = cfg.validate() {
        eprintln!("配置错误: {msg}");
        eprintln!("位置: {}", config::config_path().display());
        eprintln!();
        eprintln!("请执行以下任一操作后重试:");
        eprintln!("  aproxy config --upstream https://api.anthropic.com");
        eprintln!("  或手动编辑 {}", config::config_path().display());
        std::process::exit(1);
    }

    let listen_addr = cfg.listen_addr.clone();
    let upstream_url = cfg.upstream_url.clone();
    tracing::info!(listen = %listen_addr, upstream = %upstream_url, config = %config::config_path().display(), "启动 aProxy");

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
    println!("  上游: {upstream_url}");
    if !upstream_url.is_empty() {
        let base = upstream_url.trim_end_matches('/');
        println!("  提示: 将你的 API base URL 指向 http://{listen_addr}");
        println!("        上游路径与查询参数将完整透传到 {base}/<path>?<query>");
    }
    println!("按 Ctrl+C 退出。");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
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

fn handle_config_cmd(
    upstream: Option<String>,
    listen: Option<String>,
    api_key: Option<String>,
    extra_headers: Vec<String>,
    override_headers: Vec<String>,
    keepalive_secs: Option<u64>,
    clear_api_key: bool,
    clear_headers: bool,
    show: bool,
) {
    let path = config::config_path();
    let mut cfg = config::load();

    let mut changed = false;
    if let Some(u) = upstream {
        cfg.upstream_url = u.trim_end_matches('/').to_string();
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

    if changed {
        cfg = cfg.normalized();
        match config::save(&cfg) {
            Ok(()) => println!("已保存配置到 {}", path.display()),
            Err(e) => {
                eprintln!("保存配置失败: {e}");
                std::process::exit(1);
            }
        }
    }

    if show || !changed {
        // 重新加载以展示最终值
        let cfg = config::load().normalized();
        println!("配置文件: {}", path.display());
        println!("upstream_url = \"{}\"", cfg.upstream_url);
        println!("listen_addr  = \"{}\"", cfg.listen_addr);
        println!(
            "api_key      = {}",
            cfg.api_key
                .as_deref()
                .map(|k| format!("\"{}***\"", &k[..k.len().min(6)]))
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!("keepalive_interval_secs = {}", cfg.keepalive_interval_secs);
        if cfg.extra_headers.is_empty() {
            println!("extra_headers    = (空)");
        } else {
            println!("extra_headers:");
            for (k, v) in &cfg.extra_headers {
                println!("  {k} = \"{v}\"");
            }
        }
        if cfg.override_headers.is_empty() {
            println!("override_headers = (空)");
        } else {
            println!("override_headers:");
            for (k, v) in &cfg.override_headers {
                println!("  {k} = \"{v}\"");
            }
        }
        if cfg.upstream_url.is_empty() {
            println!();
            println!("提示: upstream_url 为空，请设置:");
            println!("  aproxy config --upstream https://api.anthropic.com");
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
