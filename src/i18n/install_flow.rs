//! 普通安装的协调结果，按消费者的语言解释。参数保存在安装者的私有文件里，不写入公开 details。
use super::{InstallFailure, reason::Cause, state::InstallPhase};
use aproxy_ipc::{i18n::Lang, message};
use serde::{Deserialize, Serialize};

message!(lock_open(path: &str, e: &str), en: "Cannot open installation lock {path}: {e}", zh: "安装锁 {path} 打开失败: {e}");

message!(locked(path: &str, secs: u64), en: "Another process held installation lock {path} for {secs} seconds. Another installation may be changing bin.", zh: "安装锁 {path} 被别的进程持有，{secs} 秒内没有等到（另一个安装正在修改 bin/？）");

message!(lock_failed(path: &str, e: &str), en: "Cannot acquire installation lock {path}: {e}", zh: "安装锁 {path} 加锁失败: {e}");

message!(lock_task(e: &str), en: "The task waiting for the installation lock ended unexpectedly: {e}", zh: "等安装锁的任务异常结束: {e}");

message!(jurisdiction(port: &str, dir: &str, image: &str), en: "The binary for the instance on port {port} is outside the installation directory ({dir}): {image}.\nUpgrade it through its original package manager, npm, or cargo, or run `aproxy install --adopt` to move it to the standard location.", zh: "实例 {port} 的二进制不在安装管辖目录（{dir}）下：{image}。\n\
                 该实例由其他渠道安装（包管理器/npm/cargo），请用对应渠道升级，\
                 或执行 `aproxy install --adopt` 显式收编到标准位置");

message!(concurrent(target: &str, phase: InstallPhase, pid: u32, secs: u64), en: "Installation is already in progress (target {target}, phase {phase:?}, pid {pid}). Wait for it to finish, or, if interrupted, allow {secs} seconds for the record to expire.", zh: "已有安装进行中（target {target}，phase {phase:?}，pid {pid}）。\
             等待其完成，或确认其已中断后等待 {secs} 秒自动失效");

message!(transition(from: InstallPhase, to: InstallPhase), en: "Invalid installation transition {from:?} → {to:?} (state machine bypassed?)", zh: "非法状态迁移 {from:?} → {to:?}（状态机被绕过？）");

message!(start(path: &str, e: &str), en: "Cannot start {path}: {e}", zh: "无法启动 {path}: {e}");

message!(gave_up(path: &str, pid: u32), en: "{path} (pid {pid}) exited before taking over", zh: "{path}（pid {pid}）没有接手就退出了");

message!(timed_out(path: &str, pid: u32, secs: u64), en: "{path} (pid {pid}) did not take over within {secs} seconds and was terminated", zh: "{path}（pid {pid}）{secs} 秒内没有接手，已终止");

message!(verify(bad: &[String]), en: "Final verification failed after one additional restart (wrong version or phase): {bad:?}", zh: "终验未通过（版本/阶段不符，已补 restart 一轮）: {bad:?}");

message!(in_progress(phase: InstallPhase, pid: u32), en: "Installation is still running (phase {phase:?}, pid {pid}); not taking over.", zh: "安装仍在进行（phase {phase:?}，pid {pid}），不重复接管");

message!(no_from(), en: "Cannot resume without the --from source record. The state file may be damaged; run the installation again.", zh: "续作缺少 --from 源记录（状态文件损坏？），请重新执行安装");

message!(no_stage(), en: "Cannot resume without the staging record. The state file may be damaged; run the installation again.", zh: "续作缺少 staging 记录（状态文件损坏？），请重新执行安装");

message!(both_missing(), en: "Both the staged binary and the --from source are missing. Run the installation again.", zh: "staging 与 --from 源均已缺失，请重新执行安装");

message!(takeover(e: &str), en: "The target version did not take over the installation ({e}); the instances and the binary in bin are unchanged.", zh: "目标版本没有接手安装（{e}）；实例与 bin 里的二进制都未改动");

message!(prepare(e: &str), en: "PrepareSwap broadcast failed ({e}).\nStop the affected instances and retry the installation. They were not forcibly terminated to avoid a service interruption.", zh: "PrepareSwap 广播终失败（{e}）。\n\
                 问题实例有隐患，处置指引：将其关闭后重试安装（不强杀——避免服务中断）");

message!(fresh(e: &str), en: "An instance started after the broadcast failed to acknowledge it: {e}", zh: "广播后新启实例 ACK 失败: {e}");

/// 同一个事务的接手者和等待者可能使用不同的终端语言。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "id", content = "args", rename_all = "snake_case")]
pub enum FlowIssue {
    /// 锁文件的路径和打开错误。
    LockOpen { path: String, e: String },
    /// 竞争者在整个期限内一直持锁。
    Locked { path: String, secs: u64 },
    /// 锁文件已打开，但系统拒绝加锁。
    LockFailed { path: String, e: String },
    /// 等待锁的阻塞任务退出。
    LockTask { e: String },
    /// 实例镜像不属于此安装目录。
    Jurisdiction {
        port: String,
        dir: String,
        image: String,
    },
    /// 目标事务仍新鲜，不能覆盖其状态。
    Concurrent {
        target: String,
        phase: InstallPhase,
        pid: u32,
        secs: u64,
    },
    /// 状态机没有这条迁移边。
    Transition {
        from: InstallPhase,
        to: InstallPhase,
    },
    /// 无法运行准备接手的可执行文件。
    Start { path: String, e: String },
    /// 已启动的接手者退出时仍未取得事务。
    GaveUp { path: String, pid: u32 },
    /// 到期时接手者仍未确认，已经终止。
    TimedOut { path: String, pid: u32, secs: u64 },
    /// 补重启后仍未达到目标的端口。
    Verify { bad: Vec<String> },
    /// 活跃事务不接受未指名的接手者。
    InProgress { phase: InstallPhase, pid: u32 },
    /// 续作依赖的源路径记录缺失。
    NoFrom,
    /// 续作依赖的暂存路径记录缺失。
    NoStage,
    /// 暂存文件与源文件都不可用。
    BothMissing,
    /// 目标安装者未取得事务的底层故障。
    Takeover { e: Box<Cause> },
    /// 最终广播失败后保留底层诊断。
    Prepare { e: Box<Cause> },
    /// 快照之后新启实例的确认结果。
    Fresh { e: Box<Cause> },
}
impl FlowIssue {
    /// 写状态或日志用英文；只有终端按调用者的语言排版。
    pub fn render(&self, lang: Lang) -> String {
        match self {
            Self::LockOpen { path, e } => lock_open(lang, path, e),
            Self::Locked { path, secs } => locked(lang, path, *secs),
            Self::LockFailed { path, e } => lock_failed(lang, path, e),
            Self::LockTask { e } => lock_task(lang, e),
            Self::Jurisdiction { port, dir, image } => jurisdiction(lang, port, dir, image),
            Self::Concurrent {
                target,
                phase,
                pid,
                secs,
            } => concurrent(lang, target, *phase, *pid, *secs),
            Self::Transition { from, to } => transition(lang, *from, *to),
            Self::Start { path, e } => start(lang, path, e),
            Self::GaveUp { path, pid } => gave_up(lang, path, *pid),
            Self::TimedOut { path, pid, secs } => timed_out(lang, path, *pid, *secs),
            Self::Verify { bad } => verify(lang, bad),
            Self::InProgress { phase, pid } => in_progress(lang, *phase, *pid),
            Self::NoFrom => no_from(lang).to_owned(),
            Self::NoStage => no_stage(lang).to_owned(),
            Self::BothMissing => both_missing(lang).to_owned(),
            Self::Takeover { e } => takeover(lang, &e.render(lang)),
            Self::Prepare { e } => prepare(lang, &e.render(lang)),
            Self::Fresh { e } => fresh(lang, &e.render(lang)),
        }
    }
}
impl From<FlowIssue> for Cause {
    fn from(e: FlowIssue) -> Self {
        Self::Flow(e)
    }
}
impl From<FlowIssue> for InstallFailure {
    fn from(e: FlowIssue) -> Self {
        Self::because("install_failed", e.into())
    }
}
impl std::fmt::Display for FlowIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}
