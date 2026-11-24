//! 迁移命令只负责选择读者的语言与输出协议；不依据显示文字判断退出码。
use super::print_doc;
use aproxy_ipc::i18n::{Lang, current};
use serde_json::json;
#[path = "../i18n/migrate_report.rs"]
mod text;

/// 输出每份文件的整体现状；即使部分失败也只交付一个机器文档。
pub(crate) fn run(home: &std::path::Path, dry: bool, json: bool) -> u8 {
    use aproxy::migrate::run;
    let lang = if json { Lang::En } else { current() };
    let files = run(home, dry, lang);
    let count = |outcome| files.iter().filter(|f| f.outcome == outcome).count();
    let (migrated, current, review, failed) = (
        count("migrated"),
        count("current"),
        count("needs-review"),
        count("failed"),
    );
    if json {
        print_doc(
            json!({"files":files,"summary":{"dry_run":dry,"migrated":migrated,"current":current,"needs_review":review,"failed":failed,"running_instances":text::running(lang)}}),
        );
    } else {
        for f in files {
            let outcome = match f.outcome {
                "migrated" if dry => text::preview(lang),
                "migrated" => text::migrated(lang),
                "current" => text::current_file(lang),
                "needs-review" => text::review(lang),
                _ => text::failed(lang),
            };
            println!("{}: {}", f.path.display(), outcome);
            for c in f.changes {
                println!(
                    "  {} → {} = {}",
                    c.old.as_deref().unwrap_or("—"),
                    c.new,
                    c.value
                );
            }
            for i in f.issues {
                let row = i.line.map_or(String::new(), |n| format!(":{n}"));
                println!(
                    "  {}{row}: {} ({})",
                    i.key.as_deref().unwrap_or(""),
                    i.message,
                    i.section
                );
            }
        }
        println!("{}", text::running(lang));
    }
    if failed > 0 {
        1
    } else if review == 0 {
        0
    } else if migrated > 0 {
        5
    } else {
        3
    }
}
