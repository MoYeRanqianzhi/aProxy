//! install 主流程编排：把状态机、备料、交换、广播、滚动重启、终验、清理
//! 串成一次完整安装（四条铁律的执行主体，顺序即铁律顺序）。
//!
//! ```text
//! 管辖检查 → marking（锁+宣告）→ downloading（备料校验）
//!   → 早交接（交给 staging 里的目标二进制，本进程只等结局）→ 快照
//!   → broadcasting（换血+广播 ACK）→ swapping（双 rename/原子覆盖）
//!   → （Windows）交给 bin 里刚落位的二进制 → restarting（逐实例滚动）
//!   → verifying（版本+阶段终验）→ cleaning（删 .old/staging）→ done
//! ```
//!
//! - 早交接：备料校验通过后，广播、交换、滚动、终验、清理一律由**目标版本**
//!   的二进制执行（`install --continue --handover-from <安装者 pid>`），安装者
//!   只等待并转告结局。这样每次升级都由新版本驱动：新版本能修旧安装器的缺陷，
//!   兼容只需要「新读旧」（新版本读旧记录、用旧方言联系旧实例），只有
//!   install.state 需要旧版本读得懂（等待中的安装者靠它报告结局，所以阶段词表
//!   冻结）。降级（目标比本进程旧）不交：较新的一方驱动，见 [`target_drives`]。
//!   `--handover-from` 与它的接手语义从此是永久接口，以后的版本都要认。
//! - 失败语义：任何一步 Err → phase=failed + 原因（last_error）落盘（保留
//!   现场）后返回。滚动重启中某实例在新版本下起不来 → 用旧二进制按原参数
//!   把它拉回（只恢复服务，阶段不回退），置 halted 中止滚动，自动续作不再
//!   重试（见 restart.rs 模块文档）。
//! - 中断语义：任何时刻崩溃/被杀 → install.state 残留，`continue_install`
//!   从残留 phase 幂等续作（断电恢复矩阵的执行体）。
//!
//! 无实例快路径：broadcasting 直接跳过（restarting 为空）。relaying 阶段只由
//! 0.1.0 的安装器写入（它在 Windows 交换后接力），本版本不再进入。

use std::path::Path;

use super::staging::stage_from_in;
use super::state::{InstallPhase, InstallSource, InstallState, SkillPhase, write_in};
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
        let Some(image) = crate::watchdog::process_image_path(info.instance.pid) else {
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
        out.push(crate::daemon::port_of(&info.instance.listen_addr).to_string());
    }
    out
}

/// 实例快照 = 存活实例 ∪ 「被本安装停止、尚未恢复」的在途实例。返回
/// `(live, snapshot)`：广播只对 live（不在跑的实例没有 IPC 可表达）；滚动
/// 重启按 snapshot（在途实例由 restart 原语用在途记录直接拉起）。
///
/// 只靠 IPC 枚举重算快照会把在途实例永久排除——安装进程在「旧实例已停、
/// 新实例未就绪」的窗口崩溃后续作，该实例既不在跑也已没了 `.restore`，
/// 续作照常完成而它从此下线。顺手为缺 `.restore` 的在途实例补写一份：
/// 续作若在拉起它之前再次失败，`aproxy restore` 仍能把它恢复。
async fn snapshot_with_pending(run_dir: &Path, state: &InstallState) -> (Vec<String>, Vec<String>) {
    let live = live_ports(run_dir).await;
    let mut snapshot = live.clone();
    for p in &state.pending_restores {
        if snapshot.contains(&p.port) {
            continue;
        }
        if !crate::daemon::restore_file_path_in(run_dir, &p.port).is_file() {
            let _ = crate::daemon::write_restore_file_in(run_dir, &p.port, &p.args, &p.log_path);
        }
        snapshot.push(p.port.clone());
    }
    (live, snapshot)
}

/// 回滚用旧二进制：本次交换记下的 old_path（Windows `.old` / unix 交换前
/// 保留的副本）。不存在（首次安装、已被清理）→ None，实例起不来时只能
/// 保住 `.restore` 下线。
fn rollback_binary(state: &InstallState) -> Option<std::path::PathBuf> {
    state
        .old_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
}

/// 宣告：节本身 + 独立 ticker 周期 beat。
struct AnnounceGuard {
    announcer: std::sync::Arc<super::announce::Announcer>,
    ticker: tokio::task::JoinHandle<()>,
}

/// 创建宣告并开始 beat。创建失败只降级（None = 无宣告，差异化行为退化为常态）。
fn spawn_announcer(run_dir: &Path) -> Option<AnnounceGuard> {
    let announcer = std::sync::Arc::new(super::announce::Announcer::create(run_dir)?);
    let beating = announcer.clone();
    let ticker = tokio::spawn(async move {
        loop {
            tokio::time::sleep(super::announce::BEAT_INTERVAL).await;
            beating.beat();
        }
    });
    Some(AnnounceGuard { announcer, ticker })
}

/// 撤宣告（完成或失败）：ticker 停下后最后一个引用释放，节随之解除。
fn stop_announcer(guard: Option<AnnounceGuard>) {
    if let Some(g) = guard {
        g.ticker.abort();
    }
}

/// 交棒之后：停止 beat，但不撤节——接手者已在同一个节名上宣告（unix 上就是
/// 同一个文件，撤销会把它的宣告一起删掉）。本进程还要留下来等结局，若继续
/// beat，两个进程会以不同 pid 交替写同一宣告节，挂死判定（宣告 pid 与
/// installer_pid 比对）随之失真。
fn release_announcer(guard: Option<AnnounceGuard>) {
    if let Some(g) = guard {
        g.announcer.release();
        g.ticker.abort();
    }
}

/// 结局对应的宣告收尾：交棒是释放，其余（完成）是撤销。
fn end_announcer(guard: Option<AnnounceGuard>, exit: FlowExit) {
    match exit {
        FlowExit::HandedOver => release_announcer(guard),
        FlowExit::Completed => stop_announcer(guard),
    }
}

/// 流程退出方式：正常完成（done）或交棒（接手者继续完成剩余阶段，本进程
/// 只剩等待并转告结局，见 commands/install.rs 的 await_handover）。
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
        // 只在状态文件仍归本进程时补写：交棒之后它归接手者，这里的「读出 → 改
        // skill → 写回」会用旧内容盖掉接手者刚推进的阶段
        if let Ok(Ok((outcome, ver))) = joined
            && let Some(mut st) = super::state::load_in(run_dir)
            && st.installer_pid == std::process::id()
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
    note_skill_choice(&mut state, skill_enabled);
    let mut state = super::state::create_new_in(run_dir, state)?;
    let announcer = spawn_announcer(run_dir);

    drive_with_skill(
        home,
        run_dir,
        &plan.target_version,
        skill_enabled,
        download_proxy,
        async {
            match run_forward(home, run_dir, &mut state, &plan.from, true).await {
                Ok(exit) => {
                    end_announcer(announcer, exit);
                    Ok(exit)
                }
                Err(e) => {
                    // failed 保留现场（staging 不清），续作由 --continue 从残留推进
                    super::state::fail_in(run_dir, &mut state, &e);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// 本次安装不更新 skill（--no-skills）就在状态里记下 skipped：安装者中途死掉、
/// 由别的进程续作时，续作据此不去下载 skill。
fn note_skill_choice(state: &mut InstallState, skill_enabled: bool) {
    if !skill_enabled {
        state.skill = Some(super::state::SkillState {
            status: SkillPhase::Skipped,
            attempt: 0,
            version: state.target_version.clone(),
        });
    }
}

/// 备料（downloading），然后从 staged 副本继续（见 run_forward_from_staged）。
/// `state` 已持锁；失败调用方置 failed。
async fn run_forward(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    from: &Path,
    may_hand_over: bool,
) -> Result<FlowExit, String> {
    // ---- downloading：先下载后 rename（铁律 2）——备料校验全过才许动 bin
    super::state::advance_in(run_dir, state, InstallPhase::Downloading)?;
    let staged =
        stage_from_in(home, from, &state.target_version).map_err(|e| format!("备料失败: {e}"))?;
    state.staged_path = Some(staged.path.display().to_string());
    state.sha256 = Some(staged.sha256.clone());
    super::state::advance_in(run_dir, state, InstallPhase::Downloaded)?;
    run_forward_from_staged(home, run_dir, state, &staged.path, may_hand_over).await
}

/// 谁来驱动交换及之后的一切：目标不比本进程旧（升级、同版本重装）就交给目标
/// 二进制；降级由本进程驱动——较新的一方认得较旧一方的记录与方言，反过来不
/// 成立（0.1.0 就不认 `--handover-from`）。版本号解析不了时不交，本进程自己驱动。
fn target_drives(target: &str) -> bool {
    match (
        semver::Version::parse(target),
        semver::Version::parse(env!("CARGO_PKG_VERSION")),
    ) {
        (Ok(target), Ok(own)) => target >= own,
        _ => false,
    }
}

/// 接手者继承本进程的环境，按它重新推导 home 与 run 目录。只有推导结果与本次
/// 安装用的一致才能交棒，否则接手者会去另一个 home 找 install.state（库层测试
/// 显式传 home、进程环境却指向别处，就是这种情形）。
fn successor_sees_same_home(home: &Path, run_dir: &Path) -> bool {
    crate::settings::home() == home && crate::daemon::run_dir() == run_dir
}

/// 本进程是否就是 `path` 这个文件（规范化后比较）。
fn is_current_exe(path: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    match (
        std::env::current_exe().ok().and_then(|p| canonical(&p)),
        canonical(path),
    ) {
        (Some(own), Some(other)) => own == other,
        _ => false,
    }
}

/// 等接手者确认的上限。接手只是读写一次 install.state，正常不到一秒；余量留给
/// 杀毒软件对新下载二进制的首次扫描。
const TAKEOVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 被拉起的接手者进程。unix 留着 Child：它是本进程的子进程，没接手就退出时会
/// 变成僵尸，按 pid 查进程态认不出（macOS 上更是查不了），try_wait 判活并顺带
/// 收割。Windows 经 spawn_detached 拉起：不继承句柄，接手者不会捏着本进程的
/// stdout 管道，让调用 install 的人等它退出才读到 EOF；判活查退出码。
struct Successor {
    pid: u32,
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    start: Option<u64>,
}

impl Successor {
    fn spawn(exe: &Path) -> std::io::Result<Self> {
        let args = [
            "install".to_string(),
            "--continue".to_string(),
            "--handover-from".to_string(),
            std::process::id().to_string(),
        ];
        #[cfg(unix)]
        {
            use std::process::Stdio;
            let child = std::process::Command::new(exe)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            Ok(Self {
                pid: child.id(),
                child,
            })
        }
        #[cfg(windows)]
        {
            let pid = crate::daemon::spawn_detached(exe, &args)?;
            Ok(Self {
                pid,
                start: crate::watchdog::process_start_time(pid),
            })
        }
    }

    fn exited(&mut self) -> bool {
        #[cfg(unix)]
        {
            !matches!(self.child.try_wait(), Ok(None))
        }
        #[cfg(windows)]
        {
            crate::watchdog::process_exited(self.pid) == Some(true)
        }
    }

    /// 已接手：本进程不再管它，但 unix 上还得收割——它退出后若一直是僵尸，
    /// 等结局时按 pid 判活会把它当成还活着，「接手者已死而安装未终结」要拖到
    /// 等待超时才报出来（见 commands/install.rs 的 await_handover）。
    fn detach(self) {
        #[cfg(unix)]
        {
            let mut child = self.child;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    /// 终止并等它退出：此后它不可能再接手。
    async fn kill(&mut self) {
        #[cfg(unix)]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        #[cfg(windows)]
        {
            if let Some(start) = self.start {
                let _ = crate::watchdog::terminate_verified_process(self.pid, start);
            }
            for _ in 0..50 {
                if self.exited() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// 把安装交给 `exe`：以 `install --continue --handover-from <本进程 pid>` 拉起它，
/// 等它把 install.state 的 installer_pid 改成自己（接手，见 continue_install）。
/// 返回 HandedOver 之后本进程不再写状态文件。它没接手就退出、或超时（先终止它并
/// 等它退出，它就不可能在本进程放弃之后才接手）→ Err，状态文件仍归本进程。
/// 调用方的宣告此间照常 beat：没被指名的续作（CLI 入口、看护者拉起的）据此
/// 判定安装者健在，不来争抢。
async fn hand_over(run_dir: &Path, exe: &Path) -> Result<FlowExit, String> {
    let me = std::process::id();
    let mut successor =
        Successor::spawn(exe).map_err(|e| format!("无法启动 {}: {e}", exe.display()))?;
    let taken = || match super::state::load_in(run_dir) {
        Some(s) => s.installer_pid != me,
        // 状态文件没了 = 接手者已经做完（done 清场）；读不出来但文件还在不算
        None => !super::state::state_path_in(run_dir).exists(),
    };
    let deadline = std::time::Instant::now() + TAKEOVER_TIMEOUT;
    let outcome = loop {
        if taken() {
            break Ok(FlowExit::HandedOver);
        }
        let gave_up = if successor.exited() {
            format!("{}（pid {}）没有接手就退出了", exe.display(), successor.pid)
        } else if std::time::Instant::now() >= deadline {
            successor.kill().await;
            format!(
                "{}（pid {}）{} 秒内没有接手，已终止",
                exe.display(),
                successor.pid,
                TAKEOVER_TIMEOUT.as_secs()
            )
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            continue;
        };
        // 它退出之前的最后一次写可能正是接手
        break if taken() {
            Ok(FlowExit::HandedOver)
        } else {
            Err(gave_up)
        };
    };
    if outcome.is_ok() {
        successor.detach();
    }
    outcome
}

/// 交换之后的公共尾部：滚动重启 → 终验 → 清理 → done。
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
            && info.instance.version == state.target_version
            && info.state == crate::daemon::InstanceState::Serving
        {
            continue;
        }
        // 任一实例失败即中止滚动、剩余实例不再动（restart 原语已就该实例
        // 尽力恢复服务并置 halted；不动剩余实例 = 不再扩大影响面）
        let fallback = rollback_binary(state);
        match super::restart::restart_instance(run_dir, state, port, new_bin, fallback.as_deref())
            .await
        {
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
                Ok(info)
                    if info.instance.version == state.target_version
                        && info.state == crate::daemon::InstanceState::Serving => {}
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
            let fallback = rollback_binary(state);
            // 补 restart 的「未被动过」类失败照旧忽略（下一轮终验如实判定）；
            // 实例级失败（新版本起不来、已回滚/下线）必须立即中止
            match super::restart::restart_instance(
                run_dir,
                state,
                port,
                new_bin,
                fallback.as_deref(),
            )
            .await
            {
                Ok(_) | Err(super::restart::RestartError::NotRestarted(_)) => {}
                Err(e) => return Err(format!("实例 {port} 终验补重启失败: {e}")),
            }
        }
    }

    // ---- cleaning：删 .old 与本次的 staging（它是 swapping 中断的恢复源，done
    // 后即无用）；入口脚本常驻不删（防线 0 是基础设施）。两者在 Windows 上都可能
    // 暂时被镜像锁住：.old 是等待结局的安装者的镜像，它看到 cleaning 才退出；
    // staging 里的副本可能是刚把剩余阶段交出来的那个进程的镜像。短重试吸收这段
    // 退出窗口，仍删不掉就保留待下次，绝不强杀。
    // 相位守卫：Cleaning 残留的续作重入不重复迁移
    if state.phase < InstallPhase::Cleaning {
        super::state::advance_in(run_dir, state, InstallPhase::Cleaning)?;
    }
    let staging = super::staging::staging_dir_in(home, &state.target_version);
    let gone = |result: std::io::Result<()>| match result {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    };
    for attempt in 0..6 {
        let old_gone = state
            .old_path
            .as_ref()
            .is_none_or(|old| gone(std::fs::remove_file(old)));
        let staging_gone = gone(std::fs::remove_dir_all(&staging));
        if old_gone && staging_gone {
            break;
        }
        if attempt == 5 {
            tracing::warn!(
                old = ?state.old_path,
                staging = %staging.display(),
                "旧二进制或 staging 删除失败（仍被占用？），保留待下次安装清理"
            );
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    super::state::advance_in(run_dir, state, InstallPhase::Done)?;
    // 状态文件删除 = 安装完成（恢复机制的触发依据：任何残留都意味着未完成）
    super::state::remove_in(run_dir);
    Ok(())
}

/// 在线渠道安装：产物已由下载链条落 staging → 从 staged 继续（交棒或自己
/// 推进，与 --from 同一条路）。与 --from 的差异只在备料来源（网络下载而非
/// 本地复制）与 source 记录。
pub async fn run_install_online(
    home: &Path,
    run_dir: &Path,
    version: &str,
    staged: &Path,
    source: InstallSource,
    skill_enabled: bool,
    download_proxy: Option<&str>,
) -> Result<FlowExit, String> {
    let mut state = InstallState::new_marking(version.to_string(), source);
    note_skill_choice(&mut state, skill_enabled);
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
    let announcer = spawn_announcer(run_dir);

    drive_with_skill(
        home,
        run_dir,
        version,
        skill_enabled,
        download_proxy,
        async {
            match run_forward_from_staged(home, run_dir, &mut state, staged, true).await {
                Ok(exit) => {
                    end_announcer(announcer, exit);
                    Ok(exit)
                }
                Err(e) => {
                    super::state::fail_in(run_dir, &mut state, &e);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// 快照里的每个实例都在应答、状态为 serving、版本是目标版本，且进程镜像就是
/// `<home>/bin` 里的二进制。只比版本不够：回滚到旧二进制的实例与目标版本号
/// 相同时（同版本重装、测试）也会对上。快照为空不下结论。
async fn fleet_already_on_target(home: &Path, run_dir: &Path, state: &InstallState) -> bool {
    if state.instance_snapshot.is_empty() {
        return false;
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    let Some(bin) = canonical(&swap::bin_path_in(home)) else {
        return false;
    };
    for port in &state.instance_snapshot {
        let Ok(status) = crate::daemon::ipc_ping_in(run_dir, port).await else {
            return false;
        };
        let image =
            crate::watchdog::process_image_path(status.instance.pid).and_then(|p| canonical(&p));
        if status.instance.version != state.target_version
            || status.state != crate::daemon::InstanceState::Serving
            || image.as_ref() != Some(&bin)
        {
            return false;
        }
    }
    true
}

/// --continue 续作（隐藏标志，恢复机制与交棒的统一入口）：读状态文件 → 判定
/// 当前 phase → 从该步幂等推进。全自动无人工询问（用户调 install 的期望
/// 就是「装完」，安装的每一步本就安全/幂等/可回滚，续作无破坏性）。
///
/// `handover_from`：`--handover-from` 的值，非 None 表示本进程是被安装者指名
/// 拉起的接手者（见 [`hand_over`]）。
pub async fn continue_install(
    home: &Path,
    run_dir: &Path,
    handover_from: Option<u32>,
) -> Result<FlowExit, String> {
    let Some(mut state) = super::state::load_in(run_dir) else {
        return Ok(FlowExit::Completed); // 无残留，无事发生
    };
    if state.phase.is_terminal() {
        // done/aborted 残留（正常结束但清文件前崩溃）→ 清文件即完成
        super::state::remove_in(run_dir);
        return Ok(FlowExit::Completed);
    }
    // 实例级失败后中止的安装不自动重试：原因（多为新版本不接受现有配置）
    // 不会自己消失，每次续作都会让该实例再经历一次「停止 → 起不来 → 拉回」
    // 的中断——看护者/CLI 入口会反复拉起续作，这里必须原地拒绝且不碰现场。
    // 用户排除原因后显式重新执行 install（新安装接管 stale 残留）收尾。
    if state.phase == InstallPhase::Failed && state.halted {
        // 例外：快照里的实例其实都已从规范位置的新二进制、以目标版本正常服务
        // （例如 0.1.0 的安装器在 Windows 上没等到接力者确认，自己核验时找不到
        // 新实例而判了失败）。没有什么可重试的，收尾清场——否则这份现场会让
        // 看护者每天拉起一次注定被拒的续作，`install latest` 又报「已是最新」
        if fleet_already_on_target(home, run_dir, &state).await {
            tracing::info!(target = %state.target_version, "失败现场的实例均已在目标版本上运行，收尾清场");
            super::state::remove_in(run_dir);
            return Ok(FlowExit::Completed);
        }
        return Err(format!(
            "上次安装因实例级失败已中止，不自动重试（{}）。排除原因后重新执行 aproxy install",
            state.last_error.as_deref().unwrap_or("原因未记录")
        ));
    }
    // 接管判定：常规 stale（超时+原进程死）或宣告心跳过期（runtime 挂死
    // 的即时证据）。原安装进程健康 → 不动（并发防重；误触发的 --continue 与
    // 健康安装并存时拒绝是正确行为）。两条豁免，安装者此刻都活着、在等接手：
    // - 指名接手：安装者以 `--handover-from <它的 pid>` 拉起本进程，且状态文件
    //   仍归它（installer_pid 相同）。CLI 入口与看护者拉起的续作不带这个参数，
    //   不会与指名的接手者抢同一个安装。
    // - relaying：0.1.0 的安装器在 Windows 交换后写下它、再拉起 bin 里的
    //   `install --continue`（不带参数）。只为从 0.1.0 原地升级保留，0.2.0 随
    //   compat_0_1_0 一起删除。
    let named = handover_from.is_some_and(|pid| pid == state.installer_pid);
    let relayed_by_0_1_0 = state.phase == InstallPhase::Relaying;
    if !named
        && !relayed_by_0_1_0
        && !super::state::is_takeable(run_dir, &state, crate::watchdog::now_secs())
    {
        return Err(format!(
            "安装仍在进行（phase {:?}，pid {}），不重复接管",
            state.phase, state.installer_pid
        ));
    }
    tracing::info!(phase = ?state.phase, target = %state.target_version, named, "install 续作接管");
    state.installer_pid = std::process::id();
    write_in(run_dir, &mut state).map_err(|e| format!("install.state 写入失败: {e}"))?;
    let announcer = spawn_announcer(run_dir);
    // skill 支线：交到手里的安装，skill 归交出它的那个进程（它还活着、带着
    // 用户的 --no-skills/--download-proxy 在跑），这里不再跑一遍；接管死掉的
    // 安装时，failed 与 skipped 都不自动重跑（failed 避免每次续作都拖一遍
    // 下载，--skills-only 手动重试；skipped 是用户的 --no-skills）
    let skill_should_run = !named
        && !relayed_by_0_1_0
        && !matches!(
            state.skill.as_ref().map(|s| s.status),
            Some(SkillPhase::Failed | SkillPhase::Skipped)
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
                    run_forward(home, run_dir, &mut state, Path::new(&from), !named).await
                }
                // 备料完成、交换前：staging 完整在盘——直接从广播重入。
                // swapping 中断：Windows bin 可能空窗（旧已改名新未落位）——swap 原语
                // 对「bin 缺失」幂等（首次安装同款路径），重新执行交换即恢复；
                // unix 两态（rename 前中断 = 原文件完好 / rename 后 = 新文件已就位）
                // 都无需修复。重入走广播（ACK 是幂等置位）+ 交换。
                InstallPhase::Downloaded
                | InstallPhase::Broadcasting
                | InstallPhase::Acked
                | InstallPhase::Swapping => {
                    let Some(staged_path) = state.staged_path.clone() else {
                        stop_announcer(announcer);
                        return Err(
                            "续作缺少 staging 记录（状态文件损坏？），请重新执行安装".into()
                        );
                    };
                    run_forward_from_staged(
                        home,
                        run_dir,
                        &mut state,
                        Path::new(&staged_path),
                        !named,
                    )
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
                Ok(exit) => {
                    end_announcer(announcer, exit);
                    Ok(exit)
                }
                Err(e) => {
                    super::state::fail_in(run_dir, &mut state, &e);
                    stop_announcer(announcer);
                    Err(e)
                }
            }
        },
    )
    .await
}

/// 从已就绪的 staged 副本推进：（早交接）→ 快照 → 广播 → 交换 → 尾部。新安装
/// 备料之后、downloaded..swapping 的续作都从这里进（不重新备料，staging 是
/// swapping 中断的恢复源）。
///
/// `may_hand_over`：本进程可以把安装交给目标二进制（早交接，见模块文档）。
/// 指名接手者传 false——它就是被交到手里的那一个，不再往下交。不靠「本进程
/// 是不是 staged 那个文件」防循环：staged 若是转调真二进制的包装，那个判断
/// 认不出来，会一直交下去。
async fn run_forward_from_staged(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    staged_path: &Path,
    may_hand_over: bool,
) -> Result<FlowExit, String> {
    if !staged_path.is_file() {
        // staging 也没了（极端中的极端）→ 从 --from 源重新备料兜底
        let Some(from) = state.from_path.clone() else {
            return Err("staging 与 --from 源均已缺失，请重新执行安装".into());
        };
        return Box::pin(run_forward(
            home,
            run_dir,
            state,
            Path::new(&from),
            may_hand_over,
        ))
        .await;
    }

    // ---- 早交接：此刻实例与 bin 都还没动，交不出去就以失败收场，现场是干净的
    if may_hand_over
        && target_drives(&state.target_version)
        && !is_current_exe(staged_path)
        && successor_sees_same_home(home, run_dir)
    {
        return hand_over(run_dir, staged_path)
            .await
            .map_err(|e| format!("目标版本没有接手安装（{e}）；实例与 bin 里的二进制都未改动"));
    }

    // ---- 实例快照（ACK 阶段清单）：续作重算时必须并回在途实例（见
    // snapshot_with_pending）
    let (live, snapshot) = snapshot_with_pending(run_dir, state).await;
    state.instance_snapshot = snapshot.clone();
    write_in(run_dir, state).map_err(|e| format!("install.state 写入失败: {e}"))?;

    // ---- broadcasting：ACK 齐了才交换（铁律 3）。无实例 → 跳过（快路径）
    if !snapshot.is_empty() {
        // 换血：广播之前停掉旧看护者——它的 respawn 用 current_exe 拉实例，
        // 换血后那条路径已不是新二进制（Windows 指向 rename 后的 .old，升级
        // 永不完成；Linux 读到的是 `… (deleted)`，重拉必然失败）。
        stop_old_watchdog(run_dir);
        // 相位守卫（与 run_tail 同款）：Acked/Swapping 残留的续作重入时
        // phase 已高于 Broadcasting/Acked——逆向迁移非法，保留高位重做即可
        //（广播是幂等置位）
        if state.phase < InstallPhase::Broadcasting {
            super::state::advance_in(run_dir, state, InstallPhase::Broadcasting)?;
        }
        if let Err(e) = super::broadcast::broadcast_prepare_swap(run_dir, &live, state).await {
            return Err(format!(
                "PrepareSwap 广播终失败（{e}）。\n\
                 问题实例有隐患，处置指引：将其关闭后重试安装（不强杀——避免服务中断）"
            ));
        }
        if state.phase < InstallPhase::Acked {
            super::state::advance_in(run_dir, state, InstallPhase::Acked)?;
        }
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
        super::broadcast::broadcast_prepare_swap(run_dir, &fresh, state)
            .await
            .map_err(|e| format!("广播后新启实例 ACK 失败: {e}"))?;
    }
    super::state::advance_in(run_dir, state, InstallPhase::Swapping)?;
    let outcome = swap::swap_in(home, staged_path).map_err(|e| format!("二进制交换失败: {e}"))?;
    state.old_path = outcome.old_path.map(|p| p.display().to_string());
    super::state::advance_in(run_dir, state, InstallPhase::Swapped)?;

    // Windows：本进程就是 staging 里那份副本（早交接的接手者）时，把剩余阶段
    // 再交给 bin 里刚落位的同一个二进制——cleaning 要删 staging，运行中的镜像
    // 删不掉。交不出去就自己做完，staging 删不掉就留着。unix 不需要：交换把
    // staged 文件 rename 进了 bin，staging 里已没有本进程的镜像
    #[cfg(windows)]
    if is_current_exe(staged_path) && successor_sees_same_home(home, run_dir) {
        match hand_over(run_dir, &swap::bin_path_in(home)).await {
            Ok(exit) => return Ok(exit),
            Err(e) => {
                tracing::warn!(error = %e, "交换后交给 bin 里的二进制失败，本进程继续完成剩余阶段")
            }
        }
    }

    run_tail(home, run_dir, state, &swap::bin_path_in(home), false).await?;
    Ok(FlowExit::Completed)
}

/// 停掉旧看护者（换血，广播之前）：身份验证 + 终止 + 删 claim。
/// 看护者不承载流量，零服务影响；换血窗口内实例崩溃 = 短时失去自动重拉，
/// 可接受微窗（install 结束、宣告消失后，新实例的自检用新二进制补种）。
fn stop_old_watchdog(run_dir: &Path) {
    let Some(claim) = crate::watchdog::read_claim_in(run_dir) else {
        return; // 无看护者，无事
    };
    crate::watchdog::remove_claim_in(run_dir);
    let alive = crate::watchdog::verify_claim_identity(claim.pid, claim.created_at_process);
    if alive {
        tracing::info!(
            pid = claim.pid,
            "停掉旧看护者（换血：其 respawn 会用旧镜像）"
        );
        let _ = crate::watchdog::terminate_verified_process(claim.pid, claim.created_at_process);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newer_side_drives_the_install() {
        let own = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let newer = semver::Version::new(own.major, own.minor, own.patch + 1).to_string();
        assert!(target_drives(&newer), "升级交给目标版本");
        assert!(
            target_drives(env!("CARGO_PKG_VERSION")),
            "同版本重装也交（测试里的交棒路径靠它走到）"
        );
        assert!(!target_drives("0.0.0"), "降级由本进程驱动");
        assert!(!target_drives("not-a-version"), "解析不了不交");
    }
}
