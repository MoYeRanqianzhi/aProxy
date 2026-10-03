//! `aproxy install`：二进制安装与升级（`upgrade` 为别名）。
//!
//! 使命对齐：「绝对不间断」在升级场景的缺失一环——替换二进制期间服务不
//! 中断、中断点不留烂摊子。计划全文见 `.agents/plan/install-v1.md`。

use crate::cli::InstallArgs;

/// 入口分派：--abort / --from / --adopt / --continue（续作）/ 无参提示。
pub(crate) async fn handle_install_cmd(args: InstallArgs) {
    let home = aproxy::settings::home();
    let run_dir = aproxy::daemon::run_dir();

    if args.abort {
        return handle_abort(&home, &run_dir);
    }
    if args.continue_ {
        // 续作模式：静默执行（由看护者/CLI 入口/接力自动拉起）。失败落
        // failed 现场等下次续作，不打扰用户——但首轮失败输出到日志可查。
        // 硬退理由同 report_outcome：续作在 Windows 上也可能再次交棒，
        // 后台任务不保证已结束
        match aproxy::install::flow::continue_install(&home, &run_dir).await {
            Ok(_) => {
                tracing::info!("install 续作完成");
                std::process::exit(0);
            }
            Err(e) => {
                tracing::error!(error = %e, "install 续作失败（现场已保留，等待下次续作）");
                std::process::exit(1);
            }
        }
    }

    // --skills-only：只更新 skill 文档（不动二进制、不建安装状态机）。
    // skill 上次 failed 后的单独重试入口；无参版本默认 latest。
    if args.skills_only {
        return run_skills_only(&args).await;
    }

    // --from <路径>：目标版本 = 源二进制自报版本
    if let Some(from) = &args.from {
        let from_path = aproxy::settings::expand_path(from);
        let target = match aproxy::install::staging::probe_version(&from_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[ERROR] {e}");
                std::process::exit(1);
            }
        };
        let plan = aproxy::install::flow::InstallPlan {
            target_version: target,
            source: aproxy::install::state::InstallSource::From,
            from: from_path,
        };
        run_plan(
            &home,
            &run_dir,
            plan,
            false,
            !args.no_skills,
            effective_proxy(&args).as_deref(),
        )
        .await;
        return;
    }

    // --adopt：当前进程镜像作为安装源（包管理器安装 → 标准位置收编）
    if args.adopt {
        let self_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[ERROR] 无法定位自身可执行文件: {e}");
                std::process::exit(1);
            }
        };
        let target = match aproxy::install::staging::probe_version(&self_exe) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[ERROR] {e}");
                std::process::exit(1);
            }
        };
        let plan = aproxy::install::flow::InstallPlan {
            target_version: target,
            source: aproxy::install::state::InstallSource::From,
            from: self_exe,
        };
        run_plan(
            &home,
            &run_dir,
            plan,
            true,
            !args.no_skills,
            effective_proxy(&args).as_deref(),
        )
        .await;
        return;
    }

    // 在线渠道：`install [latest|版本]`——下载链条按序尝试（github → npm
    // → …，settings download_chain 可配严格数组与 url 模板），产物落
    // staging 后走与 --from 同一条交换/滚动流水线
    run_online(&home, &run_dir, &args).await;
}

/// 本次生效的下载代理：CLI --download-proxy 优先，未设回落 settings——
/// 二进制链与 skill 支线共用同一归一结果（入口处统一，避免两条链分叉）。
fn effective_proxy(args: &InstallArgs) -> Option<String> {
    args.download_proxy
        .clone()
        .or_else(|| aproxy::settings::load().download_proxy)
}

/// latest 查询：github（权威来源）→ 不可达/限流时 npm dist-tags 兜底
/// （跟随 ~/.npmrc 镜像——共享出口 IP 场景 github 不可用是常态）。两级都
/// 按同一通道选版；Ok(None) = 通道内无版本。两级皆不可达 → Err(github 的
/// 错误，更具排查价值)。
async fn query_latest(
    ctx: &aproxy::install::download::DownloadCtx,
    channel: aproxy::install::download::Channel,
    artifact: aproxy::install::download::Artifact,
) -> Result<Option<String>, String> {
    println!("查询最新版本（{} 通道）...", channel.label());
    match aproxy::install::download::github::latest_version(ctx, channel, artifact).await {
        Ok(v) => Ok(v),
        Err(gh_err) => {
            match aproxy::install::download::npmpkg::latest_version(ctx, channel).await {
                Ok(v) => {
                    println!(
                        "github 查询失败（{gh_err}），npm 兜底结果：{}",
                        v.as_deref().unwrap_or("通道内无版本")
                    );
                    Ok(v)
                }
                Err(_) => Err(gh_err),
            }
        }
    }
}

/// --skills-only：只更新 skill 文档。版本默认 latest（按通道取最大，同
/// 二进制安装）；通道内无版本时取当前运行版本（skill 与在跑的二进制对齐
/// 是最合理的退路）。settings download_chain 与 --download-proxy 与主流程
/// 同一语义。成功后输出落位位置与「安装到 agent 目录由用户/agent 自行链接」
/// 提示。
async fn run_skills_only(args: &InstallArgs) {
    let home = aproxy::settings::home();
    let settings = aproxy::settings::load();
    let proxy = args
        .download_proxy
        .as_deref()
        .or(settings.download_proxy.as_deref());
    let ctx = match aproxy::install::download::DownloadCtx::build(
        env!("CARGO_PKG_VERSION"),
        proxy,
        args.variant.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[ERROR] {e}");
            std::process::exit(1);
        }
    };
    let version = match args.version.as_deref() {
        None | Some("latest") => {
            let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("内置版本号必为合法 semver");
            let channel = aproxy::install::download::Channel::resolve(args.pre, &current);
            match query_latest(&ctx, channel, aproxy::install::download::Artifact::Skills).await {
                Ok(Some(v)) => v,
                Ok(None) => {
                    println!(
                        "{} 通道暂无可用版本，skill 按当前版本 {current} 更新。",
                        channel.label()
                    );
                    current.to_string()
                }
                Err(gh_err) => {
                    eprintln!("[ERROR] {gh_err}");
                    eprintln!("可尝试：--skills-only <具体版本号>（跳过 latest 查询）。");
                    std::process::exit(1);
                }
            }
        }
        Some(v) => v.to_string(),
    };
    let mut ctx = ctx;
    ctx.version = version.clone();
    println!("开始更新 skill 文档（版本 {version}）...");
    let chain = aproxy::install::download::effective_chain(&settings);
    match aproxy::install::skills::update_skills(&ctx, &chain, &home).await {
        outcome if outcome.phase == aproxy::install::state::SkillPhase::Done => {
            println!(
                "skill 更新完成：{}\n安装到 agent 的 skills 目录由您自行链接（install 不越界触碰各 agent 的目录）：\n  例如 Claude Code: mklink /D ~/.claude/skills/aproxy-cli \"%USERPROFILE%\\.aproxy\\skills\\aproxy-cli\"",
                aproxy::install::skills::skill_dir_in(&home).display()
            );
        }
        _ => {
            eprintln!(
                "[ERROR] skill 更新失败（非强制支线，二进制安装不受影响）。可重试本命令或检查下载链条/代理配置。"
            );
            std::process::exit(1);
        }
    }
}

/// 在线安装：版本解析（latest → GitHub 查询）→ 降级防呆 → 下载链条 →
/// staged 校验 → 主流程。
async fn run_online(home: &std::path::Path, run_dir: &std::path::Path, args: &InstallArgs) {
    let settings = aproxy::settings::load();
    let proxy = args
        .download_proxy
        .as_deref()
        .or(settings.download_proxy.as_deref());
    let ctx = match aproxy::install::download::DownloadCtx::build(
        env!("CARGO_PKG_VERSION"),
        proxy,
        args.variant.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[ERROR] {e}");
            std::process::exit(1);
        }
    };

    // 版本解析：latest = 通道内 semver 最大的主线版本（stable 只取正式版，
    // pre 含预发布；默认通道跟随当前版本，见 Channel::resolve）。latest 不
    // 高于当前 → 已是最新，不重装；通道为空 → 保持现状，绝不跨通道偷装
    // 预发布（是否接受预发布是用户的显式选择 --pre）
    let current =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("内置版本号必为合法 semver");
    let target = match args.version.as_deref() {
        None | Some("latest") => {
            use aproxy::install::download::{Artifact, Channel, LatestDecision, decide_latest};
            let channel = Channel::resolve(args.pre, &current);
            let candidate = match query_latest(&ctx, channel, Artifact::Binary).await {
                Ok(c) => c,
                Err(gh_err) => {
                    eprintln!("[ERROR] {gh_err}");
                    eprintln!(
                        "可尝试：install <具体版本号>（跳过 latest 查询），或 --download-proxy <URL> 更换出口。"
                    );
                    std::process::exit(1);
                }
            };
            match decide_latest(&current, candidate.as_deref()) {
                LatestDecision::Install(v) => v,
                LatestDecision::UpToDate(latest) => {
                    println!(
                        "已是最新：当前 {current}，{} 通道最新为 {latest}，无需安装。",
                        channel.label()
                    );
                    if channel == Channel::Stable {
                        println!("（如需预发布版本，可加 --pre）");
                    }
                    return;
                }
                LatestDecision::NoneInChannel => {
                    println!(
                        "{} 通道暂无可用版本，保持当前 {current} 不变。",
                        channel.label()
                    );
                    if channel == Channel::Stable {
                        println!("（如需预发布版本，可加 --pre）");
                    }
                    return;
                }
            }
        }
        Some(v) => v.to_string(),
    };
    // ctx 初建用编译版本仅够 latest 查询用；目标版本确定后必须回写——
    // 各渠道（github tag/npm registry/crates.io）都按 ctx.version 定位产物，
    // 漏回写会让 `install <特定版本>` 全渠道查错版本（实测暴露）
    let mut ctx = ctx;
    ctx.version = target.clone();

    // 降级防呆：target < 当前运行版本 → 拒绝（--allow-downgrade 放行）。
    // latest 路径经 decide_latest 已保证高于当前，这里主要拦显式指定的版本号
    if let Err(e) =
        aproxy::install::download::check_downgrade(&target, &current, args.allow_downgrade)
    {
        eprintln!("[ERROR] {e}");
        std::process::exit(1);
    }

    // 下载链条：产物落 staging/<版本>/（与 --from 的备料同一位置）
    println!(
        "开始下载 aProxy {target}（渠道链：{}）...",
        aproxy::install::download::effective_chain(&settings)
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join(" → ")
    );
    let chain = aproxy::install::download::effective_chain(&settings);
    let staged_dir = aproxy::install::staging::staging_dir_in(home, &target);

    // 下载 + 自证（能运行且自报版本 == 目标——与 --from 的信任锚同级）。
    // gnu 产物**试跑失败**（构建机 glibc 高于本机）→ musl 静态产物回退；
    // 下载链条本身失败（网络/404）不回退——musl 重跑只会重复同一网络错误。
    enum DownloadOutcome {
        Ok(aproxy::install::download::Fetched),
        VerifyFailed(String),
        ChainFailed(String),
    }
    async fn download_verified(
        ctx: &aproxy::install::download::DownloadCtx,
        chain: &[aproxy::install::download::ChainStep],
        target: &str,
        staged_dir: &std::path::Path,
    ) -> DownloadOutcome {
        let fetched = match aproxy::install::download::fetch_artifact(
            ctx,
            chain,
            aproxy::install::download::Artifact::Binary,
            staged_dir,
        )
        .await
        {
            Ok(f) => f,
            Err(e) => return DownloadOutcome::ChainFailed(e),
        };
        match aproxy::install::staging::probe_version(&fetched.path) {
            Ok(v) if v == target => DownloadOutcome::Ok(fetched),
            Ok(v) => {
                DownloadOutcome::VerifyFailed(format!("下载产物自报 {v}，与目标版本 {target} 不符"))
            }
            Err(e) => DownloadOutcome::VerifyFailed(format!("下载产物校验失败（不可运行）: {e}")),
        }
    }
    let fetched = match download_verified(&ctx, &chain, &target, &staged_dir).await {
        DownloadOutcome::Ok(f) => f,
        DownloadOutcome::VerifyFailed(e) => match ctx.musl_fallback() {
            Some(mctx) => {
                println!("gnu 产物在本机不可运行（{e}），回退 musl 静态产物...");
                match download_verified(&mctx, &chain, &target, &staged_dir).await {
                    DownloadOutcome::Ok(f) => f,
                    other => {
                        let e2 = match other {
                            DownloadOutcome::VerifyFailed(e) | DownloadOutcome::ChainFailed(e) => e,
                            DownloadOutcome::Ok(_) => unreachable!(),
                        };
                        eprintln!("[ERROR] musl 回退仍失败: {e2}");
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!("[ERROR] {e}");
                eprintln!(
                    "可尝试：--download-proxy <URL> 配置下载代理，或在 settings.json 配置 download_chain（含 url 模板 CDN 通道，文档见 README）。"
                );
                std::process::exit(1);
            }
        },
        DownloadOutcome::ChainFailed(e) => {
            eprintln!("[ERROR] {e}");
            eprintln!(
                "可尝试：--download-proxy <URL> 配置下载代理，或在 settings.json 配置 download_chain（含 url 模板 CDN 通道，文档见 README）。"
            );
            std::process::exit(1);
        }
    };

    println!("开始安装 aProxy {target}...");
    // source 记录实际命中的链条级（status/排查用）
    let source = match fetched.source {
        "github" => aproxy::install::state::InstallSource::Github,
        "npm" => aproxy::install::state::InstallSource::Npm,
        _ => aproxy::install::state::InstallSource::Url,
    };
    let result = aproxy::install::flow::run_install_online(
        home,
        run_dir,
        &target,
        &fetched.path,
        source,
        !args.no_skills,
        proxy,
    )
    .await;
    report_outcome(home, run_dir, result).await;
}

/// 安装结局的用户可见输出与退出码（三个安装入口共用）。**一律硬退**：
/// 宣告 ticker 与 skill 支线等后台任务不保证在此刻已结束，走正常 return
/// 会让 tokio runtime drop 等待它们（实测 27 个交棒进程全体挂死的根源）。
///
/// Windows 接力交棒（`HandedOver`）≠ 安装完成：滚动重启/终验由接棒的新
/// 二进制进程执行，本进程（旧镜像）必须等它跑到终点再报告——否则接棒者
/// 失败时用户看到的是「交换完成」与退出码 0，agent 会以为升级成功。
async fn report_outcome(
    home: &std::path::Path,
    run_dir: &std::path::Path,
    result: Result<aproxy::install::flow::FlowExit, String>,
) -> ! {
    let done_msg = || {
        println!(
            "安装完成：{}（二进制已落位，实例已滚动到新版本）",
            aproxy::install::swap::bin_path_in(home).display()
        );
    };
    match result {
        Ok(aproxy::install::flow::FlowExit::Completed) => {
            done_msg();
            std::process::exit(0);
        }
        Ok(aproxy::install::flow::FlowExit::HandedOver) => {
            println!("交换完成，剩余阶段（滚动重启/终验）由新版本进程继续，等待其完成...");
            match await_handover(run_dir).await {
                Ok(()) => {
                    done_msg();
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("[ERROR] 安装失败: {e}");
                    print_failure_guidance(run_dir);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("[ERROR] 安装失败: {e}");
            print_failure_guidance(run_dir);
            std::process::exit(1);
        }
    }
}

/// 接棒者结局等待上限：与安装锁 stale 判定同量级（滚动重启每实例最坏
/// 约 20s 停止 + 8s 就绪 + 8s 回退，十分钟覆盖数十个实例）。
const HANDOVER_WAIT_SECS: u64 = aproxy::install::state::STALE_AFTER_SECS;

/// 轮询 install.state 直到接棒者到达终点。成功判据：状态文件消失（done
/// 清场）或 phase 进入 cleaning（终验已通过——cleaning 只剩删 `.old` 与
/// staging，而 `.old` 正是本进程的运行镜像，本进程必须先退出它才删得掉，
/// 所以不能等到 done）。失败判据：phase=failed（带接棒者记下的原因）、
/// 接棒进程已死而安装未终结、等待超时。
async fn await_handover(run_dir: &std::path::Path) -> Result<(), String> {
    use aproxy::install::state::{InstallPhase, is_stale, load_in, state_path_in};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(HANDOVER_WAIT_SECS);
    loop {
        match load_in(run_dir) {
            None if !state_path_in(run_dir).exists() => return Ok(()),
            // 文件在但读不出（状态损坏）：不臆断，继续等到超时
            None => {}
            Some(s) => match s.phase {
                InstallPhase::Cleaning | InstallPhase::Done => return Ok(()),
                InstallPhase::Failed => {
                    return Err(s
                        .last_error
                        .unwrap_or_else(|| "接棒进程报告失败（原因未记录）".to_string()));
                }
                InstallPhase::Aborted => return Err("安装已被中止（--abort）".to_string()),
                _ if s.installer_pid != std::process::id()
                    && is_stale(&s, aproxy::watchdog::now_secs()) =>
                {
                    return Err(format!(
                        "接棒进程（pid {}）已退出但安装停在 {:?}；现场已保留，任何 aproxy 命令会自动续作",
                        s.installer_pid, s.phase
                    ));
                }
                _ => {}
            },
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{HANDOVER_WAIT_SECS} 秒内未确认安装完成（可能仍在后台进行）；可用 aproxy status 查看实例版本"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// 失败后的处置指引：实例级失败（halted）与可续作的失败给不同的出路。
fn print_failure_guidance(run_dir: &std::path::Path) {
    let halted = aproxy::install::state::load_in(run_dir).is_some_and(|s| s.halted);
    if halted {
        eprintln!(
            "滚动已中止，不会自动重试：出问题的实例已用旧二进制拉回（拉回失败时其恢复记录已保留），\
             其余实例未动。常见原因是新版本不接受现有配置——按上方原因修正后重新执行 aproxy install 完成升级；\
             若要留在旧版本，用 `aproxy install <旧版本> --allow-downgrade` 把规范位置的二进制换回。"
        );
    } else {
        eprintln!(
            "现场已保留，中断后重试同一命令或任何 aproxy 命令可自动续作；`aproxy install --abort` 可显式回滚（仅交换前）。"
        );
    }
}

/// 执行安装计划：管辖检查 → 主流程。成功/失败的用户可见输出在此统一。
/// `download_proxy` 为本次生效的下载代理（CLI 参数优先于 settings），贯穿
/// 二进制链与 skill 支线。
async fn run_plan(
    home: &std::path::Path,
    run_dir: &std::path::Path,
    plan: aproxy::install::flow::InstallPlan,
    adopt: bool,
    skill_enabled: bool,
    download_proxy: Option<&str>,
) {
    if let Err(e) = aproxy::install::flow::check_jurisdiction(run_dir, home, adopt).await {
        eprintln!("[ERROR] {e}");
        std::process::exit(1);
    }
    let target = plan.target_version.clone();
    println!("开始安装 aProxy {target}...");
    let result =
        aproxy::install::flow::run_install(home, run_dir, &plan, skill_enabled, download_proxy)
            .await;
    report_outcome(home, run_dir, result).await;
}

/// --abort：显式回滚（仅 swapping 前可完全回滚，此后只进不退）。
fn handle_abort(home: &std::path::Path, run_dir: &std::path::Path) {
    let Some(state) = aproxy::install::state::load_in(run_dir) else {
        println!("没有进行中的安装。");
        return;
    };
    // swapping 及之后只进不退（运行实例可能已开始滚动）——拒绝并说明
    if state.phase >= aproxy::install::state::InstallPhase::Swapping {
        eprintln!(
            "[ERROR] 安装已进入交换阶段（phase {:?}），无法回滚——实例可能已开始滚动到新版本。\n中断的安装会自动续作完成；如需手动干预，按 skill 的手动更新兜底路径操作。",
            state.phase
        );
        std::process::exit(1);
    }
    // swapping 前：二进制未动，干净回滚 = 清现场（staging + 状态文件）
    if let Some(staged) = &state.staged_path {
        let _ = std::fs::remove_file(staged);
    }
    if let Some(v) = Some(state.target_version.clone()) {
        let _ = std::fs::remove_dir_all(aproxy::install::staging::staging_dir_in(home, &v));
    }
    aproxy::install::state::remove_in(run_dir);
    println!("安装已中止，现场已清理（二进制与实例未受影响）。");
}
