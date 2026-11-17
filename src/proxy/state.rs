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
    pub last_activity_ms: Arc<std::sync::atomic::AtomicU64>,
    /// IPC 实时观测源（请求数/重试数/最近错误）：热路径各一条 Relaxed 原子
    /// 操作，量级同上；serve_forever 组装进 IpcStats 供 IPC 响应读取。
    pub stats: Arc<crate::daemon::IpcStats>,
    /// 磁盘缓存的 spool 临时目录；None = disk_cache 关闭（全程纯内存）。
    /// 生产路径 `~/.aproxy/spool/<pid>-<启动标记短哈希>/`，测试可经 Config::spool_dir_override 注入。
    pub spool_dir: Option<PathBuf>,
    /// 重试规则表（源 `config.retry_rules`，空 = 一切请求无限重试），`path` 已预编译。
    /// 启动时编译一次随 AppState 共享，请求热路径只做 is_match。
    pub retry_rules: Arc<Vec<CompiledRetryRule>>,
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
    /// 实例标识：16 位十六进制，由启动时刻毫秒的低 48 位与 pid 的低 16 位拼成，实例重启后
    /// 不同。它是请求标识的前半段
    pub instance_id: Arc<str>,
    /// 入站校验拒掉的请求的序号。这些请求不计入请求数（见 handler），所以另用一个序号给
    /// 它们的错误响应编标识
    pub(super) rejected_seq: Arc<std::sync::atomic::AtomicU64>,
}

/// 一条重试规则，`path` 已编译（自动锚定两端）。
#[derive(Debug, Clone)]
pub struct CompiledRetryRule {
    /// 没有 = 任何路径
    pub path: Option<regex::Regex>,
    /// 没有 = 任何状态码
    pub status: Option<Vec<u16>>,
    /// 第 N 次尝试失败、命中这条规则且 N ≥ 它，就停；0 = 不限
    pub max_attempts: u32,
}

/// 路径已经匹配的规则里，第一条状态码也匹配的：它决定这次失败之后还重不重试。
/// 返回规则的序号（从 1 起）与它的 `max_attempts`。
pub fn first_matching_rule(
    rules: &[(usize, &CompiledRetryRule)],
    status: u16,
) -> Option<(usize, u32)> {
    rules
        .iter()
        .find(|(_, rule)| {
            rule.status
                .as_ref()
                .is_none_or(|codes| codes.contains(&status))
        })
        .map(|(index, rule)| (*index, rule.max_attempts))
}

impl AppState {
    /// 刷新活动时间戳（stop idle 与 status 的闲置判定依据）：收到请求时、每轮重试开头、
    /// 仅转发模式每转发一块时各刷新一次。一条 Relaxed store：只用于粗粒度的空闲判定，
    /// 不需要跨线程的因果序
    pub(super) fn touch_activity(&self) {
        self.last_activity_ms.store(
            crate::daemon::now_millis(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    pub fn new(config: Config) -> Self {
        // 超时策略：不设总时限（会掐断超过时限的慢流式生成，导致无限重试永不成功）。
        // 客户端上只设连接建立的时限；等响应头（首字节）与两次读到数据之间的时限各有各的
        // 配置，由发请求与读响应体的地方分别计时（见 upstream.rs 的 `send_with_timeout` 与
        // `next_chunk`）——reqwest 的 read_timeout 把两者钳成同一个值，所以不用它。
        // 0 表示该项不设限。
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
        // 开启时用测试注入覆盖，否则是本进程自己的 <home>/spool/<pid>-<启动标记短哈希>/
        let spool_dir = if config.disk_cache_enabled() {
            Some(
                config
                    .spool_dir_override
                    .clone()
                    .unwrap_or_else(crate::daemon::own_spool_dir),
            )
        } else {
            None
        };
        // 活动时间戳与观测计数同属一个 IpcStats：serve_forever 把这一份整体交给
        // IPC 线程读。**两者必须同源**——本项目曾在此各建一份（IpcStats 里另有一个
        // 活动时间戳），而 serve_forever 只把活动时间戳共享进 IPC 那份，
        // 于是「请求 / 重试 / 最近错误」三项对任何实例都恒显示 0（活动时间戳正常
        // 反而掩盖了该缺陷，直到 2026-09-14 端到端实测才发现）。
        let last_activity_ms = Arc::new(std::sync::atomic::AtomicU64::new(
            crate::daemon::now_millis(),
        ));
        // 重试规则的 path 模式：启动时编译一次（模式已在配置校验时确认合法，
        // 此处 expect 不会失败——与上方 proxy URL 的信任同一来源）
        let retry_rules = Arc::new(
            config
                .retry_rules
                .iter()
                .map(|rule| CompiledRetryRule {
                    path: rule.path.as_deref().map(|p| {
                        Config::compile_bounded_retry_pattern(p)
                            .expect("重试规则的 path 模式已在配置校验时确认合法")
                    }),
                    status: rule.status.clone(),
                    max_attempts: rule.max_attempts,
                })
                .collect::<Vec<_>>(),
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
        let started_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let instance_id = format!(
            "{:016x}",
            ((started_ms & 0xffff_ffff_ffff) << 16) | u64::from(std::process::id() & 0xffff)
        );
        Self {
            instance_id: instance_id.into(),
            rejected_seq: Arc::default(),
            spool_dir,
            config: Arc::new(config),
            client,
            stats: Arc::new(crate::daemon::IpcStats {
                last_activity_ms: last_activity_ms.clone(),
                ..Default::default()
            }),
            last_activity_ms,
            retry_rules,
            request_pool,
            response_pool,
            heartbeat_pool,
            inbound,
        }
    }

    /// 路径可能命中的重试规则：`路径?查询串` 整体匹配 `path` 的规则（没写 `path` 的规则匹配
    /// 任何路径），带着它们在规则表里的序号（从 1 起），保持书写顺序。一个请求只算一次；
    /// 每次失败再按状态码在这个子集里找第一条命中的（见 [`first_matching_rule`]）。
    pub fn retry_rules_for(&self, path_and_query: &str) -> Vec<(usize, &CompiledRetryRule)> {
        self.retry_rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| {
                rule.path
                    .as_ref()
                    .is_none_or(|re| re.is_match(path_and_query))
            })
            .map(|(index, rule)| (index + 1, rule))
            .collect()
    }

    /// 实例内第 `seq` 个请求的标识：`<instance_id>-<seq>`。HTTP 头、错误体、日志与交给 format
    /// 的信封里都是这同一个串；对外是不透明的
    pub(super) fn request_id(&self, seq: u64) -> String {
        format!("{}-{seq}", self.instance_id)
    }

    /// 入站校验拒掉的请求的标识：`<instance_id>-x<序号>`。与正常请求的标识不会相同
    pub(super) fn rejected_request_id(&self) -> String {
        let seq = self
            .rejected_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        format!("{}-x{seq}", self.instance_id)
    }

    /// 记录一次上游失败摘要（覆盖式，只保留最近一次；IPC/status 展示用）
    pub(super) fn note_upstream_failure(&self, msg: &str) {
        self.stats.record_error(msg);
    }

    /// 实例闲置时长（秒）：距最近一次收到客户端请求。
    pub fn idle_secs(&self) -> u64 {
        crate::daemon::now_millis().saturating_sub(
            self.last_activity_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        ) / 1000
    }
}
