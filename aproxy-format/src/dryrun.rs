//! 试跑只消费用户提供的请求与模拟上游尝试。不建网络客户端；输出从白名单字段另行构造。
mod peer;
use crate::tools::{self, Layer};
use aproxy_envelope as w;
use aproxy_ipc::config_file::{self as f, Stage};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

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
/// 一个处理位置的协商能力、序号与私有状态。程序之间不共享任何这些字段。
struct Unit {
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
    fn connect(&mut self, dir: &Path) -> Result<(), String> {
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
            if let crate::Role::Keys(keys) = role {
                keys.sample_time = Some((std::time::Instant::now(), std::time::SystemTime::now()));
            }
            role.hello(self.layer.label, &offer)
        } else {
            let p = peer::ChildPeer::start(e, dir)?;
            let mut h = serde_json::to_value(&offer).map_err(|_| "无法编码握手")?;
            h["v"] = 1.into();
            h["seq"] = 0.into();
            h["stage"] = "hello".into();
            let hr = serde_json::from_value(p.frame(&h, 64 * 1024, e.timeout_secs)?)
                .map_err(|_| "外部程序握手无效")?;
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
            return Err("角色协商的阶段或协议不符".into());
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
        if matches!(stage, "request" | "retry" | "decide")
            && !matches!(need, Some(w::BodyNeed::Full))
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
        let reply = if let Some(role) = &mut self.layer.role {
            let m = serde_json::from_value(msg).map_err(|_| "样例不能构成该阶段的消息")?;
            role.handle(&m)
        } else {
            let timeout = match stage {
                "notify" => 2,
                "decide" => self.layer.entry.decide_timeout_secs,
                _ => self.layer.entry.timeout_secs,
            };
            let value = self.peer.as_ref().ok_or("外部程序未连接")?.frame(
                &msg,
                8 * 1024 * 1024,
                timeout,
            )?;
            serde_json::from_value(value).map_err(|_| "外部程序回信无效")?
        };
        if reply.v != 1 || reply.seq != self.seq {
            return Err("角色回信序号不符".into());
        }
        self.seq += 1;
        if !matches!(stage, "heartbeat" | "notify")
            && let Some(state) = &reply.state
        {
            if state.len() > 64 * 1024 {
                return Err("角色状态超过限制".into());
            }
            self.state = (!state.is_empty()).then(|| state.clone());
        }
        Ok(reply)
    }
    /// 被跳过的第三方、协商失败的可选层与没有声明本阶段的角色不获得数据。
    fn has(&self, stage: Stage) -> bool {
        self.hello.is_some() && self.layer.entry.has_stage(stage)
    }
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
}
impl<W: Write> Trial<'_, W> {
    /// 程序业务拒绝与进程失败分开；发生进程故障的可选层不会继续吃样本。
    fn call(&mut self, i: usize, stage: &str, msg: Value) -> Result<w::Reply, String> {
        match self.units[i].call(stage, msg, &self.digest, self.dir) {
            Ok(r) => Ok(r),
            Err(_)
                if !self.units[i].layer.entry.required
                    || matches!(stage, "notify" | "decide" | "heartbeat") =>
            {
                self.units[i].peer = None;
                self.units[i].hello = None;
                trace(
                    self.out,
                    stage,
                    self.units[i].layer.label,
                    Value::Null,
                    Value::Null,
                    vec!["skipped"],
                )?;
                Ok(w::Reply::new(0))
            }
            Err(_) => Err("试跑角色未能完成本阶段".into()),
        }
    }
    /// 请求补丁按次序串联，投递方式只取第一票，重跑只更新参与的后段。
    fn request(
        &mut self,
        req: &mut w::Request,
        retry: Option<Value>,
    ) -> Result<Option<w::Fail>, String> {
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
    ) -> Result<(bool, bool, Option<w::ResponsePatch>), String> {
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
            let mut msg = json!({"client":self.client,"attempt":attempt,"elapsed_ms":0,"delivery":self.mode,"outcome":if failure.is_some(){"failure"}else{"success"},"request":{"method":req.method,"url":req.url},"fallback":fallback});
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
                obs.headers.remove("content-encoding");
                if self.mode == "forward" {
                    // 直通模式只观察头，不给决策单元它在真实流上还不可能看到的正文。
                    obs.body = w::Body::Absent;
                    msg["response"] = json!(obs);
                } else {
                    let data = obs.body.as_bytes().unwrap_or_default();
                    let missing = data.len().saturating_sub(128 * 1024);
                    if missing > 0 {
                        obs.body = w::Body::from_bytes(
                            [&data[..64 * 1024], &data[data.len() - 64 * 1024..]].concat(),
                        );
                    }
                    msg["response"] = json!(obs);
                    msg["response"]["truncated"] = if missing > 0 {
                        missing.into()
                    } else {
                        Value::Null
                    };
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
        Ok((
            decision.unwrap_or(fallback.decision == w::Decision::Retry),
            reroute.unwrap_or(true),
            stop,
        ))
    }
    /// 心跳走反向链；输出只说明数据是否改变，不回显事件内容。
    fn heartbeat(&mut self, n: u32, initial: &str) -> Result<(), String> {
        let mut data = initial.as_bytes().to_vec();
        for i in (0..self.units.len()).rev() {
            if self.units[i].has(Stage::Heartbeat) {
                let mut m =
                    json!({"client":self.client,"heartbeat":{"beat":n,"elapsed_ms":0,"attempt":n}});
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
    ) -> Result<(), String> {
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
                    *resp = f.response.ok_or("响应角色拒绝样例")?;
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
                    *resp = f.response.ok_or("流角色拒绝打开样例")?;
                    chunks = vec![resp.body.as_bytes().unwrap_or_default().to_vec()];
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
                } else if let Some(prefix) = open.data.as_bytes() {
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
                        return Err("流角色拒绝样例数据".into());
                    }
                    let b = r.data.as_bytes().unwrap_or(&chunk);
                    trace(
                        self.out,
                        "stream:data",
                        label,
                        Value::Null,
                        Value::Null,
                        if b == chunk { vec![] } else { vec!["data"] },
                    )?;
                    result.push(b.to_vec());
                }
                let end = if cut {
                    json!({"stream_id":stream_id,"event":"abort","reason":{"code":"upstream_body_interrupted"}})
                } else {
                    json!({"stream_id":stream_id,"event":"close"})
                };
                let r = self.call(i, "stream", end)?;
                if !matches!(resp.status, 100..=199 | 204 | 304)
                    && let Some(b) = r.data.as_bytes()
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
    /// 完成只发给订阅者；通知不是可以再改变交付结果的一票。
    fn notify(
        &mut self,
        n: u32,
        status: Option<u16>,
        upgraded: bool,
        completed: Option<bool>,
        outcome: &str,
    ) -> Result<(), String> {
        for i in 0..self.units.len() {
            if self.units[i].has(Stage::Notify) {
                self.call(i,"notify",json!({"event":"request_end","client":self.client,"delivery":self.mode,"upgraded":upgraded,"attempts":n,"duration_ms":0,"upstream_status":status,"stream_completed":completed,"outcome":outcome,"code":null}))?;
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
) -> Result<(), String> {
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
) -> Result<(), String> {
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
    let accept = req.headers.get("accept").is_some_and(|v| {
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
    };
    req.body = w::Body::from_bytes(raw);
    for layer in chain.layers {
        if external
            && layer.role.is_none()
            && home.is_none()
            && !Path::new(&layer.entry.command).is_absolute()
        {
            return Err("运行外部相对命令需要 --home；未启动程序".into());
        }
        let mut u = Unit {
            layer,
            peer: None,
            hello: None,
            seq: 0,
            state: None,
            overrides: None,
        };
        if u.layer.role.is_none() && !external {
            trace(
                t.out,
                "hello",
                "external",
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
                Err(_) if !u.layer.entry.required => trace(
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
    for (index, attempt) in sample.attempts.into_iter().enumerate() {
        let n = index as u32 + 1;
        if sample.heartbeat && streaming {
            t.heartbeat(
                n,
                chain
                    .config
                    .keepalive
                    .heartbeat
                    .as_deref()
                    .unwrap_or(": keepalive\n\n"),
            )?;
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
            r.body = w::Body::from_bytes(chunks.concat());
            r
        });
        let mut failure = attempt
            .failure
            .map(|code| json!({"code":code}))
            .or_else(|| {
                attempt
                    .status
                    .filter(|s| *s >= 400)
                    .map(|s| json!({"code":"upstream_status","details":{"status":s}}))
            });
        if let Some(rejection) = refused.take() {
            if !rejection.retry {
                trace(
                    t.out,
                    "result",
                    "proxy",
                    Value::Null,
                    json!({"status":rejection.response.as_ref().map_or(502,|r|r.status)}),
                    vec!["rejected"],
                )?;
                t.notify(n, None, upgraded, None, "failed")?;
                return Ok(());
            }
            failure = Some(json!({"code":"format_failed"}));
            resp = None;
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
        let (retry, reroute, stop) = if t.mode == "forward" && failure.is_none() {
            (false, false, None)
        } else {
            t.decide(&req, resp.as_ref(), failure.as_ref(), n, base_retry)?
        };
        if retry {
            if t.mode == "forward" {
                upgraded = true;
                t.mode = "retry";
            }
            let mut info = json!({"attempt":n+1,"last_failure":failure.unwrap_or_else(||json!({"code":"format_decision"})),"elapsed_ms":0});
            if let Some(r) = resp {
                info["last_response"] = json!({"status":r.status,"headers":r.headers});
            }
            if reroute || t.cursor != t.units.len() {
                refused = t.request(&mut req, Some(info))?;
            }
            continue;
        }
        let mut response = resp.unwrap_or_else(|| w::Response::new(502));
        if let Some(p) = stop {
            response.apply(p);
        } else if failure.is_none()
            && (t.mode != "forward" || (200..300).contains(&response.status))
        {
            t.deliver(&req, &mut response, chunks, attempt.interrupted, n)?;
        }
        trace(
            t.out,
            "result",
            "proxy",
            Value::Null,
            json!({"status":response.status}),
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
    ) -> (Result<(), String>, Vec<Value>) {
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
    /// 规则按声明先后生效，不对没有上游头的故障套状态规则；覆盖、直通与成功分别说明来源。
    #[test]
    fn dry_fallback_preserves_sources_and_backoff() {
        let (config, diags) = f::parse(
            "retry.max_backoff_secs=17\n[[retry.rules]]\nstatus=[503]\nmax_attempts=1",
            Path::new("c"),
        );
        assert!(diags.is_empty());
        let mut unit = Unit {
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
