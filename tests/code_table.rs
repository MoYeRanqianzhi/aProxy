//! 码表文档 `docs/error-codes.md` 由码表（`aproxy_ipc::code::TABLE`）渲染而来，这里比对
//! 仓库里的那份与现渲染的是否一致：码表改了而文档没跟上，这个测试就失败。
//!
//! 重新生成：`APROXY_BLESS=1 cargo test --test code_table`，然后把改动一起提交。

use aproxy_ipc::code::{Code, DetailType, ExitCode, Family, HttpStatus, Severity, TABLE};

/// 文档里一个族的一段：标题、开头的说明，以及该族特有的那一列（没有就是 None）。
struct Section {
    family: Family,
    title: &'static str,
    intro: &'static str,
    column: Option<Column>,
}

/// 一个族特有的那一列：表头，以及由码算出单元格的函数。
type Column = (&'static str, fn(&Code) -> String);

fn http_cell(code: &Code) -> String {
    match code.http.expect("数据面终态码都带 HTTP 状态") {
        HttpStatus::Fixed(status) => status.to_string(),
        HttpStatus::PassthroughOr(status) => format!("passthrough, else {status}"),
        HttpStatus::SseOnly => "SSE event only".to_string(),
    }
}

fn exit_cell(code: &Code) -> String {
    match code.exit.expect("操作码与安装码都带退出码") {
        ExitCode::Fixed(n) => n.to_string(),
        ExitCode::ChildOr(n) => format!("the child's code, else {n}"),
    }
}

fn severity_cell(code: &Code) -> String {
    match code.severity.expect("配置诊断码都带级别") {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Varies => "varies",
    }
    .to_string()
}

const SECTIONS: &[Section] = &[
    Section {
        family: Family::Terminal,
        title: "Terminal codes (data plane)",
        intro: "How a proxied request finally ended when aproxy itself ended it. The code is in the JSON error \
                body as `error.aproxy.code` (and `error.type` is `aproxy_` plus the code), in the \
                `x-aproxy-error` response header, and in the terminal SSE `event: error` when the response \
                headers were already sent. \"passthrough\" means: when there is an upstream response (or one \
                given by a format), it is returned as is with its own status.",
        column: Some(("HTTP status", http_cell)),
    },
    Section {
        family: Family::Attempt,
        title: "Attempt failure codes",
        intro: "Why one upstream attempt did not succeed. By default every one of them is retried without \
                limit; retry rules and formats can change that. They appear as `last_code` in terminal codes \
                and in request records.",
        column: None,
    },
    Section {
        family: Family::Ipc,
        title: "IPC protocol codes",
        intro: "The `code` of the error object an instance returns on its control endpoint.",
        column: None,
    },
    Section {
        family: Family::Operation,
        title: "Operation codes",
        intro: "Why an operation started from the command line or the desktop window failed. With `--json` \
                the code is in the output; the process exits with the listed exit code. General exit codes: \
                0 success or the target state already holds, 1 the operation failed, 2 usage error, \
                3 invalid configuration, 4 the target is not running or does not exist, 5 partial success. \
                Exit codes from 10 on belong to `install --force`.",
        column: Some(("Exit code", exit_cell)),
    },
    Section {
        family: Family::Diagnostic,
        title: "Configuration diagnostic codes",
        intro: "Problems found in config.toml and settings.json. Each diagnostic is an object with `code`, \
                `severity`, `file`, optional `key`, `line`, `column`, a `message` and `details`. Only \
                `error` diagnostics affect the exit code.",
        column: Some(("Severity", severity_cell)),
    },
    Section {
        family: Family::Record,
        title: "Record codes",
        intro: "Why a persistent record under the home could not be used. Such records are reported, never \
                deleted.",
        column: None,
    },
    Section {
        family: Family::Install,
        title: "Install codes",
        intro: "Why an install or upgrade failed. `install --force` does not use these; it only promises \
                its own exit codes.",
        column: Some(("Exit code", exit_cell)),
    },
];

fn type_name(ty: DetailType) -> &'static str {
    match ty {
        DetailType::String => "string",
        DetailType::StringList => "string list",
        DetailType::Integer => "integer",
        DetailType::IntegerList => "integer list",
        DetailType::Bytes => "bytes",
        DetailType::Millis => "milliseconds",
        DetailType::TimestampMs => "Unix milliseconds",
        DetailType::Path => "path",
        DetailType::Code => "code",
        DetailType::Diagnostics => "diagnostic list",
    }
}

fn details_cell(code: &Code) -> String {
    if code.details.is_empty() {
        return "—".to_string();
    }
    code.details
        .iter()
        .map(|d| {
            format!(
                "`{}`{} ({})",
                d.key,
                if d.optional { "?" } else { "" },
                type_name(d.ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render() -> String {
    let mut out = String::from(
        "# Error codes\n\
         \n\
         <!-- Generated from aproxy_ipc::code::TABLE by tests/code_table.rs. Do not edit by hand: \
         change the table, then run `APROXY_BLESS=1 cargo test --test code_table`. -->\n\
         \n\
         Every code aproxy reports, with what it carries. Codes are snake_case, unique across all \
         families, and only ever added: a published code keeps its name, its family, its HTTP status \
         or exit code, and the names and types of its `details` keys. A reader that meets a code it \
         does not know should treat it as a generic failure of its family.\n\
         \n\
         Human-readable messages are not part of the contract; match on codes. A `?` after a `details` \
         key means the key is not always present. The same table is available to programs as JSON \
         from `aproxy_ipc::code::table_json()`.\n",
    );
    for section in SECTIONS {
        out.push_str(&format!("\n## {}\n\n{}\n\n", section.title, section.intro));
        match section.column {
            Some((name, _)) => {
                out.push_str(&format!(
                    "| Code | {name} | When | `details` |\n|---|---|---|---|\n"
                ));
            }
            None => out.push_str("| Code | When | `details` |\n|---|---|---|\n"),
        }
        for code in TABLE
            .iter()
            .filter(|c| c.families.contains(&section.family))
        {
            match section.column {
                Some((_, cell)) => out.push_str(&format!(
                    "| `{}` | {} | {} | {} |\n",
                    code.name,
                    cell(code),
                    code.summary,
                    details_cell(code)
                )),
                None => out.push_str(&format!(
                    "| `{}` | {} | {} |\n",
                    code.name,
                    code.summary,
                    details_cell(code)
                )),
            }
        }
    }
    out
}

#[test]
fn the_document_matches_the_table() {
    // 每个族都有一段，表里的每个码至少落进一段：新加了族却没加段落，文档就会漏码
    for code in TABLE {
        assert!(
            code.families
                .iter()
                .all(|f| SECTIONS.iter().any(|s| s.family == *f)),
            "{} 所属的族在文档里没有段落",
            code.name
        );
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/error-codes.md");
    let rendered = render();
    if std::env::var_os("APROXY_BLESS").is_some() {
        std::fs::write(&path, &rendered).unwrap();
        return;
    }
    // 仓库在 Windows 上签出时行尾可能是 CRLF，比对之前统一成 LF
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == rendered,
        "docs/error-codes.md 与码表不一致。码表改过的话，执行 `APROXY_BLESS=1 cargo test --test code_table` \
         重新生成并一起提交"
    );
}
