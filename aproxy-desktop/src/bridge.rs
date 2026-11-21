//! 窗口的应用命令。自行从请求体解码，拒绝结构固定，不让框架把含密钥的非法值写进错误文字。
use aproxy_ipc::{
    desktop::*,
    view::{Change, Feed, HomeView, Presence, ReadFailure},
    wire::Part,
};
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
    opened: bool,
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
/// 单窗口持有所有易失状态；重新载入只换连接，不会另开视图或第二条实例订阅。
pub(crate) struct Bridge {
    view: HomeView,
    survey: Arc<RwLock<Arc<Survey>>>,
    runtime: tokio::runtime::Handle,
    serial: AtomicU64,
    attached: Mutex<Option<Attached>>,
    cursors: Mutex<[Cursor; 2]>,
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
        })
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
                    change = changes.recv() => match change {
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
            let rows = self
                .view
                .requests(key)
                .into_iter()
                .map(|r| p.row(key, r))
                .collect::<R<Vec<_>>>()?;
            let live = item.liveness == Presence::Live;
            t.seed(run.clone(), rows, live);
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
        update: None,
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
            if !s.next.as_ref().is_some_and(|(old, run, _, facts)| {
                *old == id && run == &q.run && facts.same_ports(&f)
            }) {
                return Err(Fault::DesktopCursor {});
            }
            s.next.take().map(|v| v.2)
        } else {
            s.next = None;
            None
        };
        s.epoch = epoch;
        t
    };
    let mut t = match previous {
        Some(t) => t,
        None => b.traversal(&f, q.run.as_ref())?,
    };
    let mut rows = Vec::new();
    while rows.len() < q.limit {
        if remote && let Some((run, before)) = t.needs_head() {
            let key = run.key();
            match b.view.older_requests(&key, before, q.limit).await {
                Ok((page, more)) => {
                    let secrets = f.secrets(&key).ok_or(Fault::DesktopChanged {})?;
                    let p = Projection::new(&secrets);
                    let page = page.into_iter().map(|r| p.row(&key, r)).collect::<R<_>>()?;
                    t.supply(&run, page, more);
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
    if s.epoch != epoch || !f.same_ports(&b.sample()) {
        return Err(Fault::DesktopCursor {});
    }
    let next = t.more().then(|| format!("p{epoch:x}"));
    let page = t.page(rows, next.clone());
    s.next = next.map(|id| (id, q.run, t, f));
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
            opened: false,
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
        s.opened = true;
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
        if !s.opened {
            return Err(Fault::DesktopCancelled {});
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
    let value = tokio::select! {biased;_=cancel.changed()=>return Err(Fault::DesktopCancelled{}),r=b.view.body_chunk(&key,&a.r#ref.request_id,part,attempt,a.offset.get(),a.limit)=>r.map_err(read_failure).and_then(|v|Chunk::project(v,a.offset,a.limit))};
    b.selected(&a.session, a.generation, &a.r#ref, |s| {
        s.busy = false;
        Ok(())
    })?;
    if b.sample().secrets(&key).is_none() {
        return Err(Fault::DesktopChanged {});
    }
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
    ops: State<'_, Arc<dyn InstanceOps>>,
) -> R<Vec<Choice>> {
    let _: Empty = args(&request)?;
    ops.list_configs().await
}

/// 从授权选择启动；返回实际就绪实例而不是请求里的身份。
#[tauri::command]
pub(crate) async fn start_instance(
    request: Request<'_>,
    ops: State<'_, Arc<dyn InstanceOps>>,
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
    ops: State<'_, Arc<dyn InstanceOps>>,
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
    ops: State<'_, Arc<dyn InstanceOps>>,
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
    ops: State<'_, Arc<dyn InstanceOps>>,
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
    ops: State<'_, Arc<dyn InstanceOps>>,
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
    ops: State<'_, Arc<dyn InstanceOps>>,
    b: State<'_, Arc<Bridge>>,
) -> R<()> {
    let a: DesireArgs = args(&request)?;
    let result = ops.forget_restore(a.port, a.expected_restore).await;
    b.view.rescan();
    result
}
