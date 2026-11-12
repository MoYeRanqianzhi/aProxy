//! 实例共享状态 AppState：配置、上游 client、转换器进程池、观测计数。

use super::*;

/// 共享状态
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub client: reqwest::Client,
    /// 最近一次收到客户端请求的时刻（Unix 秒）。AtomicU64 供 IPC ping 读取
    /// 展示/判闲置，请求热路径上仅一次 store（Relaxed 足够：只用于粗粒度的
    /// 空闲判定，无需跨线程因果序）。
    pub last_activity_secs: Arc<std::sync::atomic::AtomicU64>,
    /// IPC 实时观测源（请求数/重试数/最近错误）：热路径各一条 Relaxed 原子
    /// 操作，量级同上；serve_forever 组装进 IpcStats 供 IPC 响应读取。
    pub stats: Arc<crate::daemon::IpcStats>,
    /// 磁盘缓存的 spool 临时目录；None = disk_cache 关闭（全程纯内存）。
    /// 生产路径 `~/.aproxy/spool/<端口>/`，测试可经 Config::spool_dir_override 注入。
    pub spool_dir: Option<PathBuf>,
    /// 受限重试路径的预编译正则（源 `config.bounded_retry_paths`，空 = 功能
    /// 关闭）。启动时编译一次随 AppState 共享，请求热路径只做 is_match。
    pub bounded_retry_patterns: Arc<Vec<regex::Regex>>,
    /// 外部转换器进程池（request/response 各一，源 `config.request_transform`
    /// / `response_transform`）；None = 未配置，热路径零开销。pub(crate)：池是
    /// 内部编排细节（TransformPool 的接口不对外），AppState 虽 pub 但不泄漏它。
    pub(crate) request_pool: Option<Arc<crate::transform::TransformPool>>,
    pub(crate) response_pool: Option<Arc<crate::transform::TransformPool>>,
    /// 心跳转换器进程池（`heartbeat_transform`）；None = 只发固定心跳
    pub(crate) heartbeat_pool: Option<Arc<crate::transform::TransformPool>>,
    /// 入站来源校验策略（源 `config.allowed_hosts` / `allowed_origins` 与监听
    /// 地址）：启动时归一一次，热路径只做小集合比较。
    pub(super) inbound: Arc<InboundPolicy>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        // 超时策略：不设总时限（会掐断超过时限的慢流式生成，导致无限重试永不成功），
        // 只限制连接建立与两次读到数据之间的间隔（均可经 config.toml 调整）。
        // 注意 read_timeout 同样钳制首字节等待——LLM 上游排队时 TTFB 可达数十秒，
        // 阈值过小（如 60s）会把「慢但活着」的上游变成确定性无限重试。spool 设计
        // 本身容忍慢流。0 表示该项不设限。
        let mut builder = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            // 透明代理不跟随重定向：跟随会把 Authorization/api_key 与请求体外带到
            // 3xx 指向的任意主机，且 3xx 永远到不了客户端；禁用后 3xx 作为普通
            // 成功响应原样回放（is_retryable_status 本就排除 3xx）。
            .redirect(reqwest::redirect::Policy::none());
        if config.connect_timeout_secs > 0 {
            builder = builder.connect_timeout(Duration::from_secs(config.connect_timeout_secs));
        }
        if config.read_timeout_secs > 0 {
            builder = builder.read_timeout(Duration::from_secs(config.read_timeout_secs));
        }

        // 显式配置代理：所有上游请求经该代理转发；reqwest 在 .proxy() 时会自动关闭
        // 系统代理（不再读取 HTTP_PROXY 等环境变量），避免两者互相干扰。
        // 代理 URL 已在 Config::validate() 校验，此处 expect 不会失败。
        if let Some(proxy_url) = config.proxy.as_deref() {
            let mut proxy = reqwest::Proxy::all(proxy_url).expect("代理 URL 已在 validate() 校验");
            // 单独配置的用户名/密码优先于 URL 内嵌凭据；仅配置密码而无用户名则忽略
            if let Some(username) = config.proxy_username.as_deref() {
                proxy = proxy.basic_auth(username, config.proxy_password.as_deref().unwrap_or(""));
            }
            builder = builder.proxy(proxy);
        }

        let client = builder.build().expect("构建 reqwest client 失败");
        // 磁盘缓存目录：关闭（disk_cache=false）时为 None——全程纯内存；
        // 开启时用测试注入覆盖，否则按端口取 ~/.aproxy/spool/<端口>/
        let spool_dir = if config.disk_cache_enabled() {
            Some(config.spool_dir_override.clone().unwrap_or_else(|| {
                crate::daemon::spool_dir_for(crate::daemon::port_of(&config.listen_addr))
            }))
        } else {
            None
        };
        // 活动时间戳与观测计数同属一个 IpcStats：serve_forever 把这一份整体交给
        // IPC 线程读。**两者必须同源**——本项目曾在此各建一份（IpcStats 里另有一个
        // 活动时间戳），而 serve_forever 只把活动时间戳共享进 IPC 那份，
        // 于是「请求 / 重试 / 最近错误」三项对任何实例都恒显示 0（活动时间戳正常
        // 反而掩盖了该缺陷，直到 2026-09-14 端到端实测才发现）。
        let last_activity_secs = Arc::new(std::sync::atomic::AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ));
        // 受限重试路径模式：启动时编译一次（模式已在 validate() 校验过合法性，
        // 此处 expect 不会失败——与上方 proxy URL 的信任同一来源）
        let bounded_retry_patterns = Arc::new(
            config
                .bounded_retry_paths()
                .iter()
                .map(|p| {
                    Config::compile_bounded_retry_pattern(p)
                        .expect("受限重试路径模式已在 validate() 校验")
                })
                .collect(),
        );
        // 外部转换器进程池：按配置构造（未配置 = None，热路径零开销）。
        // 仅 spawn 模式惰性启动了 reaper 之外无任何后台任务，构造本身零开销
        let request_pool = config
            .request_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let response_pool = config
            .response_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let heartbeat_pool = config
            .heartbeat_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let inbound = Arc::new(InboundPolicy::from_config(&config));
        Self {
            spool_dir,
            config: Arc::new(config),
            client,
            stats: Arc::new(crate::daemon::IpcStats {
                last_activity_secs: last_activity_secs.clone(),
                ..Default::default()
            }),
            last_activity_secs,
            bounded_retry_patterns,
            request_pool,
            response_pool,
            heartbeat_pool,
            inbound,
        }
    }

    /// 请求是否命中受限重试路径：`路径?查询串` 整体匹配任一配置模式
    /// （模式在 Config::compile_bounded_retry_pattern 自动锚定，普通路径
    /// 即精准匹配）。
    pub fn bounded_retry_matches(&self, path_and_query: &str) -> bool {
        self.bounded_retry_patterns
            .iter()
            .any(|re| re.is_match(path_and_query))
    }

    /// 记录一次上游失败摘要（覆盖式，只保留最近一次；IPC/status 展示用）
    pub(super) fn note_upstream_failure(&self, msg: &str) {
        self.stats.record_error(msg);
    }

    /// 实例闲置时长（秒）：距最近一次收到客户端请求。
    pub fn idle_secs(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(
            self.last_activity_secs
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}
