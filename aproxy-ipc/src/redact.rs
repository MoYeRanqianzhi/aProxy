//! 打码的唯一出口：密钥只写不读。
//!
//! aproxy 的一切读出口——控制通道、`config show`、`--json`、诊断、日志、错误消息、请求
//! 记录，以及界面——交出去的内容都先经过这里。集中在一处，新增的出口才不会各写一套、漏掉
//! 一种。
//!
//! 四样东西：
//! - [`url`]：URL 里的 userinfo、查询串的值与片段；
//! - [`is_secret_header`]：哪些请求头的值算密钥；
//! - [`placeholder`] 与 [`placeholder_text`]：密钥字段读出来时给的占位——只说明「这里有
//!   一个密钥」，够长时附首尾各 4 个字符的提示，供人辨认是哪一把；
//! - [`Secrets`] 与 [`StreamRedactor`]：把 aproxy 自己知道的密钥原文从一段文字或一个字节流
//!   里换成 `<secret>`（请求记录的体、format 的 stderr 等，密钥可能出现在任何位置）。

use serde_json::{Value, json};

/// 已知密钥被替换成的文字，也是没有提示时占位的文本形式。
pub const SECRET_TEXT: &str = "<secret>";

/// 短于这个长度（字节）的密钥不参与「已知密钥替换」：太短的串在正文里随处可见，替换
/// 只会把无关的内容弄花。
pub const MIN_KNOWN_SECRET_LEN: usize = 8;

/// 值不短于这个长度（字符）时，占位才附提示：短密钥露出首尾 8 个字符就所剩无几了。
pub const HINT_MIN_CHARS: usize = 24;

/// URL 凭据脱敏——所有「可能被粘贴分享」的输出面（守护日志、status 的最近
/// 错误、start/config 的控制台输出、实例注册表）展示 URL 时的**唯一出口**：
/// base_url、代理 URL、上游目标 URL、请求的 `路径?查询串`、上游库的错误里的 URL
/// 一律走它。
///
/// 规则：
/// - userinfo：用户名或密码任一存在即整体替换为 `***@`——令牌常以用户名形态
///   出现（`https://TOKEN@host`），只遮密码不够
/// - 查询串：保留键名，非空值替换为 `***`（`?key=***&beta=***`）——Gemini 式
///   客户端把 key 放在 `?key=` 里
/// - 片段：非空即整体替换为 `***`
/// - 其余部分逐字保留（不经 URL 解析器重新序列化，不会凭空多出尾斜杠等，
///   展示与用户写的一致）
///
/// 输入可以是完整 URL，也可以是以 `/` 开头的 `路径?查询串`。对解析失败的
/// 畸形 URL（如密码里含未编码的 `/` `?`）退回保守策略：最后一个 `@` 之前的
/// 内容全部视为 userinfo 遮掉——宁可多遮，也不按「看似合法」的切分把密码
/// 后半段漏出来。结果幂等（对输出再脱敏不变），下游重复调用安全。
///
/// 「合法」的判断见 [`authority_is_well_formed`]，它比 URL 标准严：国际化域名、带 `%` 的
/// 主机名、不是四段点分十进制的数字主机也当作畸形。这些 URL 只有在路径或查询串里还带着
/// `@` 时结果才不同——会多遮一段，不会少遮。
pub fn url(raw: &str) -> String {
    // scheme 必须是合法的 scheme 记号（字母开头，仅字母数字 + - .）——否则
    // `/v1/x?key=S&r=http://y` 这类查询串里带 `://` 的路径会被误认成 URL，
    // 整段查询串（含 key 的值）原样漏出
    let scheme_end = raw.find("://").filter(|&i| {
        let s = &raw[..i];
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let (scheme, rest) = match scheme_end {
        Some(i) => raw.split_at(i + 3),
        None => ("", raw),
    };
    // userinfo 的范围：合法 URL 的 authority 止于第一个 `/` `?` `#`（路径里的
    // `@` 合法且常见，如 Vertex 的 `models/claude-x@20250101`，不能误伤）；
    // `/` 开头的纯路径没有 authority；其余（解析失败的 URL、无 scheme 的
    // `user:pass@host`）按最后一个 `@` 保守切分
    let userinfo_end = if scheme.is_empty() && rest.starts_with('/') {
        None
    } else if !scheme.is_empty() && authority_is_well_formed(rest) {
        let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        rest[..authority_len].rfind('@')
    } else {
        rest.rfind('@')
    };
    let (userinfo_mask, rest) = match userinfo_end {
        Some(at) => ("***@", &rest[at + 1..]),
        None => ("", rest),
    };
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f)),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let mut out = String::with_capacity(raw.len());
    out.push_str(scheme);
    out.push_str(userinfo_mask);
    out.push_str(path);
    if let Some(q) = query {
        out.push('?');
        let pairs: Vec<String> = q
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, v)) if !v.is_empty() => format!("{k}=***"),
                _ => pair.to_string(),
            })
            .collect();
        out.push_str(&pairs.join("&"));
    }
    if let Some(f) = fragment {
        out.push('#');
        if !f.is_empty() {
            out.push_str("***");
        }
    }
    out
}

/// `rest` 是 `scheme://` 之后的部分。它的 authority（到第一个 `/`、`?`、`#` 为止）是否
/// 写得规整：`[userinfo@]host[:port]`，host 非空且不含不该出现在主机名里的字符，port 是
/// 0–65535 的十进制数（可以为空）；`[…]` 形式的 IPv6 地址照此处理。
///
/// 不规整多半意味着切错了地方——典型是密码里带着没编码的 `/` 或 `?`，authority 被它提前
/// 截断，后半段密码落进了「路径」。调用方据此改用保守的切法。宁严勿宽：判错成不规整只是
/// 多遮一些。
fn authority_is_well_formed(rest: &str) -> bool {
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host_port = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let Some((inside, after)) = bracketed.split_once(']') else {
            return false;
        };
        let port = match after.strip_prefix(':') {
            Some(port) => port,
            None if after.is_empty() => "",
            None => return false,
        };
        if inside.is_empty()
            || !inside
                .chars()
                .all(|c| c.is_ascii_hexdigit() || ":.".contains(c))
        {
            return false;
        }
        (inside, port)
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => (host_port, ""),
        }
    };
    // 方括号里的地址上面已经查过；普通主机名只认 ASCII，且不含不该出现的字符
    let host_ok = host_port.starts_with('[')
        || (!host.is_empty()
            && host.is_ascii()
            && !host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || r"#%/:<>?@[\]^|".contains(c))
            // 国际化域名的 ASCII 形式（xn-- 开头的段）要解码才知道合不合法，这里不解码，一律
            // 当作不规整
            && !host.split('.').any(|label| label.to_ascii_lowercase().starts_with("xn--"))
            && numeric_host_is_ipv4(host));
    let port_ok = port.is_empty()
        || (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok());
    host_ok && port_ok
}

/// 主机名的最后一段是数字（`1.2.3.4`、`0x7f.1`）时，它得是一个合法的点分十进制 IPv4 地址；
/// 最后一段不是数字的（普通域名）不受这条限制。URL 标准把「以数字结尾」的主机一律当 IPv4
/// 解析，解析不了就是非法 URL。
fn numeric_host_is_ipv4(host: &str) -> bool {
    let last = host
        .trim_end_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or_default();
    let numeric = !last.is_empty()
        && (last.bytes().all(|b| b.is_ascii_digit())
            || last.starts_with("0x")
            || last.starts_with("0X"));
    !numeric || host.parse::<std::net::Ipv4Addr>().is_ok()
}

/// 这个请求头的值是不是密钥（「密钥头」）。名字不分大小写：`authorization`、
/// `proxy-authorization`、`cookie`、`set-cookie`，以及名字里含 `key`、`token`、`secret`、
/// `auth`、`session`、`signature` 的头。
///
/// 按名字里的词判断会多遮一些无害的头（例如 `x-keyboard-layout`），这是有意的：漏遮一个
/// 凭据的代价远大于少显示一个值。
pub fn is_secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "cookie" | "set-cookie")
        || ["key", "token", "secret", "auth", "session", "signature"]
            .iter()
            .any(|word| name.contains(word))
}

/// 密钥值的提示：值不短于 [`HINT_MIN_CHARS`] 个字符时为首 4 个字符、省略号、尾 4 个字符
/// （`sk-a…f3a9`），否则没有提示。按字符取，不会切在多字节字符中间。
pub fn hint(secret: &str) -> Option<String> {
    let chars: Vec<char> = secret.chars().collect();
    (chars.len() >= HINT_MIN_CHARS).then(|| {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    })
}

/// 密钥字段读出来时的占位对象：`{"secret":true}`，够长时带 `"hint"`。
///
/// 把占位对象原样写回等于不改这个字段（见 [`is_placeholder`]）。
pub fn placeholder(secret: &str) -> Value {
    match hint(secret) {
        Some(hint) => json!({"secret": true, "hint": hint}),
        None => json!({"secret": true}),
    }
}

/// 占位的文本形式：`<secret>`，够长时为 `<secret sk-a…f3a9>`。
pub fn placeholder_text(secret: &str) -> String {
    match hint(secret) {
        Some(hint) => format!("<secret {hint}>"),
        None => SECRET_TEXT.to_string(),
    }
}

/// 这个 JSON 值是不是占位对象（一个 `secret` 为 true 的对象）。写回配置时，占位对象表示
/// 「保持原值」。
pub fn is_placeholder(value: &Value) -> bool {
    value.get("secret").and_then(Value::as_bool) == Some(true)
}

/// aproxy 自己知道的密钥原文的集合，用来把它们从文字或字节流里换成 [`SECRET_TEXT`]。
///
/// 收什么由调用方决定（`api_key`、代理口令、format 的 `env` 值、鉴权头的值等）；短于
/// [`MIN_KNOWN_SECRET_LEN`] 字节的不收。第三方程序自己配置里的密钥这里无从知道，替换不到。
#[derive(Debug, Clone)]
pub struct Secrets {
    /// 去重后的密钥，按长度从长到短：同一位置既能匹配长的也能匹配短的时，换掉长的那个
    known: Vec<Vec<u8>>,
    /// 以这个字节开头的密钥是否存在：扫描时先查它，绝大多数位置一步就过
    first_bytes: [bool; 256],
}

impl Default for Secrets {
    /// 一个密钥都没有的集合
    fn default() -> Self {
        Secrets {
            known: Vec::new(),
            first_bytes: [false; 256],
        }
    }
}

impl Secrets {
    /// 由一组密钥原文建立。空串与过短的忽略，重复的只留一个。
    pub fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<[u8]>,
    {
        let mut known: Vec<Vec<u8>> = secrets
            .into_iter()
            .map(|s| s.as_ref().to_vec())
            .filter(|s| s.len() >= MIN_KNOWN_SECRET_LEN)
            .collect();
        known.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        known.dedup();
        let mut first_bytes = [false; 256];
        for secret in &known {
            first_bytes[usize::from(secret[0])] = true;
        }
        Secrets { known, first_bytes }
    }

    /// 一个密钥都没有：替换什么也不做
    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    /// 最长的密钥的长度（字节）；没有密钥时为 0
    fn max_len(&self) -> usize {
        self.known.first().map_or(0, Vec::len)
    }

    /// `haystack[at..]` 开头处匹配的密钥的长度（取最长的）
    fn match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        if !self.first_bytes[usize::from(haystack[at])] {
            return None;
        }
        self.known
            .iter()
            .find(|secret| haystack[at..].starts_with(secret))
            .map(Vec::len)
    }

    /// 把 `bytes` 里出现的每一处已知密钥换成 [`SECRET_TEXT`]。
    pub fn replace_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            match self.match_at(bytes, at) {
                Some(len) => {
                    out.extend_from_slice(SECRET_TEXT.as_bytes());
                    at += len;
                }
                None => {
                    out.push(bytes[at]);
                    at += 1;
                }
            }
        }
        out
    }

    /// 把 `text` 里出现的每一处已知密钥换成 [`SECRET_TEXT`]。
    pub fn replace(&self, text: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        // 密钥若不是合法 UTF-8 的整段，换掉它可能把两侧的字符切开，所以按有损方式转回
        String::from_utf8_lossy(&self.replace_bytes(text.as_bytes())).into_owned()
    }
}

/// 对分块到达的字节流做已知密钥替换。
///
/// 密钥可能被块的边界切开，所以每次留住末尾「最长密钥的长度减 1」字节不交出去，等下一块
/// 到了（或流结束）再一起处理。用法：每到一块调 [`StreamRedactor::feed`]，流结束时调
/// [`StreamRedactor::finish`]，把两者返回的字节依次接起来就是替换后的全文。
#[derive(Debug, Clone)]
pub struct StreamRedactor {
    secrets: Secrets,
    /// 上一块留下、还没交出去的尾巴
    held: Vec<u8>,
}

impl StreamRedactor {
    pub fn new(secrets: Secrets) -> Self {
        StreamRedactor {
            secrets,
            held: Vec::new(),
        }
    }

    /// 喂进一块，返回现在可以交出去的字节（已替换）。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.secrets.is_empty() {
            return chunk.to_vec();
        }
        self.held.extend_from_slice(chunk);
        let buffer = std::mem::take(&mut self.held);
        // 从这里往后的位置上，可能有一个还没到齐的密钥正要开始：留到下一次
        let hold_from = buffer.len().saturating_sub(self.secrets.max_len() - 1);
        let mut out = Vec::with_capacity(buffer.len());
        let mut at = 0;
        while at < hold_from {
            match self.secrets.match_at(&buffer, at) {
                // 整个密钥已在缓冲里（哪怕它越过了 hold_from）：现在就换掉
                Some(len) => {
                    out.extend_from_slice(SECRET_TEXT.as_bytes());
                    at += len;
                }
                None => {
                    out.push(buffer[at]);
                    at += 1;
                }
            }
        }
        self.held = buffer[at..].to_vec();
        out
    }

    /// 流结束：交出留着的尾巴（已替换）。
    pub fn finish(mut self) -> Vec<u8> {
        let held = std::mem::take(&mut self.held);
        self.secrets.replace_bytes(&held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- url：URL 凭据脱敏 ----

    #[test]
    fn mask_url_masks_any_userinfo() {
        // 无凭据：逐字原样（不经解析器重新序列化，不凭空多出尾斜杠）
        assert_eq!(url("https://api.example.com"), "https://api.example.com");
        assert_eq!(url("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(url(""), "");
        // 用户名 + 密码
        assert_eq!(
            url("http://alice:secret@127.0.0.1:7890"),
            "http://***@127.0.0.1:7890"
        );
        // 仅用户名也打码：令牌常以用户名形态出现（https://TOKEN@host）
        assert_eq!(
            url("https://TOKENUSER@api.example.com/v1"),
            "https://***@api.example.com/v1"
        );
        assert_eq!(
            url("socks5://alice@127.0.0.1:1080"),
            "socks5://***@127.0.0.1:1080"
        );
    }

    #[test]
    fn mask_url_keeps_query_keys_and_masks_values() {
        assert_eq!(url("http://alice:pw@h:1?p=x"), "http://***@h:1?p=***");
        assert_eq!(
            url("http://127.0.0.1:9/v1/models?key=QKEY999&a=1&flag&empty="),
            "http://127.0.0.1:9/v1/models?key=***&a=***&flag&empty="
        );
        // 值里含 `=`：只按第一个 `=` 切分，键名完整保留
        assert_eq!(url("/x?sig=a=b=c"), "/x?sig=***");
        // 纯路径（日志里的 path 字段）
        assert_eq!(url("/v1/messages?beta=true"), "/v1/messages?beta=***");
        assert_eq!(url("/v1/messages"), "/v1/messages");
        // 片段整体遮掉
        assert_eq!(url("https://h/p#tok=abc"), "https://h/p#***");
    }

    #[test]
    fn mask_url_does_not_mangle_at_sign_in_path() {
        // 路径里的 `@` 合法（Vertex 的模型 ID 形如 claude-x@20250101）：不得被
        // 当成 userinfo 分隔符把主机名遮掉
        let vertex = "https://us-east5-aiplatform.googleapis.com/v1/projects/p/models/claude-3@20240229:streamRawPredict";
        assert_eq!(url(vertex), vertex);
        assert_eq!(
            url("/v1/models/claude@2024?key=K"),
            "/v1/models/claude@2024?key=***"
        );
    }

    #[test]
    fn mask_url_query_containing_scheme_is_still_masked() {
        // 回归：查询串里出现 `://` 时不得被误认成 URL 的 scheme 分隔，否则整段
        // 查询串（含 key 的值）原样漏出
        assert_eq!(url("/v1/x?key=SECRET&r=http://y"), "/v1/x?key=***&r=***");
    }

    #[test]
    fn mask_url_malformed_inputs_fail_safe() {
        // 畸形 URL（密码含未编码的 `/` `?`，解析失败）：最后一个 `@` 之前全部遮掉，
        // 绝不按「看似合法」的切分漏出密码后半段
        let out = url("http://user:pa/ss@host:7890");
        assert_eq!(out, "http://***@host:7890");
        let out = url("http://user:pa?ss@host");
        assert!(!out.contains("pa") && !out.contains("ss@"), "{out}");
        // 缺 scheme 的 user:pass@host（validate 会拒，但错误消息与 --show 仍要展示）
        assert_eq!(url("user:pass@127.0.0.1:7890"), "***@127.0.0.1:7890");
        // scheme 打错（http//）同样不漏
        let out = url("http//user:pass@host");
        assert!(!out.contains("pass"), "{out}");
        // 无凭据的非 URL 原样
        assert_eq!(url("not a url"), "not a url");
    }

    #[test]
    fn mask_url_is_idempotent() {
        // 注册表里已是脱敏值，status 展示时会再脱敏一次：结果必须不变
        for raw in [
            "http://alice:secret@127.0.0.1:7890/v1?key=K&a=1#frag",
            "https://TOKEN@api.example.com",
            "/v1/x?key=SECRET",
            "user:pass@host",
        ] {
            let once = url(raw);
            assert_eq!(url(&once), once, "幂等失败: {raw}");
        }
    }

    // ---- 监听地址回环判定与非回环告警 ----

    #[test]
    fn authority_check_matches_the_cases_that_matter() {
        for good in [
            "api.example.com",
            "api.example.com/v1",
            "alice:pw@127.0.0.1:7890",
            "h:",
            "[::1]:8080/x",
            "[::1]",
            "user@host?x=1",
        ] {
            assert!(authority_is_well_formed(good), "{good}");
        }
        for bad in [
            "",
            "/path",
            "user:pa/ss@host:7890",
            "user:pa?ss@host",
            "host:99999",
            "ho st/x",
            "[::1",
            "[::1]x",
            // 以数字结尾的主机得是合法的 IPv4；国际化域名与带 % 的主机一律当不规整
            "1.2.3.999",
            "1.2.3",
            "0x7f.1",
            "xn--a",
            "api.XN--fsq.example",
            "例子.测试",
            "ex%61mple.com",
        ] {
            assert!(!authority_is_well_formed(bad), "{bad}");
        }
    }

    #[test]
    fn secret_headers_by_name() {
        for name in [
            "Authorization",
            "proxy-authorization",
            "Cookie",
            "Set-Cookie",
            "x-api-key",
            "X-Goog-Api-Key",
            "anthropic-auth-token",
            "x-session-id",
            "X-Signature",
            "x-client-secret",
        ] {
            assert!(is_secret_header(name), "{name}");
        }
        for name in ["content-type", "anthropic-version", "user-agent", "accept"] {
            assert!(!is_secret_header(name), "{name}");
        }
    }

    #[test]
    fn placeholders() {
        // 短密钥：不给提示，一个字符也不露
        assert_eq!(placeholder("short-key"), json!({"secret": true}));
        assert_eq!(placeholder_text("short-key"), "<secret>");
        assert_eq!(hint(&"x".repeat(HINT_MIN_CHARS - 1)), None);
        // 够长：首 4 尾 4
        let key = "sk-ant-0123456789abcdef-f3a9";
        assert_eq!(hint(key).as_deref(), Some("sk-a…f3a9"));
        assert_eq!(
            placeholder(key),
            json!({"secret": true, "hint": "sk-a…f3a9"})
        );
        assert_eq!(placeholder_text(key), "<secret sk-a…f3a9>");
        // 按字符取，多字节字符不会被切开
        let wide = "密".repeat(HINT_MIN_CHARS);
        assert_eq!(hint(&wide).as_deref(), Some("密密密密…密密密密"));
        // 占位对象的识别
        assert!(is_placeholder(&placeholder(key)));
        assert!(is_placeholder(&json!({"secret": true})));
        assert!(!is_placeholder(&json!({"secret": false})));
        assert!(!is_placeholder(&json!("sk-real")));
        assert!(!is_placeholder(&json!({"hint": "x"})));
    }

    #[test]
    fn known_secrets_are_replaced_everywhere() {
        let secrets = Secrets::new(["sk-secret-one", "sk-secret-one-longer", "tiny", ""]);
        // 过短的与空串不收
        assert_eq!(secrets.known.len(), 2);
        assert_eq!(
            secrets.replace("a sk-secret-one b sk-secret-one-longer c tiny"),
            "a <secret> b <secret> c tiny"
        );
        // 首尾相接、紧挨着的也都换掉
        assert_eq!(
            secrets.replace("sk-secret-onesk-secret-one"),
            "<secret><secret>"
        );
        assert_eq!(secrets.replace("nothing here"), "nothing here");
        assert!(Secrets::new(["short"]).is_empty());
        assert_eq!(Secrets::default().replace("sk-secret-one"), "sk-secret-one");
    }

    /// 无论块怎样切，流式替换的结果都与整段替换相同。
    #[test]
    fn stream_redaction_equals_whole_redaction_for_any_chunking() {
        let secrets = Secrets::new(["sk-secret-one", "0123456789abcdef"]);
        let text = "head sk-secret-one mid 0123456789abcdef sk-secret-onX 0123456789abcde tail sk-secret-one";
        let whole = secrets.replace(text);
        assert_eq!(
            whole,
            "head <secret> mid <secret> sk-secret-onX 0123456789abcde tail <secret>"
        );
        for size in 1..=text.len() {
            let mut redactor = StreamRedactor::new(secrets.clone());
            let mut out = Vec::new();
            for chunk in text.as_bytes().chunks(size) {
                let piece = redactor.feed(chunk);
                // 交出去的每一段里都不含密钥原文
                assert!(!String::from_utf8_lossy(&piece).contains("sk-secret-one"));
                out.extend(piece);
            }
            out.extend(redactor.finish());
            assert_eq!(String::from_utf8(out).unwrap(), whole, "块大小 {size}");
        }
    }

    #[test]
    fn stream_redaction_without_secrets_passes_through() {
        let mut redactor = StreamRedactor::new(Secrets::default());
        assert_eq!(redactor.feed(b"abc"), b"abc");
        assert_eq!(redactor.finish(), b"");
    }
}
