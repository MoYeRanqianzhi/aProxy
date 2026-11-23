//! 配置条目是稳定的寻址对象；每次派发才取当前池，流与已借出的会话则持住旧池。
use super::*;
use aproxy_ipc::wire::RestartOutcome;

/// 一个实例内按配置顺序寻址的条目，共享给请求、心跳和观察消费者。
#[derive(Clone, Default)]
pub(crate) struct Formats(pub(crate) Arc<[Arc<FormatSlot>]>);
impl From<Vec<Arc<Pool>>> for Formats {
    fn from(pools: Vec<Arc<Pool>>) -> Self {
        Self(pools.into_iter().map(FormatSlot::new).collect())
    }
}
impl Formats {
    /// 每次迭代取当下的池，长请求的后续阶段不会继续借用退役的进程。
    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = Arc<Pool>> + ExactSizeIterator + '_ {
        self.0.iter().map(|s| s.current())
    }
}
/// 一个配置名对应一个可交换的进程池。并发重启串行，数据面不等待控制面的握手。
pub(crate) struct FormatSlot {
    live: Mutex<Arc<Pool>>,
    restarting: tokio::sync::Mutex<()>,
}
impl FormatSlot {
    /// 初次启动的池也是第一代，探测仍走启动路径。
    pub(crate) fn new(pool: Arc<Pool>) -> Arc<Self> {
        Arc::new(Self {
            live: Mutex::new(pool),
            restarting: Default::default(),
        })
    }
    /// 借出一代的所有权；进程生命周期不依赖快照读取者。
    pub(crate) fn current(&self) -> Arc<Pool> {
        self.live.lock().unwrap().clone()
    }
    /// 与切代在同一把锁里宣布排空；即使正在握手，随后发布的池也不能再保留空闲进程。
    pub(crate) fn begin_drain(&self) {
        self.live.lock().unwrap().begin_drain();
    }
    /// 取同一代的修订号与快照，供订阅者比较顺序。
    pub(crate) fn snapshot(&self) -> (u64, serde_json::Value) {
        self.live.lock().unwrap().snapshot()
    }
    /// 在同一配置上重新探测，握手失败也切走旧代；由运行中实例的熔断策略处理后续消息。
    pub(crate) async fn restart(&self) -> RestartOutcome {
        let _guard = self.restarting.lock().await;
        let old = self.current();
        let mut next = Pool::with_limits(
            old.entry.clone(),
            old.home.clone(),
            old.secrets.clone(),
            old.events.clone(),
            old.frame_limits,
        );
        // 名额和熔断属于每代，但仍活着的编号与关停责任跨代；旧流可以一直持有自己的会话。
        let fields = Arc::get_mut(&mut next).unwrap();
        fields.ids = old.ids.clone();
        fields.stopping = old.stopping.clone();
        fields.retiring = old.retiring.clone();
        fields.stderr_window = old.stderr_window.clone();
        let result = next.probe().await;
        let name = old.secrets.replace(&old.entry.name);
        let mut live = self.live.lock().unwrap();
        let revision = {
            let mut c = old.circuit.lock().unwrap();
            c.superseded = true;
            c.revision
        };
        if old.draining.load(std::sync::atomic::Ordering::Relaxed) {
            next.begin_drain();
        }
        old.begin_drain();
        *live = next.clone();
        {
            let mut c = next.circuit.lock().unwrap();
            c.revision = revision;
            c.restarted_at = Some(crate::daemon::now_millis());
        }
        let outcome = match result {
            Ok(()) => {
                let hello = next.negotiated.lock().unwrap();
                let hello = hello.as_ref().unwrap();
                RestartOutcome::Restarted {
                    name,
                    program: Some(next.secrets.replace(&hello.name)),
                    version: Some(next.secrets.replace(&hello.version)),
                }
            }
            Err(f) => {
                let reason = f.kind().to_owned();
                next.open_at_startup(f);
                RestartOutcome::HandshakeFailed { name, reason }
            }
        };
        next.transition(&mut next.circuit.lock().unwrap());
        outcome
    }
}
