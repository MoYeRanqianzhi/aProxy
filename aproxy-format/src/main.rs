//! 每个角色是独立的协议 1 工作进程。首行协商、逐行应答，直到 aproxy 关闭标准输入。
//! `translate` 只读一个 JSON 请求，既不参与握手也不运行处理链。
mod config;
mod convert;
mod detect;
mod events;
mod heartbeat;
mod keys;
mod route;
mod translate;
use crate::config::Fault;
use aproxy_envelope as w;
use serde::Deserialize;
use std::io::{BufRead, Read, Write};

/// 用公开命令名选择职责，不按帧内容或 URL 猜测某个进程扮演的角色。
enum Role {
    /// 转换请求与返回实体。
    Convert(convert::Convert),
    /// 按目标 origin 选择认证信息。
    Keys(keys::Keys),
    /// 根据模型选择渠道。
    Route(route::Route),
    /// 只在请求方向改写字段。
    Rewrite,
    /// 只对尝试结果投票。
    Policy,
    /// 按摘要决定 forward 或 retry，并接收完成通知。
    Adaptive,
    /// 只处理保活帧。
    Heartbeat(heartbeat::Heartbeat),
}
/// 没有配置项的角色也拒绝未知键，拼写错误不能默默被吞。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
/// 命令行选项形式两种都接受，不把未知参数（可能是误填的密钥）写入错误。
#[derive(Default)]
struct Options<'a> {
    /// 角色 TOML 文件。
    config: Option<&'a str>,
    /// 客户端协议或 auto。
    from: Option<&'a str>,
    /// 上游协议。
    to: Option<&'a str>,
}
impl<'a> Options<'a> {
    /// 重复、缺值或未知选项都是配置错误。
    fn parse(args: &'a [String]) -> Result<Self, &'static str> {
        let mut result = Self::default();
        let mut iter = args.iter().map(String::as_str);
        while let Some(arg) = iter.next() {
            let (name, value) = match arg.split_once('=') {
                Some(pair) => pair,
                None => (arg, iter.next().ok_or("参数缺少值")?),
            };
            let slot = match name {
                "--config" => &mut result.config,
                "--from" => &mut result.from,
                "--to" => &mut result.to,
                _ => return Err("未知参数"),
            };
            if value.is_empty() || slot.replace(value).is_some() {
                return Err("参数重复或为空");
            }
        }
        Ok(result)
    }
}
impl Role {
    /// 握手前完成静态配置检查，不会用半套配置处理请求。
    fn load(name: &str, args: &[String]) -> Result<Self, &'static str> {
        let opt = Options::parse(args)?;
        if name == "convert" {
            return Ok(Self::Convert(match (opt.config, opt.from, opt.to) {
                (Some(p), None, None) => {
                    let c: convert::Convert = config::load(p)?;
                    c.validate()?;
                    c
                }
                (None, Some(f), Some(t)) => convert::Convert::new(f, t)?,
                _ => return Err("convert 需指定 --config 或 --from 与 --to"),
            }));
        }
        if opt.from.is_some() || opt.to.is_some() {
            return Err("此角色不接受协议参数");
        }
        Ok(match name {
            "heartbeat" => Self::Heartbeat(heartbeat::Heartbeat::load(opt.config)?),
            "keys" => Self::Keys(keys::Keys::load(opt.config.ok_or("keys 需要 --config")?)?),
            "route" => Self::Route(route::Route::load(
                opt.config.ok_or("route 需要 --config")?,
            )?),
            name => {
                if let Some(p) = opt.config {
                    config::load::<Empty>(p)?;
                }
                match name {
                    "policy" => Self::Policy,
                    "adaptive" => Self::Adaptive,
                    "rewrite" => Self::Rewrite,
                    _ => return Err(USAGE),
                }
            }
        })
    }
    /// 订阅声明是上限；实例最终只向配置选中的阶段写帧。摘要协商不能退化为默认的 full。
    fn hello(&self, name: &str) -> w::HelloReply {
        use w::BodyNeed::{Digest, Full, None};
        let (stages, needs): (&[&str], &[(&str, w::BodyNeed)]) = match self {
            Self::Convert(_) => (
                &["request", "retry", "response", "stream"],
                &[("request", Full), ("retry", Full)],
            ),
            Self::Keys(_) | Self::Rewrite => {
                (&["request", "retry"], &[("request", None), ("retry", None)])
            }
            Self::Route(route) => (route.stages(), &[("request", Full), ("retry", Full)]),
            Self::Policy => (&["decide"], &[("decide", None)]),
            Self::Adaptive => (
                &["request", "decide", "notify"],
                &[("request", Digest), ("decide", None), ("notify", Digest)],
            ),
            Self::Heartbeat(_) => (&["heartbeat"], &[]),
        };
        let mut reply = w::HelloReply::new(
            stages.iter().map(|s| s.to_string()).collect(),
            name.into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        reply.request_body = needs
            .iter()
            .map(|(s, v)| (s.to_string(), v.clone()))
            .collect();
        reply
    }
    /// 只在角色声明过的方向产生补丁；业务拒绝是带 seq 的正常回信，不打断持久进程。
    fn handle(&mut self, m: &w::Message) -> w::Reply {
        let mut r = w::Reply::new(m.seq);
        let result = match &m.stage {
            w::Stage::Request(input) => self.request(input, &mut r),
            w::Stage::Retry(retry) => self.request(&retry.input, &mut r),
            w::Stage::Response(response) => match self {
                Self::Convert(c) => c.response(response, &mut r),
                Self::Route(route) => route.response(response, &mut r),
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            w::Stage::Stream(s) => match self {
                Self::Convert(c) => c.stream(s, &mut r),
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            w::Stage::Decide(_) if matches!(self, Self::Adaptive | Self::Policy) => Ok(()),
            w::Stage::Notify(_) if matches!(self, Self::Adaptive) => Ok(()),
            w::Stage::Heartbeat(h) => match self {
                Self::Heartbeat(heartbeat) => {
                    heartbeat.beat(h, &mut r);
                    Ok(())
                }
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
        };
        if let Err(reason) = result {
            r = w::Reply::new(m.seq);
            let _ = writeln!(std::io::stderr().lock(), "aproxy-format: {}", reason.local);
            r.fail = Some(w::Fail::new(false, reason.wire.into()));
        }
        r
    }
    /// 同一请求的后续阶段可能换进程，只使用信封携带的私有状态。
    fn request(&mut self, m: &w::RequestMessage, r: &mut w::Reply) -> Result<(), Fault> {
        match self {
            Self::Convert(c) => c.request(m, r),
            Self::Keys(k) => k.request(m, r),
            Self::Route(route) => route.request(m, r),
            Self::Rewrite => Ok(()),
            Self::Adaptive => {
                r.delivery = Some("forward".into());
                Ok(())
            }
            _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
        }
    }
}
/// 只在标准输出写一行 JSON 并立即 flush；管道断开直接结束，不循环输出错误。
fn emit(out: &mut impl Write, value: &impl serde::Serialize) -> Result<(), &'static str> {
    serde_json::to_writer(&mut *out, value).map_err(|_| "无法写入标准输出")?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|_| "无法写入标准输出")
}
/// 首帧是固定 hello，后续序号只在本进程内有意义。EOF 在握手后是正常退役。
fn worker(
    name: &str,
    mut role: Role,
    input: impl BufRead,
    mut out: impl Write,
) -> Result<(), &'static str> {
    for (next, line) in (0u64..).zip(input.lines()) {
        let m: w::Message = serde_json::from_str(&line.map_err(|_| "无法读取标准输入")?)
            .map_err(|_| "协议帧无效")?;
        if m.v != 1 || m.seq != next {
            return Err("协议版本或序号不匹配");
        }
        if next == 0 {
            let w::Stage::Hello(offer) = m.stage else {
                return Err("首帧必须是 hello");
            };
            if !offer.protocols.contains(&1) {
                return Err("不支持此协议版本");
            }
            emit(&mut out, &role.hello(name))?;
        } else {
            if matches!(m.stage, w::Stage::Hello(_)) {
                return Err("重复的 hello");
            }
            emit(&mut out, &role.handle(&m))?;
        }
    }
    Ok(())
}
/// 无信封的一次性协议转换。出错只给诊断类别，不回显含消息或密钥的原始输入。
fn translate_once(args: &[String]) -> Result<(), &'static str> {
    let opt = Options::parse(args)?;
    let (None, Some(from), Some(to)) = (opt.config, opt.from, opt.to) else {
        return Err("用法: aproxy-format translate --from <协议> --to <协议>");
    };
    let from = convert::protocol(from)?;
    let to = convert::protocol(to)?;
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|_| "无法读取标准输入")?;
    let body = serde_json::from_slice(&input).map_err(|_| "请求体不是 JSON")?;
    let value = translate::translate_request(from, to, &body).map_err(|_| "请求协议转换失败")?;
    emit(&mut std::io::stdout().lock(), &value)
}
/// 角色命令只选一个职责；工具子命令不入协议循环。
const USAGE: &str =
    "用法: aproxy-format <convert|keys|route|policy|adaptive|rewrite|heartbeat|translate> [参数]";
/// 失败诊断始终写 stderr，不破坏 stdout 上的逐行协议。
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((name, rest)) if name == "translate" => translate_once(rest),
        Some((name, rest)) => Role::load(name, rest).and_then(|role| {
            worker(
                name,
                role,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )
        }),
        None => Err(USAGE),
    };
    if let Err(e) = result {
        eprintln!("aproxy-format: {e}");
        std::process::exit(1);
    }
}
