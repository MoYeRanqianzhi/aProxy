//! 配置条目是稳定的寻址对象；每次派发才取当前池，流与已借出的会话则持住旧池。
use super::*;
use aproxy_ipc::wire::{Rejection, RestartOutcome};

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
/// 一个配置名对应一个可交换的进程池。并发重启直接拒绝，数据面不等待控制面的握手。
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
    /// 与切代在同一把锁里宣布排空；正在探测的候选池不能在排空开始后发布。
    pub(crate) fn begin_drain(&self) {
        self.live.lock().unwrap().begin_drain();
    }
    /// 取同一代的修订号与快照，供订阅者比较顺序。
    pub(crate) fn snapshot(&self) -> (u64, serde_json::Value) {
        self.live.lock().unwrap().snapshot()
    }
    /// 先同步保留名额，等整组保留成功才轮询探测；放弃未轮询的 future 不会启动进程。
    pub(crate) fn reserve_restart(
        &self,
    ) -> Result<impl Future<Output = Result<RestartOutcome, Rejection>> + '_, Rejection> {
        let guard = self
            .restarting
            .try_lock()
            .map_err(|_| Rejection::RestartInProgress)?;
        Ok(async move {
            let _guard = guard;
            self.probe_replacement().await
        })
    }
    /// 探测完整成功才发布候选池；失败不改当前身份、熔断、控制会话或时间戳。
    async fn probe_replacement(&self) -> Result<RestartOutcome, Rejection> {
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
        if let (Some(new), Some(prev)) = (&mut fields.notices, &old.notices) {
            new.get_mut().unwrap().dropped = prev.lock().unwrap().dropped.clone();
        }
        let probe = next.probe().await;
        let name = old.secrets.replace(&old.entry.name);
        let mut live = self.live.lock().unwrap();
        if old.draining.load(std::sync::atomic::Ordering::Relaxed) {
            next.begin_drain();
            return Err(Rejection::InvalidState("stopping"));
        }
        if let Err(fault) = probe {
            return Ok(RestartOutcome::HandshakeFailed {
                name,
                reason: fault.kind().into(),
            });
        }
        let revision = {
            let mut c = old.circuit.lock().unwrap();
            c.superseded = true;
            c.revision
        };
        old.begin_drain();
        *live = next.clone();
        {
            let mut c = next.circuit.lock().unwrap();
            c.revision = revision;
            c.restarted_at = Some(crate::daemon::now_millis());
        }
        next.transition(&mut next.circuit.lock().unwrap());
        let hello = next.negotiated.lock().unwrap();
        let hello = hello.as_ref().unwrap();
        Ok(RestartOutcome::Restarted {
            name,
            program: Some(next.secrets.replace(&hello.name)),
            version: Some(next.secrets.replace(&hello.version)),
        })
    }
}
