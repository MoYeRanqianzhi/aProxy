//! aproxy-format：官方示例 format 程序。
//!
//! 子命令：
//! - `run [--config <路径>]`：信封循环（aproxy 的 transform command 指向它）。
//!   配置路径缺省回落**首个信封的 extra**（用户把聚合配置路径放 transform
//!   extra），读入后缓存于进程内存——persistent 模式下每 worker 一份。
//! - `convert --from <fmt> --to <fmt>`：定向协议转换（stdin 收请求 JSON，
//!   stdout 吐转换后 JSON；不走信封，独立于 aproxy 也能用）。
//!
//! 请求侧/响应侧的区分：信封带 `method` = 请求侧（aproxy 请求转换约定）；
//! 无 `method` = 响应侧。响应侧按信封 url 反查渠道表（无跨进程状态——
//! request/response 转换器是不同进程，设计铁律）。

mod aggregate;
mod config;
mod detect;
mod translate;

use std::io::{BufRead, Write};

use aproxy_envelope::TransformEnvelope;
use config::{AggConfig, ClientFormat};
use switchyard_translation::WireFormat;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("convert") => convert(&args[1..]),
        _ => {
            eprintln!(
                "用法: aproxy-format run [--config <路径>] | convert --from <fmt> --to <fmt>"
            );
            std::process::exit(1);
        }
    }
}

fn error_envelope(reason: &str) -> String {
    // error 行：单请求失败（exit 0）——进程不崩，aproxy 按方向取失败语义
    let env = TransformEnvelope {
        headers: Default::default(),
        error: Some(reason.to_string()),
        ..Default::default()
    };
    env.to_line()
        .unwrap_or_else(|_| r#"{"headers":{},"error":"内部序列化失败"}"#.to_string())
}

fn print_line(s: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn run(args: &[String]) {
    let explicit_config = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1));

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut state: Option<aggregate::AggState> = None;

    // while 循环逐行处理直到 EOF——**persistent 模式的协议义务**：实例退出
    // 靠 aproxy 关闭 stdin 管道（EOF → exit），worker 回收主机制依赖它
    while let Some(Ok(line)) = lines.next() {
        let env = match TransformEnvelope::from_line(line.trim_end()) {
            Ok(e) => e,
            Err(e) => {
                print_line(&error_envelope(&format!("信封解析失败: {e}")));
                continue;
            }
        };

        // 配置惰性加载：--config 优先；缺省回落首个信封的 extra 并缓存
        if state.is_none() {
            let path = explicit_config
                .cloned()
                .or_else(|| (!env.extra.is_empty()).then(|| env.extra.clone()));
            let Some(path) = path else {
                print_line(&error_envelope(
                    "未提供聚合配置（--config 或 transform extra 均为空）",
                ));
                continue;
            };
            match load_config(&path).and_then(|cfg| cfg.validate().map(|_| cfg)) {
                Ok(cfg) => state = Some(aggregate::AggState::new(cfg)),
                Err(e) => {
                    // 配置坏是持续状态：输出 error 行（本请求失败），下一请求
                    // 仍会重试加载——用户修好配置后无需重启 worker
                    print_line(&error_envelope(&format!("聚合配置加载失败: {e}")));
                    continue;
                }
            }
        }

        let out = match handle_envelope(state.as_ref().unwrap(), &env) {
            Ok(e) => match e.to_line() {
                Ok(l) => l,
                Err(err) => error_envelope(&format!("内部序列化失败: {err}")),
            },
            Err(reason) => error_envelope(&reason),
        };
        print_line(&out);
    }
}

fn load_config(path: &str) -> Result<AggConfig, String> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        dirs_home().join(rest)
    } else {
        std::path::PathBuf::from(path)
    };
    let content = std::fs::read_to_string(&expanded)
        .map_err(|e| format!("读取 {}: {e}", expanded.display()))?;
    toml::from_str(&content).map_err(|e| format!("解析失败: {e}"))
}

/// `convert --from <fmt> --to <fmt>`：stdin 收一个请求 JSON（纯 body，非信封）
/// → 协议转换 → stdout 吐转换后 JSON。独立便利命令，不经 aproxy 也能用。
fn convert(args: &[String]) {
    let mut from = None;
    let mut to = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" if i + 1 < args.len() => {
                from = Some(args[i + 1].clone());
                i += 2;
            }
            "--to" if i + 1 < args.len() => {
                to = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("未知参数: {other}");
                std::process::exit(1);
            }
        }
    }
    let (Some(from), Some(to)) = (from, to) else {
        eprintln!(
            "用法: aproxy-format convert --from <fmt> --to <fmt>（fmt 如 AnthropicMessages / OpenAiChat / OpenAiResponses）"
        );
        std::process::exit(1);
    };
    let parse = |s: &str| -> Result<WireFormat, String> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| format!("未知 wire format: {s}"))
    };
    let from = parse(&from).unwrap_or_else(|e| die(&e));
    let to = parse(&to).unwrap_or_else(|e| die(&e));

    let mut input = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err() {
        die("stdin 读取失败");
    }
    let body: serde_json::Value = serde_json::from_str(input.trim())
        .unwrap_or_else(|e| die(&format!("输入不是合法 JSON: {e}")));
    let out = translate::translate_request(from, to, &body)
        .unwrap_or_else(|e| die(&format!("转换失败: {e}")));
    print_line(&out.to_string());
}

fn die(msg: &str) -> ! {
    eprintln!("aproxy-format: {msg}");
    std::process::exit(1);
}

/// `~` 展开的 home 基准（无 dirs 依赖的精简实现：HOME / USERPROFILE）。
fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

fn handle_envelope(
    state: &aggregate::AggState,
    env: &TransformEnvelope,
) -> Result<TransformEnvelope, String> {
    // 请求侧/响应侧区分：信封带 method = 请求侧（aproxy 约定）
    if env.method.is_some() {
        handle_request(state, env)
    } else {
        handle_response(state, env)
    }
}

/// 请求侧：body 提 model → 路由渠道 → 选 key → 协议转换 → 改写 url+鉴权头。
fn handle_request(
    state: &aggregate::AggState,
    env: &TransformEnvelope,
) -> Result<TransformEnvelope, String> {
    let body_bytes = env
        .body_bytes()
        .map_err(|e| format!("body 提取失败: {e}"))?;
    let body: serde_json::Value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).map_err(|e| format!("body 非 JSON: {e}"))?
    };

    let model = aggregate::extract_model(&body).ok_or("body 缺少 model 字段")?;
    let channel = state
        .route(model)
        .ok_or_else(|| format!("model {model} 未命中任何渠道（检查渠道表的 models 模式）"))?;
    let channel_idx = state
        .cfg
        .channels
        .iter()
        .position(|c| c.name == channel.name)
        .expect("渠道来自同一表必有索引");
    let key = state.pick_key(channel_idx);

    // 协议转换：client_format（显式/auto 检测）→ channel.format
    let client_format = translate::resolve_client_format(state.cfg.client_format, &body)
        .ok_or("client_format=auto 检测失败：请求形态不像已知协议（检查字段名）")?;
    let translated = if client_format == channel.format {
        body.clone()
    } else {
        translate::translate_request(client_format, channel.format, &body)
            .map_err(|e| format!("协议转换失败: {e}"))?
    };

    // url 改写：完整替换；preserve_path 时拼接原 path+query
    let new_url = if channel.preserve_path {
        format!(
            "{}{}",
            channel.url.trim_end_matches('/'),
            path_and_query(env.url.as_deref().unwrap_or(""))
        )
    } else {
        channel.url.clone()
    };

    // 鉴权头：先剥客户端原头，再按渠道协议写（防泄漏 + 防冲突）
    let mut headers = env.headers.clone();
    headers.remove("authorization");
    headers.remove("x-api-key");
    match channel.format {
        WireFormat::AnthropicMessages => {
            headers.insert("x-api-key".to_string(), key);
            headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        }
        _ => {
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
        }
    }
    // content-type 对齐渠道协议（同为 JSON 无需变，但显式写明语义）
    headers
        .entry("content-type".to_string())
        .or_insert_with(|| "application/json".to_string());

    // 模型名映射：别名表里的客户端名 → 上游名（转换产物或原 body 上改写）
    let mut out_body = translated;
    if let Some(obj) = out_body.as_object_mut()
        && let Some(m) = obj.get("model").and_then(|m| m.as_str())
        && let Some(mapped) = state.cfg.models.get(m)
    {
        obj.insert(
            "model".to_string(),
            serde_json::Value::String(mapped.clone()),
        );
    }

    let mut out = TransformEnvelope {
        headers,
        extra: env.extra.clone(),
        ..Default::default()
    };
    out.url = Some(new_url);
    out.method = Some(env.method.clone().unwrap_or_else(|| "POST".to_string()));
    out.body = Some(out_body.to_string());
    Ok(out)
}

/// 取 origin 从 host 后开始的 path+query（preserve_path 拼接用）。
fn path_and_query(origin: &str) -> String {
    match origin.split_once("://") {
        Some((_, rest)) => match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => String::new(),
        },
        None => String::new(),
    }
}

/// 响应侧：按信封 url 反查渠道表 → 渠道协议转回 client_format →
/// **剔除鉴权头**（上游真实 key 不得漏给客户端——安全决策）。
fn handle_response(
    state: &aggregate::AggState,
    env: &TransformEnvelope,
) -> Result<TransformEnvelope, String> {
    let upstream_url = env.url.as_deref().ok_or("响应信封缺 url（无法反查渠道）")?;
    let channel = state
        .cfg
        .channels
        .iter()
        .find(|c| upstream_url == c.url || upstream_url.starts_with(c.url.trim_end_matches('/')))
        .ok_or_else(|| format!("url {upstream_url} 反查不到渠道（检查渠道表的 url 配置）"))?;

    let body_bytes = env
        .body_bytes()
        .map_err(|e| format!("body 提取失败: {e}"))?;
    let text = String::from_utf8_lossy(&body_bytes);

    // 客户端协议：显式；auto 时按渠道协议的反向不可知——按响应形态检测
    let client_format = match state.cfg.client_format {
        ClientFormat::Explicit(f) => f,
        ClientFormat::Auto => {
            if detect::looks_like_sse(&text) {
                // SSE 流形态无法廉价判别协议——按渠道协议同格式直通
                // （SSE 的协议检测留待 switchyard 提供权威检测后接入）
                channel.format
            } else {
                let v: serde_json::Value = serde_json::from_str(text.trim())
                    .map_err(|e| format!("响应 body 非 JSON: {e}"))?;
                detect::detect_response(&v).ok_or("响应协议检测失败（auto 模式）")?
            }
        }
    };

    let mut headers = env.headers.clone();
    // 安全决策：上游真实 key 不回传客户端
    headers.remove("authorization");
    headers.remove("x-api-key");

    let mut out = TransformEnvelope {
        headers,
        extra: env.extra.clone(),
        ..Default::default()
    };
    out.url = env.url.clone();

    if detect::looks_like_sse(&text) {
        // **流式（SSE）v1 划界：直通原样**——switchyard 的流 API 产出 wire
        // 事件对象，需按目标协议自行分帧（event 名/[DONE] 终态等帧格式是
        // 协议特定语义），v1 不引入这层分帧。协议不等时显式报错而非给出
        // 错协议的流（客户端拿到错格式 SSE 更难排障）
        if channel.format != client_format {
            return Err(format!(
                "SSE 流式响应的跨协议转换尚未支持（渠道协议 {} ≠ 客户端协议 {}）——非流式请求不受影响",
                channel.format, client_format
            ));
        }
        out.body = env.body.clone();
        out.body_b64 = env.body_b64.clone();
        return Ok(out);
    }
    let v: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("响应 body 非 JSON: {e}"))?;
    let translated = translate::translate_response(channel.format, client_format, &v)
        .map_err(|e| format!("协议转换失败: {e}"))?;
    // 模型名反向映射：上游名 → 客户端别名（[models] 表反向查询）
    let mut out_v = translated;
    if let Some(obj) = out_v.as_object_mut()
        && let Some(m) = obj.get("model").and_then(|m| m.as_str())
    {
        if let Some((client_name, _)) = state.cfg.models.iter().find(|(_, up)| up.as_str() == m) {
            obj.insert(
                "model".to_string(),
                serde_json::Value::String(client_name.clone()),
            );
        }
    }
    out.body = Some(out_v.to_string());
    Ok(out)
}
