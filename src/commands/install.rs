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
        match aproxy::install::flow::continue_install(&home, &run_dir).await {
            Ok(_) => tracing::info!("install 续作完成"),
            Err(e) => tracing::error!(error = %e, "install 续作失败（现场已保留，等待下次续作）"),
        }
        return;
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

/// --skills-only：只更新 skill 文档。版本默认 latest（github 列表第一个）；
/// settings download_chain 与 --download-proxy 与主流程同一语义。
/// 成功后输出落位位置与「安装到 agent 目录由用户/agent 自行链接」提示。
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
            println!("查询最新版本...");
            match aproxy::install::download::github::latest_version(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[ERROR] {e}");
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

    // 版本解析：latest = GitHub Releases 最新（列表第一个，<1.0 含 prerelease）
    let target = match args.version.as_deref() {
        None | Some("latest") => {
            println!("查询最新版本...");
            match aproxy::install::download::github::latest_version(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[ERROR] {e}");
                    std::process::exit(1);
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
    // semver 比较：prerelease 语义 alpha.9 < alpha.10 与 0.2.0 > 0.1.0-x
    let current =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("内置版本号必为合法 semver");
    match semver::Version::parse(&target) {
        Ok(t) if t < current && !args.allow_downgrade => {
            eprintln!(
                "[ERROR] 目标版本 {target} 低于当前 {current}。确认要降级请加 --allow-downgrade。"
            );
            std::process::exit(1);
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("[ERROR] 版本号 {target} 非法（semver 解析失败: {e}）");
            std::process::exit(1);
        }
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
    let fetched = match aproxy::install::download::fetch_artifact(
        &ctx,
        &chain,
        aproxy::install::download::Artifact::Binary,
        &staged_dir,
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[ERROR] {e}");
            eprintln!(
                "可尝试：--download-proxy <URL> 配置下载代理，或在 settings.json 配置 download_chain（含 url 模板 CDN 通道，文档见 README）。"
            );
            std::process::exit(1);
        }
    };
    // staged 自证校验：能运行且自报版本 == 目标（与 --from 的信任锚同级）
    match aproxy::install::staging::probe_version(&fetched.path) {
        Ok(v) if v == target => {}
        Ok(v) => {
            eprintln!("[ERROR] 下载产物自报 {v}，与目标版本 {target} 不符，中止");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("[ERROR] 下载产物校验失败（不可运行）: {e}");
            std::process::exit(1);
        }
    }

    println!("开始安装 aProxy {target}...");
    // source 记录实际命中的链条级（status/排查用）
    let source = match fetched.source {
        "github" => aproxy::install::state::InstallSource::Github,
        "npm" => aproxy::install::state::InstallSource::Npm,
        _ => aproxy::install::state::InstallSource::Url,
    };
    match aproxy::install::flow::run_install_online(
        home,
        run_dir,
        &target,
        &fetched.path,
        source,
        !args.no_skills,
        proxy,
    )
    .await
    {
        Ok(exit) => {
            if exit == aproxy::install::flow::FlowExit::HandedOver {
                println!("交换完成，剩余阶段由新版本继续（接力交棒）。");
                std::process::exit(0);
            }
            println!(
                "安装完成：{}（二进制已落位，实例已滚动到新版本）",
                aproxy::install::swap::bin_path_in(home).display()
            );
        }
        Err(e) => {
            eprintln!("[ERROR] 安装失败: {e}");
            eprintln!(
                "现场已保留，中断后重试同一命令或任何 aproxy 命令可自动续作；`aproxy install --abort` 可显式回滚。"
            );
            std::process::exit(1);
        }
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
    match aproxy::install::flow::run_install(home, run_dir, &plan, skill_enabled, download_proxy)
        .await
    {
        Ok(exit) => {
            if exit == aproxy::install::flow::FlowExit::HandedOver {
                // Windows 接力：续作由新二进制进程完成，本进程（旧镜像）
                // 到此结束——对用户表现为同一条命令装完。**必须硬退**：
                // 交棒路径刻意不 abort 宣告 ticker（进程退出即节消失），
                // 走正常 return 会让 tokio runtime drop 永久等待无限循环
                // 的 ticker 任务（实测 27 个交棒进程全体挂死的根源）
                println!("交换完成，剩余阶段由新版本继续（接力交棒）。");
                std::process::exit(0);
            }
            println!(
                "安装完成：{}（二进制已落位，实例已滚动到新版本）",
                aproxy::install::swap::bin_path_in(home).display()
            );
        }
        Err(e) => {
            eprintln!("[ERROR] 安装失败: {e}");
            eprintln!(
                "现场已保留，中断后重试同一命令或任何 aproxy 命令可自动续作；`aproxy install --abort` 可显式回滚。"
            );
            std::process::exit(1);
        }
    }
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
