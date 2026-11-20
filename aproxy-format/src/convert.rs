//! convert 为每个请求记住客户端协议。回程即使换了 worker，也不从上游响应内容反推客户端。
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
    pub to: F,
    /// 每个在途流独占进程，仅在 open 和终止帧之间保留这份状态。
    #[serde(skip)]
    stream: Option<Entity>,
}
/// 非流式实体也能经过 stream 阶段；同协议的流则无需收集正文。
enum Entity {
    /// 直接转发每个数据片段。
    Pass,
    /// 只在跨协议非 SSE 实体结束后转换 JSON。
    Json(F, Vec<u8>),
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
        let c = Self {
            from: from.into(),
            to: protocol(to)?,
            stream: None,
        };
        c.validate()?;
        Ok(c)
    }
    /// 检查显式输入协议；auto 留到实际请求再确定。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.from != "auto" {
            protocol(&self.from)?;
        }
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
        let mut patch = w::RequestPatch::new();
        let mut headers = m.request.headers.clone();
        if from != self.to {
            patch.body = w::Body::Text(
                translate::translate_request(from, self.to, &body)
                    .map_err(|_| {
                        Fault::new("请求协议转换失败", "request protocol conversion failed")
                    })?
                    .to_string(),
            );
            let mut url = config::endpoint(&m.request.url)
                .map_err(|e| Fault::new(e, "invalid upstream URL"))?;
            if url.path() == path(from) {
                url.set_path(path(self.to));
                patch.url = Some(url.into());
            }
            headers.remove("content-encoding");
            headers.remove("content-length");
        }
        headers
            .entry("content-type".into())
            .or_insert_with(|| "application/json".into());
        if self.to == F::AnthropicMessages {
            headers
                .entry("anthropic-version".into())
                .or_insert_with(|| "2023-06-01".into());
        }
        patch.headers = Some(headers);
        r.request = Some(patch);
        r.state = Some(from.to_string());
        r.report = config::report(
            "Selected the protocol conversion",
            w::StandardAction::ProtocolConvert {
                from: from.to_string(),
                to: self.to.to_string(),
            },
        );
        Ok(())
    }
    /// 响应专用配置可直接声明客户端协议；自动检测则必须拿到请求留下的状态。
    fn client(&self, ctx: &w::Context) -> Result<F, Fault> {
        ctx.state
            .as_deref()
            .or((self.from != "auto").then_some(self.from.as_str()))
            .ok_or(Fault::new(
                "响应缺少客户端协议状态",
                "client protocol state is missing",
            ))
            .and_then(|name| protocol(name).map_err(|e| Fault::new(e, "invalid client protocol")))
    }
    /// 缓冲转换只解释成功实体，不把源正文或库错误中的字段值带到日志。
    fn json(&self, client: F, bytes: &[u8]) -> Result<String, Fault> {
        if std::str::from_utf8(bytes).is_ok_and(detect::looks_like_sse) {
            return Err(Fault::new(
                "跨协议事件流需要 stream 阶段",
                "cross-protocol SSE requires the stream stage",
            ));
        }
        let body = serde_json::from_slice(bytes)
            .map_err(|_| Fault::new("响应体不是 JSON", "response body is not valid JSON"))?;
        Ok(translate::translate_response(self.to, client, &body)
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
        let client = self.client(&m.context)?;
        let mut p = self.response_headers(&m.response.headers);
        if client != self.to {
            p.body =
                w::Body::Text(self.json(client, m.response.body.as_bytes().unwrap_or_default())?);
        }
        r.response = Some(p);
        Ok(())
    }
    /// 头部在 open 即交付。跨协议 JSON 在 close 处理，半截实体绝不作为完整响应提交。
    pub fn stream(&mut self, m: &w::StreamMessage, r: &mut w::Reply) -> Result<(), Fault> {
        match &m.event {
            w::StreamEvent::Open(o) => {
                let client = self.client(&o.context)?;
                r.response = Some(self.response_headers(&o.response.headers));
                self.stream = Some(if client == self.to {
                    Entity::Pass
                } else if o.response.headers.get("content-type").is_some_and(|s| {
                    s.split(';')
                        .next()
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("text/event-stream"))
                }) {
                    return Err(Fault::new(
                        "暂不支持跨协议事件流",
                        "cross-protocol streaming is not supported",
                    ));
                } else {
                    Entity::Json(client, Vec::new())
                });
            }
            w::StreamEvent::Data(d) => match self
                .stream
                .as_mut()
                .ok_or(Fault::new("流未打开", "stream is not open"))?
            {
                Entity::Pass => r.data = w::Data::from_bytes(d.as_bytes().to_vec()),
                Entity::Json(_, b) => {
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
            w::StreamEvent::Close => {
                if let Some(Entity::Json(client, b)) = self.stream.take() {
                    r.data = w::Data::Text(self.json(client, &b)?);
                }
            }
            w::StreamEvent::Abort { .. } => {
                self.stream = None;
            }
            _ => return Err(Fault::new("不支持的流事件", "unsupported stream event")),
        };
        Ok(())
    }
}
