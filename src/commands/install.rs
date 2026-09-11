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
        run_plan(&home, &run_dir, plan, false).await;
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
        run_plan(&home, &run_dir, plan, true).await;
        return;
    }

    // 无参：提示在线渠道随后续版本提供（下载链条基建完成前 --from 是唯一入口）
    eprintln!(
        "用法:\n  aproxy install --from <二进制路径>   从本地文件安装/升级\n  aproxy install --adopt              把当前运行的 aProxy 收编到 ~/.aproxy/bin\n  aproxy install --abort              中止进行中的安装\n\n在线渠道（github/npm/cargo，`aproxy install [版本]`）随后续版本提供。"
    );
    std::process::exit(1);
}

/// 执行安装计划：管辖检查 → 主流程。成功/失败的用户可见输出在此统一。
async fn run_plan(
    home: &std::path::Path,
    run_dir: &std::path::Path,
    plan: aproxy::install::flow::InstallPlan,
    adopt: bool,
) {
    if let Err(e) = aproxy::install::flow::check_jurisdiction(run_dir, home, adopt).await {
        eprintln!("[ERROR] {e}");
        std::process::exit(1);
    }
    let target = plan.target_version.clone();
    println!("开始安装 aProxy {target}...");
    match aproxy::install::flow::run_install(home, run_dir, &plan).await {
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
