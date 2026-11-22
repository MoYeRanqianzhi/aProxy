//! skill 命令只管理文档。读取结果写 stdout，诊断永远不与正文混写。
#[path = "../i18n/skill.rs"]
mod text;
use aproxy::install::skills::issue::Issue;
use aproxy_ipc::i18n::current;

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
    // JSON 的位置是供显示的字符串，非 Unicode 路径用替代字符，不影响实际安装路径。
    let location = home.join("skills").display().to_string();
    let settings = aproxy::settings::load();
    let result = async {
        let ctx = DownloadCtx::build(version, proxy.or(settings.download_proxy.as_deref()), None)
            .map_err(|e| InstallFailure::because("install_download_failed", e.into()))?;
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
                    "{}",
                    text::installed(current(), version, file_count, &location)
                );
            }
            EXIT_OK
        }
        Err(e) => {
            eprintln!("{}", e.render(current()));
            if json_output {
                super::print_doc(
                    json!({"result":"failed","version":version,"location":location,"file_count":0,"error":{"code":e.code,"message":e.message,"details":e.details}}),
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
        SkillCmd::VerifyPackage {
            archive,
            version,
            packaged_crate,
        } => match skills::archive::verify_package(&archive, &version, packaged_crate.as_deref()) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                eprintln!("{e}");
                EXIT_FAILED
            }
        },
        SkillCmd::Install {
            download_proxy,
            json,
        } => install(download_proxy.as_deref(), json).await,
        SkillCmd::List { json: as_json } => {
            let listing = match installed::list(&home) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("{}", Issue::from_io(&e).render(current()));
                    return EXIT_FAILED;
                }
            };
            let (state, skills, action, code) = match listing {
                Listing::Absent => (
                    "not_installed",
                    vec![],
                    Issue::Absent.render(current()),
                    EXIT_NOT_FOUND,
                ),
                Listing::Legacy => (
                    "without_manifest",
                    vec![],
                    Issue::Legacy.render(current()),
                    EXIT_FAILED,
                ),
                Listing::Installed(s) => {
                    ("installed", s, text::show(current()).to_owned(), EXIT_OK)
                }
            };
            if as_json {
                super::print_doc(
                    json!({"state":state,"skills":skills,"next":if code==EXIT_OK{"aproxy skill show <name>"}else{"aproxy skill install"}}),
                );
            } else {
                if !skills.is_empty() {
                    println!("{}", text::headings(current()));
                }
                for s in skills {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        s.name,
                        s.version,
                        if s.matches_binary {
                            text::yes(current())
                        } else {
                            text::no(current())
                        },
                        s.location,
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
                        Issue::Absent,
                    )),
                    Listing::Legacy => Err(std::io::Error::other(Issue::Legacy)),
                    Listing::Installed(s) => s
                        .into_iter()
                        .find(|s| s.name == name)
                        .map(|s| format!("{}\n", s.files.join("\n")).into_bytes())
                        .ok_or_else(|| {
                            std::io::Error::new(std::io::ErrorKind::NotFound, Issue::UnknownName)
                        }),
                })
            } else {
                installed::show(&home, &name, file.as_deref())
            };
            if let Err(e) = &bytes
                && e.kind() == std::io::ErrorKind::NotFound
            {
                eprintln!("{}", Issue::from_io(e).render(current()));
                return EXIT_NOT_FOUND;
            }
            match bytes.and_then(|b| write_document(std::io::stdout().lock(), &b)) {
                Ok(()) => EXIT_OK,
                Err(e) => {
                    eprintln!("{}", Issue::from_io(&e).render(current()));
                    EXIT_FAILED
                }
            }
        }
    }
}

/// 缓冲输出在 flush 后才算交付；短文档没有尾换行也必须报告底层写入错误。
fn write_document(mut out: impl Write, bytes: &[u8]) -> std::io::Result<()> {
    out.write_all(bytes)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 写入缓冲区成功而最终提交失败，不得把它当作已交付。
    #[test]
    fn buffered_failure_is_propagated() {
        let sink = &mut [0u8; 0][..];
        assert!(write_document(std::io::BufWriter::new(sink), b"short").is_err());
    }
}
