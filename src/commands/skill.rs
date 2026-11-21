//! skill 命令只管理文档。读取结果写 stdout，诊断永远不与正文混写。
use aproxy::{
    cli::SkillCmd,
    install::{
        InstallFailure,
        download::{DownloadCtx, effective_chain},
        skills::{
            self,
            installed::{self, Listing},
        },
    },
};
use aproxy_ipc::code::{EXIT_FAILED, EXIT_NOT_FOUND, EXIT_OK};
use serde_json::json;
use std::io::Write;

/// 独立安装始终取本二进制的精确版本，包括预发布后缀，不发起 latest 查询。
pub(crate) async fn install(proxy: Option<&str>, json_output: bool) -> u8 {
    let version = env!("CARGO_PKG_VERSION");
    let home = aproxy_ipc::home::home();
    let location = home.join("skills");
    let settings = aproxy::settings::load();
    let result = async {
        let ctx = DownloadCtx::build(version, proxy.or(settings.download_proxy.as_deref()), None)
            .map_err(|e| InstallFailure::new("install_download_failed", e))?;
        skills::install(&ctx, &effective_chain(&settings), &home).await
    }
    .await;
    match result {
        Ok(file_count) => {
            if json_output {
                super::print_doc(
                    json!({"result":"installed","version":version,"location":location,"file_count":file_count}),
                );
            } else {
                println!(
                    "已安装 {version} 的 skill 文档（{file_count} 个文件）：{}",
                    location.display()
                );
            }
            EXIT_OK
        }
        Err(e) => {
            eprintln!("{e}");
            if json_output {
                super::print_doc(
                    json!({"result":"failed","version":version,"location":location,"file_count":0,"error":{"code":e.code,"message":aproxy_ipc::code::lookup(&e.code).map(|c|c.summary),"details":e.details}}),
                );
            }
            e.exit_code() as u8
        }
    }
}

/// 列表用状态和退出码同时区分没有安装和缺少清单；正文命令不输出任何包装。
pub(crate) async fn run(cmd: SkillCmd) -> u8 {
    let home = aproxy_ipc::home::home();
    match cmd {
        SkillCmd::VerifyPackage { archive, version } => {
            match skills::archive::verify_package(&archive, &version) {
                Ok(()) => EXIT_OK,
                Err(e) => {
                    eprintln!("{e}");
                    EXIT_FAILED
                }
            }
        }
        SkillCmd::Install {
            download_proxy,
            json,
        } => install(download_proxy.as_deref(), json).await,
        SkillCmd::List { json: as_json } => {
            let listing = match installed::list(&home) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("{e}");
                    return EXIT_FAILED;
                }
            };
            let (state, skills, action, code) = match listing {
                Listing::Absent => ("not_installed", vec![], installed::ABSENT, EXIT_NOT_FOUND),
                Listing::Legacy => ("without_manifest", vec![], installed::LEGACY, EXIT_FAILED),
                Listing::Installed(s) => (
                    "installed",
                    s,
                    "执行 aproxy skill show <name> 阅读文档。",
                    EXIT_OK,
                ),
            };
            if as_json {
                super::print_doc(
                    json!({"state":state,"skills":skills,"next":if code==EXIT_OK{"aproxy skill show <name>"}else{"aproxy skill install"}}),
                );
            } else {
                if !skills.is_empty() {
                    println!("名称\t版本\t与当前版本一致\t位置\t文件");
                }
                for s in skills {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        s.name,
                        s.version,
                        if s.matches_binary { "是" } else { "否" },
                        s.location.display(),
                        s.files.join(", ")
                    );
                }
                println!("{action}");
            }
            code
        }
        SkillCmd::Show { name, file, files } => {
            let bytes = if files {
                installed::list(&home).and_then(|l| match l {
                    Listing::Absent => Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        installed::ABSENT,
                    )),
                    Listing::Legacy => Err(std::io::Error::other(installed::LEGACY)),
                    Listing::Installed(s) => s
                        .into_iter()
                        .find(|s| s.name == name)
                        .map(|s| format!("{}\n", s.files.join("\n")).into_bytes())
                        .ok_or_else(|| {
                            std::io::Error::new(std::io::ErrorKind::NotFound, "skill 名不在清单中")
                        }),
                })
            } else {
                installed::show(&home, &name, file.as_deref())
            };
            if let Err(e) = &bytes
                && e.kind() == std::io::ErrorKind::NotFound
            {
                eprintln!("{e}");
                return EXIT_NOT_FOUND;
            }
            match bytes.and_then(|b| std::io::stdout().lock().write_all(&b)) {
                Ok(()) => EXIT_OK,
                Err(e) => {
                    eprintln!("{e}");
                    EXIT_FAILED
                }
            }
        }
    }
}
