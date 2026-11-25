//! 普通安装的逐行事件。从任务里的状态通知和交棒后的同一份落盘状态取数，不从日志猜测进度。
use aproxy::install::state::{InstallPhase, InstallState, OBSERVER, Observer};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeSet, future::Future, path::Path};

/// 仅用户的安装任务有输出游标，接手者的 stdout 不参与这条流。
#[derive(Default)]
struct Stream {
    /// 目标还未决定的查询或探测失败以 null 表示，不冒称已经有目标版本。
    version: Option<String>,
    /// 状态反复读取、续作重入都不能重复发布同一相位。
    phases: BTreeSet<InstallPhase>,
    /// 同一快照端口只交付一次滚动结局。
    ports: BTreeSet<u16>,
}
tokio::task_local! {static STREAM: RefCell<Stream>;}

/// 只包住显式的 JSON 安装；并行的后台任务不继承这个输出目的地。
pub(super) async fn stream(future: impl Future<Output = ()>) {
    let observer = Observer {
        id: (std::process::id(), aproxy::watchdog::now_millis()),
        state: observe,
        progress: |bytes, total| {
            super::super::print_doc(
                json!({"event":"progress","phase":"downloading","bytes":bytes,"total":total}),
            )
        },
    };
    STREAM
        .scope(RefCell::default(), OBSERVER.scope(observer, future))
        .await;
}

/// 人读的说明在 JSON 模式下让出 stdout。
pub(super) fn say(text: impl std::fmt::Display) {
    super::super::say(STREAM.try_with(|_| ()).is_ok(), text);
}

/// 决定版本时就保存：之后即使没有创建安装状态，失败也能说出是哪一版。
pub(super) fn target(version: &str) {
    let _ = STREAM.try_with(|s| s.borrow_mut().version = Some(version.to_owned()));
}

/// 只有通过了入口的安全检查（或无需安装）才输出计划。
pub(super) fn plan(ports: &[u16]) {
    let _ = STREAM.try_with(|_| super::super::print_doc(json!({"event":"plan","ports":ports})));
}

/// 按首次出现的相位与端口发事件；老接手者没有时间表时，只报告实际读到的相位。
pub(super) fn observe(st: &InstallState) {
    let _ = STREAM.try_with(|stream| {
        let mut stream = stream.borrow_mut();
        let fallback = (st.phase, st.updated_at.saturating_mul(1000));
        for &(phase, at) in st.phase_times.iter().chain(std::iter::once(&fallback)) {
            if stream.phases.insert(phase) {
                super::super::print_doc(json!({"event":"phase","phase":phase,"at":at}));
            }
        }
        for item in &st.instance_outcomes {
            if stream.ports.insert(item.port) {
                super::super::print_doc(
                    json!({"event":"instance","port":item.port,"outcome":item.outcome}),
                );
            }
        }
    });
}

/// 快速完成的接手者把最后一份状态留给等待者，避免 done 后删状态时漏掉最后的实例。
pub(super) fn take_outcome(home: &Path) -> Result<bool, aproxy::install::InstallFailure> {
    let Ok(id) = OBSERVER.try_with(|o| o.id) else {
        return Ok(false);
    };
    let path = aproxy::install::state::outcome_path(home, id);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string().into()),
    };
    let state: InstallState = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    observe(&state);
    std::fs::remove_file(path).map_err(|e| e.to_string())?;
    Ok(true)
}

/// 收编只在镜像落位并完成交接后报告用户环境，其他安装结果的形状不变。
pub(super) fn adopted(steps: &super::integration::Steps) {
    let _ = STREAM.try_with(|s| super::super::print_doc(json!({"event":"result","outcome":"installed","version":s.borrow().version,"integration":steps})));
}

/// 最后一行只在失败时带 error，message 与同一路径给人读的首行一致。
pub(super) fn result(outcome: &str, error: Option<(&aproxy::install::InstallFailure, &str)>) {
    let _ = STREAM.try_with(|s| {
        let mut doc = json!({"event":"result","outcome":outcome,"version":s.borrow().version});
        if let Some((e, msg)) = error {
            doc["error"] = json!({"code":e.code,"message":msg,"details":e.details});
        }
        super::super::print_doc(doc);
    });
}
