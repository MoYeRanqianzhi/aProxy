//! 试跑只消费用户提供的请求与模拟上游尝试。不建网络客户端；输出从白名单字段另行构造。
mod peer;
use crate::{
    CliError,
    tools::{self, Layer},
};
use aproxy_envelope as w;
use aproxy_ipc::config_file::{self as f, Stage};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

/// 样例保留原始路径以及每轮上游的实测或手写结果；没有上游样本时只走请求方向。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    /// 原客户端路径（含查询）；不将它拼成可见诊断。
    path: String,
    /// 省略时是常见的 JSON POST。
    method: Option<String>,
    /// 请求头值只进入模拟管道。
    #[serde(default)]
    headers: w::Headers,
    /// 对象作为 JSON，字符串作为精确的原始请求正文。
    body: Value,
    /// 有限的上游尝试序列，即使策略无限重试也不会访问网络或无限运行。
    #[serde(default)]
    attempts: Vec<Attempt>,
    /// 显式要求在有意义的位置演练一次保活，不根据墙钟或睡眠生成事件。
    #[serde(default)]
    heartbeat: bool,
}
/// 一次上游结果；分块只影响传输边界，不改变缓冲阶段看到的实体。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    /// 无响应的失败用 failure 代替状态码。
    status: Option<u16>,
    /// 上游原始响应头。
    #[serde(default)]
    headers: w::Headers,
    /// 非流式样例的完整实体。
    body: Option<Value>,
    /// 流式样例按原有分块顺序交付。
    chunks: Option<Vec<String>>,
    /// 连接、读取等已知故障的协议诊断码。
    failure: Option<String>,
    /// 明确表示流在最后一块之后中断，应当发送 abort 而非 close。
    #[serde(default)]
    interrupted: bool,
}
/// 字符串不再加一层 JSON 引号，便于构造 SSE 或不完整实体。
fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .map_or_else(|| value.to_string().into_bytes(), |s| s.as_bytes().to_vec())
}
/// 样本与实例使用同一套 JSON/SSE 和行扫描判据；大实体的磁盘路径不先解压再误作完整 JSON。
fn sample_error_body(r: &w::Response, chunks: &[Vec<u8>], forward: bool, disk: bool) -> bool {
    use aproxy_ipc::inspection as inspect;
    let raw = r.body.as_bytes().unwrap_or_default();
    if raw.len() > inspect::RESIDENT_LIMIT && (forward || disk) {
        let mut scan = inspect::StreamErrorScanner::new();
        if forward {
            for chunk in chunks {
                for piece in chunk.chunks(16 * 1024) {
                    scan.feed(piece);
                }
            }
        } else {
            for chunk in chunks {
                scan.feed(chunk);
            }
        }
        return scan.finish();
    }
    let decoded = aproxy_ipc::decode::for_inspection(
        r.headers.get("content-encoding").map(String::as_str),
        raw,
    );
    let body = decoded.as_deref().unwrap_or(raw);
    if r.headers
        .get("content-type")
        .is_some_and(|ct| inspect::stream_content_type(ct))
        || inspect::sniffs_stream(body)
    {
        inspect::is_stream_error_body(body)
    } else {
        inspect::is_error_body(body)
    }
}
/// 只计数不留存，进程内角色不能因省略真实管道而越过实例的行长限制。
struct LineBudget(usize);
impl Write for LineBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or(std::io::ErrorKind::FileTooLarge)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// 一个处理位置的协商能力、序号与私有状态。程序之间不共享任何这些字段。
struct Unit {
    /// 握手公布的实例上限，同时约束管道和进程内回信，按实际阶段选择预算。
    limits: w::ReplyLimits,
    /// 在链中的角色与配置。
    layer: Layer,
    /// 外部显式授权后使用的进程管道。
    peer: Option<peer::ChildPeer>,
    /// 没运行的外部单元没有能力表，自然不会收到任何样本。
    hello: Option<w::HelloReply>,
    /// 每次握手归零，数据阶段从 1 递增。
    seq: u64,
    /// 只在发给同一个单元的上下文里交回原值。
    state: Option<String>,
    /// 每次重跑本层前撤销上次给出的覆盖。
    overrides: Option<w::Overrides>,
}
/// 输出只含固定角色、阶段与结构变化。头值、任意定制头名、主机/路径/查询、参数、状态和错误原文一律不进入投影。
fn view(url: &str, headers: &w::Headers) -> Value {
    let names: Vec<_> = headers
        .keys()
        .map(|k| match k.as_str() {
            "authorization" | "x-api-key" | "content-type" | "accept" | "cookie" | "set-cookie"
            | "content-length" | "content-encoding" | "anthropic-version" => k.as_str(),
            _ => "[custom]",
        })
        .collect();
    json!({"url":match url::Url::parse(url).ok().as_ref().map(url::Url::scheme){Some("http")=>"http://[redacted]",Some("https")=>"https://[redacted]",_=>"[redacted]"},"headers":names})
}
/// 一行是一个独立的结构投影，不能把协议回信拼接到这里。
fn trace(
    out: &mut impl Write,
    stage: &str,
    role: &str,
    input: Value,
    output: Value,
    changes: Vec<&'static str>,
) -> Result<(), String> {
    writeln!(
        out,
        "{}",
        json!({"stage":stage,"role":role,"input":input,"output":output,"changed":changes})
    )
    .map_err(|_| "无法写出试跑结果".into())
}
impl Unit {
    /// 能力声明须覆盖要求的每个阶段；不通过时不能发送样本正文。
    fn connect(&mut self, dir: &Path) -> Result<(), peer::PeerFault> {
        let e = &self.layer.entry;
        let stages = e.stages.iter().map(|s| s.as_str().to_owned()).collect();
        let mut offer = w::Hello::new(
            env!("CARGO_PKG_VERSION").into(),
            e.name.clone(),
            if e.process == f::Process::Spawn {
                w::Process::Spawn
            } else {
                w::Process::Persistent
            },
            stages,
            0,
        );
        offer.limits = Some(self.limits.clone());
        offer.features = vec!["decide.success".into()];
        offer.overrides = [
            "timeouts.connect_secs",
            "timeouts.first_byte_secs",
            "timeouts.read_idle_secs",
            "keepalive.interval_secs",
            "keepalive.trigger",
            "keepalive.heartbeat",
            "retry.max_backoff_secs",
            "retry.max_attempts",
        ]
        .map(str::to_owned)
        .into();
        let hello = if let Some(role) = &mut self.layer.role {
            role.hello(self.layer.label, &offer)
        } else {
            let p = peer::ChildPeer::start(e, dir).map_err(|_| peer::PeerFault::Unavailable)?;
            let mut h = serde_json::to_value(&offer).map_err(|_| peer::PeerFault::Invalid)?;
            h["v"] = 1.into();
            h["seq"] = 0.into();
            h["stage"] = "hello".into();
            let hr = serde_json::from_value(p.frame(&h, 64 * 1024, 10)?)
                .map_err(|_| peer::PeerFault::Invalid)?;
            self.peer = Some(p);
            hr
        };
        if hello.v != 1
            || hello.seq != 0
            || hello.protocol != 1
            || e.stages
                .iter()
                .any(|s| !hello.stages.iter().any(|h| h == s.as_str()))
        {
            return Err(peer::PeerFault::Invalid);
        }
        self.hello = Some(hello);
        self.seq = 1;
        Ok(())
    }
    /// 将已协商的体粒度、序号和本层状态附在消息上，失败时只返回固定诊断。
    fn call(
        &mut self,
        stage: &str,
        mut msg: Value,
        digest: &str,
        dir: &Path,
        now: (Instant, SystemTime),
    ) -> Result<w::Reply, String> {
        if self.seq > 1
            && self.layer.entry.process == f::Process::Spawn
            && (stage != "stream" || msg["event"] == "open")
        {
            self.peer = None;
            if self.layer.role.is_some() {
                self.layer.role = Some(
                    tools::load(
                        self.layer.label,
                        &self.layer.entry.args[1..],
                        dir,
                        crate::config::Variables {
                            overlay: self.layer.entry.env.clone(),
                            structural: false,
                        },
                    )
                    .map_err(|_| "角色不能重新启动")?,
                );
            }
            self.connect(dir)?;
        }
        let need = self.hello.as_ref().and_then(|h| h.request_body.get(stage));
        // 省略粒度时请求与重试为 full；观察阶段默认不接收正文。
        let full = need.map_or(matches!(stage, "request" | "retry"), |v| {
            *v == w::BodyNeed::Full
        });
        if matches!(stage, "request" | "retry" | "decide")
            && !full
            && let Some(r) = msg["request"].as_object_mut()
        {
            r.remove("body");
            r.remove("body_b64");
        }
        if matches!(need, Some(w::BodyNeed::Digest)) {
            msg["digest"] = digest.into();
        }
        if stage != "stream" || msg["event"] == "open" {
            msg["request_id"] = "sample".into();
            if let Some(state) = &self.state {
                msg["state"] = state.clone().into();
            }
        }
        msg["v"] = 1.into();
        msg["seq"] = self.seq.into();
        msg["stage"] = stage.into();
        let limit = match stage {
            "request" | "retry" => self.limits.request,
            "response" | "stream" => self.limits.response,
            _ => self.limits.control,
        }
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX);
        let mut reply = if let Some(role) = &mut self.layer.role {
            match role {
                crate::Role::Keys(k) => k.sample_time = Some(now),
                crate::Role::Policy(p) => p.sample_time = Some(now.1),
                crate::Role::Adaptive(a) => a.sample_time = Some(now.0),
                _ => {}
            }
            let m = serde_json::from_value(msg).map_err(|_| "样例不能构成该阶段的消息")?;
            let reply = role.handle(&m);
            // 不再分配一份序列化回信；逐段扣除 JSON 字节，最后一字节留给换行。
            let mut budget = LineBudget(limit.saturating_sub(1));
            serde_json::to_writer(&mut budget, &reply).map_err(|_| "试跑回信超过实例上限")?;
            reply
        } else {
            let timeout = match stage {
                "notify" => 2,
                "decide" => self.layer.entry.decide_timeout_secs,
                _ => self.layer.entry.timeout_secs,
            };
            let value = self
                .peer
                .as_ref()
                .ok_or("外部程序未连接")?
                .frame(&msg, limit, timeout)?;
            serde_json::from_value(value).map_err(|_| "外部程序回信无效")?
        };
        if reply.v != 1 || reply.seq != self.seq {
            return Err("角色回信序号不符".into());
        }
        if reply
            .request
            .as_ref()
            .and_then(|p| p.method.as_deref())
            .is_some_and(|m| http::Method::from_bytes(m.as_bytes()).is_err())
            || reply
                .request
                .as_ref()
                .and_then(|p| p.url.as_deref())
                .is_some_and(|u| {
                    !url::Url::parse(u).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https") && url.host().is_some()
                    })
                })
            || reply
                .response
                .as_ref()
                .and_then(|p| p.status)
                .into_iter()
                .chain(
                    reply
                        .fail
                        .as_ref()
                        .and_then(|f| f.response.as_ref())
                        .map(|r| r.status),
                )
                .any(|s| !(100..=599).contains(&s))
            || reply.state.as_ref().is_some_and(|s| s.len() > 64 * 1024)
        {
            return Err("角色回信的 HTTP 元数据或状态无效".into());
        }
        // 只检查本阶段真正生效的整张替换表；非法单头丢弃，表容量不足则视为进程回信故障。
        let headers = if let Some(f) = &mut reply.fail {
            f.response.as_mut().map(|r| &mut r.headers)
        } else if matches!(stage, "request" | "retry") {
            reply.request.as_mut().and_then(|r| r.headers.as_mut())
        } else {
            reply.response.as_mut().and_then(|r| r.headers.as_mut())
        };
        if let Some(table) = headers {
            let mut parsed = http::HeaderMap::new();
            for (k, v) in std::mem::take(table) {
                if aproxy_ipc::redact::is_managed_request_header(&k) {
                    continue;
                }
                if let Ok(k) = http::HeaderName::from_bytes(k.as_bytes())
                    && let Ok(v) = http::HeaderValue::from_str(&v)
                    && v.to_str().is_ok()
                {
                    parsed.try_insert(k, v).map_err(|_| "回信头表超过限制")?;
                }
            }
            parsed.try_reserve(8).map_err(|_| "回信头表超过限制")?;
            table.extend(parsed.iter().map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().expect("已检查可打印的头值").to_owned(),
                )
            }));
        }
        self.seq += 1;
        if !matches!(stage, "heartbeat" | "notify")
            && let Some(state) = &reply.state
        {
            self.state = (!state.is_empty()).then(|| state.clone());
        }
        Ok(reply)
    }
    /// 被跳过的第三方、协商失败的可选层与没有声明本阶段的角色不获得数据。
    fn has(&self, stage: Stage) -> bool {
        self.hello.is_some() && self.layer.entry.has_stage(stage)
    }
}
/// 最先有效的决定、重选和等待，各自保持实例的优先级，供下一次尝试与终态使用。
struct Settlement {
    /// 是否再次尝试。
    retry: bool,
    /// 已跑完请求链时是否重新选路。
    reroute: bool,
    /// stop 票可随附完整的替代响应补丁。
    stop: Option<w::ResponsePatch>,
    /// 胜出 retry 票指定的等待，否则采用实例给出的默认退避。
    delay_ms: u64,
    /// 是程序投了决定票，还是只有实例保底决定。
    decided: bool,
}
/// 共享的只读上下文与有限试跑状态；没有网络或实际时间退避。
struct Trial<'a, W> {
    /// 投影写入端；测试可以直接审查所有输出。
    out: &'a mut W,
    /// 声明顺序，响应与心跳倒序遍历。
    units: Vec<Unit>,
    /// 链本身的目录，用于显式外部程序及重新加载 spawn 角色。
    dir: &'a Path,
    /// 原始请求的身份，转发补丁不改变它。
    digest: String,
    /// 原始客户端的路径与流意图。
    client: Value,
    /// 请求模式投票与后续升级的当前结果。
    mode: &'static str,
    /// 在第一个 retry 单元之前缓存的请求，重跑不能重用上次渠道的输出。
    prefix: Option<(usize, w::Request)>,
    /// 未完成的层仍使用原阶段；只有到达末尾后才从第一个 retry 订阅者重新开始。
    cursor: usize,
    /// 首轮发生链内拒绝后恢复执行，不能切换到 retry 消息。
    request_stage: Stage,
    /// 每个逻辑请求只接受一次 delivery 选择，恢复游标不重新开票。
    voted: bool,
    /// 只累计实际选中的等待值，不用外部进程调度时间模拟网络耗时。
    elapsed_ms: u64,
    /// 同步推进单调时间和 HTTP 日期时间；所有进程内角色观察同一个时刻。
    now: (Instant, SystemTime),
}
impl<W: Write> Trial<'_, W> {
    /// 程序业务拒绝与进程失败分开；发生进程故障的可选层不会继续吃样本。
    fn call(&mut self, i: usize, stage: &str, msg: Value) -> Result<w::Reply, CliError> {
        self.units[i]
            .call(stage, msg, &self.digest, self.dir, self.now)
            .map_err(|_| {
                let behavior = if matches!(stage, "notify" | "decide" | "heartbeat") {
                    "记录故障，按此控制阶段的回退规则继续"
                } else if self.units[i].layer.entry.required {
                    "记录必需程序故障，按交付阶段重试或结束请求"
                } else {
                    "记录可选程序故障并按该阶段的旁路规则继续"
                };
                CliError::Unsimulated(format!(
                    "{stage} 阶段的工作进程故障、替换与熔断；真实实例会{behavior}"
                ))
            })
    }
    /// 请求补丁按次序串联，投递方式只取第一票，重跑只更新参与的后段。
    fn request(
        &mut self,
        req: &mut w::Request,
        retry: Option<Value>,
    ) -> Result<Option<w::Fail>, CliError> {
        if self.cursor == self.units.len() {
            let Some((i, prefix)) = &self.prefix else {
                return Ok(None);
            };
            *req = prefix.clone();
            self.cursor = *i;
            self.request_stage = Stage::Retry;
            // 正常结束后才回滚整个后缀；尚未完成的请求链只撤掉失败层的覆盖。
            for u in &mut self.units[*i..] {
                u.overrides = None;
            }
        } else if retry.is_some() {
            self.units[self.cursor].overrides = None;
        }
        let stage = self.request_stage;
        while self.cursor < self.units.len() {
            let i = self.cursor;
            if self.prefix.is_none() && self.units[i].layer.entry.has_stage(Stage::Retry) {
                self.prefix = Some((i, req.clone()));
            }
            if !self.units[i].has(stage) {
                self.cursor += 1;
                continue;
            }
            let mut msg = json!({"delivery":self.mode,"client":self.client,"request":req});
            if stage == Stage::Retry
                && let Some(r) = &retry
            {
                msg["retry"] = r.clone();
            }
            let before = req.clone();
            let mut r = self.call(i, stage.as_str(), msg)?;
            let mut changed = Vec::new();
            if r.fail.is_none()
                && let Some(p) = r.request.take()
            {
                req.apply(p);
            }
            if req.url != before.url {
                changed.push("url");
            }
            if req.headers != before.headers {
                changed.push("headers");
            }
            if req.body != before.body {
                changed.push("body");
            }
            if req.method != before.method {
                changed.push("method");
            }
            if !self.voted
                && stage == Stage::Request
                && let Some(mode) = match r.delivery.as_ref().and_then(Value::as_str) {
                    Some("forward") => Some("forward"),
                    Some("retry") => Some("retry"),
                    _ => None,
                }
            {
                self.mode = mode;
                self.voted = true;
                changed.push(mode);
            }
            if r.overrides.is_some() {
                changed.push("overrides");
            }
            self.units[i].overrides = r.overrides;
            if r.fail.is_some() {
                changed.push("rejected");
            }
            trace(
                self.out,
                stage.as_str(),
                self.units[i].layer.label,
                view(&before.url, &before.headers),
                view(&req.url, &req.headers),
                changed,
            )?;
            if r.fail.is_some() {
                return Ok(r.fail);
            }
            self.cursor += 1;
        }
        Ok(None)
    }
    /// 只交付已经协商的响应预览，头、尾和省略字节数与代理的判定视图一致。
    fn decide(
        &mut self,
        req: &w::Request,
        response: Option<&w::Response>,
        failure: Option<&Value>,
        attempt: u32,
        fallback: w::Fallback,
    ) -> Result<Settlement, CliError> {
        let mut delay_ms = fallback.delay_ms.unwrap_or(0);
        let mut decision = None;
        let mut reroute = None;
        let mut stop = None;
        for i in 0..self.units.len() {
            if !self.units[i].has(Stage::Decide)
                || failure.is_none()
                    && !self.units[i]
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.features.iter().any(|f| f == "decide.success"))
            {
                continue;
            }
            let mut msg = json!({"client":self.client,"attempt":attempt,"elapsed_ms":self.elapsed_ms,"delivery":self.mode,"outcome":if failure.is_some(){"failure"}else{"success"},"request":{"method":req.method,"url":req.url},"fallback":fallback});
            if let Some(b) = req.body.as_bytes() {
                let body = serde_json::to_value(w::Body::from_bytes(b.to_vec()))
                    .map_err(|_| "样例正文无法编码")?;
                msg["request"]
                    .as_object_mut()
                    .unwrap()
                    .extend(body.as_object().unwrap().clone());
            }
            if let Some(f) = failure {
                msg["failure"] = f.clone();
            }
            if let Some(resp) = response {
                let mut obs = resp.clone();
                if let Some(data) = resp.body.as_bytes() {
                    let encoding = resp
                        .headers
                        .get("content-encoding")
                        .map_or("", String::as_str);
                    let (preview, missing, decoded) = aproxy_ipc::decode::read_preview(
                        std::io::Cursor::new(data),
                        data.len() as u64,
                        Some(encoding),
                        |n| n > 8 * 1024 * 1024,
                    )
                    .ok_or_else(|| {
                        CliError::Unsimulated(
                            "超过 8 MiB 的解码预览；真实实例会流式扫描完整实体".into(),
                        )
                    })?;
                    obs.body = if decoded {
                        obs.headers.remove("content-encoding");
                        w::Body::from_bytes(preview)
                    } else {
                        w::Body::Binary(preview)
                    };
                    msg["response"] = json!(obs);
                    msg["response"]["truncated"] = if missing == 0 {
                        Value::Null
                    } else {
                        missing.into()
                    };
                } else {
                    // 只收到头时不猜测正文或截断量；编码和长度仍属于上游原响应。
                    msg["response"] = json!(obs);
                }
            }
            let r = self.call(i, "decide", msg)?;
            let mut changes = Vec::new();
            if r.fail.is_some() {
                trace(
                    self.out,
                    "decide",
                    self.units[i].layer.label,
                    view(&req.url, &req.headers),
                    Value::Null,
                    changes,
                )?;
                continue;
            }
            if let Some(d) = r.decision {
                changes.push(if d == w::Decision::Retry {
                    "retry"
                } else {
                    "stop"
                });
                if decision.is_none() {
                    decision = Some(d == w::Decision::Retry);
                    if let Some(delay) = r.delay_ms {
                        delay_ms = delay.millis();
                    }
                    stop = r.response;
                }
            }
            if let Some(v) = r.reroute {
                reroute.get_or_insert(v);
                changes.push("reroute");
            }
            if r.delay_ms.is_some() {
                changes.push("delay");
            }
            trace(
                self.out,
                "decide",
                self.units[i].layer.label,
                view(&req.url, &req.headers),
                Value::Null,
                changes,
            )?;
            if decision == Some(false)
                || (decision.is_some()
                    && (reroute.is_some()
                        || !self
                            .units
                            .iter()
                            .any(|u| u.layer.entry.has_stage(Stage::Retry))))
            {
                break;
            }
        }
        Ok(Settlement {
            retry: decision.unwrap_or(fallback.decision == w::Decision::Retry),
            reroute: reroute.unwrap_or(true),
            stop,
            delay_ms,
            decided: decision.is_some(),
        })
    }
    /// 心跳走反向链；输出只说明数据是否改变，不回显事件内容。
    fn heartbeat(&mut self, n: u32, initial: &str) -> Result<(), CliError> {
        let mut data = initial.as_bytes().to_vec();
        for i in (0..self.units.len()).rev() {
            if self.units[i].has(Stage::Heartbeat) {
                let mut m = json!({"client":self.client,"heartbeat":{"beat":n,"elapsed_ms":self.elapsed_ms,"attempt":n}});
                m.as_object_mut().unwrap().extend(
                    json!(w::Data::from_bytes(data.clone()))
                        .as_object()
                        .unwrap()
                        .clone(),
                );
                let r = self.call(i, "heartbeat", m)?;
                let mut changes = Vec::new();
                if let Some(b) = r.data.as_bytes() {
                    if b != data {
                        changes.push("data");
                    }
                    data = b.to_vec();
                }
                trace(
                    self.out,
                    "heartbeat",
                    self.units[i].layer.label,
                    Value::Null,
                    Value::Null,
                    changes,
                )?;
            }
        }
        Ok(())
    }
    /// 各层从最终请求及自己的 state 开始，缓冲响应与逐块流按洋葱顺序返回。
    fn deliver(
        &mut self,
        req: &w::Request,
        resp: &mut w::Response,
        mut chunks: Vec<Vec<u8>>,
        cut: bool,
        attempt: u32,
    ) -> Result<(), CliError> {
        let nested = self.units.iter().filter(|u| u.has(Stage::Stream)).count() > 1;
        let nested_end = || {
            CliError::Unsimulated(
                "多层流的中断或失败；真实实例会按已打开的流租约传递中止、回退或终止交付".into(),
            )
        };
        if cut && nested {
            return Err(nested_end());
        }
        let stream_id = format!("sample/{attempt}");
        let meta = json!({"method":req.method,"url":req.url});
        for i in (0..self.units.len()).rev() {
            let label = self.units[i].layer.label;
            if self.units[i].has(Stage::Response) {
                resp.body = w::Body::from_bytes(chunks.concat());
                let before = view(&req.url, &resp.headers);
                let r = self.call(
                    i,
                    "response",
                    json!({"client":self.client,"request":meta,"response":resp}),
                )?;
                if let Some(f) = r.fail {
                    let Some(replacement) = f.response else {
                        trace(
                            self.out,
                            "response",
                            label,
                            before,
                            view(&req.url, &resp.headers),
                            vec!["skipped"],
                        )?;
                        continue;
                    };
                    *resp = replacement;
                } else if let Some(p) = r.response {
                    resp.apply(p);
                }
                chunks = vec![resp.body.as_bytes().unwrap_or_default().to_vec()];
                trace(
                    self.out,
                    "response",
                    label,
                    before,
                    view(&req.url, &resp.headers),
                    vec!["response"],
                )?;
            } else if self.units[i].has(Stage::Stream)
                && req.method != "HEAD"
                && !matches!(resp.status, 100..=199 | 204 | 304)
            {
                let before = view(&req.url, &resp.headers);
                let open=self.call(i,"stream",json!({"event":"open","stream_id":stream_id,"client":self.client,"request":meta,"response":{"status":resp.status,"headers":resp.headers}}))?;
                if let Some(f) = open.fail {
                    let changed = if let Some(replacement) = f.response {
                        *resp = replacement;
                        chunks = vec![resp.body.as_bytes().unwrap_or_default().to_vec()];
                        "response"
                    } else {
                        "skipped"
                    };
                    trace(
                        self.out,
                        "stream:open",
                        label,
                        before,
                        view(&req.url, &resp.headers),
                        vec![changed],
                    )?;
                    continue;
                }
                if let Some(p) = open.response {
                    resp.apply(p);
                }
                trace(
                    self.out,
                    "stream:open",
                    label,
                    before,
                    view(&req.url, &resp.headers),
                    vec!["head"],
                )?;
                let mut result = Vec::new();
                if matches!(resp.status, 100..=199 | 204 | 304) {
                    chunks.clear();
                } else if let Some(prefix) = open.data.as_bytes().filter(|b| !b.is_empty()) {
                    result.push(prefix.to_vec());
                }
                for chunk in chunks {
                    let mut m = json!({"stream_id":stream_id,"event":"data"});
                    m.as_object_mut().unwrap().extend(
                        json!(w::Data::from_bytes(chunk.clone()))
                            .as_object()
                            .unwrap()
                            .clone(),
                    );
                    let r = self.call(i, "stream", m)?;
                    if r.fail.is_some() {
                        if nested {
                            return Err(nested_end());
                        }
                        return Err("流角色拒绝样例数据".into());
                    }
                    let b = r.data.as_bytes().unwrap_or_default();
                    trace(
                        self.out,
                        "stream:data",
                        label,
                        Value::Null,
                        Value::Null,
                        if b == chunk { vec![] } else { vec!["data"] },
                    )?;
                    if !b.is_empty() {
                        result.push(b.to_vec());
                    }
                }
                let end = if cut {
                    json!({"stream_id":stream_id,"event":"abort","reason":{"code":"upstream_body_interrupted"}})
                } else {
                    json!({"stream_id":stream_id,"event":"close"})
                };
                let r = self.call(i, "stream", end)?;
                if nested && r.fail.is_some() {
                    return Err(nested_end());
                }
                if !matches!(resp.status, 100..=199 | 204 | 304)
                    && let Some(b) = r.data.as_bytes().filter(|b| !b.is_empty())
                {
                    result.push(b.to_vec());
                }
                trace(
                    self.out,
                    if cut { "stream:abort" } else { "stream:close" },
                    label,
                    Value::Null,
                    Value::Null,
                    if r.data.as_bytes().is_some() {
                        vec!["data"]
                    } else {
                        vec![]
                    },
                )?;
                chunks = result;
            }
        }
        resp.body = w::Body::from_bytes(chunks.concat());
        Ok(())
    }
    /// 业务拒绝在接触上游之前终结尝试；自定义响应覆盖默认状态，但诊断仍归属于格式拒绝。
    fn rejected(&mut self, fail: &w::Fail, attempt: u32, upgraded: bool) -> Result<(), CliError> {
        trace(
            self.out,
            "result",
            "proxy",
            Value::Null,
            json!({"status":fail.response.as_ref().map_or(422,|r|r.status),"code":"format_rejected"}),
            vec!["rejected"],
        )?;
        self.notify(attempt, None, upgraded, None, "failed")
    }
    /// 完成只发给订阅者；通知不是可以再改变交付结果的一票。
    fn notify(
        &mut self,
        n: u32,
        status: Option<u16>,
        upgraded: bool,
        completed: Option<bool>,
        outcome: &str,
    ) -> Result<(), CliError> {
        for i in 0..self.units.len() {
            if self.units[i].has(Stage::Notify) {
                self.call(i,"notify",json!({"event":"request_end","client":self.client,"delivery":self.mode,"upgraded":upgraded,"attempts":n,"duration_ms":self.elapsed_ms,"upstream_status":status,"stream_completed":completed,"outcome":outcome,"code":null}))?;
                trace(
                    self.out,
                    "notify",
                    self.units[i].layer.label,
                    Value::Null,
                    Value::Null,
                    vec!["request_end"],
                )?;
            }
        }
        Ok(())
    }
}
/// 还原首个匹配的重试规则，格式角色按请求给的 cap 优先。forward 在没有明确的升级票时不重试。
fn fallback(
    config: &f::FileConfig,
    units: &[Unit],
    path: &str,
    n: u32,
    status: Option<u16>,
    failed: bool,
    mode: &str,
) -> w::Fallback {
    let effective = |leaf: &str| {
        units
            .iter()
            .rev()
            .find_map(|u| u.overrides.as_ref()?.get("retry")?.get(leaf)?.as_u64())
    };
    let (decision, by) = if !failed || mode == "forward" {
        (w::Decision::Stop, "default".into())
    } else {
        let matched = effective("max_attempts")
            .map(|cap| (cap, "override".to_owned()))
            .or_else(|| {
                let status = status?;
                config
                    .retry
                    .rules
                    .as_ref()?
                    .iter()
                    .enumerate()
                    .find(|(_, r)| {
                        r.status
                            .as_ref()
                            .is_none_or(|codes| codes.contains(&status))
                            && r.path.as_ref().is_none_or(|p| {
                                f::compile_retry_pattern(p).is_ok_and(|r| r.is_match(path))
                            })
                    })
                    .map(|(i, r)| (u64::from(r.max_attempts), format!("rule:{}", i + 1)))
            });
        let (cap, by) = matched.unwrap_or_else(|| (0, "default".into()));
        (
            if cap == 0 || u64::from(n) < cap {
                w::Decision::Retry
            } else {
                w::Decision::Stop
            },
            by,
        )
    };
    let cap = effective("max_backoff_secs")
        .or(config.retry.max_backoff_secs)
        .unwrap_or(f::DEFAULT_BACKOFF_SECS);
    let mut answer = w::Fallback::new(decision, by);
    answer.delay_ms = Some(
        f::delay_for_attempt(n, cap)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    );
    answer
}
/// 协议体是有限样例；限制样例文件本身避免把无限输入作为一次 dry run。
pub(super) fn run(
    path: &Path,
    sample: &Path,
    home: Option<&Path>,
    external: bool,
) -> Result<(), CliError> {
    if external {
        eprintln!(
            "注意：试跑工具自身不打开网络连接。外部程序会收到样例和环境变量，能任意使用它们，包括访问网络；只运行你信任的程序。"
        );
    }
    let input = tools::offline_read(sample)?;
    let sample: Sample =
        serde_json::from_slice(&input).map_err(|_| "试跑样例不是有效的请求描述")?;
    rehearse(path, sample, home, external, &mut std::io::stdout().lock())
}
/// 模拟有限次数的尝试；耗尽样例不是一次成功交付，明确报告仍需更多上游样本。
fn rehearse(
    path: &Path,
    sample: Sample,
    home: Option<&Path>,
    external: bool,
    out: &mut impl Write,
) -> Result<(), CliError> {
    let chain = tools::chain(path, home, false)?;
    let base = chain
        .config
        .base_url
        .as_deref()
        .ok_or("试跑需要 base_url")?;
    if !sample.path.starts_with('/') || sample.path.contains(['#', '\r', '\n']) {
        return Err("试跑请求路径无效".into());
    }
    let (path_part, query) = sample
        .path
        .split_once('?')
        .map_or((sample.path.as_str(), None), |(p, q)| (p, Some(q)));
    let mut url = tools::base_url(base)?;
    url.set_path(&format!("{}{path_part}", url.path().trim_end_matches('/')));
    url.set_query(query);
    let raw = bytes(&sample.body);
    let max_request = chain.config.max_request_mb.unwrap_or(128);
    if max_request != 0 && raw.len() as u64 > max_request.saturating_mul(1024 * 1024) {
        return Err("样例请求正文超过实例的 max_request_mb（413）".into());
    }
    let stream_body = serde_json::from_slice::<Value>(&raw).is_ok_and(|v| v["stream"] == true);
    let mut req = w::Request::new(sample.method.unwrap_or_else(|| "POST".into()), url.into());
    http::Method::from_bytes(req.method.as_bytes()).map_err(|_| "样例的 HTTP 方法无效")?;
    // 按真实出站优先级叠放：缺省补充、客户端、api_key 快捷设置、显式覆盖。代理管理头不进角色输入。
    let mut credentials = w::Headers::new();
    if let Some(key) = &chain.config.api_key {
        for (name, value) in [
            ("authorization", format!("Bearer {key}")),
            ("x-api-key", key.clone()),
        ] {
            if http::HeaderValue::from_str(&value).is_ok() {
                credentials.insert(name.into(), value);
            }
        }
    }
    for (k, v) in chain
        .config
        .extra_headers
        .iter()
        .flatten()
        .chain(&sample.headers)
        .chain(&credentials)
        .chain(chain.config.override_headers.iter().flatten())
    {
        let key = http::HeaderName::from_bytes(k.as_bytes()).map_err(|_| "样例头名称无效")?;
        http::HeaderValue::from_str(v).map_err(|_| "样例头值无效")?;
        if !aproxy_ipc::redact::is_managed_request_header(key.as_str()) {
            req.headers.insert(key.as_str().into(), v.clone());
        }
    }
    let accept = sample
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("accept"))
        .is_some_and(|(_, v)| {
            v.split(',').any(|s| {
                s.split(';')
                    .next()
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("text/event-stream"))
            })
        });
    let streaming = match chain
        .config
        .keepalive
        .trigger
        .unwrap_or(f::KeepaliveTrigger::Any)
    {
        f::KeepaliveTrigger::Accept => accept,
        f::KeepaliveTrigger::BodyStream => stream_body,
        f::KeepaliveTrigger::Any => stream_body || accept,
    };
    let mut t = Trial {
        out,
        units: Vec::new(),
        dir: &chain.directory,
        digest: Sha256::digest(&raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        client: json!({"path":sample.path,"stream":streaming}),
        mode: chain.config.delivery.unwrap_or(f::Delivery::Retry).as_str(),
        prefix: None,
        cursor: 0,
        request_stage: Stage::Request,
        voted: false,
        elapsed_ms: 0,
        now: (Instant::now(), SystemTime::now()),
    };
    req.body = w::Body::from_bytes(raw);
    let group = |mb: u64| {
        let bytes = mb
            .saturating_mul(2 * 1024 * 1024)
            .saturating_add(1024 * 1024);
        (mb != 0 && bytes < usize::MAX as u64).then_some(bytes)
    };
    let limits = w::ReplyLimits::new(
        Some(64 * 1024),
        group(max_request),
        group(chain.config.max_response_mb.unwrap_or(256)),
        Some(1024 * 1024),
    );
    for layer in chain.layers {
        if external
            && layer.label == "external"
            && home.is_none()
            && !Path::new(&layer.entry.command).is_absolute()
        {
            return Err("运行外部相对命令需要 --home；未启动程序".into());
        }
        let mut u = Unit {
            limits: limits.clone(),
            layer,
            peer: None,
            hello: None,
            seq: 0,
            state: None,
            overrides: None,
        };
        if u.layer.role.is_none() && (!external || u.layer.label != "external") {
            trace(
                t.out,
                "hello",
                u.layer.label,
                Value::Null,
                Value::Null,
                vec!["skipped"],
            )?;
        } else {
            match u.connect(t.dir) {
                Ok(()) => trace(
                    t.out,
                    "hello",
                    u.layer.label,
                    Value::Null,
                    Value::Null,
                    vec!["negotiated"],
                )?,
                Err(peer::PeerFault::Invalid) => {
                    return Err("试跑握手不兼容，请检查协议和阶段".into());
                }
                Err(peer::PeerFault::Unavailable) if !u.layer.entry.required => trace(
                    t.out,
                    "hello",
                    u.layer.label,
                    Value::Null,
                    Value::Null,
                    vec!["skipped"],
                )?,
                Err(_) => return Err("试跑握手失败".into()),
            }
        }
        t.units.push(u);
    }
    let mut refused = t.request(&mut req, None)?;
    let mut upgraded = false;
    if let Some(fail) = refused.as_ref().filter(|f| !f.retry) {
        return t.rejected(fail, 1, false);
    }
    if sample.attempts.is_empty() {
        trace(
            t.out,
            "sample",
            "proxy",
            Value::Null,
            view(&req.url, &req.headers),
            vec![if refused.is_some() {
                "rejected"
            } else {
                "request_only"
            }],
        )?;
        return Ok(());
    }
    let mut committed_beat: Option<String> = None;
    for (index, attempt) in sample.attempts.into_iter().enumerate() {
        let n = index as u32 + 1;
        if sample.heartbeat && t.mode == "retry" {
            // 首次提交后实例不再接受改变保活通道的覆盖，下一次尝试继续使用已提交的帧。
            if committed_beat.is_none() {
                let values = |leaf: &'static str| {
                    t.units
                        .iter()
                        .rev()
                        .filter_map(move |u| u.overrides.as_ref()?.get("keepalive")?.get(leaf))
                };
                let interval = values("interval_secs")
                    .find_map(Value::as_u64)
                    .or(chain.config.keepalive.interval_secs)
                    .unwrap_or(15);
                let eligible = values("trigger")
                    .find_map(|v| match v.as_str()? {
                        "always" => Some(true),
                        "accept" => Some(accept),
                        "body_stream" => Some(stream_body),
                        "any" => Some(accept || stream_body),
                        _ => None,
                    })
                    .unwrap_or(streaming);
                if interval != 0 && eligible {
                    committed_beat = Some(
                        values("heartbeat")
                            .find_map(Value::as_str)
                            .or(chain.config.keepalive.heartbeat.as_deref())
                            .unwrap_or(": keepalive\n\n")
                            .into(),
                    );
                }
            }
            if let Some(text) = &committed_beat {
                t.heartbeat(n, text)?;
            }
        }
        if attempt.status.is_some_and(|s| !(100..=599).contains(&s))
            || (attempt.status.is_none() && attempt.failure.is_none())
            || attempt.body.is_some() && attempt.chunks.is_some()
        {
            return Err("模拟尝试的状态、失败或实体无效".into());
        }
        let mut chunks: Vec<Vec<u8>> = attempt.chunks.map_or_else(
            || attempt.body.as_ref().map(bytes).into_iter().collect(),
            |c| c.into_iter().map(String::into_bytes).collect(),
        );
        let mut resp = attempt.status.map(|status| {
            let mut r = w::Response::new(status);
            r.headers = attempt.headers;
            if !attempt.interrupted && attempt.failure.is_none() {
                r.body = w::Body::from_bytes(chunks.concat());
            }
            r
        });
        let forward = t.mode == "forward";
        let has_decide = t.units.iter().any(|u| u.has(Stage::Decide));
        let has_response = t.units.iter().any(|u| u.has(Stage::Response));
        let success = attempt.status.is_some_and(|s| (200..300).contains(&s));
        let partial = attempt.interrupted || attempt.failure.is_some();
        let mut live = forward
            && attempt.status.is_some()
            && (!attempt.status.is_some_and(|s| s >= 400) || !has_decide);
        if live && partial && success && has_response && !has_decide {
            return Err(CliError::Unsimulated(
                "整包层读取未完成的直通实体；真实实例会按实际流故障结束交付".into(),
            ));
        }
        if live && partial && success && has_response && has_decide {
            // 纯整包层要等到 EOF 才能交付头；混合链中已经打开的内层还持有租约。
            if t.units.iter().any(|u| u.has(Stage::Stream)) {
                return Err(CliError::Unsimulated(
                    "混合回程链在提交前中断；真实实例会先释放已打开的流，再交回决定层".into(),
                ));
            }
            live = false;
        }
        let mut failure = attempt.failure.map(|code| json!({"code":code})).or_else(|| {
            if attempt.interrupted {
                let received = chunks.iter().map(Vec::len).sum::<usize>();
                Some(json!({"code":"upstream_body_interrupted","details":{"received_bytes":received,"reason":"sample interrupted"}}))
            } else {
                let r = resp.as_ref()?;
                let code = if r.status >= 400 {"upstream_status"} else if (!forward || success) && sample_error_body(r, &chunks, forward, chain.config.disk_cache.unwrap_or(true)) {"upstream_error_body"} else {return None};
                Some(json!({"code":code,"details":{"status":r.status}}))
            }
        });
        if let Some(rejection) = refused.take() {
            if !rejection.retry {
                return t.rejected(&rejection, n, upgraded);
            }
            failure = Some(
                json!({"code":"format_fail","details":{"format":t.units[t.cursor].layer.entry.name,"stage":t.request_stage.as_str()}}),
            );
            resp = None;
            live = false;
            chunks.clear();
        }
        let base_retry = fallback(
            &chain.config,
            &t.units,
            &sample.path,
            n,
            resp.as_ref().map(|r| r.status),
            failure.is_some(),
            t.mode,
        );
        let settled = if live {
            Settlement {
                retry: false,
                reroute: false,
                stop: None,
                delay_ms: 0,
                decided: false,
            }
        } else {
            t.decide(&req, resp.as_ref(), failure.as_ref(), n, base_retry)?
        };
        if settled.retry {
            let delta = Duration::from_millis(settled.delay_ms);
            let overflow = || {
                CliError::Unsimulated("逻辑时钟超出可表示范围；真实实例会按配置的退避等待".into())
            };
            t.now = (
                t.now.0.checked_add(delta).ok_or_else(overflow)?,
                t.now.1.checked_add(delta).ok_or_else(overflow)?,
            );
            t.elapsed_ms = t.elapsed_ms.saturating_add(settled.delay_ms);
            if t.mode == "forward" {
                upgraded = true;
                t.mode = "retry";
            }
            let mut info = json!({"attempt":n+1,"last_failure":failure.unwrap_or_else(||json!({"code":"format_decision"})),"elapsed_ms":t.elapsed_ms});
            if let Some(r) = resp {
                info["last_response"] = json!({"status":r.status,"headers":r.headers});
            }
            if settled.reroute || t.cursor != t.units.len() {
                refused = t.request(&mut req, Some(info))?;
                if let Some(fail) = refused.as_ref().filter(|f| !f.retry) {
                    return t.rejected(fail, n + 1, upgraded);
                }
            }
            continue;
        }
        let mut response = resp
            .filter(|_| live || !partial)
            .unwrap_or_else(|| w::Response::new(502));
        if let Some(p) = settled.stop {
            response.apply(p);
        } else if (failure.is_none() || live) && (!forward || success) {
            t.deliver(&req, &mut response, chunks, live && partial, n)?;
        }
        trace(
            t.out,
            "result",
            "proxy",
            Value::Null,
            json!({"status":response.status,"decided_by":if settled.decided {"format"} else {"fallback"}}),
            vec![],
        )?;
        t.notify(
            n,
            attempt.status,
            upgraded,
            streaming.then_some(!attempt.interrupted),
            if failure.is_some() || attempt.interrupted {
                "upstream_error"
            } else {
                "ok"
            },
        )?;
        return Ok(());
    }
    trace(
        t.out,
        "sample",
        "proxy",
        Value::Null,
        Value::Null,
        vec!["more_attempts_needed"],
    )
    .map_err(CliError::from)
}

#[cfg(test)]
/// 决策的静态兜底与生产配置共用退避计算；即使 stop 也为可能覆盖它的票提供原等待值。
mod tests {
    use super::*;
    /// 在独立目录重放样例，检查的就是面向操作者的逐行投影，不需要真的访问上游。
    fn trace_sample(
        proxy: &str,
        sample: Value,
        files: &[(&str, &str)],
    ) -> (Result<(), CliError>, Vec<Value>) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("proxy.toml");
        std::fs::write(&p, format!("base_url='https://relay.test'\n{proxy}")).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        let mut out = Vec::new();
        let result = rehearse(
            &p,
            serde_json::from_value(sample).unwrap(),
            Some(dir.path()),
            false,
            &mut out,
        );
        let rows = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (result, rows)
    }
    /// 第一层只能看到真正发往上游的头，鉴权快捷配置注入两种鉴权头，但投影不包含值。
    #[test]
    fn first_layer_observes_outbound_header_names_not_values() {
        let (result, rows) = trace_sample(
            "api_key='secret-fixture'\n[[format]]\ncommand='aproxy-format'\nargs=['rewrite']\nstages=['request']",
            json!({"path":"/","body":{},"headers":{"Content-Length":"2","Host":"private-host"}}),
            &[],
        );
        result.unwrap();
        let request = rows.iter().find(|r| r["stage"] == "request").unwrap();
        assert_eq!(
            request["input"]["headers"],
            json!(["authorization", "x-api-key"])
        );
        assert!(!json!(rows).to_string().contains("secret-fixture"));
    }
    /// 直通成功不征询要求观察成功的策略；重定向也不能误入回程转换。
    #[test]
    fn forward_success_never_votes_and_redirects_bypass_return_layers() {
        let p = "delivery='forward'\n[[format]]\nname='policy'\ncommand='aproxy-format'\nargs=['policy','--config','policy.toml']\nstages=['decide']\n[[format]]\nname='convert'\ncommand='aproxy-format'\nargs=['convert','--from','openai_chat','--to','openai_chat']\nstages=['response']";
        for status in [200, 302] {
            let (result, rows) = trace_sample(
                p,
                json!({"path":"/","body":{},"attempts":[{"status":status,"body":{}}]}),
                &[(
                    "policy.toml",
                    "[[rule]]\nstatus=[200,302]\ndecision='retry'",
                )],
            );
            result.unwrap();
            assert!(!rows.iter().any(|r| r["stage"] == "decide"));
            assert_eq!(rows.iter().any(|r| r["stage"] == "response"), status == 200);
            assert_eq!(rows.last().unwrap()["output"]["status"], status);
        }
    }

    /// 第一张终局票已确定结果后，后续策略连消息也不应收到。
    #[test]
    fn two_policies_stop_dispatch_at_the_settled_vote() {
        let mut p = String::new();
        for name in ["first", "second"] {
            p += &format!(
                "[[format]]\nname='{name}'\ncommand='aproxy-format'\nargs=['policy','--config','p']\nstages=['decide']\n"
            );
        }
        let (result, rows) = trace_sample(
            &p,
            json!({"path":"/","body":{},"attempts":[{"status":503}]}),
            &[("p", "[[rule]]\nstatus=[503]\ndecision='stop'")],
        );
        result.unwrap();
        let votes = rows
            .iter()
            .filter(|r| r["stage"] == "decide")
            .collect::<Vec<_>>();
        assert_eq!(votes.len(), 1);
        assert_eq!(votes[0]["changed"], json!(["stop"]));
    }
    /// 请求层的终端拒绝不伪造成上游 502；无须等待上游样本即可得到 422。
    #[test]
    fn default_request_rejection_is_422_without_an_upstream() {
        for attempts in [json!([]), json!([{"status":200}])] {
            let (result, rows) = trace_sample(
                "[[format]]\ncommand='aproxy-format'\nargs=['rewrite','--config','rules']\nstages=['request']",
                json!({"path":"/","body":"invalid json","attempts":attempts}),
                &[("rules", "remove=['/x']")],
            );
            result.unwrap();
            let end = rows.iter().find(|r| r["stage"] == "result").unwrap();
            assert_eq!(end["output"]["status"], 422);
            assert_eq!(end["output"]["code"], "format_rejected");
        }
    }
    /// 请求预算按原始正文而非样例文件长度检查，等于上限仍可进入第一层。
    #[test]
    fn sample_request_obeys_instance_body_limit() {
        for (size, valid) in [(1024 * 1024, true), (1024 * 1024 + 1, false)] {
            let (result, rows) = trace_sample(
                "max_request_mb=1",
                json!({"path":"/","body":"x".repeat(size)}),
                &[],
            );
            assert_eq!(result.is_ok(), valid);
            if !valid {
                assert!(rows.is_empty());
            }
        }
    }
    /// 必需密钥缺失时可选官方程序无法启动；它不能变成外部程序，也不能终止整条链。
    #[test]
    fn optional_unavailable_official_worker_is_skipped() {
        let p = "[[format]]\ncommand='aproxy-format'\nargs=['keys','--config','pool']\nstages=['request']\nrequired=false";
        let (result, rows) = trace_sample(
            p,
            json!({"path":"/","body":{}}),
            &[(
                "pool",
                "[[pool]]\norigin='https://relay.test'\nheader='authorization'\nkeys=[{id='a',env='APROXY_OFFLINE_MISSING_CREDENTIAL_TEST_379'}]",
            )],
        );
        result.unwrap();
        assert_eq!(rows[0]["role"], "keys");
        assert_eq!(rows[0]["changed"], json!(["skipped"]));
    }
    /// 输入很小也可能展开成超长请求补丁，进程内处理不得绕开真实管道的行长预算。
    #[test]
    fn in_process_request_reply_obeys_negotiated_group_limit() {
        let rules = format!("[set]\n'/x'='{}'", "x".repeat(3 * 1024 * 1024));
        let (result, rows) = trace_sample(
            "max_request_mb=1\n[[format]]\ncommand='aproxy-format'\nargs=['rewrite','--config','rules']\nstages=['request']",
            json!({"path":"/","body":{}}),
            &[("rules", &rules)],
        );
        assert!(matches!(result, Err(CliError::Unsimulated(_))));
        assert!(!rows.iter().any(|r| r["stage"] == "sample"));
    }
    /// 多层流在途时需要真实租约决定 abort 和剩余输出，试跑不能给出虚构的终态。
    #[test]
    fn nested_stream_interruption_is_explicitly_unsimulated() {
        let mut cfg = String::from("delivery='forward'\n");
        for name in ["a", "b"] {
            cfg += &format!(
                "[[format]]\nname='{name}'\ncommand='aproxy-format'\nargs=['convert','--from','openai_chat','--to','openai_chat']\nprocess='persistent'\nstages=['stream']\n"
            );
        }
        let (result, rows) = trace_sample(
            &cfg,
            json!({"path":"/","body":{},"attempts":[{"status":200,"chunks":["head"],"interrupted":true}]}),
            &[],
        );
        assert!(matches!(result,Err(CliError::Unsimulated(ref s)) if s.contains("多层流")));
        assert!(!rows.iter().any(|r| r["stage"] == "result"));
    }
    /// 保活由当前交付模式和叠加后的配置决定；always 可启用非流式请求，零间隔不可被样例强行打开。
    #[test]
    fn sample_heartbeat_respects_mode_interval_and_request_overrides() {
        let beat = "\n[[format]]\nname='h'\ncommand='aproxy-format'\nargs=['heartbeat']\nstages=['heartbeat']";
        let route = "\n[[format]]\nname='r'\ncommand='aproxy-format'\nargs=['route','--config','routes']\nstages=['request']";
        for (prefix, stream, enabled) in [
            ("delivery='forward'", true, false),
            ("keepalive.interval_secs=0", true, false),
            ("", true, true),
            (route, false, true),
        ] {
            let (result, rows) = trace_sample(
                &(prefix.to_owned() + beat),
                json!({"path":"/","body":{"stream":stream,"model":"demo"},"heartbeat":true,"attempts":[{"status":200}]}),
                &[(
                    "routes",
                    "[[channel]]\nid='c'\nurl='https://test.invalid'\noverrides={'keepalive.trigger'='always'}",
                )],
            );
            result.unwrap();
            assert_eq!(
                rows.iter().any(|r| r["stage"] == "heartbeat"),
                enabled,
                "{prefix}"
            );
        }
    }
    /// 完整的失败响应在直通与重试下给策略同一份头尾预览，不能只保留最后 64 KiB。
    #[test]
    fn complete_decide_preview_includes_the_large_tail_in_both_modes() {
        let body = format!(
            "{}preview-marker{}",
            "x".repeat(1024 * 1024),
            "y".repeat(300 * 1024)
        );
        for mode in ["forward", "retry"] {
            let cfg = format!(
                "delivery='{mode}'\n[[format]]\ncommand='aproxy-format'\nargs=['policy','--config','p.toml']\nstages=['decide']"
            );
            let (result, rows) = trace_sample(
                &cfg,
                json!({"path":"/","body":{},"attempts":[{"status":503,"body":body}]}),
                &[(
                    "p.toml",
                    "[[rule]]\nbody_regex='preview-marker'\ndecision='stop'\nresponse='context_too_long'",
                )],
            );
            result.unwrap();
            assert!(
                rows.iter()
                    .any(|r| r["stage"] == "result" && r["output"]["status"] == 400),
                "{mode}: {rows:?}"
            );
        }
    }

    /// 完整错误体和缓冲中断可以投票；直通已提交的响应不能再交给决定层。
    #[test]
    fn sampled_failures_only_vote_before_forward_commit() {
        let policy = "[[format]]\ncommand='aproxy-format'\nargs=['policy','--config','p.toml']\nstages=['decide']";
        let buffered = "\n[[format]]\nname='buffer'\ncommand='aproxy-format'\nargs=['convert','--from','openai_chat','--to','openai_chat']\nstages=['response']";
        for (mode, layers, attempt, code, votes) in [
            (
                "retry",
                "",
                json!({"status":200,"body":{"error":"bad"}}),
                "upstream_error_body",
                true,
            ),
            (
                "retry",
                "",
                json!({"status":200,"chunks":["data: {\"type\":\"error\"}\n\n"]}),
                "upstream_error_body",
                true,
            ),
            (
                "retry",
                "",
                json!({"status":200,"interrupted":true}),
                "upstream_body_interrupted",
                true,
            ),
            (
                "forward",
                buffered,
                json!({"status":200,"interrupted":true}),
                "upstream_body_interrupted",
                true,
            ),
            (
                "forward",
                "",
                json!({"status":200,"interrupted":true}),
                "upstream_body_interrupted",
                false,
            ),
            (
                "forward",
                "",
                json!({"status":200,"body":{"error":"bad"}}),
                "upstream_error_body",
                false,
            ),
            (
                "retry",
                "",
                json!({"status":200,"body":{"error":null}}),
                "upstream_error_body",
                false,
            ),
        ] {
            let (result, rows) = trace_sample(
                &format!("delivery='{mode}'\n{policy}{layers}"),
                json!({"path":"/","body":{},"attempts":[attempt]}),
                &[(
                    "p.toml",
                    &format!(
                        "[[rule]]\nfailure='{code}'\ndecision='stop'\nresponse='context_too_long'"
                    ),
                )],
            );
            result.unwrap();
            assert_eq!(
                rows.iter().any(|r| r["stage"] == "decide"),
                votes,
                "{mode}: {rows:?}"
            );
            assert_eq!(
                rows.last().unwrap()["output"]["status"],
                if votes { 400 } else { 200 }
            );
        }
    }

    /// 规则按声明先后生效，不对没有上游头的故障套状态规则；覆盖、直通与成功分别说明来源。
    #[test]
    fn dry_fallback_preserves_sources_and_backoff() {
        let (config, diags) = f::parse(
            "retry.max_backoff_secs=17\n[[retry.rules]]\nstatus=[503]\nmax_attempts=1",
            Path::new("c"),
        );
        assert!(diags.is_empty());
        let mut unit = Unit {
            limits: w::ReplyLimits::default(),
            layer: Layer {
                entry: f::parse(
                    "[[format]]\ncommand='example'\nstages=['request']",
                    Path::new("c"),
                )
                .0
                .format
                .unwrap()
                .pop()
                .unwrap(),
                role: None,
                label: "external",
            },
            peer: None,
            hello: None,
            seq: 0,
            state: None,
            overrides: None,
        };
        let matched = fallback(&config, &[], "/", 6, Some(503), true, "retry");
        assert_eq!(matched.by, "rule:1");
        assert_eq!(matched.decision, w::Decision::Stop);
        assert_eq!(matched.delay_ms, Some(17000));
        assert_eq!(
            fallback(&config, &[], "/", 6, None, true, "retry").decision,
            w::Decision::Retry
        );
        unit.overrides = Some(
            [(
                "retry".into(),
                json!({"max_attempts":0,"max_backoff_secs":3}),
            )]
            .into_iter()
            .collect(),
        );
        let units = [unit];
        let overridden = fallback(&config, &units, "/", 4, Some(503), true, "retry");
        assert_eq!(overridden.by, "override");
        assert_eq!(overridden.decision, w::Decision::Retry);
        assert_eq!(overridden.delay_ms, Some(3000));
        for (failed, mode) in [(true, "forward"), (false, "retry")] {
            let observed = fallback(&config, &units, "/", 4, Some(503), failed, mode);
            assert_eq!(observed.by, "default");
            assert_eq!(observed.decision, w::Decision::Stop);
            assert_eq!(observed.delay_ms, Some(3000));
        }
    }
}
