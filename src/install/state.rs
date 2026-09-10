//! install 状态文件（`<home>/run/install.state`）：安装的真相源 + 安装锁。
//!
//! 分工原则（用户定调）：状态文件**只有 install 自己读**——残留 ≠ 在安装，
//! 看护者/守护绝不解读它的语义（它们只认共享内存宣告节）；处于哪一步、
//! 续跑还是清理，全部由 install 进程自己判断。
//!
//! 关键性质：
//! - **create_new = 安装锁**：已存在且新鲜 → 并发安装拒绝；stale（updated_at
//!   超时且原安装进程已死）→ 视为残留，不询问直接接管续跑。
//! - **原子重写**：每次阶段推进整文件重写（tmp + rename），updated_at 自动
//!   刷新——接力协议靠「updated_at 持续刷新」自证存活。
//! - **无 .json 后缀**：与 `.pid`/`.restore` 注册表惯例一致，按内容而非
//!   扩展名识别。
//! - 状态文件删除 = 安装完成。任何非终态残留都意味着未完成（恢复机制的
//!   触发依据）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 阶段状态机。线性主线 + failed/aborted 旁路，合法迁移见 [`can_transition`]。
///
/// ```text
/// marking → downloading → downloaded → broadcasting → acked
///         → swapping → swapped → relaying → restarting → verifying → cleaning → done
/// 失败/中止：failed（保留现场，续跑重试）| aborted（干净回滚，终态）
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Marking,
    Downloading,
    Downloaded,
    Broadcasting,
    Acked,
    Swapping,
    Swapped,
    Relaying,
    Restarting,
    Verifying,
    Cleaning,
    Done,
    Failed,
    Aborted,
}

impl InstallPhase {
    /// 线性主线的运行阶段（不含终态 failed/aborted 与 done）。
    pub const RUN_ORDER: &'static [InstallPhase] = &[
        InstallPhase::Marking,
        InstallPhase::Downloading,
        InstallPhase::Downloaded,
        InstallPhase::Broadcasting,
        InstallPhase::Acked,
        InstallPhase::Swapping,
        InstallPhase::Swapped,
        InstallPhase::Relaying,
        InstallPhase::Restarting,
        InstallPhase::Verifying,
        InstallPhase::Cleaning,
    ];

    fn run_index(self) -> Option<usize> {
        Self::RUN_ORDER.iter().position(|p| *p == self)
    }

    /// 终态：done（成功清场）/ aborted（显式回滚）。failed 可续跑，非终态。
    pub fn is_terminal(self) -> bool {
        matches!(self, InstallPhase::Done | InstallPhase::Aborted)
    }
}

/// 迁移合法性：线性主线只允许前进一格；任意运行阶段可转 failed（保留现场）
/// ；--abort 仅 swapping 前可完全回滚（ack 及之前）——此后只进不退；
/// failed 续跑从 downloading 重来（幂等重下）。终态无出边（重装 = 删状态
/// 文件重新 create_new）。
pub fn can_transition(from: InstallPhase, to: InstallPhase) -> bool {
    use InstallPhase::*;
    if let (Some(i), Some(j)) = (from.run_index(), to.run_index()) {
        return j == i + 1;
    }
    match (from, to) {
        (Failed, Downloading) => true,
        (_, Failed) if from.run_index().is_some() => true,
        (_, Aborted) if from.run_index().is_some() => {
            // swapping 及之后只进不退：ack 及之前才允许 abort
            matches!(from.run_index(), Some(i) if i <= InstallPhase::Acked.run_index().unwrap())
        }
        _ => false,
    }
}

/// 安装来源（获取通道）。`--from` 是本地路径源，其余为网络渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallSource {
    /// GitHub Releases 直下
    #[default]
    Github,
    /// 本地路径（`--from <路径>`；--adopt 复用同一流水线）
    From,
    /// crates.io（cargo build 本地编译）
    Cargo,
    /// npm registry（平台包 tgz）
    Npm,
    /// cargo-binstall（模板指向的预编译产物）
    Binstall,
}

/// skill 支线子状态（单独可观察；下载幂等，不建断电恢复状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillPhase {
    Pending,
    Downloading,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillState {
    pub status: SkillPhase,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub version: String,
}

/// install.state 全量 schema。全部字段 serde default——旧版本状态文件缺
/// 字段可读，新字段加入不破坏旧二进制续跑（恢复矩阵「状态文件损坏」之外
/// 的兼容底线）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstallState {
    pub phase: InstallPhase,
    /// 目标版本（如 "0.1.0-alpha.9"）
    #[serde(default)]
    pub target_version: String,
    #[serde(default)]
    pub source: InstallSource,
    /// staging 里的新二进制绝对路径（swapping 的恢复源）
    #[serde(default)]
    pub staged_path: Option<String>,
    /// 新二进制 sha256（github 渠道强校验值；--from 为计算值）
    #[serde(default)]
    pub sha256: Option<String>,
    /// swap 后旧二进制的去向（Windows 为 .old 路径；unix 无 .old 不填）
    #[serde(default)]
    pub old_path: Option<String>,
    /// ACK 阶段的实例清单（swapping 前快照 diff 比对 + restarting 进度记录）
    #[serde(default)]
    pub instance_snapshot: Vec<String>,
    /// skill 支线（默认 Some；--no-skills/skill_auto_update=false 时不建）
    #[serde(default)]
    pub skill: Option<SkillState>,
    #[serde(default)]
    pub started_at: u64,
    /// 每次保存自动刷新（接力存活判据 + stale 判定的基准）
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub installer_pid: u32,
}

impl InstallState {
    /// 全新安装的初始状态（phase = marking）。
    pub fn new_marking(target_version: impl Into<String>, source: InstallSource) -> Self {
        let now = now_secs();
        Self {
            phase: InstallPhase::Marking,
            target_version: target_version.into(),
            source,
            staged_path: None,
            sha256: None,
            old_path: None,
            instance_snapshot: Vec::new(),
            skill: None,
            started_at: now,
            updated_at: now,
            installer_pid: std::process::id(),
        }
    }
}

/// 锁 stale 判定时长：updated_at 超此时长**且** installer_pid 不存活才视为
/// 残留（双条件——长阶段本身不慢到 10 分钟，pid 判活兜住误判）。
pub const STALE_AFTER_SECS: u64 = 600;

/// 状态文件路径：`<home>/run/install.state`（run/ 子目录放状态类文件——
/// 根目录只放长期稳定件；run/ 经 APROXY_RUN_DIR 重定向，测试隔离白送）。
pub fn state_path() -> PathBuf {
    state_path_in(&crate::daemon::run_dir())
}

/// 同上，run 目录可指定（测试注入用）。
pub fn state_path_in(run_dir: &Path) -> PathBuf {
    run_dir.join("install.state")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 读取状态文件。不存在 → None（无安装）；损坏 → None（调用方按恢复矩阵
/// 「状态文件损坏」处置：按 aborted 处理 + 审计日志）。
pub fn load_in(run_dir: &Path) -> Option<InstallState> {
    let content = std::fs::read_to_string(state_path_in(run_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 同上，真实位置。
pub fn load() -> Option<InstallState> {
    load_in(&crate::daemon::run_dir())
}

/// stale 判定：updated_at 超时 **且** 原安装进程已死。pid = 0（未填/损坏）
/// 直接视为死；pid 为当前进程（同一进程重入）视为活。
pub fn is_stale(state: &InstallState, now_secs: u64) -> bool {
    if state.updated_at.saturating_add(STALE_AFTER_SECS) > now_secs {
        return false;
    }
    match state.installer_pid {
        0 => true,
        pid if pid == std::process::id() => false,
        pid => crate::watchdog::process_start_time(pid).is_none(),
    }
}

/// 续作接管判定（--continue 入口用）：常规 stale，或「宣告节心跳过期且
/// 宣告 pid 与状态文件 installer_pid 一致」。后者是 install runtime 挂死的
/// 即时证据——挂死进程 pid 仍存活，常规 stale 判定（10 分钟 + pid 死亡）
/// 抓不住它；宣告心跳停摆（独立 ticker 停止）与 runtime 死锁等价，接管
/// 无需等满 10 分钟。接管后原挂死进程若恢复，写状态文件前的归属校验
/// （refresh 同款）会让它让位退出。
pub fn is_takeable(state: &InstallState, now_secs: u64) -> bool {
    if is_stale(state, now_secs) {
        return true;
    }
    match crate::install::announce::read() {
        Some(ann) if ann.installer_pid == state.installer_pid => {
            crate::watchdog::now_millis().saturating_sub(ann.heartbeat_ms)
                > crate::install::announce::FRESH_MS
        }
        _ => false,
    }
}

/// 安装锁下的新建：状态文件不存在或已 stale 才成功（stale 即接管续跑），
/// 存在且新鲜 → Err（并发安装拒绝）。成功返回带锁信息（pid/时间戳）的状态。
pub fn create_new_in(run_dir: &Path, mut state: InstallState) -> Result<InstallState, String> {
    if let Some(existing) = load_in(run_dir)
        && !is_stale(&existing, now_secs())
    {
        return Err(format!(
            "已有安装进行中（target {}，phase {:?}，pid {}）。\
             等待其完成，或确认其已中断后等待 {} 秒自动失效",
            existing.target_version, existing.phase, existing.installer_pid, STALE_AFTER_SECS
        ));
    }
    // 走到这里的残留（stale/不存在）即接管：不询问直接续跑（用户调 install
    // 的期望就是装完）
    write_in(run_dir, &mut state).map_err(|e| format!("install.state 写入失败: {e}"))?;
    Ok(state)
}

/// 原子重写（tmp + rename，与实例注册表/settings 同思路）并刷新 updated_at。
pub fn write_in(run_dir: &Path, state: &mut InstallState) -> std::io::Result<()> {
    state.updated_at = now_secs();
    let path = state_path_in(run_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state).expect("序列化 install.state 失败");
    let tmp = path.with_extension("state.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 阶段推进：校验迁移合法性后原子落盘。非法迁移是编程错误（状态机被绕过
/// 的信号），返回 Err 拒绝落盘。
pub fn advance_in(
    run_dir: &Path,
    state: &mut InstallState,
    to: InstallPhase,
) -> Result<(), String> {
    if !can_transition(state.phase, to) {
        return Err(format!(
            "非法状态迁移 {:?} → {to:?}（状态机被绕过？）",
            state.phase
        ));
    }
    state.phase = to;
    write_in(run_dir, state)
        .map(|_| ())
        .map_err(|e| format!("install.state 写入失败: {e}"))
}

/// 删除状态文件（done/aborted 后清场；不存在静默成功）。
pub fn remove_in(run_dir: &Path) {
    let _ = std::fs::remove_file(state_path_in(run_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_run_phases() -> impl Iterator<Item = InstallPhase> {
        InstallPhase::RUN_ORDER.iter().copied()
    }

    #[test]
    fn linear_transitions_advance_one_step() {
        // 线性主线：每阶段只能到下一格
        for pair in InstallPhase::RUN_ORDER.windows(2) {
            assert!(
                can_transition(pair[0], pair[1]),
                "{:?} → {:?} 应合法",
                pair[0],
                pair[1]
            );
        }
        // 隔行跳跃全部拒绝
        for i in 0..InstallPhase::RUN_ORDER.len() {
            for j in 0..InstallPhase::RUN_ORDER.len() {
                if j != i + 1 {
                    assert!(
                        !can_transition(InstallPhase::RUN_ORDER[i], InstallPhase::RUN_ORDER[j]),
                        "{:?} → {:?} 应非法（只能前进一格）",
                        InstallPhase::RUN_ORDER[i],
                        InstallPhase::RUN_ORDER[j]
                    );
                }
            }
        }
    }

    #[test]
    fn failed_and_abort_sidelines() {
        use InstallPhase::*;
        // 任意运行阶段 → failed 合法（保留现场）
        for p in all_run_phases() {
            assert!(can_transition(p, Failed), "{p:?} → failed 应合法");
        }
        // abort 只在 ack 及之前（swapping 前可完全回滚），之后只进不退
        for p in [Marking, Downloading, Downloaded, Broadcasting, Acked] {
            assert!(can_transition(p, Aborted), "{p:?} → aborted 应合法");
        }
        for p in [Swapping, Swapped, Relaying, Restarting, Verifying, Cleaning] {
            assert!(!can_transition(p, Aborted), "{p:?} → aborted 应非法");
        }
        // failed 续跑从 downloading 重来
        assert!(can_transition(Failed, Downloading));
        // failed 不能跳到其他运行阶段
        for p in [Marking, Swapping, Restarting, Cleaning] {
            assert!(!can_transition(Failed, p), "failed → {p:?} 应非法");
        }
        // 终态无出边
        for to in all_run_phases().chain([Failed, Aborted, Done]) {
            assert!(!can_transition(Done, to), "done → {to:?} 应非法");
            assert!(!can_transition(Aborted, to), "aborted → {to:?} 应非法");
        }
        // done/failed 不是运行阶段（run_index 为 None）
        assert!(Done.run_index().is_none() && Failed.run_index().is_none());
    }

    #[test]
    fn create_new_lock_and_stale_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");

        // 空目录：创建成功，写入 marking 初始态
        let s = create_new_in(
            &run,
            InstallState::new_marking("0.1.0-alpha.9", InstallSource::Github),
        )
        .unwrap();
        assert_eq!(s.phase, InstallPhase::Marking);
        assert_eq!(s.installer_pid, std::process::id());

        // 新鲜锁：并发安装拒绝
        let err = create_new_in(
            &run,
            InstallState::new_marking("0.1.0-alpha.8", InstallSource::From),
        )
        .unwrap_err();
        assert!(err.contains("已有安装进行中"), "{err}");

        // 残留接管：updated_at 过期 + pid 已死。直接写文件构造（write_in 会
        // 刷新 updated_at，无法用它伪造过期的锁——这正是它自动刷新的意义）
        let write_raw = |s: &InstallState| {
            std::fs::write(state_path_in(&run), serde_json::to_string(s).unwrap()).unwrap();
        };
        let mut stale = load_in(&run).unwrap();
        stale.updated_at = now_secs().saturating_sub(STALE_AFTER_SECS + 60);
        // 双条件验证一：时间过期但 pid 存活（本进程）→ 不算 stale，仍拒
        let mut fresh_alive = stale.clone();
        fresh_alive.installer_pid = std::process::id();
        write_raw(&fresh_alive);
        assert!(
            create_new_in(&run, InstallState::new_marking("x", InstallSource::Github)).is_err(),
            "时间过期但 pid 存活 → 仍是有效锁"
        );
        // 双条件验证二：pid 为确定不存在的值（watchdog 同款哨兵，已验证
        // process_start_time 对其返回 None）→ 接管成功，锁归当前进程
        stale.installer_pid = u32::MAX - 7;
        write_raw(&stale);
        let taken = create_new_in(
            &run,
            InstallState::new_marking("0.1.0-alpha.9", InstallSource::Github),
        )
        .unwrap();
        assert_eq!(
            taken.installer_pid,
            std::process::id(),
            "接管后锁归当前进程"
        );
    }

    #[test]
    fn save_refreshes_updated_at_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let mut s = InstallState::new_marking("0.1.0-alpha.9", InstallSource::Npm);
        s.updated_at = 0;
        write_in(&run, &mut s).unwrap();
        assert!(s.updated_at > 0, "每次写入应自动刷新 updated_at");

        let loaded = load_in(&run).unwrap();
        assert_eq!(loaded.phase, InstallPhase::Marking);
        assert_eq!(loaded.target_version, "0.1.0-alpha.9");
        assert_eq!(loaded.source, InstallSource::Npm);
        assert_eq!(loaded.updated_at, s.updated_at);
    }

    #[test]
    fn advance_validates_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let mut s =
            create_new_in(&run, InstallState::new_marking("v", InstallSource::Github)).unwrap();

        // 非法迁移被拒且不落盘
        assert!(advance_in(&run, &mut s, InstallPhase::Swapping).is_err());
        assert_eq!(load_in(&run).unwrap().phase, InstallPhase::Marking);

        // 合法迁移落盘
        advance_in(&run, &mut s, InstallPhase::Downloading).unwrap();
        assert_eq!(load_in(&run).unwrap().phase, InstallPhase::Downloading);
    }

    #[test]
    fn corrupted_state_file_loads_none() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(state_path_in(&run), "{corrupted").unwrap();
        assert!(
            load_in(&run).is_none(),
            "损坏状态按无/失效处理（调用方走恢复矩阵）"
        );
    }

    #[test]
    fn state_file_roundtrips_full_schema_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        // 全字段状态落盘往返
        let mut s = InstallState::new_marking("0.1.0-alpha.9", InstallSource::From);
        s.phase = InstallPhase::Swapped;
        s.staged_path = Some("C:/h/staging/0.1.0-alpha.9/aproxy.exe".into());
        s.sha256 = Some("abc123".into());
        s.old_path = Some("C:/h/bin/aproxy.old.exe".into());
        s.instance_snapshot = vec!["12345".into(), "12349".into()];
        s.skill = Some(SkillState {
            status: SkillPhase::Downloading,
            attempt: 2,
            version: "0.1.0-alpha.9".into(),
        });
        let mut cur = s.clone();
        write_in(&run, &mut cur).unwrap();
        assert_eq!(load_in(&run).unwrap(), s);

        // 旧版本状态文件（缺新字段）serde default 兼容
        std::fs::write(
            state_path_in(&run),
            r#"{"phase":"restarting","target_version":"0.1.0-alpha.8"}"#,
        )
        .unwrap();
        let old = load_in(&run).unwrap();
        assert_eq!(old.phase, InstallPhase::Restarting);
        assert!(old.instance_snapshot.is_empty() && old.skill.is_none());
    }

    #[test]
    fn remove_clears_lock() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        create_new_in(&run, InstallState::new_marking("v", InstallSource::Github)).unwrap();
        remove_in(&run);
        assert!(load_in(&run).is_none());
        // 幂等：不存在再删不报错
        remove_in(&run);
    }
}
