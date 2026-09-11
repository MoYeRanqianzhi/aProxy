//! install 主流程编排：把状态机、备料、交换、广播、滚动重启、终验、清理
//! 串成一次完整安装（四条铁律的执行主体，顺序即铁律顺序）。
//!
//! ```text
//! 管辖检查 → marking（锁+宣告）→ downloading（备料校验）→ 快照
//!   → broadcasting（换血+广播 ACK）→ swapping（双 rename/原子覆盖）
//!   → relaying（Windows 接力换镜像）→ restarting（逐实例滚动）
//!   → verifying（版本+阶段终验）→ cleaning（删 .old/staging）→ done
//! ```
//!
//! - 失败语义：任何一步 Err → phase=failed 落盘（保留现场）后返回。
//! - 中断语义：任何时刻崩溃/被杀 → install.state 残留，`continue_install`
//!   从残留 phase 幂等续作（断电恢复矩阵的执行体）。
//!
//! 无实例快路径：broadcasting/relaying 直接跳过（restarting 为空）——
//! 后续只剩 cleaning，.old 删不掉（自镜像锁）就保留待下次，计划的有意简化。

use std::path::Path;

use super::staging::stage_from_in;
use super::state::{InstallPhase, InstallSource, InstallState, write_in};
use super::swap;

/// 安装输入（--from/--adopt 归一后的产物）。
pub struct InstallPlan {
    /// staging 校验过的目标版本
    pub target_version: String,
    /// 安装来源
    pub source: InstallSource,
    /// --from 源路径（--adopt = 当前进程镜像）
    pub from: std::path::PathBuf,
}

/// 管辖检查（铁律前置）：运行实例的进程镜像必须在 `<home>/bin/` 下——
/// 包管理器管辖目录的实例交给对应渠道升级，或显式 `--adopt` 收编。
/// 镜像路径不可判（进程刚死/权限）→ 不拦（只拦「确认在管辖外」）。
/// `--adopt` 自身豁免——它就是管辖外迁移的正规通道。
pub async fn check_jurisdiction(run_dir: &Path, home: &Path, adopt: bool) -> Result<(), String> {
    if adopt {
        return Ok(());
    }
    let bin_dir = swap::bin_dir_in(home);
    for port in live_ports(run_dir).await {
        let Ok(info) = crate::daemon::ipc_ping_in(run_dir, &port).await else {
            continue;
        };
        let Some(image) = crate::watchdog::process_image_path(info.pid) else {
            continue;
        };
        if !is_under(&image, &bin_dir) {
            return Err(format!(
                "实例 {port} 的二进制不在安装管辖目录（{}）下：{}。\n\
                 该实例由其他渠道安装（包管理器/npm/cargo），请用对应渠道升级，\
                 或执行 `aproxy install --adopt` 显式收编到标准位置",
                bin_dir.display(),
                image.display()
            ));
        }
    }
    Ok(())
}

/// 路径祖先判断（归一：绝对化 + 小写 + 分隔符统一——Windows 文件系统
/// 大小写不敏感，与 settings::path_match_key 同一比较纪律）。
fn is_under(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        std::path::absolute(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase()
    };
    let (mut p, mut d) = (norm(path), norm(dir));
    if !d.ends_with('\\') {
        d.push('\\');
    }
    if !p.ends_with('\\') {
        p.push('\\');
    }
    p.starts_with(&d)
}

/// 当前存活实例端口（IPC 探活过滤后的真实清单）
async fn live_ports(run_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for info in crate::daemon::list_instances_in(run_dir).await {
        out.push(crate::daemon::port_of(&info.listen_addr).to_string());
    }
    out
}

/// 宣告句柄：创建节 + 独立 ticker 周期 beat。句柄 Drop（任务 abort 或进程
/// 退出）= 宣告解除。创建失败只降级（None = 无宣告，差异化行为退化为常态）。
fn spawn_announcer() -> Option<tokio::task::JoinHandle<()>> {
    super::announce::Announcer::create().map(|a| {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(super::announce::BEAT_INTERVAL).await;
                a.beat();
            }
        })
    })
}

fn stop_announcer(handle: Option<tokio::task::JoinHandle<()>>) {
    if let Some(h) = handle {
        h.abort();
    }
}

/// 流程退出方式：正常完成（done）或 Windows 接力交棒（接管者继续完成
/// 剩余阶段——「旧二进制自动停止」，本进程的任务到此结束）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlowExit {
    Completed,
    HandedOver,
}

/// 三入口共用的外壳：skill 支线并行任务 + 唯一同步点 join + 终态落盘。
///
/// `download_proxy`：本次安装的生效下载代理（CLI --download-proxy 优先于
/// settings 的归一结果，续作入口传 settings 值）——skill 支线与二进制链
/// 同一代理语义，避免「CLI 参数只管二进制、skill 悄悄走别路」的分裂。
///
/// skill 非强制：任务失败不影响 fut 的结果；终态只在 install.state 仍存在
/// 时补写（安装成功场景 state 已被 done 清掉，skill 状态随之不持久——
/// 「安装成功 ⇒ skill 已尽力更新」是合理推断；failed 现场保留时终态可查）。
async fn drive_with_skill(
    home: &Path,
    run_dir: &Path,
    version: &str,
    skill_enabled: bool,
    download_proxy: Option<&str>,
    fut: impl std::future::Future<Output = Result<FlowExit, String>>,
) -> Result<FlowExit, String> {
    let settings = crate::settings::load();
    let mut skill_handle = None;
    if skill_enabled
        && settings.skill_auto_update
        && let Ok(ctx) = super::download::DownloadCtx::build(version, download_proxy, None)
    {
        let chain = super::download::effective_chain(&settings);
        let home_buf = home.to_path_buf();
        let version_buf = version.to_string();
        skill_handle = Some(tokio::spawn(async move {
            let outcome = super::skills::update_skills(&ctx, &chain, &home_buf).await;
            (outcome, version_buf)
        }));
    }

    let result = fut.await;

    if let Some(handle) = skill_handle {
        // 唯一同步点：join 支线收尾（计划定调），但**有预算**（90s）——
        // 支线非强制，网络挂死不得阻塞安装进程退出（HTTP 客户端超时之外
        // 的兜底；超时即放弃终态落盘，任务随进程终止消亡）
        let joined = tokio::time::timeout(std::time::Duration::from_secs(90), handle).await;
        if let Ok(Ok((outcome, ver))) = joined
            && let Some(mut st) = super::state::load_in(run_dir)
        {
            st.skill = Some(super::state::SkillState {
                status: outcome.phase,
                attempt: outcome.attempt,
                version: ver,
            });
            let _ = super::state::write_in(run_dir, &mut st);
        }
    }
    result
}

/// 一次完整安装（--from/--adopt 流水线）。`home`/`run_dir` 显式传入
/// （测试注入 tempdir；生产为同一 APROXY_HOME 派生值）。`download_proxy`
/// 为本次生效的下载代理（CLI 参数优先于 settings 的归一结果，skill 支线
/// 与二进制链共用）。
pub async fn run_install(
    home: &Path,
    run_dir: &Path,
    plan: &InstallPlan,
    skill_enabled: bool,
    download_proxy: Option<&str>,
) -> Result<FlowExit, String> {
    // ---- marking：第一件事写状态文件（铁律 1）——此后任何时刻崩溃，
    // 下一次启动都能识别「正在 install」并恢复。宣告节在停看护者之前创建
    //（守护自检的补种抑制依赖它的有效性，时序严格咬合）。
    let mut state = InstallState::new_marking(plan.target_version.clone(), plan.source);
    state.from_path = Some(plan.from.display().to_string());
    let mut state = super::state::create_new_in(run_dir, state)?;
    let announcer = spawn_announcer();

    drive_with_skill(
        home,
        run_dir,
        &plan.target_version,
        skill_enabled,
        download_proxy,
        async {
            match run_forward(home, run_dir, &mut state, &plan.from).await {
                // 接力交棒：本进程即将退出，宣告 ticker 不 abort（随进程消亡）；
                // 接管者的 --continue 入口创建自己的宣告
                Ok(FlowExit::HandedOver) => Ok(FlowExit::HandedOver),
                Ok(exit) => {
                    stop_announcer(announcer);
                    Ok(exit)
                }
                Err(e) => {
                    // failed 保留现场（staging 不清），续作由 --continue 从残留推进
                    let _ = super::state::advance_in(run_dir, &mut state, InstallPhase::Failed);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// 正向推进（marking 之后的全部阶段）。`state` 已持锁；失败调用方置 failed。
/// `announcer` 由调用方持有（接力交棒的路径不 abort——接管者有自己的宣告，
/// 本进程退出时 ticker 任务随之消亡，节按介质语义自然解除）。
async fn run_forward(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    from: &Path,
) -> Result<FlowExit, String> {
    let new_bin = swap::bin_path_in(home);

    // ---- downloading：先下载后 rename（铁律 2）——备料校验全过才许动 bin
    super::state::advance_in(run_dir, state, InstallPhase::Downloading)?;
    let staged =
        stage_from_in(home, from, &state.target_version).map_err(|e| format!("备料失败: {e}"))?;
    state.staged_path = Some(staged.path.display().to_string());
    state.sha256 = Some(staged.sha256.clone());
    super::state::advance_in(run_dir, state, InstallPhase::Downloaded)?;

    // ---- 实例快照（ACK 阶段清单）
    let snapshot = live_ports(run_dir).await;
    state.instance_snapshot = snapshot.clone();
    write_in(run_dir, state).map_err(|e| format!("install.state 写入失败: {e}"))?;

    // ---- broadcasting：ACK 齐了才交换（铁律 3）。无实例 → 跳过（快路径）
    if !snapshot.is_empty() {
        // 换血（Windows 专属顺序）：广播之前停掉旧看护者——它的 respawn
        // 会用旧镜像（current_exe 指向 rename 后的 .old）拉起旧二进制，
        // 升级永不完成。unix 无此问题（exec 按路径解析自动新二进制）。
        #[cfg(windows)]
        stop_old_watchdog(run_dir);
        super::state::advance_in(run_dir, state, InstallPhase::Broadcasting)?;
        if let Err(bad) = super::broadcast::broadcast_prepare_swap(run_dir, &snapshot).await {
            return Err(format!(
                "PrepareSwap 广播终失败（实例未表达，已按重试/restart 收敛）: {bad:?}。\n\
                 问题实例有隐患，处置指引：将其关闭后重试安装（不强杀——避免服务中断）"
            ));
        }
        super::state::advance_in(run_dir, state, InstallPhase::Acked)?;
    }

    // ---- swapping：竞态窗口防护（acked 与 swapping 之间用户新启实例 →
    // swap 前重比对快照，新增实例也要 ACK）
    let now_live = live_ports(run_dir).await;
    let fresh: Vec<String> = now_live
        .iter()
        .filter(|p| !snapshot.contains(p))
        .cloned()
        .collect();
    if !fresh.is_empty() {
        super::broadcast::broadcast_prepare_swap(run_dir, &fresh)
            .await
            .map_err(|bad| format!("广播后新启实例 ACK 失败: {bad:?}"))?;
    }
    super::state::advance_in(run_dir, state, InstallPhase::Swapping)?;
    let outcome = swap::swap_in(home, &staged.path).map_err(|e| format!("二进制交换失败: {e}"))?;
    state.old_path = outcome.old_path.map(|p| p.display().to_string());
    super::state::advance_in(run_dir, state, InstallPhase::Swapped)?;

    // ---- relaying：Windows 换镜像接力（有实例才需要——无实例时后续只剩
    // cleaning，.old 删不掉就留给下次）。unix 跳过：安装进程路径不变、
    // 内容已换血，直接续跑（状态机 relaying 天然豁免）。
    #[cfg(windows)]
    if !snapshot.is_empty() {
        super::state::advance_in(run_dir, state, InstallPhase::Relaying)?;
        let pid = super::swap::spawn_continuator(&new_bin)
            .map_err(|e| format!("接力 spawn 失败: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        if super::swap::wait_for_takeover(run_dir, deadline) {
            // 新进程接管（phase 已推进 = 自证存活）→ 旧镜像进程自行退出
            //（=「旧二进制自动停止」）。本进程的宣告 ticker 随进程退出消亡，
            // 接管者的 --continue 入口会创建自己的宣告。
            return Ok(FlowExit::HandedOver);
        }
        // 接管失败：不退不阻——续作稍后由看护者拉起；本进程（旧镜像）继续
        // 兜底做完 restarting/verifying（restart_instance 用显式新 exe 路径，
        // 不依赖本进程镜像），仅 cleaning 的 .old 删除（自镜像锁）留给下次
        tracing::warn!(pid, "接力进程未在预期时间内接管，本进程继续兜底");
    }

    run_tail(home, run_dir, state, &new_bin, false).await?;
    Ok(FlowExit::Completed)
}

/// relaying 之后的公共尾部：滚动重启 → 终验 → 清理 → done。
/// `skip_ready`：续作幂等开关——续作跳过已就绪实例（崩溃前已滚动的不再
/// 动）；forward 首次推进必须全部滚动（广播已把实例置入 swap_phase，
/// verifying 要求退出更换阶段，不重启永不过验——「退出更换阶段由逐实例
/// 重启天然完成」）。
async fn run_tail(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    new_bin: &Path,
    skip_ready: bool,
) -> Result<(), String> {
    let snapshot = state.instance_snapshot.clone();

    // ---- restarting：逐实例滚动（任一时刻至多一个下线）
    if !snapshot.is_empty() && state.phase < InstallPhase::Restarting {
        super::state::advance_in(run_dir, state, InstallPhase::Restarting)?;
    }
    for port in &snapshot {
        // 跳过已就绪（仅续作：崩溃前已滚动的实例不再动）
        if skip_ready
            && let Ok(info) = crate::daemon::ipc_ping_in(run_dir, port).await
            && info.version == state.target_version
            && !info.swap_phase
        {
            continue;
        }
        match super::restart::restart_instance(run_dir, port, new_bin).await {
            Ok(pid) => {
                tracing::info!(port = %port, new_pid = pid, "实例已滚动到新版本");
            }
            Err(e) => return Err(format!("实例 {port} 滚动重启失败: {e}")),
        }
        // 每实例重启后推进状态文件（进度可观察 + updated_at 保活）
        write_in(run_dir, state).map_err(|e| format!("install.state 写入失败: {e}"))?;
    }

    // ---- verifying：全实例版本==target 且已退出更换阶段。不满足（restarting
    // 中用户新启的旧版本实例等竞态）→ 补 restart 一轮，仍不满足 → failed。
    // 相位守卫：Cleaning 及以后的续作重入不回退重验（逆向迁移非法）
    if state.phase < InstallPhase::Verifying {
        super::state::advance_in(run_dir, state, InstallPhase::Verifying)?;
    }
    for round in 0..2 {
        let mut bad = Vec::new();
        for port in &snapshot {
            match crate::daemon::ipc_ping_in(run_dir, port).await {
                Ok(info) if info.version == state.target_version && !info.swap_phase => {}
                _ => bad.push(port.clone()),
            }
        }
        if bad.is_empty() {
            break;
        }
        if round == 1 {
            return Err(format!(
                "终验未通过（版本/阶段不符，已补 restart 一轮）: {bad:?}"
            ));
        }
        for port in &bad {
            let _ = super::restart::restart_instance(run_dir, port, new_bin).await;
        }
    }

    // ---- cleaning：删 .old（Windows；短重试吸收进程退出末尾的镜像锁残留
    // 窗口，仍被锁则保留待下次，绝不强杀）+ staging 清理（它是 swapping
    // 中断的恢复源，done 后即无用）+ 入口脚本常驻不删（防线 0 是基础设施）。
    // 相位守卫：Cleaning 残留的续作重入不重复迁移
    if state.phase < InstallPhase::Cleaning {
        super::state::advance_in(run_dir, state, InstallPhase::Cleaning)?;
    }
    if let Some(old) = &state.old_path {
        let mut removed = false;
        for attempt in 0..6 {
            match std::fs::remove_file(old) {
                Ok(()) => {
                    removed = true;
                    break;
                }
                Err(e) if attempt < 5 => {
                    tracing::debug!(path = %old, error = %e, "旧二进制暂不可删，重试");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Err(e) => {
                    tracing::warn!(
                        path = %old,
                        error = %e,
                        "旧二进制删除失败（仍被引用？），保留待下次安装清理"
                    );
                }
            }
        }
        let _ = removed;
    }
    let _ = std::fs::remove_dir_all(super::staging::staging_dir_in(home, &state.target_version));
    super::state::advance_in(run_dir, state, InstallPhase::Done)?;
    // 状态文件删除 = 安装完成（恢复机制的触发依据：任何残留都意味着未完成）
    super::state::remove_in(run_dir);
    Ok(())
}

/// 在线渠道安装：产物已由下载链条落 staging → 从 staged 重入 forward
/// （广播/交换/接力/尾部全链复用）。与 --from 的差异只在备料来源（网络
/// 下载而非本地复制）与 source 记录。
pub async fn run_install_online(
    home: &Path,
    run_dir: &Path,
    version: &str,
    staged: &Path,
    source: InstallSource,
    skill_enabled: bool,
    download_proxy: Option<&str>,
) -> Result<FlowExit, String> {
    let state = InstallState::new_marking(version.to_string(), source);
    let mut state = super::state::create_new_in(run_dir, state)?;
    // 在线路径的备料（下载链条落 staging）发生在建状态之前——state 直接
    // 推进到 Downloaded（staged_path/sha256 补记）。缺此步时 phase 停在
    // Marking，无实例场景 run_forward_from_staged 跳过广播段直进
    // advance(Swapping) → 「非法状态迁移 Marking → Swapping」（实测暴露：
    // 此前在线路径一直被下载问题挡住，从未跑通到安装段）
    super::state::advance_in(run_dir, &mut state, InstallPhase::Downloading)?;
    state.staged_path = Some(staged.display().to_string());
    state.sha256 = Some(crate::install::staging::sha256_hex(staged)?);
    super::state::advance_in(run_dir, &mut state, InstallPhase::Downloaded)?;
    let announcer = spawn_announcer();

    drive_with_skill(
        home,
        run_dir,
        version,
        skill_enabled,
        download_proxy,
        async {
            match run_forward_from_staged(home, run_dir, &mut state, staged).await {
                Ok(FlowExit::HandedOver) => Ok(FlowExit::HandedOver),
                Ok(exit) => {
                    stop_announcer(announcer);
                    Ok(exit)
                }
                Err(e) => {
                    let _ = super::state::advance_in(run_dir, &mut state, InstallPhase::Failed);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// --continue 续作（隐藏标志，恢复机制的统一入口）：读状态文件 → 判定
/// 当前 phase → 从该步幂等推进。全自动无人工询问（用户调 install 的期望
/// 就是「装完」，安装的每一步本就安全/幂等/可回滚，续作无破坏性）。
pub async fn continue_install(home: &Path, run_dir: &Path) -> Result<FlowExit, String> {
    let Some(mut state) = super::state::load_in(run_dir) else {
        return Ok(FlowExit::Completed); // 无残留，无事发生
    };
    if state.phase.is_terminal() {
        // done/aborted 残留（正常结束但清文件前崩溃）→ 清文件即完成
        super::state::remove_in(run_dir);
        return Ok(FlowExit::Completed);
    }
    // 接管判定：常规 stale（超时+原进程死）或宣告心跳过期（runtime 挂死
    // 的即时证据）。**relaying 是接棒约定态**——它只可能由安装者在 spawn
    // 接棒者之后写入，本进程读到它即是被接棒者（豁免健康性检查：安装者
    // 此刻必然活着且在等接管确认）。其余 phase 原安装进程健康 → 不动
    // （并发防重；误触发的 --continue 与健康安装并存时拒绝是正确行为）。
    if state.phase != InstallPhase::Relaying
        && !super::state::is_takeable(&state, crate::watchdog::now_secs())
    {
        return Err(format!(
            "安装仍在进行（phase {:?}，pid {}），不重复接管",
            state.phase, state.installer_pid
        ));
    }
    tracing::info!(phase = ?state.phase, target = %state.target_version, "install 续作接管");
    state.installer_pid = std::process::id();
    write_in(run_dir, &mut state).map_err(|e| format!("install.state 写入失败: {e}"))?;
    let announcer = spawn_announcer();
    // skill 续作语义：failed **不自动重试**（避免每次续作都拖一遍下载），
    // --skills-only 手动重试；其余状态（Downloading 中断等）照常跑
    let skill_should_run = !matches!(
        state.skill.as_ref().map(|s| s.status),
        Some(crate::install::state::SkillPhase::Failed)
    );
    let target_version = state.target_version.clone();
    // 续作无 CLI 上下文，下载代理按 settings（与首跑 --download-proxy 未设
    // 时的归一结果一致）
    let settings = crate::settings::load();

    drive_with_skill(
        home,
        run_dir,
        &target_version,
        skill_should_run,
        settings.download_proxy.as_deref(),
        async {
            let result = match state.phase {
                // 备料前/备料中中断：staging 可能半截——重新备料（幂等重做；
                // --from 源路径已记录；网络渠道续作因 from_path 缺失而请用户重跑）
                InstallPhase::Marking | InstallPhase::Downloading | InstallPhase::Failed => {
                    let Some(from) = state.from_path.clone() else {
                        stop_announcer(announcer);
                        return Err(
                            "续作缺少 --from 源记录（状态文件损坏？），请重新执行安装".into()
                        );
                    };
                    run_forward(home, run_dir, &mut state, Path::new(&from)).await
                }
                // 备料完成、交换前：staging 完整在盘——直接从广播重入
                InstallPhase::Downloaded | InstallPhase::Broadcasting | InstallPhase::Acked => {
                    let Some(staged_path) = state.staged_path.clone() else {
                        stop_announcer(announcer);
                        return Err(
                            "续作缺少 staging 记录（状态文件损坏？），请重新执行安装".into()
                        );
                    };
                    run_forward_from_staged(home, run_dir, &mut state, Path::new(&staged_path))
                        .await
                }
                // swapping 中断：Windows bin 可能空窗（旧已改名新未落位）——swap 原语
                // 对「bin 缺失」幂等（首次安装同款路径），重新执行交换即恢复；
                // unix 两态（rename 前中断 = 原文件完好 / rename 后 = 新文件已就位）
                // 都无需修复。重入走广播（ACK 是幂等置位）+ 交换。
                InstallPhase::Swapping => {
                    let Some(staged_path) = state.staged_path.clone() else {
                        stop_announcer(announcer);
                        return Err(
                            "续作缺少 staging 记录（状态文件损坏？），请重新执行安装".into()
                        );
                    };
                    run_forward_from_staged(home, run_dir, &mut state, Path::new(&staged_path))
                        .await
                }
                // 交换完成及以后：新 bin 已在规范位置——直接进尾部（滚动/终验/清理）
                InstallPhase::Swapped
                | InstallPhase::Relaying
                | InstallPhase::Restarting
                | InstallPhase::Verifying
                | InstallPhase::Cleaning => {
                    run_tail(home, run_dir, &mut state, &swap::bin_path_in(home), true)
                        .await
                        .map(|_| FlowExit::Completed)
                }
                InstallPhase::Done | InstallPhase::Aborted => unreachable!("终态已在入口处理"),
            };

            match result {
                // 接力交棒：同 run_install——本进程即将退出，ticker 不 abort
                Ok(FlowExit::HandedOver) => Ok(FlowExit::HandedOver),
                Ok(exit) => {
                    stop_announcer(announcer);
                    Ok(exit)
                }
                Err(e) => {
                    let _ = super::state::advance_in(run_dir, &mut state, InstallPhase::Failed);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// 从已就绪的 staged 副本重入（broadcasting 及以后的续作——不重新备料，
/// staging 是 swapping 中断的恢复源）。与 run_forward 共享广播/交换/尾部
/// 逻辑，差异只在 downloading 段被跳过。
async fn run_forward_from_staged(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    staged_path: &Path,
) -> Result<FlowExit, String> {
    if !staged_path.is_file() {
        // staging 也没了（极端中的极端）→ 从 --from 源重新备料兜底
        let Some(from) = state.from_path.clone() else {
            return Err("staging 与 --from 源均已缺失，请重新执行安装".into());
        };
        return run_forward(home, run_dir, state, Path::new(&from)).await;
    }
    let snapshot = live_ports(run_dir).await;
    state.instance_snapshot = snapshot.clone();
    write_in(run_dir, state).map_err(|e| format!("install.state 写入失败: {e}"))?;
    if !snapshot.is_empty() {
        #[cfg(windows)]
        stop_old_watchdog(run_dir);
        // 相位守卫（与 run_tail 同款）：Acked/Swapping 残留的续作重入时
        // phase 已高于 Broadcasting/Acked——逆向迁移非法，保留高位重做即可
        //（广播是幂等置位）
        if state.phase < InstallPhase::Broadcasting {
            super::state::advance_in(run_dir, state, InstallPhase::Broadcasting)?;
        }
        if let Err(bad) = super::broadcast::broadcast_prepare_swap(run_dir, &snapshot).await {
            return Err(format!("PrepareSwap 广播终失败: {bad:?}"));
        }
        if state.phase < InstallPhase::Acked {
            super::state::advance_in(run_dir, state, InstallPhase::Acked)?;
        }
    }
    let now_live = live_ports(run_dir).await;
    let fresh: Vec<String> = now_live
        .iter()
        .filter(|p| !snapshot.contains(p))
        .cloned()
        .collect();
    if !fresh.is_empty() {
        super::broadcast::broadcast_prepare_swap(run_dir, &fresh)
            .await
            .map_err(|bad| format!("广播后新启实例 ACK 失败: {bad:?}"))?;
    }
    super::state::advance_in(run_dir, state, InstallPhase::Swapping)?;
    let outcome = swap::swap_in(home, staged_path).map_err(|e| format!("二进制交换失败: {e}"))?;
    state.old_path = outcome.old_path.map(|p| p.display().to_string());
    super::state::advance_in(run_dir, state, InstallPhase::Swapped)?;
    #[cfg(windows)]
    if !snapshot.is_empty() {
        super::state::advance_in(run_dir, state, InstallPhase::Relaying)?;
        let new_bin = swap::bin_path_in(home);
        if let Ok(pid) = super::swap::spawn_continuator(&new_bin) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            if super::swap::wait_for_takeover(run_dir, deadline) {
                return Ok(FlowExit::HandedOver);
            }
            tracing::warn!(pid, "接力进程未在预期时间内接管，本进程继续兜底");
        }
    }
    run_tail(home, run_dir, state, &swap::bin_path_in(home), false).await?;
    Ok(FlowExit::Completed)
}

/// 停掉旧看护者（Windows 换血，广播之前）：身份验证 + 终止 + 删 claim。
/// 看护者不承载流量，零服务影响；换血窗口内实例崩溃 = 短时失去自动重拉，
/// 可接受微窗（新看护者由 relaying 后的新二进制 ensure 逻辑重新出簇）。
#[cfg(windows)]
fn stop_old_watchdog(run_dir: &Path) {
    let Some(claim) = crate::watchdog::read_claim_in(run_dir) else {
        return; // 无看护者，无事
    };
    crate::watchdog::remove_claim_in(run_dir);
    let alive = crate::watchdog::is_aproxy_process(claim.pid)
        && crate::watchdog::verify_claim_identity(claim.pid, claim.created_at_process);
    if alive {
        tracing::info!(
            pid = claim.pid,
            "停掉旧看护者（换血：其 respawn 会用旧镜像）"
        );
        crate::watchdog::terminate_verified_process(claim.pid);
    }
}
