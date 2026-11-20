//! convert 为每个请求记住客户端与上游的协议。回程即使换了 worker，也不从上游响应内容反推客户端。
use crate::config::Fault;
use crate::{config, detect, translate};
use aproxy_envelope as w;
use serde::Deserialize;
use serde_json::Value;
use switchyard_translation::WireFormat as F;

/// 输入格式逐请求检测，输出格式由上游契约决定。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Convert {
    /// `auto` 或明确的客户端协议。
    pub from: String,
    /// 上游使用的协议。
    pub to: String,
    /// to=auto 的目的端协议表，加载后按路径前缀从长到短排好。
    #[serde(default)]
    upstream: Vec<Destination>,
    /// 每个在途流独占进程，仅在 open 和终止帧之间保留这份状态。
    #[serde(skip)]
    stream: Option<Entity>,
}
/// 匹配规范化后的 origin 和路径段，避免 `/tenant` 也命中 `/tenant2`。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Destination {
    /// 必须是完整 HTTP(S) origin，不可含路径、凭据或查询。
    origin: String,
    /// 空或根表示整个 origin；其他值必须从完整路径段开始。
    #[serde(default)]
    path_prefix: String,
    /// 该地址真正使用的上游协议。
    protocol: F,
}
/// 非流式实体也能经过 stream 阶段；同协议的流则无需收集正文。
enum Entity {
    /// 直接转发每个数据片段。
    Pass,
    /// 已交付客户端协议的错误结尾，继续消费上游但绝不再转出字节。
    Ended,
    /// 跨协议 SSE：持有单个未结束事件和各协议的语义状态。
    Sse(Box<crate::events::Events>),
    /// 只在跨协议非 SSE 实体结束后转换 JSON。
    Json(F, F, Vec<u8>),
}
/// 不向诊断复制用户输入的协议名。
pub fn protocol(s: &str) -> Result<F, &'static str> {
    serde_json::from_value(Value::String(s.into())).map_err(|_| "未知协议名称")
}
/// 标准客户端路径比共享的 messages 字段更能区分 Anthropic 与 Chat。
fn path(f: F) -> &'static str {
    match f {
        F::AnthropicMessages => "/v1/messages",
        F::OpenAiChat => "/v1/chat/completions",
        F::OpenAiResponses => "/v1/responses",
    }
}
impl Convert {
    /// 指定方向的命令行与配置文件使用同样的协议校验。
    pub fn new(from: &str, to: &str) -> Result<Self, &'static str> {
        let mut c = Self {
            from: from.into(),
            to: to.into(),
            upstream: Vec::new(),
            stream: None,
        };
        c.validate()?;
        Ok(c)
    }
    /// 握手前校验协议、映射唯一性与匹配边界，auto 留到看到实际请求再解析。
    pub fn validate(&mut self) -> Result<(), &'static str> {
        if self.from != "auto" {
            protocol(&self.from)?;
        }
        if self.to != "auto" {
            protocol(&self.to)?;
            if !self.upstream.is_empty() {
                return Err("目的端协议表只用于 to=auto");
            }
        }
        let mut seen = std::collections::HashSet::new();
        for d in &mut self.upstream {
            let url = config::endpoint(&d.origin)?;
            if url.path() != "/" || url.query().is_some() {
                return Err("目的端映射必须给出 origin");
            }
            d.origin = url.origin().ascii_serialization();
            if !d.path_prefix.is_empty()
                && (!d.path_prefix.starts_with('/')
                    || d.path_prefix.contains(['?', '#', '\\'])
                    || d.path_prefix.split('/').any(|v| matches!(v, "." | "..")))
            {
                return Err("目的端路径前缀无效");
            }
            d.path_prefix
                .truncate(d.path_prefix.trim_end_matches('/').len());
            if !seen.insert((d.origin.clone(), d.path_prefix.clone())) {
                return Err("目的端协议映射重复");
            }
        }
        self.upstream
            .sort_by_key(|d| std::cmp::Reverse(d.path_prefix.len()));
        Ok(())
    }
    /// 只改写标准协议端点的路径，自定义上游路径与查询串保持原样。
    pub fn request(&self, m: &w::RequestMessage, r: &mut w::Reply) -> Result<(), Fault> {
        let body: Value = serde_json::from_slice(m.request.body.as_bytes().unwrap_or_default())
            .map_err(|_| Fault::new("请求体不是 JSON", "request body is not valid JSON"))?;
        let from = if self.from == "auto" {
            [F::AnthropicMessages, F::OpenAiChat, F::OpenAiResponses]
                .into_iter()
                .find(|f| Some(path(*f)) == m.client.path.split('?').next())
                .or_else(|| detect::detect_request(&body))
                .ok_or(Fault::new(
                    "无法识别客户端协议，请指定 --from",
                    "unknown client protocol; specify --from",
                ))?
        } else {
            protocol(&self.from).map_err(|e| Fault::new(e, "invalid client protocol"))?
        };
        let mut url =
            config::endpoint(&m.request.url).map_err(|e| Fault::new(e, "invalid upstream URL"))?;
        let to = if self.to == "auto" {
            let origin = url.origin().ascii_serialization();
            self.upstream
                .iter()
                .find(|d| {
                    d.origin == origin
                        && url
                            .path()
                            .strip_prefix(&d.path_prefix)
                            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
                })
                .map(|d| d.protocol)
                .or_else(|| {
                    [F::AnthropicMessages, F::OpenAiChat, F::OpenAiResponses]
                        .into_iter()
                        .find(|f| url.path().ends_with(path(*f)))
                })
                .ok_or(Fault::new(
                    "无法识别上游协议，请配置目的端映射",
                    "unknown upstream protocol; configure a destination mapping",
                ))?
        } else {
            protocol(&self.to).map_err(|e| Fault::new(e, "invalid upstream protocol"))?
        };
        let mut patch = w::RequestPatch::new();
        let mut headers = m.request.headers.clone();
        if from != to {
            patch.body = w::Body::Text(
                translate::translate_request(from, to, &body)
                    .map_err(|_| {
                        Fault::new("请求协议转换失败", "request protocol conversion failed")
                    })?
                    .to_string(),
            );
            if url.path() == path(from) {
                url.set_path(path(to));
                patch.url = Some(url.into());
            }
            headers.remove("content-encoding");
            headers.remove("content-length");
        }
        headers
            .entry("content-type".into())
            .or_insert_with(|| "application/json".into());
        if to == F::AnthropicMessages {
            headers
                .entry("anthropic-version".into())
                .or_insert_with(|| "2023-06-01".into());
        }
        patch.headers = Some(headers);
        r.request = Some(patch);
        r.state = Some(serde_json::json!([from, to]).to_string());
        r.report = config::report(
            "Selected the protocol conversion",
            w::StandardAction::ProtocolConvert {
                from: from.to_string(),
                to: to.to_string(),
            },
        );
        Ok(())
    }
    /// 回程的两端协议来自同一个请求的状态；无状态的独立响应仅限两端都显式指定。
    fn pair(&self, ctx: &w::Context) -> Result<(F, F), Fault> {
        if let Some(s) = &ctx.state {
            return serde_json::from_str(s)
                .map_err(|_| Fault::new("协议状态无效", "invalid protocol state"));
        }
        if self.from == "auto" || self.to == "auto" {
            return Err(Fault::new(
                "响应缺少客户端协议状态",
                "client protocol state is missing",
            ));
        }
        Ok((
            protocol(&self.from).map_err(|e| Fault::new(e, "invalid client protocol"))?,
            protocol(&self.to).map_err(|e| Fault::new(e, "invalid upstream protocol"))?,
        ))
    }
    /// 回程失败要替换整份实体；裸 fail 会让代理原样透传上游私有错误页和鉴权头。
    /// 丢失协商状态时只用两族 SDK 都能读的 error 对象，不从原正文猜协议或复制原文。
    pub fn error_response(&self, ctx: &w::Context) -> w::Response {
        let message = "The upstream response could not be converted.";
        let body = match self
            .pair(ctx)
            .map(|(client, _)| client)
            .or_else(|_| protocol(&self.from))
        {
            Ok(F::OpenAiChat | F::OpenAiResponses) => {
                serde_json::json!({"error":{"type":"server_error","message":message,"code":"conversion_failed","param":null}})
            }
            Ok(F::AnthropicMessages) | Err(_) => {
                serde_json::json!({"type":"error","error":{"type":"api_error","message":message}})
            }
        };
        let mut response = w::Response::new(502);
        response
            .headers
            .insert("content-type".into(), "application/json".into());
        response.body = w::Body::Text(body.to_string());
        response
    }
    /// 缓冲转换只解释成功实体，不把源正文或库错误中的字段值带到日志。
    fn json(client: F, upstream: F, bytes: &[u8]) -> Result<String, Fault> {
        if std::str::from_utf8(bytes).is_ok_and(detect::looks_like_sse) {
            return Err(Fault::new(
                "跨协议事件流需要 stream 阶段",
                "cross-protocol SSE requires the stream stage",
            ));
        }
        let body: Value = serde_json::from_slice(bytes)
            .map_err(|_| Fault::new("响应体不是 JSON", "response body is not valid JSON"))?;
        // 编解码器允许缺字段以兼容不同提供方，但缺少协议的响应容器不能被补成一次成功的空回复。
        let valid = match upstream {
            F::OpenAiChat => body["choices"].as_array().is_some_and(|rows| {
                !rows.is_empty() && rows.iter().all(|choice| choice["message"].is_object())
            }),
            F::AnthropicMessages => body["role"] == "assistant" && body["content"].is_array(),
            F::OpenAiResponses => body["output"].is_array(),
        };
        if !valid {
            return Err(Fault::new(
                "响应缺少上游协议的必要字段",
                "response does not match the upstream protocol",
            ));
        }
        Ok(translate::translate_response(upstream, client, &body)
            .map_err(|_| Fault::new("响应协议转换失败", "response protocol conversion failed"))?
            .to_string())
    }
    /// 无论是否转换都剔除上游的鉴权头，不能回传给客户端。
    fn response_headers(&self, headers: &w::Headers) -> w::ResponsePatch {
        let mut p = w::ResponsePatch::new();
        let mut h = headers.clone();
        config::strip_credentials(&mut h);
        p.headers = Some(h);
        p
    }
    /// 同协议不解析实体，保留原字节（含 SSE 与二进制）。
    pub fn response(&self, m: &w::ResponseMessage, r: &mut w::Reply) -> Result<(), Fault> {
        let (client, upstream) = self.pair(&m.context)?;
        let mut p = self.response_headers(&m.response.headers);
        if client != upstream {
            p.body = w::Body::Text(Self::json(
                client,
                upstream,
                m.response.body.as_bytes().unwrap_or_default(),
            )?);
        }
        r.response = Some(p);
        Ok(())
    }
    /// 头部在 open 即交付。跨协议 JSON 在 close 处理，半截实体绝不作为完整响应提交。
    pub fn stream(&mut self, m: &w::StreamMessage, r: &mut w::Reply) -> Result<(), Fault> {
        match &m.event {
            w::StreamEvent::Open(o) => {
                let (client, upstream) = self.pair(&o.context)?;
                r.response = Some(self.response_headers(&o.response.headers));
                self.stream = Some(if client == upstream {
                    Entity::Pass
                } else if o.response.headers.get("content-type").is_some_and(|s| {
                    s.split(';')
                        .next()
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("text/event-stream"))
                }) {
                    let events = crate::events::Events::new(upstream, client);
                    r.abort_tail = Some(events.abort().into());
                    Entity::Sse(Box::new(events))
                } else {
                    Entity::Json(client, upstream, Vec::new())
                });
            }
            w::StreamEvent::Data(d) => match self
                .stream
                .as_mut()
                .ok_or(Fault::new("流未打开", "stream is not open"))?
            {
                Entity::Pass => r.data = w::Data::from_bytes(d.as_bytes().to_vec()),
                Entity::Ended => r.data = w::Data::Text(String::new()),
                Entity::Sse(events) => match events.feed(d.as_bytes()) {
                    Ok(out) => r.data = w::Data::Text(out),
                    Err(_) => {
                        r.data = w::Data::Text(events.abort().into());
                        self.stream = Some(Entity::Ended);
                    }
                },
                Entity::Json(_, _, b) => {
                    if b.len().saturating_add(d.as_bytes().len()) > 64 * 1024 * 1024 {
                        return Err(Fault::new(
                            "非流式响应超过 64 MiB",
                            "non-stream response exceeds 64 MiB",
                        ));
                    }
                    b.extend_from_slice(d.as_bytes());
                    r.data = w::Data::Text(String::new());
                }
            },
            w::StreamEvent::Close => match self.stream.take() {
                Some(Entity::Json(client, upstream, b)) => {
                    r.data = w::Data::Text(Self::json(client, upstream, &b)?)
                }
                Some(Entity::Sse(events)) => {
                    let tail = events.abort();
                    r.data = w::Data::Text(events.finish().unwrap_or_else(|_| tail.into()));
                }
                _ => (),
            },
            w::StreamEvent::Abort { .. } => {
                if let Some(Entity::Sse(events)) = self.stream.take() {
                    r.data = w::Data::Text(events.abort().into());
                }
            }
            _ => return Err(Fault::new("不支持的流事件", "unsupported stream event")),
        };
        Ok(())
    }
}
