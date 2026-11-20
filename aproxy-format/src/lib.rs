//! 每个角色是独立的协议 1 工作进程。首行协商、逐行应答，直到 aproxy 关闭标准输入。
//! `translate` 只读一个 JSON 请求，既不参与握手也不运行处理链。
mod adaptive;
mod config;
mod convert;
mod detect;
mod dryrun;
mod events;
mod heartbeat;
mod keys;
mod policy;
mod rewrite;
mod route;
mod selection;
mod tools;
mod translate;
use crate::config::Fault;
use aproxy_envelope as w;
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
    Rewrite(rewrite::Rewrite),
    /// 只对尝试结果投票。
    Policy(policy::Policy),
    /// 按摘要决定 forward 或 retry，并接收完成通知。
    Adaptive(adaptive::Adaptive),
    /// 只处理保活帧。
    Heartbeat(heartbeat::Heartbeat),
}
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
    fn load(name: &str, args: &[String]) -> Result<Self, String> {
        Self::configured(name, Options::parse(args)?, config::Variables::default())
    }
    /// 工具与协议进程复用同一份加载与校验；只隔离角色文件的位置和环境作用域。
    fn configured(name: &str, opt: Options<'_>, vars: config::Variables) -> Result<Self, String> {
        if name == "convert" {
            return Ok(Self::Convert(match (opt.config, opt.from, opt.to) {
                (Some(p), None, None) => {
                    let mut c: convert::Convert = config::load(p)?;
                    c.validate()?;
                    c
                }
                (None, Some(f), Some(t)) => convert::Convert::new(f, t)?,
                _ => return Err("convert 需指定 --config 或 --from 与 --to".into()),
            }));
        }
        if opt.from.is_some() || opt.to.is_some() {
            return Err("此角色不接受协议参数".into());
        }
        Ok(match name {
            "heartbeat" => Self::Heartbeat(heartbeat::Heartbeat::load(opt.config)?),
            "policy" => Self::Policy(policy::Policy::load(opt.config)?),
            "rewrite" => Self::Rewrite(rewrite::Rewrite::load(opt.config)?),
            "keys" => Self::Keys(keys::Keys::load(
                opt.config.ok_or("keys 需要 --config")?,
                &vars,
            )?),
            "route" => {
                let mut r = route::Route::load(opt.config.ok_or("route 需要 --config")?)?;
                r.variables = vars;
                Self::Route(r)
            }
            "adaptive" => Self::Adaptive(adaptive::Adaptive::load(opt.config)?),
            _ => return Err(USAGE.into()),
        })
    }
    /// 能力同时约束握手与消息分派，配置不启用的回程也不能进入角色处理器。
    fn stages(&self) -> &'static [&'static str] {
        match self {
            Self::Convert(_) => &["request", "retry", "response", "stream"],
            Self::Keys(_) | Self::Rewrite(_) => &["request", "retry"],
            Self::Route(route) => route.stages(),
            Self::Policy(_) => &["decide"],
            Self::Adaptive(_) => &["request", "decide", "notify"],
            Self::Heartbeat(_) => &["heartbeat"],
        }
    }
    /// 订阅声明是上限；实例最终只向配置选中的阶段写帧。摘要协商不能退化为默认的 full。
    fn hello(&mut self, name: &str, offer: &w::Hello) -> w::HelloReply {
        use w::BodyNeed::{Digest, Full, None};
        if let Self::Convert(c) = self {
            c.negotiate(offer.limits.as_ref());
        }
        if let Self::Route(route) = self {
            route.negotiate(&offer.overrides);
        }
        let needs: &[(&str, w::BodyNeed)] = match self {
            Self::Convert(_) | Self::Route(_) => &[("request", Full), ("retry", Full)],
            Self::Rewrite(r) if r.needs_body() => &[("request", Full), ("retry", Full)],
            Self::Keys(k) if k.sticky => &[("request", Full), ("retry", Full)],
            Self::Keys(_) | Self::Rewrite(_) => &[("request", None), ("retry", None)],
            Self::Policy(_) => &[("decide", None)],
            Self::Adaptive(_) => &[("request", Digest), ("decide", None), ("notify", Digest)],
            Self::Heartbeat(_) => &[],
        };
        let mut reply = w::HelloReply::new(
            self.stages().iter().map(|s| s.to_string()).collect(),
            name.into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        if matches!(self,Self::Policy(p) if p.needs_success())
            && offer.features.iter().any(|s| s == "decide.success")
        {
            reply.features.push("decide.success".into());
        }
        reply.request_body = needs
            .iter()
            .map(|(s, v)| (s.to_string(), v.clone()))
            .collect();
        reply
    }
    /// 只在角色声明过的方向产生补丁；业务拒绝是带 seq 的正常回信，不打断持久进程。
    fn handle(&mut self, m: &w::Message) -> w::Reply {
        let mut r = w::Reply::new(m.seq);
        let stage = match &m.stage {
            w::Stage::Request(_) => "request",
            w::Stage::Retry(_) => "retry",
            w::Stage::Response(_) => "response",
            w::Stage::Stream(_) => "stream",
            w::Stage::Decide(_) => "decide",
            w::Stage::Notify(_) => "notify",
            w::Stage::Heartbeat(_) => "heartbeat",
            _ => "",
        };
        let result = match &m.stage {
            _ if !self.stages().contains(&stage) => {
                Err(Fault::new("阶段不受支持", "unsupported stage"))
            }
            w::Stage::Request(input) => self.request(input, None, &mut r),
            w::Stage::Retry(retry) => self.request(&retry.input, Some(&retry.retry), &mut r),
            w::Stage::Response(response) => match self {
                Self::Convert(c) => c.response(response, &mut r),
                Self::Route(route) => route.response(response, &mut r),
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            w::Stage::Stream(s) => match self {
                Self::Convert(c) => c.stream(s, &mut r),
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            w::Stage::Decide(m) => match self {
                Self::Policy(p) => {
                    p.decide(m, &mut r);
                    Ok(())
                }
                Self::Adaptive(a) => {
                    a.decide(m, &mut r);
                    Ok(())
                }
                _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
            },
            w::Stage::Notify(w::NotifyEvent::RequestEnd(m))
                if matches!(self, Self::Adaptive(_)) =>
            {
                if let Self::Adaptive(a) = self {
                    a.completed(m, std::time::Instant::now());
                }
                Ok(())
            }
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
            let mut fail = w::Fail::new(false, reason.wire.into());
            if let Self::Convert(c) = self {
                let context = match &m.stage {
                    w::Stage::Response(response) => Some(&response.context),
                    w::Stage::Stream(stream) => match &stream.event {
                        w::StreamEvent::Open(open) => Some(&open.context),
                        _ => None,
                    },
                    _ => None,
                };
                fail.response = context.map(|ctx| c.error_response(ctx));
            }
            r.fail = Some(fail);
        }
        r
    }
    /// 同一请求的后续阶段可能换进程，只使用信封携带的私有状态。
    fn request(
        &mut self,
        m: &w::RequestMessage,
        retry: Option<&w::Retry>,
        r: &mut w::Reply,
    ) -> Result<(), Fault> {
        match self {
            Self::Convert(c) => c.request(m, r),
            Self::Keys(k) => k.request(m, retry, r),
            Self::Route(route) => route.request(m, retry, r),
            Self::Rewrite(rewrite) => rewrite.request(m, r),
            Self::Adaptive(a) => {
                a.request(m, r);
                Ok(())
            }
            _ => Err(Fault::new("阶段不受支持", "unsupported stage")),
        }
    }
}
/// 在指定的时间快照上重放密钥角色的标准流协议。冷却不受读取样例或外部进程调度的真实耗时影响。
pub fn replay_keys_at(
    args: &[String],
    input: impl BufRead,
    output: impl Write,
    now: std::time::Instant,
    wall: std::time::SystemTime,
) -> Result<(), String> {
    let Role::Keys(mut keys) = Role::load("keys", args)? else {
        unreachable!()
    };
    keys.sample_time = Some((now, wall));
    worker("keys", Role::Keys(keys), input, output).map_err(String::from)
}
/// 只在标准输出写一行 JSON 并立即 flush；管道断开直接结束，不循环输出错误。
fn emit(out: &mut impl Write, value: &impl serde::Serialize) -> Result<(), &'static str> {
    serde_json::to_writer(&mut *out, value).map_err(|_| "无法写入标准输出")?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|_| "无法写入标准输出")
}
/// 纯元数据帧包括 decide 预览；协商整包或订阅回程的角色须容纳经 JSON 或 base64 转义的实体。
fn input_limit(hello: &w::HelloReply) -> u64 {
    if hello.request_body.values().any(|n| *n == w::BodyNeed::Full)
        || hello
            .stages
            .iter()
            .any(|s| matches!(s.as_str(), "response" | "stream"))
    {
        257 * 1024 * 1024
    } else {
        8 * 1024 * 1024
    }
}
/// 首帧是固定 hello，后续序号只在本进程内有意义。EOF 在握手后是正常退役。
fn worker(
    name: &str,
    mut role: Role,
    mut input: impl BufRead,
    mut out: impl Write,
) -> Result<(), &'static str> {
    let mut limit = 64 * 1024;
    for next in 0u64.. {
        let mut line = Vec::new();
        // 按线上的字节数限制，而不是先读完整行（攻击性输入可以一直没有换行）。
        input
            .by_ref()
            .take(limit)
            .read_until(b'\n', &mut line)
            .map_err(|_| "无法读取标准输入")?;
        if line.is_empty() {
            return Ok(());
        }
        if !line.ends_with(b"\n") {
            return Err(if line.len() as u64 == limit {
                "协议帧超过长度上限"
            } else {
                "协议帧缺少结尾换行"
            });
        }
        let m: w::Message = serde_json::from_slice(&line).map_err(|_| "协议帧无效")?;
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
            let hello = role.hello(name, &offer);
            limit = input_limit(&hello);
            emit(&mut out, &hello)?;
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
    let body = std::str::from_utf8(&input)
        .ok()
        .and_then(|s| serde_json::from_str(s.trim()).ok())
        .ok_or("请求体不是 JSON")?;
    let value = translate::translate_request(from, to, &body).map_err(|_| "请求协议转换失败")?;
    emit(&mut std::io::stdout().lock(), &value)
}
/// 角色命令只选一个职责；工具子命令不入协议循环。
const USAGE: &str = "用法: aproxy-format <convert|keys|route|policy|adaptive|rewrite|heartbeat|init|check|try|translate> [参数]";
/// 失败诊断始终写 stderr，不破坏 stdout 上的逐行协议。
pub fn run() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((name, rest)) if matches!(name.as_str(), "init" | "check" | "try") => {
            tools::run(name, rest)
        }
        Some((name, rest)) if name == "translate" => translate_once(rest).map_err(String::from),
        Some((name, rest)) => Role::load(name, rest).and_then(|role| {
            worker(
                name,
                role,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )
            .map_err(String::from)
        }),
        None => Err(USAGE.into()),
    };
    if let Err(e) = result {
        eprintln!("aproxy-format: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
/// 在内存管道直接检查读取游标，无换行的输入不能被整份吞下后才报错。
mod frame_tests {
    use super::*;
    /// 首帧与数据帧都按换行定界，达到预算即拒绝；最后一个完整帧之后的 EOF 是正常关闭。
    #[test]
    fn bounded_frames_require_a_delimiter_before_eof() {
        let hello=br#"{"v":1,"seq":0,"stage":"hello","protocols":[1],"aproxy_version":"test","name":"role","process":"persistent","stages":[],"worker_id":0,"features":[],"overrides":[]}"#;
        let mut out = Vec::new();
        assert!(
            worker(
                "adaptive",
                Role::Adaptive(adaptive::Adaptive::load(None).unwrap()),
                &hello[..],
                &mut out
            )
            .is_err()
        );
        assert!(out.is_empty());
        let mut input = hello.to_vec();
        input.push(b'\n');
        assert!(
            worker(
                "adaptive",
                Role::Adaptive(adaptive::Adaptive::load(None).unwrap()),
                &input[..],
                Vec::new()
            )
            .is_ok()
        );
        let mut data = input.clone();
        data.extend_from_slice(br#"{"v":1,"seq":1,"stage":"request","request_id":"r","delivery":"retry","client":{"path":"/"},"request":{"method":"GET","url":"https://a.test/"}}"#);
        let mut out = Vec::new();
        assert!(
            worker(
                "adaptive",
                Role::Adaptive(adaptive::Adaptive::load(None).unwrap()),
                &data[..],
                &mut out
            )
            .is_err()
        );
        assert_eq!(out.iter().filter(|&&b| b == b'\n').count(), 1);
        // 换行本身占最后一个允许字节；下一行不应被预读或算进当前预算。
        let mut boundary = hello.to_vec();
        boundary.resize(64 * 1024 - 1, b' ');
        boundary.push(b'\n');
        let header = boundary.len();
        boundary.extend_from_slice(&data[input.len()..]);
        boundary.resize(header + (8 << 20) - 1, b' ');
        boundary.push(b'\n');
        let adaptive = || Role::Adaptive(adaptive::Adaptive::load(None).unwrap());
        assert!(worker("adaptive", adaptive(), &boundary[..], Vec::new()).is_ok());
        boundary.insert(boundary.len() - 1, b' ');
        assert_eq!(
            worker("adaptive", adaptive(), &boundary[..], Vec::new()),
            Err("协议帧超过长度上限")
        );
        let mut convert =
            Role::Convert(convert::Convert::new("openai_chat", "openai_chat").unwrap());
        let msg: w::Message = serde_json::from_slice(hello).unwrap();
        let w::Stage::Hello(offer) = msg.stage else {
            unreachable!()
        };
        assert_eq!(input_limit(&convert.hello("convert", &offer)), 257 << 20);
        assert_eq!(input_limit(&adaptive().hello("adaptive", &offer)), 8 << 20);
        let mut full = Vec::new();
        assert!(worker("convert", convert, &boundary[..], &mut full).is_ok());
        assert_eq!(full.iter().filter(|&&b| b == b'\n').count(), 2);
        for (prefix, cap) in [(&[][..], 64 * 1024), (&input[..], 8 * 1024 * 1024)] {
            let mut over = prefix.to_vec();
            over.resize(prefix.len() + cap + 4096, b' ');
            let mut cursor = std::io::Cursor::new(over);
            assert!(
                worker(
                    "adaptive",
                    Role::Adaptive(adaptive::Adaptive::load(None).unwrap()),
                    &mut cursor,
                    Vec::new()
                )
                .is_err()
            );
            assert!(cursor.position() as usize <= prefix.len() + cap + 1);
        }
    }
}
