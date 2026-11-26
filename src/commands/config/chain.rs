//! 文件的静态链路。先得到一份脱敏事实，再选择终端排版或机器文档；不查询实例或握手。

use super::{mask_transform_args, print_doc, source_label, text};
use aproxy::cli::ConfigChain;
use aproxy::config::{
    self, Resolved, Source,
    file::*,
    resolve::{FileTrace, Layer, Withheld},
    view,
};
use aproxy_ipc::i18n::{Lang, current};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// 按 clap 接受的名字列出目录，不要求存在配置文件。
pub(crate) fn list_chains(json: bool) {
    let chains = ConfigChain::value_variants();
    if json {
        print_doc(
            json!({"chains": chains.iter().map(|c| json!({"name": c, "description": c.description(Lang::En)})).collect::<Vec<_>>() }),
        );
    } else {
        for chain in chains {
            let name = chain.to_possible_value().expect("visible chain");
            println!("{}: {}", name.get_name(), chain.description(current()));
        }
    }
}

/// 两条链与普通 show 使用相同的文件名、诊断与单文档边界。
fn document(chain: ConfigChain, path: &Path, resolved: &Resolved, mut doc: Value) {
    doc["chain"] = json!(chain);
    doc["file"] = path.display().to_string().into();
    doc["diagnostics"] = json!(resolved.diagnostics);
    print_doc(doc);
}

#[derive(Serialize)]
struct KeyRow<'a> {
    key: String,
    value: Value,
    source: Option<Source>,
    shadowed: Vec<Layer>,
    instance_only: bool,
    withheld: Vec<&'a Withheld>,
    #[serde(skip_serializing_if = "Option::is_none")]
    environment: Option<&'a BTreeMap<String, String>>,
}

/// 标量与数组沿用 --effective 的阅读顺序；两张头表各自按头名展开，包括只出现在 withheld 的头。
fn key_order(values: &Value, resolved: &Resolved) -> Vec<String> {
    let mut keys: Vec<String> = [
        "base_url",
        "listen_addr",
        "api_key",
        "delivery",
        "max_request_mb",
        "max_response_mb",
        "disk_cache",
        "allowed_hosts",
        "allowed_origins",
        "log_file",
        "timeouts.connect_secs",
        "timeouts.first_byte_secs",
        "timeouts.read_idle_secs",
        "history.body_budget_mb",
        "keepalive.interval_secs",
        "keepalive.trigger",
        "keepalive.heartbeat",
        "retry.max_backoff_secs",
        "retry.rules",
        "format",
        "proxy",
        "proxy_username",
        "proxy_password",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for table in ["extra_headers", "override_headers"] {
        let mut names: Vec<String> = values[table].as_object().unwrap().keys().cloned().collect();
        for item in &resolved.withheld {
            if let Some(parts) = config::parse_key_path(&item.key)
                && let [parent, header] = parts.as_slice()
                && parent.name == table
                && !names.iter().any(|n| n.eq_ignore_ascii_case(&header.name))
            {
                names.push(header.name.clone());
            }
        }
        names.sort();
        if names.is_empty() {
            keys.push(table.into());
        }
        keys.extend(
            names
                .into_iter()
                .map(|n| format!("{table}.{}", config::quote_key(&n))),
        );
    }
    keys
}

/// 首行是生效值；只有较低层实际设了该键时才有缩进的后续行。
pub(super) fn overrides(
    path: &Path,
    resolved: &Resolved,
    mut trace: FileTrace,
    file: Option<&Value>,
    json: bool,
) {
    let values = view::override_values(resolved, file);
    let rows: Vec<_> = key_order(&values, resolved)
        .into_iter()
        .map(|key| {
            let source = resolved.sources.get(&key).copied();
            let instance_only = matches!(key.as_str(), "listen_addr" | "log_file" | "format");
            KeyRow {
                value: view::get(&values, &key)
                    .expect("resolver key")
                    .cloned()
                    .unwrap_or_default(),
                instance_only,
                shadowed: if instance_only {
                    Vec::new()
                } else {
                    trace.shadowed.remove(&key).unwrap_or_default()
                },
                withheld: resolved
                    .withheld
                    .iter()
                    .filter(|w| w.key.eq_ignore_ascii_case(&key))
                    .collect(),
                environment: (source == Some(Source::Env)).then_some(&trace.proxy_environment),
                key,
                source,
            }
        })
        .collect();
    if json {
        document(
            ConfigChain::Overrides,
            path,
            resolved,
            json!({"start_options_included": false, "keys": rows}),
        );
        return;
    }
    let lang = current();
    println!("{}", text::file(lang, path.display()));
    println!("{}", text::chain_precedence(lang));
    for row in rows {
        let shown = if let Some(vars) = row.environment {
            json!(vars).to_string()
        } else if row.value.is_null() {
            text::unset(lang).into()
        } else {
            view::for_display(row.value).to_string()
        };
        let line = text::value(lang, &row.key, &shown, source_label(row.source));
        println!(
            "{line}{}",
            if row.instance_only {
                text::instance_only(lang)
            } else {
                ""
            }
        );
        for layer in row.shadowed {
            println!(
                "{}",
                text::lower_layer(
                    lang,
                    source_label(Some(layer.source)),
                    &view::for_display(layer.value).to_string()
                )
            );
        }
        for item in row.withheld {
            println!("  {}", text::withheld(lang));
            println!(
                "{}",
                text::targets(
                    lang,
                    &item.key,
                    &item.default_target,
                    if item.target.is_empty() {
                        text::unset(lang)
                    } else {
                        &item.target
                    }
                )
            );
        }
    }
}

/// 环境只暴露名字；只列偏离解析器默认值的超时与熔断参数。
fn entry_view(entry: &FormatEntry) -> Value {
    let mut out = json!({
        "name": entry.name, "command": entry.command, "args": mask_transform_args(&entry.args),
        "env_names": entry.env.keys().collect::<Vec<_>>(), "process": entry.process,
        "required": entry.required, "stages": entry.stages
    });
    for (key, value, default) in [
        (
            "worker_idle_secs",
            entry.worker_idle_secs,
            DEFAULT_FORMAT_WORKER_IDLE_SECS,
        ),
        (
            "timeout_secs",
            entry.timeout_secs,
            DEFAULT_FORMAT_TIMEOUT_SECS,
        ),
        (
            "decide_timeout_secs",
            entry.decide_timeout_secs,
            DEFAULT_FORMAT_DECIDE_TIMEOUT_SECS,
        ),
        (
            "breaker_failures",
            entry.breaker_failures.into(),
            DEFAULT_FORMAT_BREAKER_FAILURES.into(),
        ),
        (
            "breaker_cooldown_secs",
            entry.breaker_cooldown_secs,
            DEFAULT_FORMAT_BREAKER_COOLDOWN_SECS,
        ),
    ] {
        if value != default {
            out[key] = value.into();
        }
    }
    out
}

/// 顺序和协议事实只算一次。notify 列的是观察者，不是一串传递内容的变换。
pub(super) fn formats(path: &Path, resolved: &Resolved, json: bool) {
    use Stage::*;
    let formats = &resolved.config.formats;
    let lang = current();
    if !json && formats.is_empty() {
        println!("{}", text::no_formats(lang));
        return;
    }
    let entries: Vec<_> = formats.iter().map(entry_view).collect();
    let names = |stage| {
        let mut names: Vec<_> = formats
            .iter()
            .filter(|e| e.has_stage(stage))
            .map(|e| e.name.as_str())
            .collect();
        if matches!(stage, Response | Stream | Heartbeat) {
            names.reverse();
        }
        names
    };
    let order = [Request, Retry, Decide, Response, Stream, Heartbeat, Notify];
    let stages: BTreeMap<_, _> = order.into_iter().map(|s| (s.as_str(), names(s))).collect();
    let mut before_retry = Vec::new();
    let mut first_only = Vec::new();
    for entry in formats.iter().filter(|e| e.has_stage(Request)) {
        if entry.has_stage(Retry) {
            before_retry.push(entry.name.as_str());
        } else {
            first_only.push(entry.name.as_str());
        }
    }
    let optional: Vec<_> = formats
        .iter()
        .filter(|e| !e.required)
        .map(|e| e.name.as_str())
        .collect();
    let decisions = [
        ("delivery", &stages["request"]),
        ("decision", &stages["decide"]),
        ("reroute", &stages["decide"]),
    ];
    let last = ["request", "response", "stream"].map(|s| (s, stages[s].last().copied()));
    if json {
        document(
            ConfigChain::Formats,
            path,
            resolved,
            json!({
                "entries": entries, "stages": stages,
                "facts": {
                    "first_answer": decisions.into_iter().collect::<BTreeMap<_,_>>(),
                    "last_content": last.into_iter().collect::<BTreeMap<_,_>>(),
                    "request_on_retry": before_retry, "first_attempt_only": first_only,
                    "skip_on_failure_or_open_breaker": optional, "notify_observes_only": true
                },
                "not_known": ["content_effects", "final_protocol"],
                "observe_at": ["desktop_request_detail.protocol_path", "aproxy logs"]
            }),
        );
        return;
    }
    println!("{}", text::file(lang, path.display()));
    for entry in entries {
        println!("{}", text::format_entry(lang, &entry["name"].to_string()));
        for (key, value) in entry.as_object().unwrap() {
            if key != "name" {
                println!("  {key} = {value}");
            }
        }
    }
    println!("{}", text::stage_order(lang));
    let mut idle = Vec::new();
    for stage in order.map(|s| s.as_str()) {
        let names = &stages[stage];
        if names.is_empty() {
            idle.push(stage);
        } else if stage == "notify" {
            println!("{}", text::observers(lang, &names.join(", ")));
        } else {
            println!("{stage}: {}", names.join(" → "));
        }
    }
    if !idle.is_empty() {
        println!("{}", text::no_stage_program(lang, &idle.join(", ")));
    }
    let list = |names: &[&str]| {
        if names.is_empty() {
            "—".into()
        } else {
            names.join(" → ")
        }
    };
    for (field, names) in decisions {
        println!("{}", text::first_answer(lang, field, &list(names)));
    }
    for (stage, name) in last {
        if let Some(name) = name {
            println!(
                "{}",
                if stage == "request" {
                    text::last_upstream(lang, name)
                } else {
                    text::last_client(lang, stage, name)
                }
            );
        }
    }
    println!(
        "{}",
        text::retry_entries(lang, &list(&before_retry), &list(&first_only))
    );
    println!("{}", text::optional_entries(lang, &list(&optional)));
    println!("{}", text::format_limits(lang));
}
