//! 每个角色是独立的协议 1 工作进程。首行协商、逐行应答，直到 aproxy 关闭标准输入。
//! `translate` 只读一个 JSON 请求，既不参与握手也不运行处理链。
/// 帮助、工具用法与安全的文件诊断。
#[path = "i18n/cli.rs"]
mod text;
use aproxy_ipc::i18n::{Lang, current};
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
/// 角色故障共用一份说明；协议帧及工作进程 stderr 明确选择英文。
#[path = "i18n/role.rs"]
mod role_text;
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
    fn parse(args: &'a [String], lang: Lang) -> Result<Self, &'static str> {
        let mut result = Self::default();
        let mut iter = args.iter().map(String::as_str);
        while let Some(arg) = iter.next() {
            let (name, value) = match arg.split_once('=') {
                Some(pair) => pair,
                None => (arg, iter.next().ok_or(text::missing_value(lang))?),
            };
            let slot = match name {
                "--config" => &mut result.config,
                "--from" => &mut result.from,
                "--to" => &mut result.to,
                _ => return Err(text::unknown_option(lang)),
            };
            if value.is_empty() || slot.replace(value).is_some() {
                return Err(text::duplicate(lang));
            }
        }
        Ok(result)
    }
}
impl Role {
    /// 握手前完成静态配置检查，不会用半套配置处理请求。
    fn load(name: &str, args: &[String]) -> Result<Self, String> {
        let opt = Options::parse(args, Lang::En)?;
        let file = opt.config.map(config::Source::Worker);
        Self::configured(name, opt, file, config::Variables::default())
            .map_err(config::LoadIssue::worker)
    }
    /// 工具与协议进程复用同一份加载与校验；只隔离角色文件的位置和环境作用域。
    fn configured(
        name: &str,
        opt: Options<'_>,
        file: Option<config::Source<'_>>,
        vars: config::Variables,
    ) -> Result<Self, config::LoadIssue> {
        if name == "convert" {
            return Ok(Self::Convert(match (file, opt.from, opt.to) {
                (Some(p), None, None) => {
                    let mut c: convert::Convert = config::load(p)?;
                    c.validate()?;
                    c
                }
                (None, Some(f), Some(t)) => convert::Convert::new(f, t)?,
                _ => return Err("convert requires --config or both --from and --to".into()),
            }));
        }
        if opt.from.is_some() || opt.to.is_some() {
            return Err(text::no_protocol_options(Lang::En).into());
        }
        Ok(match name {
            "heartbeat" => Self::Heartbeat(heartbeat::Heartbeat::load(file)?),
            "policy" => Self::Policy(policy::Policy::load(file)?),
            "rewrite" => Self::Rewrite(rewrite::Rewrite::load(file)?),
            "keys" => Self::Keys(keys::Keys::load(
                file.ok_or("keys requires --config")?,
                &vars,
            )?),
            "route" => {
                let mut r = route::Route::load(file.ok_or("route requires --config")?)?;
                r.variables = vars;
                Self::Route(r)
            }
            "adaptive" => Self::Adaptive(adaptive::Adaptive::load(file)?),
            _ => return Err(text::usage(Lang::En).into()),
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
    fn handle(&mut self, m: &w::Message, diagnostic_lang: aproxy_ipc::i18n::Lang) -> w::Reply {
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
            _ if !self.stages().contains(&stage) => Err(Fault::Stage),
            w::Stage::Request(input) => self.request(input, None, &mut r),
            w::Stage::Retry(retry) => self.request(&retry.input, Some(&retry.retry), &mut r),
            w::Stage::Response(response) => match self {
                Self::Convert(c) => c.response(response, &mut r),
                Self::Route(route) => route.response(response, &mut r),
                _ => Err(Fault::Stage),
            },
            w::Stage::Stream(s) => match self {
                Self::Convert(c) => c.stream(s, &mut r),
                _ => Err(Fault::Stage),
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
                _ => Err(Fault::Stage),
            },
            w::Stage::Notify(w::NotifyEvent::RequestEnd(m))
                if matches!(self, Self::Adaptive(_)) =>
            {
                if let Self::Adaptive(a) = self {
                    a.completed(m, a.sample_time.unwrap_or_else(std::time::Instant::now));
                }
                Ok(())
            }
            w::Stage::Heartbeat(h) => match self {
                Self::Heartbeat(heartbeat) => {
                    heartbeat.beat(h, &mut r);
                    Ok(())
                }
                _ => Err(Fault::Stage),
            },
            _ => Err(Fault::Stage),
        };
        if let Err(reason) = result {
            r = w::Reply::new(m.seq);
            let _ = writeln!(
                std::io::stderr().lock(),
                "aproxy-format: {}",
                reason.render(diagnostic_lang)
            );
            let mut fail = w::Fail::new(false, reason.wire().into());
            if let Self::Convert(c) = self {
                let context = match &m.stage {
                    w::Stage::Response(response) => Some(&response.context),
                    w::Stage::Stream(stream) => match &stream.event {
                        w::StreamEvent::Open(open) => Some(&open.context),
                        _ => None,
                    },
                    _ => None,
                };
                fail.response = context.map(|ctx| c.error_response(ctx, &reason));
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
            _ => Err(Fault::Stage),
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
fn emit(
    out: &mut impl Write,
    value: &impl serde::Serialize,
    lang: Lang,
) -> Result<(), &'static str> {
    serde_json::to_writer(&mut *out, value).map_err(|_| text::stdout(lang))?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|_| text::stdout(lang))
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
            .map_err(|_| text::stdin(Lang::En))?;
        if line.is_empty() {
            return Ok(());
        }
        if !line.ends_with(b"\n") {
            return Err(if line.len() as u64 == limit {
                "protocol frame exceeds the length limit"
            } else {
                "protocol frame is missing its terminating newline"
            });
        }
        let m: w::Message = serde_json::from_slice(&line).map_err(|_| "invalid protocol frame")?;
        if m.v != 1 || m.seq != next {
            return Err("protocol version or sequence number mismatch");
        }
        if next == 0 {
            let w::Stage::Hello(offer) = m.stage else {
                return Err("the first frame must be hello");
            };
            if !offer.protocols.contains(&1) {
                return Err("unsupported protocol version");
            }
            let hello = role.hello(name, &offer);
            limit = input_limit(&hello);
            emit(&mut out, &hello, Lang::En)?;
        } else {
            if matches!(m.stage, w::Stage::Hello(_)) {
                return Err("duplicate hello");
            }
            emit(&mut out, &role.handle(&m, Lang::En), Lang::En)?;
        }
    }
    Ok(())
}
/// 无信封的一次性协议转换。出错只给诊断类别，不回显含消息或密钥的原始输入。
fn translate_once(args: &[String], lang: Lang) -> Result<(), &'static str> {
    let opt = Options::parse(args, lang)?;
    let (None, Some(from), Some(to)) = (opt.config, opt.from, opt.to) else {
        return Err(text::translate_usage(lang));
    };
    let from = convert::protocol(from).map_err(|_| text::unknown_protocol(lang))?;
    let to = convert::protocol(to).map_err(|_| text::unknown_protocol(lang))?;
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|_| text::stdin(lang))?;
    let body = std::str::from_utf8(&input)
        .ok()
        .and_then(|s| serde_json::from_str(s.trim()).ok())
        .ok_or(Fault::RequestJson.render(lang))?;
    let value = translate::translate_request(from, to, &body)
        .map_err(|_| Fault::RequestConversion.render(lang))?;
    emit(&mut std::io::stdout().lock(), &value, lang)
}
/// 同一入口处理帮助，不触碰配置或标准流协议；示例可独立阅读，不要求先启动实例。
fn help(name: &str, lang: Lang) -> Result<(), String> {
    let (usage, options, example) = match name {
        "convert" => (
            text::convert_usage(lang),
            text::convert_help(lang),
            "--from anthropic_messages --to openai_chat",
        ),
        "translate" => (
            text::translate_args(lang),
            text::translate_help(lang),
            "--from openai_chat --to openai_responses < request.json",
        ),
        "keys" => (
            text::config_file(lang),
            text::keys_help(lang),
            "--config keys.toml",
        ),
        "route" => (
            text::config_file(lang),
            text::route_help(lang),
            "--config route.toml",
        ),
        "rewrite" => (
            text::config_optional(lang),
            text::rewrite_help(lang),
            "--config rewrite.toml",
        ),
        "heartbeat" => (
            text::config_optional(lang),
            text::heartbeat_help(lang),
            "--config heartbeat.toml",
        ),
        "adaptive" => (
            text::config_optional(lang),
            text::adaptive_help(lang),
            "--config adaptive.toml",
        ),
        "policy" => (
            text::config_optional(lang),
            text::policy_help(lang),
            "--config policy.toml",
        ),
        "init" => (
            text::init_args(lang),
            text::init_help(lang),
            "key-rotation --output relay",
        ),
        "check" => (
            text::check_args(lang),
            text::check_help(lang),
            "proxy.toml --home instance",
        ),
        "try" => (
            text::try_args(lang),
            text::try_help(lang),
            "proxy.toml --request sample.json --home instance",
        ),
        _ => return Err(text::usage(lang).into()),
    };
    println!("{}", text::help(lang, name, usage, options, example));
    Ok(())
}
/// 普通参数或输入错误与试跑明确不模拟的路径有不同的退出状态，调用者不需解析中文文案。
#[derive(Debug)]
enum CliError {
    /// 命令或角色无法处理输入。
    Invalid(String),
    /// 已到达需要真实实例才能给出答案的步骤。
    Unsimulated(String),
}
impl From<String> for CliError {
    fn from(s: String) -> Self {
        Self::Invalid(s)
    }
}
impl From<&str> for CliError {
    fn from(s: &str) -> Self {
        Self::Invalid(s.into())
    }
}
impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => f.write_str(s),
            Self::Unsimulated(s) => write!(f, "未模拟：{s}"),
        }
    }
}
/// 失败诊断始终写 stderr，不破坏 stdout 上的逐行协议。
pub fn run() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // 协议角色只写给程序，不读取调用者的终端偏好。显式查角色帮助仍属于终端操作。
    let is_role = matches!(
        args.first().map(String::as_str),
        Some("adaptive" | "convert" | "heartbeat" | "keys" | "policy" | "rewrite" | "route")
    );
    if !is_role
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        aproxy_ipc::i18n::init_terminal(|| aproxy_ipc::home::resolve_home().ok());
    }
    let result = match args.split_first() {
        Some((name, [])) if name == "--help" || name == "-h" => {
            println!("{}", text::usage(current()));
            Ok(())
        }
        Some((name, rest))
            if rest
                .iter()
                .any(|arg| matches!(arg.as_str(), "--help" | "-h")) =>
        {
            help(name, current()).map_err(CliError::from)
        }
        Some((name, rest)) if matches!(name.as_str(), "init" | "check" | "try") => {
            tools::run(name, rest)
        }
        Some((name, rest)) if name == "translate" => {
            translate_once(rest, current()).map_err(CliError::from)
        }
        Some((_, rest)) if !is_role => Options::parse(rest, current())
            .map_err(CliError::from)
            .and_then(|opt| {
                Err(if opt.from.is_some() || opt.to.is_some() {
                    text::no_protocol_options(current())
                } else {
                    text::usage(current())
                }
                .into())
            }),
        Some((name, rest)) => Role::load(name, rest)
            .and_then(|role| {
                worker(
                    name,
                    role,
                    std::io::stdin().lock(),
                    std::io::stdout().lock(),
                )
                .map_err(String::from)
            })
            .map_err(CliError::from),
        None => Err(text::usage(current()).into()),
    };
    if let Err(e) = result {
        eprintln!("aproxy-format: {e}");
        std::process::exit(if matches!(e, CliError::Unsimulated(_)) {
            2
        } else {
            1
        });
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
            Err("protocol frame exceeds the length limit")
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
