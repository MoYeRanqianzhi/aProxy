//! 普通安装的协调结果，按消费者的语言解释。参数保存在安装者的私有文件里，不写入公开 details。
use super::{InstallFailure, reason::Cause, state::InstallPhase};
use aproxy_ipc::{i18n::Lang, message};
use serde::{Deserialize, Serialize};

message!(lock_open(path: &str, e: &str), en: "Cannot open installation lock {path}: {e}", zh: "安装锁 {path} 打开失败: {e}");

message!(locked(path: &str, secs: u64), en: "Timed out after {secs} seconds waiting for installation lock {path}. Another installation may still be changing bin/.", zh: "安装锁 {path} 被别的进程持有，{secs} 秒内没有等到（另一个安装正在修改 bin/？）");

message!(lock_failed(path: &str, e: &str), en: "Cannot acquire installation lock {path}: {e}", zh: "安装锁 {path} 加锁失败: {e}");

message!(lock_task(e: &str), en: "The task waiting for the installation lock ended unexpectedly: {e}", zh: "等安装锁的任务异常结束: {e}");

message!(jurisdiction(port: &str, dir: &str, image: &str), en: "The binary for the instance on port {port} is outside the installation directory ({dir}): {image}.\nUpgrade it with the tool that installed it (for example npm or Cargo), or run `aproxy install --adopt` with that binary to install it in the standard location.", zh: "实例 {port} 的二进制不在安装管辖目录（{dir}）下：{image}。\n\
                 该实例由其他渠道安装（包管理器/npm/cargo），请用对应渠道升级，\
                 或用那个二进制执行 `aproxy install --adopt`，把它装到标准位置");

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

message!(prepare(e: &str), en: "Not all instances confirmed they were ready for the binary to be replaced ({e}).\nStop the affected instances and retry the installation. They were not forcibly terminated, to avoid interrupting service.", zh: "有实例没有确认已准备好替换二进制（{e}）。\n\
                 请先停掉这些实例再重试安装（没有强制结束它们，以免中断服务）");

message!(fresh(e: &str), en: "A newly started instance did not confirm it was ready for the binary to be replaced: {e}", zh: "新启动的实例没有确认已准备好替换二进制: {e}");

message!(rolled_back(reason: &str, path: &str, pid: u32), en: "The new version failed to start ({reason}). The old binary {path} brought the instance back (pid {pid}). Rolling restarts stopped; instances not yet reached are unchanged.", zh: "新版本启动失败（{reason}）；已用旧二进制 {path} 拉回实例（pid {pid}），\
                 滚动已中止，尚未轮到的实例未动");

message!(down(reason: &str, fallback_error: &str), en: "The new version failed to start ({reason}), and the old binary could not restore the instance ({fallback_error}). The instance did not come back. Its desired-state record was kept; correct the cause and run `aproxy restore`.", zh: "新版本启动失败（{reason}），旧二进制拉回也失败（{fallback_error}）；\
                 实例没有恢复，期望状态记录已保留，排除原因后执行 `aproxy restore` 恢复");

message!(no_restore(port: &str), en: "The instance on port {port} has no desired-state record (it may be a foreground instance).", zh: "实例 {port} 没有期望状态记录（前台实例？）");

message!(unreadable(port: &str, reason: &str), en: "Cannot read the desired-state record for port {port} ({reason})", zh: "实例 {port} 的期望状态记录读不懂（{reason}）");

message!(missing_config(path: &str), en: "The configuration file no longer exists: {path}", zh: "配置文件已不存在: {path}");

message!(stop_timeout(port: &str), en: "The instance on port {port} did not exit in time; refusing to terminate it forcibly.", zh: "实例 {port} 未在预期时间内退出（仍拒绝强杀）");

message!(exit_timeout(port: &str, pid: u32), en: "The process for port {port} (pid {pid}) did not exit in time; refusing to terminate it forcibly.", zh: "实例 {port} 的进程（pid {pid}）未在预期时间内终止（仍拒绝强杀）");

message!(at_path(message: &str, path: &str), en: "{message} ({path})", zh: "{message}（{path}）");

message!(no_fallback(), en: "No old binary is available.", zh: "没有可用的旧二进制");

message!(not_prepared(state: &aproxy_ipc::wire::InstanceState), en: "The instance did not enter the replacement phase (state {state:?})", zh: "实例没有进入更换阶段（状态 {state:?}）");

message!(converge(port: &str, e: &str), en: "Could not restart the instance on port {port} to get its confirmation that the binary can be replaced: {e}", zh: "为让实例 {port} 确认可以替换二进制而重启它，重启失败: {e}");

message!(unacked(unacked: &[String]), en: "These instances still have not confirmed that the binary can be replaced, even after retries and restarts: {unacked:?}", zh: "重试并重启之后，这些实例仍未确认可以替换二进制: {unacked:?}");

message!(rolling(port: &str, e: &str), en: "Rolling restart failed for the instance on port {port}: {e}", zh: "实例 {port} 滚动重启失败: {e}");

message!(final_restart(port: &str, e: &str), en: "The additional verification restart failed on port {port}: {e}", zh: "实例 {port} 终验补重启失败: {e}");

/// 同一个事务的接手者和等待者可能使用不同的终端语言。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "id", content = "args", rename_all = "snake_case")]
pub enum FlowIssue {
    /// 旧镜像成功恢复服务；滚动就此停止。
    RolledBack {
        reason: Box<Cause>,
        path: String,
        pid: u32,
    },
    /// 新旧镜像均无法恢复实例。
    Down {
        reason: Box<Cause>,
        fallback_error: Box<Cause>,
    },
    /// 实例没有可用于重启的期望状态。
    NoRestore { port: String },
    /// 恢复记录不兼容或读取失败。
    Unreadable { port: String, reason: Box<Cause> },
    /// 当初启动所用的配置已不在磁盘上。
    MissingConfig { path: String },
    /// 控制端点在停止期限内仍存在。
    StopTimeout { port: String },
    /// 控制端点消失但进程仍持有镜像。
    ExitTimeout { port: String, pid: u32 },
    /// 启动镜像本身失败时附上执行路径。
    AtPath { message: Box<Cause>, path: String },
    /// 没有可用于回滚的旧镜像。
    NoFallback,
    /// 控制应答未确认进入交换相位。
    NotPrepared {
        state: aproxy_ipc::wire::InstanceState,
    },
    /// 为取得确认而重启时失败。
    Converge { port: String, e: Box<Cause> },
    /// 重试与重启后仍未确认的端口。
    Unacked { unacked: Vec<String> },
    /// 逐端口滚动时的外层诊断。
    Rolling { port: String, e: Box<Cause> },
    /// 验证阶段补做的重启也失败。
    FinalRestart { port: String, e: Box<Cause> },
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
            Self::RolledBack { reason, path, pid } => {
                rolled_back(lang, &reason.render(lang), path, *pid)
            }
            Self::Down {
                reason,
                fallback_error,
            } => down(lang, &reason.render(lang), &fallback_error.render(lang)),
            Self::NoRestore { port } => no_restore(lang, port),
            Self::Unreadable { port, reason } => unreadable(lang, port, &reason.render(lang)),
            Self::MissingConfig { path } => missing_config(lang, path),
            Self::StopTimeout { port } => stop_timeout(lang, port),
            Self::ExitTimeout { port, pid } => exit_timeout(lang, port, *pid),
            Self::AtPath { message, path } => at_path(lang, &message.render(lang), path),
            Self::NoFallback => no_fallback(lang).to_owned(),
            Self::NotPrepared { state } => not_prepared(lang, state),
            Self::Converge { port, e } => converge(lang, port, &e.render(lang)),
            Self::Unacked { unacked } => self::unacked(lang, unacked),
            Self::Rolling { port, e } => rolling(lang, port, &e.render(lang)),
            Self::FinalRestart { port, e } => final_restart(lang, port, &e.render(lang)),
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
