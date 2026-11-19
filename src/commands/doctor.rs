//! `aproxy doctor`：配置体检。

use aproxy::{doctor, settings};
use aproxy_ipc::diag::Level;
use serde_json::json;

/// 删除命令要保留完整的路径或键名，不能让其中的引号结束外层 shell 引号。
fn quoted(arg: &str) -> String {
    let mut text = String::from("'");
    for c in arg.chars() {
        // PowerShell 的弯单引号也能关闭字符串；加一个单引号便保留原字符。
        if cfg!(windows) && matches!(c, '\'' | '\u{2018}'..='\u{201b}') {
            text.push('\'');
        } else if !cfg!(windows) && c == '\'' {
            text.push_str("'\\'");
        }
        text.push(c);
    }
    text.push('\'');
    text
}

/// 人与程序看同一份体检；即使有错误也先写完整报告，再用退出码说明结果。
pub(crate) async fn handle_doctor_cmd(json: bool) {
    let report = doctor::run(&settings::settings_path()).await;
    let (errors, warnings, infos) = (report.errors(), report.warnings(), report.infos());
    if json {
        let mut doc = json!({"files": report.files});
        if let Some(paused) = report.gc_paused {
            doc["gc_paused"] = json!(paused);
        }
        doc["summary"] = json!({"errors": errors, "warnings": warnings});
        super::print_doc(doc);
    } else {
        println!("aProxy 配置体检");
        println!("════════════════");
        if errors == 0 && warnings == 0 && infos == 0 && report.gc_paused.is_none() {
            println!("全部配置正常，未发现问题。");
            return;
        }
        for file in &report.files {
            for d in &file.diagnostics {
                let level = match d.severity {
                    Level::Error => "ERROR",
                    Level::Warning => "WARN",
                    Level::Info => "INFO",
                };
                println!("[{level}] {}", aproxy::ops::start::render_diagnostic(d));
                if d.code == "config_unknown_key"
                    && let Some(key) = &d.key
                {
                    let key = quoted(key);
                    match file.kind {
                        "settings" => println!("    删除这个键: aproxy settings unset {key}"),
                        "config" => println!(
                            "    删除这个键: aproxy config --config {} unset {key}",
                            quoted(&file.file)
                        ),
                        _ => (),
                    }
                }
            }
        }
        if report.gc_paused.is_some() {
            println!("日志清理已暂停：run/ 里有读不懂的记录。");
        }
        println!("检查结果: {errors} error, {warnings} warning, {infos} info");
        if errors > 0 {
            println!(
                "有 error 的配置不能用来启动；settings.json 有 error 时多数命令不能运行。warning 与 info 仅供参考。"
            );
        }
    }
    if errors > 0 {
        crate::util::exit(aproxy_ipc::code::EXIT_CONFIG as i32);
    }
}
