//! 完成通知和业务消息共用普通进程，但队列、失败计数与冷却只属于观察方向。
use super::*;
use aproxy_envelope::{BodyNeed, Completion, NotifyEvent};
use std::collections::VecDeque;

/// 内存按实际保留的结构与字符串容量计费，不因状态是 JSON 转义串而漏掉它。
const QUEUE_BYTES: usize = 16 * 1024 * 1024;
/// 小通知另受条数限制，长期忙碌的业务程序不会积下无限个旧请求。
const QUEUE_ITEMS: usize = 1024;
/// 每个池同一时间至多有一个通知在写或等待应答，故障不会拖住所有会话。
#[derive(Default)]
pub(super) struct Lane {
    /// 通知自己的窗口、连续失败和试探资格，不唤醒或撤销业务票。
    pub(super) breaker: Circuit,
    /// 每份完成消息与其保留字节数，满时从头丢。
    queue: VecDeque<(Completion, usize)>,
    /// 只计仍排队的消息；取出时立即归还。
    bytes: usize,
    /// 累计因容量或打开的熔断器未获派发的消息数。
    pub(super) dropped: u64,
    /// 丢弃可持续很久，每个程序分别限频而不是每条写日志。
    warned: Option<Instant>,
    /// 尚未拿到业务进程的调用数；包括流的 open，与从队列借走进程在同一把锁上排序。
    waiting_data: usize,
    /// 消费任务在同一把锁里决定退出或再取一条，入队不会错过唤醒。
    running: bool,
}
impl Lane {
    /// 两种丢弃使用同一计数；从不记录客户端字段或程序的状态串。
    fn lost(&mut self, pool: &Pool, n: u64) {
        if n == 0 {
            return;
        }
        self.dropped = self.dropped.saturating_add(n);
        if self
            .warned
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
        {
            self.warned = Some(Instant::now());
            tracing::warn!(format=%pool.secrets.replace(&pool.entry.name),stage="notify",dropped=self.dropped,"format completion notifications dropped");
        }
    }
    /// 任一熔断器打开都放弃已排队的旧请求，只在冷却结束后用新通知试探。
    pub(super) fn discard_all(&mut self, pool: &Pool) {
        let n = self.queue.len();
        self.queue.clear();
        self.bytes = 0;
        self.lost(pool, n as u64);
    }
    /// 冷却期间不会动普通进程；只把观察方向的下一次试探推到新的截止时间。
    fn open(&mut self, pool: &Pool, now: u64) {
        let kind = self.breaker.last.as_ref().expect("opening fault").0.kind();
        self.breaker.window = Some(Cooldown {
            kind,
            start: Instant::now(),
            opened_at: now,
            probe_at: now.saturating_add(pool.entry.breaker_cooldown_secs.saturating_mul(1000)),
        });
        self.breaker.probing = false;
        self.discard_all(pool);
        tracing::warn!(format=%pool.secrets.replace(&pool.entry.name),stage="notify",failures=self.breaker.failures,cooldown_secs=pool.entry.breaker_cooldown_secs,kind,"format circuit opened");
    }
    /// 这里按实际持有的字符串容量和结构大小计费，不估计可能更小的压缩或省略后的大小。
    fn charge(end: &Completion) -> usize {
        let c = &end.context;
        std::mem::size_of::<(Completion, usize)>()
            + c.request_id.capacity()
            + c.state.as_ref().map_or(0, String::capacity)
            + c.digest
                .as_ref()
                .and_then(Option::as_ref)
                .map_or(0, String::capacity)
            + end.client.as_ref().map_or(0, |c| c.path.capacity())
            + end.outcome.capacity()
            + end.code.as_ref().map_or(0, String::capacity)
    }
}
/// 等待业务进程的凭据；排队者取消、熔断或取得进程时都要唤醒观察方向。
pub(super) struct Waiting<'a>(&'a Pool);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut lane = self.0.notices.as_ref().unwrap().lock().unwrap();
        lane.waiting_data -= 1;
        if lane.waiting_data == 0 {
            self.0.parked.notify_waiters();
        }
    }
}
impl Pool {
    /// 不订阅通知的程序不加锁；订阅程序不允许通知抢走排队业务的空闲进程。
    pub(super) fn waiting_data(&self) -> Option<Waiting<'_>> {
        self.notices.as_ref().map(|n| {
            n.lock().unwrap().waiting_data += 1;
            Waiting(self)
        })
    }
    /// 上传原始字节只读一次。按配置与最近一次成功 hello 的交集算摘要，不预测替换进程会更改订阅。
    /// 从未握手成功的可选项是例外：首次冷却试探可能发生在上传结束后，届时已无法补算原始字节。
    pub(crate) fn may_need_upload_digest(&self, stages: &[crate::config::file::Stage]) -> bool {
        let hello = self.negotiated.lock().unwrap();
        stages.iter().any(|&stage| {
            self.entry.has_stage(stage)
                && hello
                    .as_ref()
                    .is_none_or(|h| h.request_body.get(stage.as_str()) == Some(&BodyNeed::Digest))
        })
    }
    /// 只有 notify 的可选程序握手不可用时，由观察方向负责冷却和恢复，业务熔断保持关闭。
    pub(super) fn notify_failure_at_start(&self, f: Fault) {
        let mut data = self.circuit.lock().unwrap();
        let mut lane = self.notices.as_ref().unwrap().lock().unwrap();
        let now = crate::daemon::now_millis();
        lane.breaker.failures = 1;
        lane.breaker.last = Some((f, now));
        if self.entry.breaker_failures != 0 {
            lane.open(self, now);
        } else {
            tracing::warn!(format=%self.secrets.replace(&self.entry.name),stage="notify","optional format unavailable at startup");
        }
        drop(lane);
        self.transition(&mut data);
    }
    /// 交付线程只在有界队列上操作；进程启动、管道与超时都由独立消费者处理。
    pub(crate) fn enqueue_notify(self: &Arc<Self>, end: Completion) {
        let mut data = self.circuit.lock().unwrap();
        let mut lane = self
            .notices
            .as_ref()
            .expect("configured notify")
            .lock()
            .unwrap();
        let half = lane.breaker.window.is_some();
        if data.window.is_some()
            || lane.breaker.probing
            || lane.breaker.window.as_ref().is_some_and(|w| {
                w.start.elapsed() < Duration::from_secs(self.entry.breaker_cooldown_secs)
            })
            || self.draining.load(std::sync::atomic::Ordering::Relaxed)
        {
            lane.lost(self, 1);
            return;
        }
        let bytes = Lane::charge(&end);
        while !lane.queue.is_empty()
            && (lane.queue.len() >= QUEUE_ITEMS || lane.bytes + bytes > QUEUE_BYTES)
        {
            let (_, old) = lane.queue.pop_front().unwrap();
            lane.bytes -= old;
            lane.lost(self, 1);
        }
        if bytes > QUEUE_BYTES {
            lane.lost(self, 1);
            return;
        }
        if half {
            lane.breaker.probing = true;
            tracing::info!(format=%self.secrets.replace(&self.entry.name),stage="notify","format circuit half-open");
        }
        lane.queue.push_back((end, bytes));
        lane.bytes += bytes;
        let start = !lane.running;
        lane.running = true;
        drop(lane);
        if half {
            self.transition(&mut data);
        }
        drop(data);
        if start {
            tokio::spawn(self.clone().drain_notices());
        }
    }
    /// 消费者不在普通业务队列里占号。先登记唤醒，再在同一把锁里检查等待者并取空闲进程。
    async fn drain_notices(self: Arc<Self>) {
        let notices = self.notices.as_ref().unwrap();
        let mut stopping = self.stopping.subscribe();
        loop {
            let parked = self.parked.notified();
            let (ticket, borrowed, replacement) = {
                let data = self.circuit.lock().unwrap();
                let mut lane = notices.lock().unwrap();
                if lane.queue.is_empty()
                    || data.window.is_some()
                    || *stopping.borrow()
                    || self.draining.load(std::sync::atomic::Ordering::Relaxed)
                {
                    lane.discard_all(&self);
                    lane.running = false;
                    return;
                }
                let ticket = Admission {
                    pool: &self,
                    epoch: data.epoch,
                    changed: data.changed.subscribe(),
                    probe: false,
                };
                let busy = lane.waiting_data != 0;
                let borrowed = (!busy).then(|| self.take()).flatten().map(|w| {
                    let (mut end, n) = lane.queue.pop_front().unwrap();
                    lane.bytes -= n;
                    if w.hello.as_ref().unwrap().request_body.get("notify")
                        != Some(&BodyNeed::Digest)
                    {
                        end.context.digest = None;
                    }
                    (w, end)
                });
                // 只在池里一个普通进程也没有时才补进程，不为观察者扩容。一次性尝试，
                // 不在信号量上预约全部名额，否则等待中的业务也会排在观察者后面。
                let replacement = (!busy && borrowed.is_none())
                    .then(|| {
                        let mut all = self
                            .capacity
                            .clone()
                            .try_acquire_many_owned(self.entry.pool_max)
                            .ok()?;
                        all.split(1)
                    })
                    .flatten();
                (ticket, borrowed, replacement)
            };
            if let Some((mut worker, end)) = borrowed {
                let result = self
                    .exchange(
                        &mut worker,
                        Stage::Notify(NotifyEvent::RequestEnd(end)),
                        2,
                        Some(&ticket),
                    )
                    .await
                    .and_then(|_| worker.reusable());
                self.finish_notice(&ticket, result.as_ref().err(), false);
                if matches!(result, Ok(true)) {
                    self.park_notice_worker(worker, &ticket);
                }
            } else if let Some(capacity) = replacement {
                // 握手后先归池再竞争，期间到达的业务先用。
                match self.spawn_with(Some(&ticket), Some(capacity)).await {
                    Ok(w) => self.park_notice_worker(w, &ticket),
                    Err(f) => {
                        self.finish_notice(&ticket, Some(&f), true);
                    }
                }
            } else {
                tokio::select! {biased; _=stopping.changed()=>return, _=ticket.revoked()=>(), _=parked=>()};
            }
        }
    }
    /// 打开数据熔断和归池有确定的顺序，不复用冷却前的进程。
    fn park_notice_worker(self: &Arc<Self>, worker: Worker, ticket: &Admission<'_>) {
        let data = self.circuit.lock().unwrap();
        if data.window.is_none() && data.epoch == ticket.epoch {
            self.park(worker);
        }
    }
    /// 数据面打开后旧代即失效：不为它的通知处罚观察熔断器，也不把进程塞回已经清空的池。
    fn finish_notice(&self, ticket: &Admission<'_>, fault: Option<&Fault>, spawn_failed: bool) {
        let mut data = self.circuit.lock().unwrap();
        let mut lane = self.notices.as_ref().unwrap().lock().unwrap();
        if data.window.is_some() || data.epoch != ticket.epoch {
            if !spawn_failed {
                lane.lost(self, 1);
            }
            return;
        }
        if spawn_failed && let Some((_, n)) = lane.queue.pop_front() {
            lane.bytes -= n;
        }
        let half = lane.breaker.probing;
        let mut transition = false;
        if let Some(f) = fault {
            let now = crate::daemon::now_millis();
            lane.breaker.failures = lane.breaker.failures.saturating_add(1);
            lane.breaker.last = Some((f.clone(), now));
            if half
                || (self.entry.breaker_failures != 0
                    && lane.breaker.failures >= self.entry.breaker_failures)
            {
                lane.open(self, now);
                transition = true;
            }
        } else {
            lane.breaker.failures = 0;
            if half {
                lane.breaker.probing = false;
                lane.breaker.window = None;
                transition = true;
                tracing::info!(format=%self.secrets.replace(&self.entry.name),stage="notify","format circuit closed");
            }
        }
        drop(lane);
        if transition {
            self.transition(&mut data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    /// 排空开始时立即释放尚未派发的状态，不等占着同一个 worker 的业务归还名额。
    #[tokio::test]
    async fn begin_drain_releases_queued_notices_before_data_returns() {
        let (cfg, diags) = crate::config::file::parse(
            "[[format]]\ncommand='unstarted'\nstages=['notify']\nprocess='persistent'",
            Path::new("unit.toml"),
        );
        assert!(diags.is_empty());
        let p = Pool::new(
            cfg.format.unwrap().pop().unwrap(),
            PathBuf::new(),
            Secrets::default(),
            None,
            [1024; 2],
        );
        let _waiting = p.waiting_data();
        let send = || {
            p.enqueue_notify(Completion::new(
                aproxy_envelope::Context::new("r".into()),
                aproxy_envelope::Delivery::Retry,
                1,
                1,
                "ok".into(),
            ))
        };
        send();
        send();
        tokio::task::yield_now().await;
        p.begin_drain();
        send();
        let (len, bytes, dropped) = {
            let lane = p.notices.as_ref().unwrap().lock().unwrap();
            (lane.queue.len(), lane.bytes, lane.dropped)
        };
        assert_eq!((len, bytes, dropped), (0, 0, 3));
    }
    /// 不放行业务等待者，检查双重容量按旧到新淘汰；数据熔断清队列而不处罚观察方向。
    #[tokio::test]
    async fn notify_queue_yields_to_waiting_data_and_charges_private_state() {
        let (cfg, diags) = crate::config::file::parse(
            "[[format]]\ncommand='unstarted'\nstages=['request','notify']\nprocess='persistent'",
            Path::new("unit.toml"),
        );
        assert!(diags.is_empty());
        let p = Pool::new(
            cfg.format.unwrap().pop().unwrap(),
            PathBuf::new(),
            Secrets::default(),
            None,
            [1024; 2],
        );
        let waiting = p.waiting_data();
        let enqueue = |id: usize, state: usize| {
            let mut ctx = aproxy_envelope::Context::new(id.to_string());
            ctx.state = Some("x".repeat(state));
            p.enqueue_notify(Completion::new(
                ctx,
                aproxy_envelope::Delivery::Retry,
                1,
                1,
                "ok".into(),
            ));
        };
        for id in 0..1030 {
            enqueue(id, 0);
        }
        tokio::task::yield_now().await;
        let notices = p.notices.as_ref().unwrap();
        {
            let lane = notices.lock().unwrap();
            assert_eq!(lane.dropped, 6);
            assert_eq!(lane.queue.front().unwrap().0.context.request_id, "6");
        }
        for id in 1030..1330 {
            enqueue(id, 64 * 1024);
        }
        {
            let lane = notices.lock().unwrap();
            assert!(lane.bytes <= QUEUE_BYTES);
            assert!(lane.queue.len() < 256);
            assert_eq!(lane.queue.back().unwrap().0.context.request_id, "1329");
            assert_eq!(lane.dropped as usize + lane.queue.len(), 1330);
        }
        {
            let mut c = p.circuit.lock().unwrap();
            c.last = Some((Fault::Protocol, 0));
            p.open(&mut c, 0);
        }
        enqueue(1330, 0);
        drop(waiting);
        tokio::task::yield_now().await;
        let lane = notices.lock().unwrap();
        assert!(!lane.running);
        assert_eq!(lane.bytes, 0);
        assert_eq!(lane.dropped, 1331);
        assert_eq!(lane.breaker.failures, 0);
        assert!(p.ids.lock().unwrap().is_empty());
    }
}
