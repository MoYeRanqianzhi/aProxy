//! 窗口的应用命令。自行从请求体解码，拒绝结构固定，不让框架把含密钥的非法值写进错误文字。
use aproxy_ipc::{
    desktop::*,
    view::{Change, Feed, HomeView, Presence, ReadFailure},
    wire::Part,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{
    State, Webview,
    ipc::{Channel, InvokeBody, Request},
};
use tokio::{
    sync::{Notify, broadcast, watch},
    task::JoinHandle,
};

fn args<T: DeserializeOwned>(req: &Request<'_>) -> R<T> {
    let InvokeBody::Json(json) = req.body() else {
        return Err(Fault::DesktopBadInput {});
    };
    T::deserialize(json).map_err(|_| Fault::DesktopBadInput {})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

struct Selection {
    generation: SafeUint,
    reference: Ref,
    parts: Option<Vec<BodyPart>>,
    busy: bool,
    cancel: watch::Sender<()>,
}
struct Attached {
    id: String,
    frames: Frames,
    wake: Arc<Notify>,
    reader: Option<JoinHandle<()>>,
    generation: SafeUint,
    selection: Option<Selection>,
}
impl Drop for Attached {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}
#[derive(Default)]
struct Cursor {
    epoch: u64,
    next: Option<(Token, Option<Run>, Traversal, Arc<Survey>)>,
}
#[derive(Default)]
struct UpdateWindow {
    active: bool,
    closing: bool,
    last: Option<Job>,
}
/// 单窗口持有所有易失状态；重新载入只换连接，不会另开视图或第二条实例订阅。
pub(crate) struct Bridge {
    view: HomeView,
    survey: Arc<RwLock<Arc<Survey>>>,
    runtime: tokio::runtime::Handle,
    serial: AtomicU64,
    attached: Mutex<Option<Attached>>,
    cursors: Mutex<[Cursor; 2]>,
    update: Mutex<UpdateWindow>,
}
impl Bridge {
    /// 发现线程先取宿主私有数据再原子替换投影。同步读只需克隆一份 Arc。
    pub(crate) fn open(
        home: PathBuf,
        inspect: InspectHome,
        runtime: tokio::runtime::Handle,
    ) -> Arc<Self> {
        let survey = Arc::new(RwLock::new(Arc::new(Survey::default())));
        let writer = survey.clone();
        let view = HomeView::open_observed(
            home,
            Duration::from_secs(2),
            Some(Arc::new(move |members, wd| {
                let next = Survey::observe(members, wd, inspect(members));
                let mut old = writer.write().unwrap();
                if **old == next {
                    false
                } else {
                    *old = Arc::new(next);
                    true
                }
            })),
        );
        Arc::new(Self {
            view,
            survey,
            runtime,
            serial: AtomicU64::new(1),
            attached: Mutex::default(),
            cursors: Mutex::default(),
            update: Mutex::default(),
        })
    }
    /// 与接受安装共用一把锁，关闭之后不能再启动新事务；在途子进程由自己的任务读到退出。
    pub(crate) fn close_requested(&self) -> bool {
        let mut u = self.update.lock().unwrap();
        u.closing = true;
        u.active
    }
    fn serial(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed)
    }
    fn sample(&self) -> Arc<Survey> {
        self.survey.read().unwrap().clone()
    }
    fn clear_cursors(&self) {
        *self.cursors.lock().unwrap() = Default::default();
    }
    /// 停止页面的接收者和读取，再等唯一的发现任务释放实例连接。
    pub(crate) async fn shutdown(&self) {
        let reader = self
            .attached
            .lock()
            .unwrap()
            .take()
            .and_then(|mut a| a.reader.take());
        if let Some(reader) = reader {
            reader.abort();
            let _ = reader.await;
        }
        self.view.shutdown().await;
    }
    fn offer(&self, event: DesktopEvent) {
        if let Some(a) = self.attached.lock().unwrap().as_mut() {
            a.frames.offer(event);
            a.wake.notify_one();
        }
    }
    fn change(&self, c: Change) -> Option<DesktopEvent> {
        match c {
            Change::Instance(_) | Change::Home => Some(DesktopEvent::InstancesChanged),
            Change::Requests(k) => {
                let f = self.sample();
                let secrets = f.secrets(&k)?;
                Some(DesktopEvent::RequestsChanged {
                    run: Projection::new(&secrets).run(&k)?,
                })
            }
            Change::Completed { run, request_id } => {
                let f = self.sample();
                let secrets = f.secrets(&run)?;
                let row = self
                    .view
                    .requests(&run)
                    .into_iter()
                    .find(|r| r.request_id == request_id)?;
                let row = Projection::new(&secrets).row(&run, row).ok()?;
                Some(DesktopEvent::Rescued {
                    at: row.rescued_at()?,
                    r#ref: row.r#ref,
                })
            }
            _ => Some(DesktopEvent::Resync),
        }
    }
    fn attach(self: &Arc<Self>, channel: Channel<Frame>) -> String {
        let id = format!("s{:x}", self.serial());
        let mut changes = self.view.changes();
        let wake = Arc::new(Notify::new());
        let w = wake.clone();
        let this = self.clone();
        let session = id.clone();
        let mut slot = self.attached.lock().unwrap();
        self.clear_cursors();
        let reader = self.runtime.spawn(async move {
            loop {
                let (frame, deadline) = {
                    let mut slot = this.attached.lock().unwrap();
                    let Some(a) = slot.as_mut().filter(|a| a.id == session) else {
                        return;
                    };
                    let now = Instant::now();
                    (a.frames.poll(now), a.frames.deadline(now))
                };
                if let Some(frame) = frame
                    && channel.send(frame).is_err()
                {
                    let mut slot = this.attached.lock().unwrap();
                    if slot.as_ref().is_some_and(|a| a.id == session) {
                        slot.take();
                        this.clear_cursors();
                    }
                    return;
                }
                let timer = async {
                    match deadline {
                        Some(t) => tokio::time::sleep_until(t.into()).await,
                        None => std::future::pending().await,
                    }
                };
                let event = tokio::select! {
                    _ = w.notified() => continue,
                    _ = timer => continue,
                    change = window_change(&mut changes) => match change {
                        Ok(change) => this.change(change),
                        Err(broadcast::error::RecvError::Lagged(_)) => Some(DesktopEvent::Resync),
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                };
                if let Some(event) = event {
                    let mut slot = this.attached.lock().unwrap();
                    if let Some(a) = slot.as_mut().filter(|a| a.id == session) {
                        a.frames.offer(event);
                    } else {
                        return;
                    }
                }
            }
        });
        *slot = Some(Attached {
            id: id.clone(),
            frames: Frames::new(id.clone()),
            wake,
            reader: Some(reader),
            generation: SafeUint::default(),
            selection: None,
        });
        id
    }
    fn traversal(&self, f: &Survey, scope: Option<&Run>) -> R<Traversal> {
        let mut t = Traversal::default();
        let mut found = false;
        for item in self.view.instances() {
            if scope.is_some_and(|r| item.run.as_ref() != Some(&r.key())) {
                continue;
            }
            found = true;
            let Some(key) = &item.run else { continue };
            let readable = item.history_unavailable.is_none()
                && (item.reads.history_feed || item.liveness != Presence::Live);
            if !readable {
                if scope.is_some() {
                    return Err(Fault::DesktopHistory {});
                }
                continue;
            }
            let Some(secrets) = f.secrets(key) else {
                if scope.is_some() {
                    return Err(Fault::DesktopUnavailable {});
                }
                continue;
            };
            let p = Projection::new(&secrets);
            let Some(run) = p.run(key) else {
                if scope.is_some() {
                    return Err(Fault::DesktopChanged {});
                }
                continue;
            };
            let (raw, bound) = self.view.recent_window(key);
            let rows = raw
                .into_iter()
                .map(|r| {
                    let seq = r.request_seq;
                    Ok((seq, p.list_row(key, r)?))
                })
                .collect::<R<_>>()?;
            let live = item.liveness == Presence::Live;
            t.seed(run.clone(), rows, bound, live);
            if !live || item.feed == Feed::Disconnected {
                t.missing(run, MissingReason::Disconnected);
            }
        }
        if scope.is_some() && !found {
            return Err(Fault::DesktopChanged {});
        }
        Ok(t)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attach {
    channel: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    session: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    session: String,
    seq: SafeUint,
}

/// 页面安装接收函数后传入频道；无效描述符只产生统一的输入错误。
#[tauri::command]
pub(crate) fn attach(
    request: Request<'_>,
    webview: Webview,
    b: State<'_, Arc<Bridge>>,
) -> R<String> {
    let a: Attach = args(&request)?;
    let id = a
        .channel
        .parse::<tauri::ipc::JavaScriptChannelId>()
        .map_err(|_| Fault::DesktopBadInput {})?;
    Ok(b.inner().attach(id.channel_on(webview)))
}
/// 旧页面的卸载不能关闭已经替换它的附件。
#[tauri::command]
pub(crate) fn detach(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<()> {
    let a: Session = args(&request)?;
    let mut slot = b.attached.lock().unwrap();
    if slot.as_ref().is_some_and(|s| s.id == a.session) {
        slot.take();
        b.clear_cursors();
    }
    Ok(())
}
/// 只有匹配本附件和待确认序号的消息才允许发送下一帧。
#[tauri::command]
pub(crate) fn ack(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<()> {
    let x: Ack = args(&request)?;
    if let Some(a) = b.attached.lock().unwrap().as_mut() {
        a.frames.ack(&x.session, x.seq);
        a.wake.notify_one();
    }
    Ok(())
}
/// 唤醒现有发现任务，不在窗口线程上枚举。
#[tauri::command]
pub(crate) fn rescan(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<()> {
    let _: Empty = args(&request)?;
    b.view.rescan();
    Ok(())
}
/// 首页只从视图内存取样，打开窗口或重读卡片不会连到实例上。
#[tauri::command]
pub(crate) async fn snapshot(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<Snapshot> {
    let _: Empty = args(&request)?;
    let f = b.sample();
    let mut t = b.traversal(&f, None)?;
    Ok(Snapshot {
        instances: b
            .view
            .instances()
            .into_iter()
            .map(|i| f.card(i))
            .collect::<R<_>>()?,
        recent: std::iter::from_fn(|| t.pop()).take(50).collect(),
        watchdog: f.watchdog(),
        update: b.update.lock().unwrap().last.clone(),
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    run: Option<Run>,
    cursor: Option<Token>,
    limit: usize,
}
/// 只有续页才读取实例归档。旧的异步请求不能覆盖刷新后发给页面的游标。
#[tauri::command]
pub(crate) async fn query_logs(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<LogPage> {
    let q: Query = args(&request)?;
    if !(1..=100).contains(&q.limit) {
        return Err(Fault::DesktopBadInput {});
    }
    let index = usize::from(q.run.is_some());
    let epoch = b.serial();
    let f = b.sample();
    let remote = q.cursor.is_some();
    let previous = {
        let mut slots = b.cursors.lock().unwrap();
        let s = &mut slots[index];
        let t = if let Some(id) = q.cursor {
            if !s
                .next
                .as_ref()
                .is_some_and(|(old, run, _, _)| *old == id && run == &q.run)
            {
                return Err(Fault::DesktopCursor {});
            }
            s.next.take().map(|v| (v.2, v.3))
        } else {
            s.next = None;
            None
        };
        s.epoch = epoch;
        t
    };
    let (mut t, initial) = match previous {
        Some(v) => v,
        None => (b.traversal(&f, q.run.as_ref())?, f.clone()),
    };
    t.recheck(|run| initial.source_status(run, &f));
    let mut rows = Vec::new();
    while rows.len() < q.limit {
        if remote && let Some((run, before)) = t.needs_head() {
            let key = run.key();
            match b.view.older_requests(&key, before, q.limit).await {
                Ok((page, more)) => {
                    let secrets = f.secrets(&key).ok_or(Fault::DesktopChanged {})?;
                    let p = Projection::new(&secrets);
                    let before = page.last().map(|r| r.request_seq);
                    let page = page
                        .into_iter()
                        .filter_map(|r| p.list_row(&key, r).transpose())
                        .collect::<R<_>>()?;
                    t.supply(&run, page, before, more);
                }
                Err(e) => {
                    let reason = if e == ReadFailure::RunChanged {
                        MissingReason::Changed
                    } else {
                        MissingReason::Unavailable
                    };
                    t.missing(run, reason);
                }
            }
            continue;
        }
        let Some(row) = t.pop() else { break };
        rows.push(row);
    }
    let mut slots = b.cursors.lock().unwrap();
    let s = &mut slots[index];
    if s.epoch != epoch {
        return Err(Fault::DesktopCursor {});
    }
    let latest = b.sample();
    t.recheck(|run| initial.source_status(run, &latest));
    rows.retain(|r| initial.source_status(&r.r#ref.run, &latest).0);
    let next = t.more().then(|| format!("p{epoch:x}"));
    let page = t.page(rows, next.clone());
    s.next = next.map(|id| (id, q.run, t, initial));
    Ok(page)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenDetail {
    session: String,
    generation: SafeUint,
    r#ref: Ref,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBody {
    session: String,
    generation: SafeUint,
    r#ref: Ref,
    part: BodyPart,
    offset: SafeUint,
    limit: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retire {
    session: String,
    generation: SafeUint,
}
fn read_failure(e: ReadFailure) -> Fault {
    match e {
        ReadFailure::RunChanged => Fault::DesktopChanged {},
        ReadFailure::Gone => Fault::DesktopGone {},
        _ => Fault::DesktopHistory {},
    }
}
impl Bridge {
    fn selected<T>(
        &self,
        session: &str,
        generation: SafeUint,
        reference: &Ref,
        use_it: impl FnOnce(&mut Selection) -> R<T>,
    ) -> R<T> {
        let mut a = self.attached.lock().unwrap();
        let s = a
            .as_mut()
            .filter(|s| s.id == session)
            .and_then(|s| s.selection.as_mut())
            .filter(|s| s.generation == generation && &s.reference == reference)
            .ok_or(Fault::DesktopCancelled {})?;
        use_it(s)
    }
}
/// 登记选择代数之后才进行异步读取；切换或关闭时丢弃读取 future，不能只丢弃结果。
#[tauri::command]
pub(crate) async fn request_detail(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<Detail> {
    let a: OpenDetail = args(&request)?;
    if a.generation.get() == 0 {
        return Err(Fault::DesktopBadInput {});
    }
    let mut cancel = {
        let mut slot = b.attached.lock().unwrap();
        let att = slot
            .as_mut()
            .filter(|v| v.id == a.session && a.generation > v.generation)
            .ok_or(Fault::DesktopCancelled {})?;
        let (tx, rx) = watch::channel(());
        att.generation = a.generation;
        att.selection = Some(Selection {
            generation: a.generation,
            reference: a.r#ref.clone(),
            parts: None,
            busy: false,
            cancel: tx,
        });
        rx
    };
    let key = a.r#ref.run.key();
    let secrets = b.sample().secrets(&key).ok_or(Fault::DesktopChanged {})?;
    Projection::new(&secrets).check_ref(&a.r#ref)?;
    let raw = tokio::select! {biased;_ = cancel.changed()=>return Err(Fault::DesktopCancelled{}), r=b.view.detail(&key,&a.r#ref.request_id)=>r.map_err(read_failure)?};
    let f = b.sample();
    let secrets = f.secrets(&key).ok_or(Fault::DesktopChanged {})?;
    let detail = Projection::new(&secrets).detail(&key, raw)?;
    b.selected(&a.session, a.generation, &a.r#ref, |s| {
        s.parts = Some(detail.readable_parts());
        Ok(())
    })?;
    Ok(detail)
}
/// 只允许读取已经打开的选择，一次最多一块。即便 IPC 失败也归还本选择的读取名额。
#[tauri::command]
pub(crate) async fn body_chunk(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<Chunk> {
    let a: ReadBody = args(&request)?;
    if !(1..=aproxy_ipc::wire::MAX_BODY_READ).contains(&a.limit) {
        return Err(Fault::DesktopBadInput {});
    }
    let mut cancel = b.selected(&a.session, a.generation, &a.r#ref, |s| {
        if !s
            .parts
            .as_ref()
            .ok_or(Fault::DesktopCancelled {})?
            .contains(&a.part)
        {
            return Err(Fault::DesktopHistory {});
        }
        if s.busy {
            return Err(Fault::DesktopBusy {});
        }
        s.busy = true;
        Ok(s.cancel.subscribe())
    })?;
    let key = a.r#ref.run.key();
    let (part, attempt) = match a.part {
        BodyPart::Request => (Part::Request, None),
        BodyPart::Sent { attempt } => (Part::Sent, Some(attempt.get())),
        BodyPart::Response { attempt } => (Part::Response, Some(attempt.get())),
    };
    let inspect = async {
        let invalid = || Fault::DesktopHistory {};
        let secrets = b.sample().secrets(&key).ok_or(Fault::DesktopChanged {})?;
        Projection::new(&secrets).check_ref(&a.r#ref)?;
        let margin = secrets.max_len().saturating_sub(1) as u64;
        if margin > aproxy_ipc::wire::MAX_BODY_READ {
            return Err(invalid());
        }
        let read = |offset: u64, limit| {
            let fut = b
                .view
                .body_chunk(&key, &a.r#ref.request_id, part, attempt, offset, limit);
            async move {
                let chunk = Chunk::project(
                    fut.await.map_err(read_failure)?,
                    SafeUint::saturating(offset),
                    limit,
                )?;
                let bytes = STANDARD.decode(&chunk.data_b64).map_err(|_| invalid())?;
                Ok::<_, Fault>((chunk, bytes))
            }
        };
        let offset = a.offset.get();
        let (mut chunk, requested) = read(offset, a.limit).await?;
        let end = offset + requested.len() as u64;
        let left = offset.min(margin);
        let right = (chunk.total_bytes.get() - end).min(margin);
        // An unfinished tail could become the prefix of a secret on the next append.
        if !chunk.complete && right < margin {
            return Err(invalid());
        }
        let mut inspected = Vec::with_capacity(left as usize + requested.len() + right as usize);
        for (at, len) in [(offset - left, left), (end, right)] {
            if at == end {
                inspected.extend_from_slice(&requested);
            }
            if len != 0 {
                let (other, bytes) = read(at, len).await?;
                if chunk.total_bytes != other.total_bytes || chunk.complete != other.complete {
                    return Err(invalid());
                }
                inspected.extend(bytes);
            }
        }
        if b.sample().secrets(&key).as_deref() != Some(secrets.as_ref()) {
            return Err(Fault::DesktopChanged {});
        }
        chunk.data_b64 = STANDARD
            .encode(secrets.body_bytes(&inspected, left as usize..left as usize + requested.len()));
        Ok(chunk)
    };
    let value = tokio::select! {biased;_=cancel.changed()=>return Err(Fault::DesktopCancelled{}),r=inspect=>r};
    b.selected(&a.session, a.generation, &a.r#ref, |s| {
        s.busy = false;
        Ok(())
    })?;
    value
}
/// 撤销直到指定代数；先到的取消也能拦住稍后到达的旧打开操作。
#[tauri::command]
pub(crate) fn cancel_detail(request: Request<'_>, b: State<'_, Arc<Bridge>>) -> R<()> {
    let a: Retire = args(&request)?;
    if a.generation.get() == 0 {
        return Err(Fault::DesktopBadInput {});
    }
    if let Some(s) = b
        .attached
        .lock()
        .unwrap()
        .as_mut()
        .filter(|s| s.id == a.session && a.generation >= s.generation)
    {
        s.generation = a.generation;
        s.selection = None;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetArgs {
    target: StartTarget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    run: Run,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesireArgs {
    port: u16,
    expected_restore: Token,
}

/// 列举宿主认可的启动目标，显示路径不是操作参数。
#[tauri::command]
pub(crate) async fn list_configs(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<Vec<Choice>> {
    let _: Empty = args(&request)?;
    ops.list_configs().await
}

/// 从授权选择启动；返回实际就绪实例而不是请求里的身份。
#[tauri::command]
pub(crate) async fn start_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Run> {
    let a: TargetArgs = args(&request)?;
    let result = ops.start_instance(a.target).await;
    b.view.rescan();
    result
}

/// 只停止仍与页面观察相符的一代实例。
#[tauri::command]
pub(crate) async fn stop_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<()> {
    let a: RunArgs = args(&request)?;
    let result = ops.stop_instance(a.run).await;
    b.view.rescan();
    result
}

/// 重新核验、预检、停旧实例，返回实际启动的新一代。
#[tauri::command]
pub(crate) async fn restart_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Run> {
    let a: RunArgs = args(&request)?;
    let result = ops.restart_instance(a.run).await;
    b.view.rescan();
    result
}

/// 只有仍不应答且进程身份可以核验的实例才允许强制停止。
#[tauri::command]
pub(crate) async fn force_stop_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<()> {
    let a: RunArgs = args(&request)?;
    let result = ops.force_stop_instance(a.run).await;
    b.view.rescan();
    result
}

/// 按当前端口的已观察到的期望状态恢复，而非按路径启动。
#[tauri::command]
pub(crate) async fn restore_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Run> {
    let a: DesireArgs = args(&request)?;
    let result = ops.restore_instance(a.port, a.expected_restore).await;
    b.view.rescan();
    result
}

/// 只撤销同一端口未被替换的期望，不停止进程。
#[tauri::command]
pub(crate) async fn forget_restore(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<()> {
    let a: DesireArgs = args(&request)?;
    let result = ops.forget_restore(a.port, a.expected_restore).await;
    b.view.rescan();
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArg {
    target: ConfigTarget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateArg {
    target: ConfigTarget,
    changes: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WriteArg {
    target: ConfigTarget,
    changes: Vec<Edit>,
    acknowledge_external_acl: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    enabled: bool,
}

/// 读取经过脱敏的生效文件字段，不向页面公开原文。
#[tauri::command]
pub(crate) async fn read_config(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<ConfigView> {
    let a: FileArg = args(&request)?;
    ops.read_config(a.target).await
}

/// 在内存中验证本次编辑，不落盘。
#[tauri::command]
pub(crate) async fn validate_config(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<Validation> {
    let a: ValidateArg = args(&request)?;
    ops.validate_config(a.target, a.changes).await
}

/// 提交一次持锁写入；只提示应重启的实例，不自动重启。
#[tauri::command]
pub(crate) async fn write_config(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Saved> {
    let a: WriteArg = args(&request)?;
    let result = ops
        .write_config(a.target, a.changes, a.acknowledge_external_acl)
        .await?;
    b.view.rescan();
    Ok(result)
}

/// 全局设置的只读行不携带配置凭据。
#[tauri::command]
pub(crate) async fn read_settings(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<SettingsView> {
    let _: Empty = args(&request)?;
    ops.read_settings().await
}

/// 切换本主目录看护开关，当前进程是否在任仍以发现结果为准。
#[tauri::command]
pub(crate) async fn set_watchdog(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<bool> {
    let a: Switch = args(&request)?;
    let result = ops.set_watchdog(a.enabled).await?;
    b.offer(DesktopEvent::SettingsChanged);
    b.view.rescan();
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrefArgs {
    changes: PreferencesPatch,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinArgs {
    r#ref: Ref,
    pinned: bool,
}
/// 窗口只接收已验证的偏好和书签；重置通知与内容一起返回。
#[tauri::command]
pub(crate) async fn read_desktop_state(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<DesktopState> {
    let _: Empty = args(&request)?;
    ops.read_desktop_state().await
}
/// 偏好合并成功后才让其他页面重读。
#[tauri::command]
pub(crate) async fn set_preferences(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Preferences> {
    let a: PrefArgs = args(&request)?;
    let p = ops.set_preferences(a.changes).await?;
    b.offer(DesktopEvent::DesktopStateChanged);
    Ok(p)
}
/// 新书签必须取自已核验的摘要；删除离线书签只比较身份，不读取实例或请求体。
#[tauri::command]
pub(crate) async fn set_pinned(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<Vec<Bookmark>> {
    let a: PinArgs = args(&request)?;
    let change = if a.pinned {
        let key = a.r#ref.run.key();
        let facts = b.sample();
        let secrets = facts.secrets(&key).ok_or(Fault::DesktopChanged {})?;
        let p = Projection::new(&secrets);
        p.check_ref(&a.r#ref)?;
        let raw = b
            .view
            .detail(&key, &a.r#ref.request_id)
            .await
            .map_err(read_failure)?
            .record;
        let row = p.row(&key, raw)?;
        if row.r#ref != a.r#ref || !facts.source_status(&a.r#ref.run, &b.sample()).0 {
            return Err(Fault::DesktopChanged {});
        }
        PinChange::Pin(p.bookmark(row))
    } else {
        PinChange::Unpin(a.r#ref)
    };
    let list = ops.set_pinned(change).await?;
    b.offer(DesktopEvent::DesktopStateChanged);
    Ok(list)
}
/// 返回运行中二进制的版本，不从安装位置探测或联网。
#[tauri::command]
pub(crate) async fn about(request: Request<'_>, ops: State<'_, Arc<dyn DesktopOps>>) -> R<About> {
    let _: Empty = args(&request)?;
    let ops = ops.inner().clone();
    tokio::task::spawn_blocking(move || ops.about())
        .await
        .map_err(|_| Fault::DesktopUnavailable {})?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArg {
    channel: ChannelChoice,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallArg {
    check: Token,
}
/// 调用安装位置的命令查询版本，页面只得到一次性确认凭据。
#[tauri::command]
pub(crate) async fn check_update(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<UpdateInfo> {
    let a: UpdateArg = args(&request)?;
    ops.check_update(a.channel).await
}
/// 安装不随页面卸载取消；进度和结果在子进程结束之前由独立于 invoke 的任务持有。
#[tauri::command]
pub(crate) async fn install_update(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
    b: State<'_, Arc<Bridge>>,
    app: tauri::AppHandle,
) -> R<InstallEnd> {
    let a: InstallArg = args(&request)?;
    {
        let mut state = b.update.lock().unwrap();
        if state.active || state.closing {
            return Err(Fault::DesktopBusy {});
        }
        state.active = true;
    }
    let bridge = b.inner().clone();
    let ops = ops.inner().clone();
    let publish = bridge.clone();
    b.runtime
        .spawn(async move {
            let end = ops
                .install_update(
                    a.check,
                    Arc::new(move |job| {
                        publish.update.lock().unwrap().last = Some(job.clone());
                        publish.offer(DesktopEvent::UpdateProgress { job: Box::new(job) });
                    }),
                )
                .await;
            let close = {
                let mut u = bridge.update.lock().unwrap();
                u.active = false;
                u.closing
            };
            if close {
                app.exit(0)
            }
            end
        })
        .await
        .map_err(|_| Fault::DesktopUnavailable {})?
}

/// 每次读取系统中本 home 的登记，不用本地偏好冒充系统开关。
#[tauri::command]
pub(crate) async fn read_autostart(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<Registration> {
    let _: Empty = args(&request)?;
    ops.read_autostart().await
}
/// 只设置或删除这一 home 的精确登记，不能指定命令、路径或系统位置。
#[tauri::command]
pub(crate) async fn set_autostart(
    request: Request<'_>,
    ops: State<'_, Arc<dyn DesktopOps>>,
) -> R<Registration> {
    let a: Switch = args(&request)?;
    ops.set_autostart(a.enabled).await
}
