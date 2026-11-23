//! 控制调用逐个占用独立会话；没有请求阶段的票、熔断器或 worker 名额。
use super::*;
use aproxy_ipc::wire::{ControlIssue, ControlReply};
use serde_json::{Value, json};

/// 排队上限不含正在执行的一条。每个池代只保留一个空闲控制进程。
pub(super) struct Lane {
    permits: Semaphore,
    worker: tokio::sync::Mutex<Option<Worker>>,
    idle_timer: Mutex<Option<tokio::task::AbortHandle>>,
    stop: tokio::sync::watch::Sender<bool>,
}
impl Default for Lane {
    fn default() -> Self {
        Self {
            permits: Semaphore::new(17),
            worker: Default::default(),
            idle_timer: Default::default(),
            stop: tokio::sync::watch::channel(false).0,
        }
    }
}
impl Pool {
    /// 排空或换代时立即关闭空闲会话；正在交换帧则由持有管道的任务收走进程。
    pub(super) fn stop_control(&self) {
        self.control.stop.send_replace(true);
        if let Ok(mut slot) = self.control.worker.try_lock()
            && let Some(w) = slot.take()
        {
            self.retire(w)
        }
    }
    /// 期限从入队起算；在队列中取消或超时不会打断正在为其他调用服务的进程。
    pub(crate) async fn control_call(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<ControlReply, ControlIssue> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let offer = self
            .negotiated
            .lock()
            .unwrap()
            .as_ref()
            .filter(|h| h.control.is_some())
            .cloned()
            .ok_or(ControlIssue::NoControl)?;
        let _permit = self
            .control
            .permits
            .try_acquire()
            .map_err(|_| ControlIssue::Busy)?;
        let mut stop = self.control.stop.subscribe();
        let mut slot = tokio::time::timeout_at(deadline, self.control.worker.lock())
            .await
            .map_err(|_| ControlIssue::Timeout)?;
        if let Some(timer) = self.control.idle_timer.lock().unwrap().take() {
            timer.abort()
        }
        const IDLE: Duration = Duration::from_secs(300);
        if slot
            .as_ref()
            .is_some_and(|w| w.idle_since.elapsed() >= IDLE)
        {
            self.retire(slot.take().unwrap())
        }
        let exchange = async {
            // 先取走再 await：取消、崩溃或半帧时 Drop 只强退此进程，不能把错位的管道交给下一人。
            let mut w = match slot.take() {
                Some(w) if !w.exited.is_finished() => w,
                _ => {
                    let (mut w, id) = self
                        .start_process(&offer.control.as_ref().unwrap().args, None, None)
                        .await
                        .map_err(|_| ControlIssue::Spawn)?;
                    self.handshake(&mut w, id, None, true)
                        .await
                        .map_err(|_| ControlIssue::Protocol)?;
                    w
                }
            };
            let mut line = serde_json::to_vec(
                &json!({"v":1,"seq":w.seq,"stage":"control","method":method,"params":params}),
            )
            .unwrap();
            line.push(b'\n');
            w.stdin
                .as_mut()
                .unwrap()
                .write_all(&line)
                .await
                .map_err(|_| ControlIssue::Protocol)?;
            let line = read_reply(&mut w.stdout, FRAME_OVERHEAD)
                .await
                .map_err(|e| {
                    if matches!(e, Fault::Invalid) {
                        ControlIssue::TooLarge
                    } else {
                        ControlIssue::Protocol
                    }
                })?;
            let mut frame: serde_json::Map<String, Value> =
                serde_json::from_slice(&line).map_err(|_| ControlIssue::Protocol)?;
            if frame.get("v").and_then(Value::as_u64) != Some(1)
                || frame.get("seq").and_then(Value::as_u64) != Some(w.seq)
            {
                return Err(ControlIssue::Protocol);
            }
            let reply = match (frame.remove("result"), frame.remove("fail")) {
                (Some(result), None) => ControlReply::Result {
                    result: redact(result, &self.secrets)?,
                },
                (None, Some(f)) => ControlReply::Fail {
                    fail: aproxy_ipc::wire::ControlFail {
                        code: self
                            .secrets
                            .replace(f["code"].as_str().ok_or(ControlIssue::Protocol)?),
                        reason: self
                            .secrets
                            .replace(f["reason"].as_str().ok_or(ControlIssue::Protocol)?),
                    },
                },
                _ => return Err(ControlIssue::Protocol),
            };
            if !w.reusable().map_err(|_| ControlIssue::Protocol)? {
                return Err(ControlIssue::Protocol);
            }
            w.seq += 1;
            w.idle_since = Instant::now();
            *slot = Some(w);
            Ok(reply)
        };
        let result = tokio::select! {biased;_=stop.wait_for(|s|*s)=>Err(ControlIssue::NoControl),r=tokio::time::timeout_at(deadline,exchange)=>r.unwrap_or(Err(ControlIssue::Timeout))};
        let reply = match result {
            Ok(v) => v,
            Err(e) => {
                if let Some(w) = slot.take() {
                    self.retire(w)
                }
                return Err(e);
            }
        };
        let weak = Arc::downgrade(self);
        let mut stop = self.control.stop.subscribe();
        let until = slot.as_ref().unwrap().idle_since + IDLE;
        *self.control.idle_timer.lock().unwrap() = Some(
            tokio::spawn(async move {
                tokio::select! {_=tokio::time::sleep_until(until)=>(),_=stop.wait_for(|s|*s)=>()}
                if let Some(pool) = weak.upgrade() {
                    let mut slot = pool.control.worker.lock().await;
                    if slot
                        .as_ref()
                        .is_some_and(|w| w.idle_since.elapsed() >= IDLE || *stop.borrow())
                    {
                        pool.retire(slot.take().unwrap());
                    }
                }
            })
            .abort_handle(),
        );
        Ok(reply)
    }
}
/// 键也是任意程序产生的字符串；两个键脱敏后相同就拒绝，避免默默丢掉其中一个值。
fn redact(v: Value, secrets: &Secrets) -> Result<Value, ControlIssue> {
    Ok(match v {
        Value::String(s) => Value::String(secrets.replace(&s)),
        Value::Array(a) => Value::Array(
            a.into_iter()
                .map(|v| redact(v, secrets))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(o) => {
            let mut safe = serde_json::Map::new();
            for (k, v) in o {
                if safe
                    .insert(secrets.replace(&k), redact(v, secrets)?)
                    .is_some()
                {
                    return Err(ControlIssue::Protocol);
                }
            }
            Value::Object(safe)
        }
        other => other,
    })
}
