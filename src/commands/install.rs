//! `aproxy install`：二进制安装与升级（`upgrade` 为别名）。
//!
//! 使命对齐：「绝对不间断」在升级场景的缺失一环——替换二进制期间服务不
//! 中断、中断点不留烂摊子。计划全文见 `.agents/plan/install-v1.md`。

mod events;

use aproxy::cli::InstallArgs;
use aproxy::install::InstallFailure;

/// 跨版本的 last_error 用英文；给当前用户的处置建议按码选择，不重查可能已变化的实例。
fn refusal(error: &InstallFailure) -> Option<String> {
    match error.code.as_str() {
        "install_unmanaged_instances" => {
            let (_, rest) = error.message.split_once("(pid ")?;
            let (pids, _) = rest.split_once(')')?;
            Some(format!(
                "这个 home 里有旧版本启动、本次安装管不了的实例还在运行（pid {pids}）。先用启动它们的那个版本执行 aproxy stop all，再重新安装；或者改用 aproxy install --force（会结束全部 aproxy 进程）。"
            ))
        }
        "install_foreground_instances" => {
            let ports = error.details["ports"].as_array()?;
            if let [p] = ports.as_slice() {
                Some(format!(
                    "端口 {p} 上的实例在前台运行，安装无法替它重启。什么都没有改动：实例照常运行，二进制没换。先停掉它（在启动它的终端里按 Ctrl+C，或执行 aproxy stop {p}；交给服务管理器运行的，用服务管理器停止），再执行 aproxy install，装好之后照原来的方式启动它。"
                ))
            } else {
                let ports = ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join("、");
                Some(format!(
                    "端口 {ports} 上的实例在前台运行，安装无法替它们重启。什么都没有改动：实例照常运行，二进制没换。先停掉它们（在各自的终端里按 Ctrl+C，或逐个执行 aproxy stop <端口>；交给服务管理器运行的，用服务管理器停止），再执行 aproxy install，装好之后照原来的方式启动它们。"
                ))
            }
        }
        "install_precheck_failed" => {
            let port = error.details["port"].as_u64()?;
            Some(format!(
                "安装前的预检没有通过：端口 {port} 的实例按新版本解析配置时出错。什么都没有改动：实例照常运行，二进制没换。"
            ))
        }
        _ => None,
    }
}

/// 安装者把原因写给用户，按安装码退出；提前失败时也不伪造安装状态。
fn fail(error: InstallFailure) -> ! {
    let localized = refusal(&error);
    let message = localized.as_deref().unwrap_or(&error.message);
    eprintln!("[ERROR] {message}");
    if error.code == "install_precheck_failed" {
        if let Some(ds) = error.details["diagnostics"].as_array() {
            for d in ds {
                let line = aproxy::ops::start::render_diagnostic_fields(
                    d["file"].as_str().unwrap_or_default(),
                    d["line"].as_u64().and_then(|n| n.try_into().ok()),
                    d["column"].as_u64().and_then(|n| n.try_into().ok()),
                    d["key"].as_str(),
                    d["message"].as_str().unwrap_or_default(),
                    d["code"].as_str().unwrap_or_default(),
                );
                eprintln!("  {line}");
            }
        }
        eprintln!("修正上面的问题后重新执行 aproxy install。");
    }
    events::result(
        "failed",
        Some((&error, message.lines().next().unwrap_or(message))),
    );
    crate::util::exit(error.exit_code())
}

/// `install --force` / `upgrade --force`（永久兼容契约 C7、C8，流程见
/// aproxy::install::force）。main 在读配置与 settings.json 之前就分流到这里。
///
/// 先把本进程作为安装者登记进台账：另一个强制安装据此结束它（后来的结束先来的），
/// 本进程结束安装者时跳过自己的 pid。退出走 util::exit，删掉自己的条目。
pub(crate) async fn handle_force_cmd(args: &InstallArgs) -> ! {
    use aproxy::install::force;
    use std::io::Write;
    // home 解析不了（永久兼容契约 C1：APROXY_HOME 不是绝对路径）或不存在，就什么都不做：
    // 还没登记台账（登记会把 home 建出来），直接以 10 退出。写失败忽略（C7：写输出失败不得
    // 改变退出码）
    let home = match force::locate_home() {
        Ok(home) => home,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "[ERROR] {e}\n什么都没有改动（退出码 {}）。",
                force::EXIT_UNCHANGED
            );
            std::process::exit(force::EXIT_UNCHANGED);
        }
    };
    let _ledger = crate::util::register_in_ledger(
        &home,
        aproxy_ipc::ledger::ROLE_INSTALLER,
        None,
        Some("install --force"),
        None,
    );
    let source = match &args.from {
        Some(from) => force::ForceSource::File(aproxy::settings::expand_path(from)),
        None => force::ForceSource::Online(args.version.clone()),
    };
    let request = force::ForceRequest {
        source,
        variant: args.variant.clone(),
        cpu: args.cpu.clone(),
        pre: args.pre,
        yes: args.yes,
        download_proxy: args.download_proxy.clone(),
    };
    let code = force::run(&home, &request).await;
    crate::util::exit(code)
}

/// 入口分派：--abort / --from / --adopt / --continue（续作）/ 无参提示。
pub(crate) async fn handle_install_cmd(args: InstallArgs) {
    if args.json && !args.check {
        return events::stream(install(args)).await;
    }
    install(args).await;
}

/// 参数已分流：只查询的命令不登记台账，普通安装从登记开始。
async fn install(args: InstallArgs) {
    if args.check {
        return run_check(&args).await;
    }
    let home = aproxy_ipc::home::home();
    let run_dir = aproxy::daemon::run_dir();

    if args.abort {
        return handle_abort(&home);
    }
    // 进程台账（永久兼容契约 C3）：会动 bin/ 或实例的安装进程——在线安装、--from、
    // --adopt、续作与交接后的接手者——在做任何事之前登记，`aproxy install --force` 据此
    // 先结束它们、再去拿安装锁。只下载 skill 文档的 --skills-only 不碰 bin/，不登记。
    // 本文件里的退出一律走 util::exit：process::exit 不跑析构，守卫删不到条目
    let _ledger = (args.continue_ || !args.skills_only).then(|| {
        crate::util::register_in_ledger(
            &home,
            aproxy_ipc::ledger::ROLE_INSTALLER,
            None,
            Some(if args.continue_ {
                "install --continue"
            } else {
                "install"
            }),
            None,
        )
    });
    if args.continue_ {
        // 分离的接手者没有可读的 stderr；只记 panic 位置，不记可能含配置值的载荷。
        let prior = std::panic::take_hook();
        let panic_home = home.clone();
        std::panic::set_hook(Box::new(move |info| {
            use aproxy::install::state::{ExitEvidence, load_in};
            if let Some(s) = load_in(&panic_home).filter(|s| s.installer_pid == std::process::id())
            {
                let location = info.location().map(|l| l.to_string()).unwrap_or_default();
                let _ = ExitEvidence::record(
                    &panic_home,
                    &s,
                    &format!("installer panic at {location}"),
                );
            }
            prior(info);
        }));
        // 续作模式：静默执行（由看护者/CLI 入口/交棒自动拉起）。失败落
        // failed 现场等下次续作，不打扰用户——但首轮失败输出到日志可查。
        // 硬退理由同 report_outcome：续作也可能再次交棒，后台任务不保证已结束
        let result =
            aproxy::install::flow::continue_install(&home, &run_dir, args.handover_from).await;
        // 无指名的续作也有自己的调用者：转交后等最终退出码。指名接手者应尽早释放 staging 镜像。
        let result = match result {
            Ok(aproxy::install::flow::FlowExit::HandedOver) if args.handover_from.is_none() => {
                await_handover(&home).await
            }
            other => other.map(|_| ()),
        };
        match result {
            Ok(_) => {
                tracing::info!("install 续作完成");
                crate::util::exit(0);
            }
            Err(e) => {
                if refusal(&e).is_some() {
                    fail(e);
                }
                tracing::error!(error = %e, "install 续作失败（现场已保留，等待下次续作）");
                crate::util::exit(e.exit_code());
            }
        }
    }

    // --skills-only：只更新 skill 文档（不动二进制、不建安装状态机）。
    // 同版本的旧写法仍可用，不能为当前二进制装入另一版的文档。
    if args.skills_only {
        if args
            .version
            .as_deref()
            .is_some_and(|v| v != env!("CARGO_PKG_VERSION"))
        {
            eprintln!(
                "skill 描述的是当前二进制，只能安装 {} 的文档。要使用新版文档，请先升级 aProxy。",
                env!("CARGO_PKG_VERSION")
            );
            crate::util::exit(i32::from(aproxy_ipc::code::EXIT_USAGE));
        }
        let code = super::skill::install(args.download_proxy.as_deref(), false).await;
        if code != 0 {
            crate::util::exit(i32::from(code));
        }
        return;
    }

    // --from <路径>：目标版本 = 源二进制自报版本；版本下限与在线安装同一道
    // （见 aproxy::install::INSTALL_FLOOR）。同时给了 --variant 时是核对：文件必须正是
    // 这个变体（与 install --force --from 同一语义）
    if let Some(from) = &args.from {
        let from_path = aproxy::settings::expand_path(from);
        let probed = aproxy::install::staging::run_version(&from_path)
            .map_err(|e| InstallFailure::new("install_probe_failed", e))
            .and_then(|output| {
                let target = aproxy::install::staging::version_of(&from_path, &output)?;
                Ok((target, output))
            });
        let (target, output) = match probed {
            Ok(probed) => probed,
            Err(e) => fail(e),
        };
        events::target(&target);
        if let Some(want) = args.variant.as_deref() {
            let line = aproxy::install::force::VersionLine::parse(&output)
                .expect("version_of 认得的输出 VersionLine 也认得");
            if line.variant.as_deref() != Some(want) {
                fail(InstallFailure::new(
                    "install_probe_failed",
                    format!(
                        "{} 是 {} 版本，与 --variant {want} 不符",
                        from_path.display(),
                        line.variant.as_deref().unwrap_or("未知变体")
                    ),
                ));
            }
        }
        let version = semver::Version::parse(&target).expect("version_of 只返回合法的 semver");
        if let Err(e) = aproxy::install::check_floor(&version) {
            fail(InstallFailure::new("install_below_floor", e));
        }
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
                fail(format!("无法定位自身可执行文件: {e}").into());
            }
        };
        let target = match aproxy::install::staging::probe_version(&self_exe) {
            Ok(v) => v,
            Err(e) => fail(e),
        };
        events::target(&target);
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
    json: bool,
) -> Result<Option<aproxy::install::download::github::LatestRelease>, String> {
    super::say(
        json,
        format_args!("查询最新版本（{} 通道）...", channel.label()),
    );
    match aproxy::install::download::github::latest_version(ctx, channel, artifact).await {
        Ok(v) => Ok(v),
        Err(gh_err) => {
            match aproxy::install::download::npmpkg::latest_version(ctx, channel).await {
                Ok(v) => {
                    super::say(
                        json,
                        format_args!(
                            "github 查询失败（{gh_err}），npm 兜底结果：{}",
                            v.as_deref().unwrap_or("通道内无版本")
                        ),
                    );
                    Ok(
                        v.map(|version| aproxy::install::download::github::LatestRelease {
                            version,
                            ..Default::default()
                        }),
                    )
                }
                Err(_) => Err(gh_err),
            }
        }
    }
}

/// 只做二进制的 latest 查询，不进入安装台账、锁、备料和续作流程。
async fn run_check(args: &InstallArgs) {
    use aproxy::install::download::{
        Artifact, Channel, DownloadCtx, LatestDecision, decide_latest,
    };
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("编译版本是 semver");
    let channel = Channel::resolve(args.pre, &current);
    let label = channel.label();
    let query = async {
        let mut ctx = DownloadCtx::build(
            env!("CARGO_PKG_VERSION"),
            effective_proxy(args).as_deref(),
            args.cpu.as_deref(),
        )?;
        if let Some(variant) = &args.variant {
            ctx.variant = if variant == "desktop" {
                "desktop"
            } else {
                "headless"
            };
        }
        let release = query_latest(&ctx, channel, Artifact::Binary, args.json).await?;
        Ok::<_, String>((ctx.variant, release))
    }
    .await;
    let (variant, release) = match query {
        Ok(pair) => pair,
        Err(e) => {
            let failure = InstallFailure::new("install_download_failed", e);
            eprintln!("[ERROR] {failure}");
            eprintln!("可尝试：--download-proxy <URL> 更换出口。");
            if args.json {
                super::print_doc(
                    serde_json::json!({"error":{"code":failure.code,"message":failure.message,"details":failure.details}}),
                );
            }
            crate::util::exit(failure.exit_code());
        }
    };
    let latest = release.as_ref().map(|r| r.version.as_str());
    let decision = decide_latest(&current, latest);
    let update = matches!(decision, LatestDecision::Install(_));
    match decision {
        LatestDecision::Install(latest) => super::say(
            args.json,
            format_args!(
                "有新版本 {latest}（当前 {current}，{label} 通道）。执行 aproxy install 升级。"
            ),
        ),
        LatestDecision::UpToDate(latest) => super::say(
            args.json,
            format_args!("已是最新：当前 {current}，{label} 通道最新为 {latest}。"),
        ),
        LatestDecision::NoneInChannel => super::say(
            args.json,
            format_args!("{label} 通道暂无可用版本，当前 {current}。"),
        ),
    }
    if !update && channel == Channel::Stable {
        super::say(args.json, "（如需预发布版本，可加 --pre）");
    }
    if args.json {
        let mut doc = serde_json::json!({"current":current.to_string(),"latest":latest,"update_available":update,"channel":label,"variant":variant});
        if let Some(r) = release {
            if let Some(v) = r.published_at {
                doc["published_at"] = v.into();
            }
            if let Some(v) = r.url {
                doc["url"] = v.into();
            }
        }
        super::print_doc(doc);
    }
}

/// 在线安装：版本解析（latest → GitHub 查询）→ 降级防呆 → 管辖检查 →
/// 下载链条 → staged 校验 → 主流程。
///
/// 变体（desktop / headless）：写了 `--variant` 就装它，否则沿用正在运行的这个二进制的
/// （升级保持原变体）。本机跑不跑得了只看下载下来的二进制自己的 `--version`，不预先猜：
/// 没写 `--variant` 的带界面版本跑不起来就改装同目标的 headless，再不行换 musl（见
/// `DownloadCtx::attempts`）；写了的不换变体。换变体而通道里没有更新的版本（已是最新，或
/// 通道为空）时，重装当前版本的另一个变体。
async fn run_online(home: &std::path::Path, run_dir: &std::path::Path, args: &InstallArgs) {
    use aproxy::install::force::{VARIANT_DESKTOP, VARIANT_HEADLESS};
    let settings = aproxy::settings::load();
    let proxy = args
        .download_proxy
        .as_deref()
        .or(settings.download_proxy.as_deref());
    let mut ctx = match aproxy::install::download::DownloadCtx::build(
        env!("CARGO_PKG_VERSION"),
        proxy,
        args.cpu.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => fail(InstallFailure::new("install_download_failed", e)),
    };
    let own_variant = ctx.variant;
    let explicit_variant = args.variant.is_some();
    match args.variant.as_deref() {
        Some(VARIANT_HEADLESS) => ctx.variant = VARIANT_HEADLESS,
        Some(_) => ctx.variant = VARIANT_DESKTOP,
        None => {}
    }
    // 只有写了 --variant 才会换变体
    let switching = ctx.variant != own_variant;

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
            let candidate = match query_latest(&ctx, channel, Artifact::Binary, args.json).await {
                Ok(c) => c.map(|r| r.version),
                Err(gh_err) => {
                    eprintln!("[ERROR] {gh_err}");
                    eprintln!(
                        "可尝试：install <具体版本号>（跳过 latest 查询），或 --download-proxy <URL> 更换出口。"
                    );
                    let e = InstallFailure::new("install_download_failed", gh_err);
                    events::result("failed", Some((&e, &e.message)));
                    crate::util::exit(e.exit_code());
                }
            };
            match decide_latest(&current, candidate.as_deref()) {
                LatestDecision::Install(v) => v,
                // 换变体而通道里没有更新的版本：重装当前版本的另一个变体。通道为空也一样
                // （例如 stable 通道还没有正式版，当前版本是预发布）：用户要的是换变体，以 0
                // 结束却什么都没换，脚本与 agent 会以为换好了
                LatestDecision::UpToDate(_) | LatestDecision::NoneInChannel if switching => {
                    current.to_string()
                }
                LatestDecision::UpToDate(latest) => {
                    events::say(format_args!(
                        "已是最新：当前 {current}，{} 通道最新为 {latest}，无需安装。",
                        channel.label()
                    ));
                    if channel == Channel::Stable {
                        events::say("（如需预发布版本，可加 --pre）");
                    }
                    events::target(env!("CARGO_PKG_VERSION"));
                    events::plan(&[]);
                    events::result("up_to_date", None);
                    return;
                }
                LatestDecision::NoneInChannel => {
                    events::say(format_args!(
                        "{} 通道暂无可用的 {} 版本，保持当前 {current} 不变。",
                        channel.label(),
                        ctx.variant
                    ));
                    if channel == Channel::Stable {
                        events::say("（如需预发布版本，可加 --pre）");
                    }
                    events::target(env!("CARGO_PKG_VERSION"));
                    events::plan(&[]);
                    events::result("up_to_date", None);
                    return;
                }
            }
        }
        Some(v) => v.to_string(),
    };
    // ctx 初建用编译版本仅够 latest 查询用；目标版本确定后必须回写——
    // 各渠道（github tag/npm registry/crates.io）都按 ctx.version 定位产物，
    // 漏回写会让 `install <特定版本>` 全渠道查错版本（实测暴露）
    events::target(&target);
    ctx.version = target.clone();

    // 降级防呆：target < 当前运行版本 → 拒绝（--allow-downgrade 放行）。
    // latest 路径经 decide_latest 已保证高于当前，这里主要拦显式指定的版本号
    if let Err(e) =
        aproxy::install::download::check_downgrade(&target, &current, args.allow_downgrade)
    {
        fail(e);
    }

    // 管辖检查与 --from 同一道（见 run_plan），放在下载之前：bin 外的实例本就
    // 不该被滚动到 `<home>/bin`，没必要先下载一个注定装不上的产物。放在版本
    // 决议之后：已是最新时什么都不会发生，不必拦
    match aproxy::install::flow::check_restartable(home).await {
        Ok(ports) => events::plan(&ports),
        Err(e) => fail(e),
    }
    if let Err(e) = aproxy::install::flow::check_jurisdiction(run_dir, home, false).await {
        fail(e.into());
    }

    // 下载链条：产物落 install/staging/<版本>/（与 --from 的备料同一位置）
    let chain = aproxy::install::download::effective_chain(&settings);
    events::say(format_args!(
        "开始下载 aProxy {target}（{}；渠道链：{}）...",
        ctx.variant,
        chain
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join(" → ")
    ));
    let staged_dir = aproxy::install::staging::staging_dir_in(home, &target);

    // 依次尝试的产物（次序见 DownloadCtx::attempts）。下载之后在本机执行 `--version` 自证
    // （能运行、自报版本 == 目标、变体对得上——与 --from 的信任锚同级），跑不起来就换下一个。
    // 下载链条本身失败（网络/404）不换——重跑只会重复同一错误。
    let attempts = ctx.attempts(explicit_variant);
    enum DownloadOutcome {
        Ok(aproxy::install::download::Fetched),
        VerifyFailed(String),
        ChainFailed(InstallFailure),
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
        let output = match aproxy::install::staging::run_version(&fetched.path) {
            Ok(output) => output,
            Err(e) => {
                return DownloadOutcome::VerifyFailed(format!("下载产物不可运行: {e}"));
            }
        };
        let line = match aproxy::install::staging::version_of(&fetched.path, &output) {
            Ok(_) => aproxy::install::force::VersionLine::parse(&output)
                .expect("version_of 认得的输出 VersionLine 也认得"),
            Err(e) => return DownloadOutcome::VerifyFailed(format!("下载产物校验失败: {e}")),
        };
        if line.version != target {
            return DownloadOutcome::VerifyFailed(format!(
                "下载产物自报 {}，与目标版本 {target} 不符",
                line.version
            ));
        }
        // 变体不符是产物与名字对不上（例如 url 模板指错了文件），换一个产物也不会好
        if line.variant.as_deref() != Some(ctx.variant) {
            return DownloadOutcome::ChainFailed(InstallFailure::new(
                "install_probe_failed",
                format!(
                    "下载的 {} 自报变体 {}，不是要装的 {}",
                    ctx.binary_asset(),
                    line.variant.as_deref().unwrap_or("未知"),
                    ctx.variant
                ),
            ));
        }
        DownloadOutcome::Ok(fetched)
    }
    let mut failures: Vec<InstallFailure> = Vec::new();
    let mut installed = None;
    // 有带界面的产物下载下来了、却被动态加载器挡在 main 之前（本机缺系统库，见
    // staging::is_loader_failure）；超时、输出不对之类的失败与系统库无关，不算
    let mut desktop_unusable = false;
    // 带界面的产物在下载链的哪个渠道都没拿到。下载链分不清「这一版没有为本机发布它」（404）
    // 与网络暂时出错，所以不像跑不起来那样自动改装 headless——一次网络抖动就会把用户悄悄
    // 换成 headless；只在失败时指出两条出路。强制安装按发布清单判断，确定没有才改装
    let mut desktop_not_fetched = false;
    for attempt in &attempts {
        if let Some(previous) = failures.last() {
            events::say(format_args!(
                "{previous}，改用 {}...",
                attempt.binary_asset()
            ));
        }
        match download_verified(attempt, &chain, &target, &staged_dir).await {
            DownloadOutcome::Ok(f) => {
                installed = Some((f, attempt.variant));
                break;
            }
            DownloadOutcome::VerifyFailed(e) => {
                desktop_unusable |= attempt.variant == VARIANT_DESKTOP
                    && aproxy::install::staging::is_loader_failure(&e);
                failures.push(InstallFailure::new(
                    "install_probe_failed",
                    format!("{} 在本机不可运行（{e}）", attempt.binary_asset()),
                ));
            }
            DownloadOutcome::ChainFailed(e) => {
                desktop_not_fetched = attempt.variant == VARIANT_DESKTOP;
                failures.push(e);
                break;
            }
        }
    }
    let Some((fetched, installed_variant)) = installed else {
        for failure in &failures {
            eprintln!("[ERROR] {failure}");
        }
        if !explicit_variant && desktop_not_fetched {
            eprintln!(
                "没写 --variant，要装的是带桌面界面的版本。若这一版没有为本机发布它，可加 --variant headless 改装无界面版本；或者用 aproxy install --force（会先结束全部 aproxy 进程、中断在途请求），它按发布清单判断，确实没有带界面的版本时改装 headless。"
            );
        }
        // 没写 --variant 时 headless 已经试过了（见 DownloadCtx::attempts），这句只给写明了 desktop 的。
        // 不写具体的库与版本：要求属于目标版本的构建，这里的文字却随正在运行的旧版本永远不变
        if explicit_variant && desktop_unusable && cfg!(target_os = "linux") {
            eprintln!(
                "带桌面界面的版本在本机起不来：动态加载器的报错见上，多半是缺 WebKitGTK 等系统库、这些库太旧，或者 glibc 太旧。不需要窗口，就改装无界面版本：aproxy install --variant headless"
            );
        }
        eprintln!(
            "可尝试：--download-proxy <URL> 配置下载代理，或在 settings.json 配置 download_chain（含 url 模板 CDN 通道，文档见 README）。"
        );
        let e = failures.last().expect("每次下载尝试都有结果");
        events::result(
            "failed",
            Some((e, e.message.lines().next().unwrap_or(&e.message))),
        );
        crate::util::exit(e.exit_code());
    };
    if installed_variant != attempts[0].variant {
        events::say(format_args!(
            "改装的是无界面的 headless 版本：代理与命令行完全相同，只是没有 aproxy desktop 窗口。带界面的版本起不来的原因见上（Linux 上多半是缺 WebKitGTK 等系统库，或者 glibc 太旧），解决之后可用 aproxy install --variant desktop 换回。"
        ));
    }

    events::say(format_args!(
        "开始安装 aProxy {target}（{installed_variant}）..."
    ));
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
    report_outcome(home, result).await;
}

/// 安装结局的用户可见输出与退出码（三个安装入口共用）。**一律硬退**：
/// 宣告 ticker 与 skill 支线等后台任务不保证在此刻已结束，走正常 return
/// 会让 tokio runtime drop 等待它们（实测 27 个交棒进程全体挂死的根源）。
///
/// 交棒（`HandedOver`）≠ 安装完成：广播、交换、滚动重启、终验由接手的目标
/// 版本进程执行，本进程必须等它跑到终点再报告——否则接手者失败时用户看到的
/// 是退出码 0，agent 会以为升级成功。
async fn report_outcome(
    home: &std::path::Path,
    result: Result<aproxy::install::flow::FlowExit, InstallFailure>,
) -> ! {
    let result = match result {
        Ok(aproxy::install::flow::FlowExit::HandedOver) => {
            events::say("新版本进程已接手安装（广播、交换、滚动重启、终验），等待其完成...");
            await_handover(home).await
        }
        other => other.map(|_| ()),
    };
    if let Err(e) = result {
        if refusal(&e).is_some() {
            fail(e);
        }
        let message = format!("安装失败: {e}");
        eprintln!("[ERROR] {message}");
        print_failure_guidance(home);
        events::result(
            "failed",
            Some((&e, message.lines().next().unwrap_or(&message))),
        );
        crate::util::exit(e.exit_code());
    }
    events::say(format_args!(
        "安装完成：{}（二进制已落位，实例已滚动到新版本）",
        aproxy::install::swap::bin_path_in(home).display()
    ));
    events::result("installed", None);
    crate::util::exit(0);
}

/// 接手者结局等待上限：与安装锁 stale 判定同量级（滚动重启每实例最坏
/// 约 20s 停止 + 8s 就绪 + 8s 回退，十分钟覆盖数十个实例）。
const HANDOVER_WAIT_SECS: u64 = aproxy::install::state::STALE_AFTER_SECS;

/// 轮询 install.state 直到接手者到达终点。成功判据：状态文件消失（done
/// 清场）或 phase 为 done。cleaning 的持锁与状态写入仍可能失败，不能把它当成成功；
/// 等待者退出才删得掉的 Windows 旧镜像由接手者在写成 done 后重试。失败判据：phase=failed、
/// 接手进程已死而安装未终结、等待超时。
async fn await_handover(home: &std::path::Path) -> Result<(), InstallFailure> {
    let mut owner = OwnerExit::default();
    wait_for_outcome(home, |pid| owner.observe(pid)).await
}

/// 首次看到属主便持有只读的 Windows 进程句柄：内核对象不随 pid 消失，退出后仍可查退出码。
#[derive(Default)]
struct OwnerExit {
    #[cfg(windows)]
    handle: Option<(u32, std::os::windows::io::OwnedHandle)>,
}

impl OwnerExit {
    fn observe(&mut self, pid: u32) -> Option<String> {
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
            use windows_sys::Win32::System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, WaitForSingleObject,
            };
            if self.handle.as_ref().is_none_or(|(id, _)| *id != pid) {
                let h = unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        0,
                        pid,
                    )
                };
                self.handle =
                    (h != 0).then(|| (pid, unsafe { OwnedHandle::from_raw_handle(h as _) }));
            }
            if let Some((_, handle)) = &self.handle {
                let handle = handle.as_raw_handle() as _;
                if unsafe { WaitForSingleObject(handle, 0) }
                    != windows_sys::Win32::Foundation::WAIT_OBJECT_0
                {
                    return None;
                }
                let mut code = 0;
                return Some(if unsafe { GetExitCodeProcess(handle, &mut code) } != 0 {
                    format!("exit status {code} (0x{code:08x})")
                } else {
                    "exit status unavailable".into()
                });
            }
        }
        (aproxy::process::process_exited(pid) == Some(true))
            .then(|| "exit status unavailable".to_owned())
    }
}

async fn wait_for_outcome(
    home: &std::path::Path,
    mut exited: impl FnMut(u32) -> Option<String>,
) -> Result<(), InstallFailure> {
    use aproxy::install::state::{ExitEvidence, InstallPhase, load_in, state_path_in};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(HANDOVER_WAIT_SECS);
    loop {
        if events::take_outcome(home)? {
            return Ok(());
        }
        match load_in(home).inspect(events::observe) {
            None if !state_path_in(home).exists() => {
                // rename 可能恰在两次读取之间完成。
                events::take_outcome(home)?;
                return Ok(());
            }
            // 文件在但读不出（状态损坏）：不臆断，继续等到超时
            None => {}
            Some(s) => match s.phase {
                InstallPhase::Done
                    if s.report_to.is_some_and(|id| {
                        aproxy::install::state::OBSERVER
                            .try_with(|o| o.id == id)
                            .unwrap_or(false)
                    }) && exited(s.installer_pid).is_none() => {}
                InstallPhase::Done => return Ok(()),
                InstallPhase::Failed => {
                    return Err(InstallFailure::recorded(
                        &s,
                        s.last_error
                            .clone()
                            .unwrap_or_else(|| "接手进程报告失败（原因未记录）".to_string()),
                    ));
                }
                InstallPhase::Aborted => return Err("安装已被中止（--abort）".to_string().into()),
                _ if s.installer_pid != std::process::id()
                    && let Some(status) = exited(s.installer_pid) =>
                {
                    // 读取与查判活之间可能已发布终态、再次交棒或清场；不能拿旧快照解释当前退出。
                    if !load_in(home).is_some_and(|fresh| {
                        (
                            fresh.installer_pid,
                            fresh.started_at,
                            fresh.phase,
                            fresh.updated_at,
                        ) == (s.installer_pid, s.started_at, s.phase, s.updated_at)
                    }) {
                        continue;
                    }
                    let reason = ExitEvidence::read(home, &s)
                        .map(|e| e.reason)
                        .unwrap_or_else(|| {
                            let reason = format!("no terminal state; {status}");
                            let _ = ExitEvidence::record(home, &s, &reason);
                            reason
                        });
                    return Err(format!("接手进程（pid {}）已退出但安装停在 {:?}：{reason}；重新执行 aproxy install 即可接着装完", s.installer_pid, s.phase).into());
                }
                _ => {}
            },
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{HANDOVER_WAIT_SECS} 秒内未确认安装完成（可能仍在后台进行）；可用 aproxy status 查看实例版本"
            ).into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// 失败后的处置指引：实例级失败（halted）与可续作的失败给不同的出路。
fn print_failure_guidance(home: &std::path::Path) {
    let halted = aproxy::install::state::load_in(home).is_some_and(|s| s.halted);
    if halted {
        eprintln!(
            "滚动已中止，不会自动重试：出问题的实例已用旧二进制拉回（拉回失败时其恢复记录已保留），\
             其余实例未动。常见原因是新版本不接受现有配置——按上方原因修正后重新执行 aproxy install 完成升级；\
             若要留在旧版本，用 `aproxy install <旧版本> --allow-downgrade` 把规范位置的二进制换回。"
        );
    } else {
        eprintln!(
            "现场已保留：重新执行同一条命令即可接着装完；看护者，以及会改动实例或设置的 aproxy 命令\
             （start、stop、restart、restore 等），也会在后台接着装。交换二进制之前可以用 \
             `aproxy install --abort` 回滚。"
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
    match aproxy::install::flow::check_restartable(home).await {
        Ok(ports) => events::plan(&ports),
        Err(e) => fail(e),
    }
    if let Err(e) = aproxy::install::flow::check_jurisdiction(run_dir, home, adopt).await {
        fail(e.into());
    }
    let target = plan.target_version.clone();
    events::say(format_args!("开始安装 aProxy {target}..."));
    let result =
        aproxy::install::flow::run_install(home, run_dir, &plan, skill_enabled, download_proxy)
            .await;
    report_outcome(home, result).await;
}

/// --abort：显式回滚（仅 swapping 前可完全回滚，此后只进不退）。
fn handle_abort(home: &std::path::Path) {
    let Some(state) = aproxy::install::state::load_in(home) else {
        println!("没有进行中的安装。");
        return;
    };
    // swapping 及之后只进不退（运行实例可能已开始滚动）——拒绝并说明
    if state.phase >= aproxy::install::state::InstallPhase::Swapping {
        fail(InstallFailure::new(
            "install_failed",
            format!(
                "安装已进入交换阶段（phase {:?}），无法回滚——实例可能已开始滚动到新版本。\n中断的安装会自动续作完成；如需手动干预，按 skill 的手动更新兜底路径操作。",
                state.phase
            ),
        ));
    }
    // swapping 前：二进制未动，干净回滚 = 清现场（staging + 状态文件）
    if let Some(staged) = &state.staged_path {
        let _ = std::fs::remove_file(staged);
    }
    if let Some(v) = Some(state.target_version.clone()) {
        let _ = std::fs::remove_dir_all(aproxy::install::staging::staging_dir_in(home, &v));
    }
    aproxy::install::state::remove_in(home);
    println!("安装已中止，现场已清理（二进制与实例未受影响）。");
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn signalled_owner_can_exit_with_still_active_as_its_real_code() {
        use std::{
            io::Write,
            os::windows::process::CommandExt,
            process::{Command, Stdio},
        };
        let mut c = Command::new("cmd.exe")
            .creation_flags(0x08000000)
            .args(["/d", "/c", "set /p answer= & exit /b 259"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut owner = super::OwnerExit::default();
        let pid = c.id();
        assert_eq!(owner.observe(pid), None);
        c.stdin.take().unwrap().write_all(b"done\r\n").unwrap();
        assert_eq!(c.wait().unwrap().code(), Some(259));
        assert_eq!(
            owner.observe(pid).as_deref(),
            Some("exit status 259 (0x00000103)")
        );
        assert_eq!(aproxy::process::process_exited(pid), Some(true));
    }

    use super::*;

    #[tokio::test]
    async fn terminal_publish_between_state_and_exit_probe_wins() {
        use aproxy::install::state::{self, InstallPhase, InstallSource, InstallState};
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for success in [false, true] {
            let mut st = InstallState::new_marking("x", InstallSource::From);
            st.phase = InstallPhase::Verifying;
            st.installer_pid = 1;
            state::write_in(home, &mut st).unwrap();
            let result = wait_for_outcome(home, |_| {
                if success {
                    state::remove_in(home);
                } else {
                    state::fail_in(
                        home,
                        &mut st,
                        &InstallFailure::new("install_version_not_found", "terminal reason"),
                    );
                }
                Some("exited".into())
            })
            .await;
            if success {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().message, "terminal reason");
            }
        }
    }

    /// cleaning 仍可能失败；等待者不能抢在失败落盘前退出，也不能丢掉接手者的退出码。
    #[tokio::test]
    async fn handover_reports_a_failure_after_cleaning() {
        use aproxy::install::state::{self, InstallPhase, InstallSource, InstallState};
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut st = InstallState::new_marking("x", InstallSource::From);
        st.phase = InstallPhase::Cleaning;
        state::write_in(home, &mut st).unwrap();
        let publish = async {
            tokio::task::yield_now().await;
            state::fail_in(
                home,
                &mut st,
                &InstallFailure::new("install_version_not_found", "from successor"),
            );
        };
        let (outcome, ()) = tokio::join!(await_handover(home), publish);
        let error = outcome.unwrap_err();
        assert_eq!(error.message, "from successor");
        assert_eq!(error.exit_code(), 4);
        st.error_code = None;
        state::write_in(home, &mut st).unwrap();
        assert_eq!(await_handover(home).await.unwrap_err().exit_code(), 1);
    }
}
