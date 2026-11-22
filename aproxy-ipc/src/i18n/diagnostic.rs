//! 解析器只保留事实，不替稍后才读取诊断的人选择语言。公共文档中的 `message`
//! 始终是同一原因的英文投影；只有私有的安装状态可以单独保存带参数的原因。

use super::Lang;
use serde::{Deserialize, Serialize, Serializer};

type Nested = Box<Reason>;

// 存储用自有值，排版借用字符串；嵌套原因在同一读者语言下展开。
macro_rules! param_type {
    (String) => {
        &str
    };
    (Nested) => {
        &str
    };
    ($n:ident) => {
        $n
    };
}
macro_rules! param_value {
    ($lang:ident, $v:ident, String) => {
        $v.as_str()
    };
    ($lang:ident, $v:ident, Nested) => {
        &$v.render($lang)
    };
    ($lang:ident, $v:ident, $n:ident) => {
        *$v
    };
}
macro_rules! phrase {
    ($lang:ident, (), $en:literal, $zh:literal) => {{
        crate::message!(sentence(), en: $en, zh: $zh);
        sentence($lang).to_owned()
    }};
    ($lang:ident, ($($arg:ident : $ty:ident),+), $en:literal, $zh:literal) => {{
        crate::message!(sentence($($arg: param_type!($ty)),+), en: $en, zh: $zh);
        sentence($lang, $(param_value!($lang, $arg, $ty)),+)
    }};
}

macro_rules! reasons {
    ($($id:ident $({ $($arg:ident : $ty:ident),+ })?, en: $en:literal, zh: $zh:literal;)+) => {
        /// 同一个诊断码下的具体原因。变体名只供本程序保留排版所需的事实，不是线协议码。
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(tag = "id", rename_all = "snake_case")]
        pub enum Reason {
            $( #[doc = $en] $id $({ $($arg: $ty),+ })?, )+
            /// 默认直通时依赖的程序可能不可用；与码表共用一句英文。
            ForwardUnguarded,
            /// 响应程序需要收集完整响应；与码表共用一句英文。
            ForwardBuffered,
        }
        impl Reason {
            /// 由最终显示端选语言，递归使用同一种语言排版嵌套原因。
            pub fn render(&self, lang: Lang) -> String {
                match self {
                    $( Self::$id $({ $($arg),+ })? => phrase!(lang, ($($($arg: $ty),+)?), $en, $zh), )+
                    Self::ForwardUnguarded => forward_default_unguarded(lang).to_owned(),
                    Self::ForwardBuffered => forward_response_buffered(lang).to_owned(),
                }
            }
        }
    };
}

crate::message!(forward_default_unguarded(), en: "The default delivery is forward while the chain has a decide program or an optional request program: when they are unavailable, requests are not retried.", zh: "默认投递方式为 forward，且处理链含有 decide 程序或可选的 request 程序：这些程序不可用时，请求不会重试。");
crate::message!(forward_response_buffered(), en: "The default delivery is forward and the chain has a response program; that layer gathers each response, so those responses are not streamed. Declare stream instead where the program supports it.", zh: "默认投递方式为 forward，且处理链含有 response 程序；该层会收集完整响应，因此这些响应不会流式传送。如果程序支持，请改为声明 stream。");

reasons! {
    AlwaysTrigger,
        en: "keepalive.trigger = \"always\" cannot be set in the config file: it would break non-streaming clients. Only a format can ask for it, per request",
        zh: "配置文件不能设置 keepalive.trigger = \"always\"，否则非流式客户端无法工作。只有 format 才能按请求指定此值";
    BodyMatcher,
        en: "the body matcher of retry rules is not implemented in this version",
        zh: "此版本尚不支持重试规则的 body 匹配器";
    RetryLimit,
        en: "a retry rule needs max_attempts (0 means no limit)",
        zh: "重试规则需要 max_attempts（0 表示不限制）";
    MissingCommand,
        en: "a format needs command",
        zh: "format 必须指定 command";
    EmptyCommand,
        en: "command must not be empty",
        zh: "command 不能为空";
    BatchCommand,
        en: "batch files cannot be format commands; put the interpreter in command and the script in args",
        zh: "不能将批处理文件用作 format 命令；请将解释器写入 command，将脚本写入 args";
    FormatName,
        en: "set name to 1–64 UTF-8 bytes without control characters",
        zh: "name 应为 1–64 个 UTF-8 字节，不能包含控制字符";
    MissingStages,
        en: "a format needs stages",
        zh: "format 必须指定 stages";
    EmptyStages,
        en: "stages must name at least one stage",
        zh: "stages 至少要指定一个阶段";
    AdmitStage,
        en: "the admit stage is not implemented in this version",
        zh: "此版本尚不支持 admit 阶段";
    EmptyPool,
        en: "pool_max must be at least 1",
        zh: "pool_max 至少为 1";
    BreakerCooldown,
        en: "breaker_cooldown_secs must not be 0; set breaker_failures = 0 to turn the breaker off",
        zh: "breaker_cooldown_secs 不能为 0；要关闭熔断器，请设 breaker_failures = 0";
    StageConflict,
        en: "a format cannot declare both response and stream",
        zh: "同一个 format 不能同时声明 response 和 stream";
    PersistentStage,
        en: "a format that declares stream or notify must use process = \"persistent\"",
        zh: "声明 stream 或 notify 的 format 必须使用 process = \"persistent\"";
    TomlSyntax { error: String },
        en: "TOML syntax error: {error}",
        zh: "TOML 语法错误：{error}";
    Trigger { value: String },
        en: "keepalive.trigger must be \"accept\", \"body_stream\" or \"any\", not {value:?}",
        zh: "keepalive.trigger 必须为 \"accept\"、\"body_stream\" 或 \"any\"，不能为 {value:?}";
    Regex { error: String },
        en: "not a valid regular expression: {error}",
        zh: "正则表达式无效：{error}";
    HttpStatus { code: i64 },
        en: "{code} is not an HTTP status code (100-599)",
        zh: "{code} 不是有效的 HTTP 状态码（100–599）";
    UnknownStage { stage: String },
        en: "{stage:?} is not a stage; the stages are request, retry, decide, response, stream, heartbeat and notify",
        zh: "{stage:?} 不是有效阶段；可用阶段为 request、retry、decide、response、stream、heartbeat 和 notify";
    DuplicateFormat { name: String },
        en: "two formats are named {name:?}; names must be unique in the list",
        zh: "两个 format 都名为 {name:?}；列表中的名称必须唯一";
    RetryChain { name: String, earlier: String },
        en: "format {name:?} comes after {earlier:?}, which declares retry, so it must declare retry too: on a retry the earlier format produces a new request and this one has to process it again",
        zh: "format {name:?} 位于声明了 retry 的 {earlier:?} 之后，因此也必须声明 retry：重试时前一个 format 会生成新的请求，后一个必须重新处理该请求";
    TooLarge { value: u64 },
        en: "{value} is too large",
        zh: "{value} 太大";
    Negative { key: String },
        en: "{key} must not be negative",
        zh: "{key} 不能为负数";
    OneOf { key: String, choices: String, value: String },
        en: "{key} must be one of {choices}, not {value:?}",
        zh: "{key} 必须为 {choices} 之一，不能为 {value:?}";
    UnknownKey { key: String },
        en: "unknown key {key}",
        zh: "未知键 {key}";
    ExpectedString { key: String },
        en: "{key} must be a string",
        zh: "{key} 必须为字符串";
    ExpectedNullableString { key: String },
        en: "{key} must be a string or null",
        zh: "{key} 必须为字符串或 null";
    ExpectedBoolean { key: String },
        en: "{key} must be true or false",
        zh: "{key} 必须为 true 或 false";
    ExpectedUnsigned { key: String },
        en: "{key} must be a non-negative integer",
        zh: "{key} 必须为非负整数";
    ExpectedObject { key: String },
        en: "{key} must be an object",
        zh: "{key} 必须为对象";
    ExpectedStrings { key: String },
        en: "{key} must be an array of strings",
        zh: "{key} 必须为字符串数组";
    ExpectedInteger { key: String },
        en: "{key} must be an integer",
        zh: "{key} 必须为整数";
    ExpectedIntegers { key: String },
        en: "{key} must be an array of integers",
        zh: "{key} 必须为整数数组";
    ExpectedStringTable { key: String },
        en: "{key} must be a table of strings",
        zh: "{key} 必须为值为字符串的表";
    ExpectedTable { key: String },
        en: "{key} must be a table",
        zh: "{key} 必须为表";
    ExpectedTables { key: String },
        en: "{key} must be an array of tables",
        zh: "{key} 必须为表数组";
    SettingsObject,
        en: "settings.json must be a JSON object",
        zh: "settings.json 必须为 JSON 对象";
    HeartbeatMinimum,
        en: "watchdog.heartbeat_secs must be at least 1",
        zh: "watchdog.heartbeat_secs 至少为 1";
    StaleMinimum,
        en: "watchdog.stale_after_cycles must be at least 1",
        zh: "watchdog.stale_after_cycles 至少为 1";
    UiObject,
        en: "ui must be an object",
        zh: "ui 必须为对象";
    DefaultsBase,
        en: "defaults has an api_key or a credential header but no base_url: set defaults.base_url first, so the credential is only sent to that upstream",
        zh: "defaults 中有 api_key 或凭据头，但没有 base_url：请先设置 defaults.base_url，以免将凭据发给其他上游";
    DefaultsProxy,
        en: "defaults has proxy credentials but no proxy: set defaults.proxy first",
        zh: "defaults 中有代理凭据，但未指定代理：请先设置 defaults.proxy";
    JsonSyntax { error: String },
        en: "JSON syntax error: {error}",
        zh: "JSON 语法错误：{error}";
    NewerSchema { schema: u64, supported: u64 },
        en: "settings.json has schema {schema}, written by a newer aproxy; this version reads schema {supported}. Upgrade aproxy, or go back to the version you need with `aproxy install --force <version>`",
        zh: "settings.json 使用较新版本 aproxy 写入的 schema {schema}，此版本只能读取 schema {supported}。请升级 aproxy，或运行 `aproxy install --force <version>` 回到需要的版本";
    Schema { supported: u64 },
        en: "schema must be {supported}",
        zh: "schema 必须为 {supported}";
    Language { language: String },
        en: "language must be \"system\", \"en\" or \"zh-CN\", not {language:?}",
        zh: "language 必须为 \"system\"、\"en\" 或 \"zh-CN\"，不能为 {language:?}";
    UnknownDownloadSource { other: String },
        en: "unknown download source \"{other}\" (expected github, npm, cargo-binstall, cargo or {{\"url\": \"template\"}})",
        zh: "未知下载渠道 \"{other}\"（取值：github | npm | cargo-binstall | cargo，或 {{\"url\": \"模板\"}}）";
    DownloadChain { error: Nested },
        en: "update.download_chain must be an array of \"github\", \"npm\", \"cargo-binstall\", \"cargo\" or {{\"url\": …}}: {error}",
        zh: "update.download_chain 必须为数组，元素为 \"github\"、\"npm\"、\"cargo-binstall\"、\"cargo\" 或 {{\"url\": …}}：{error}";
    BadAlias { name: String, reason: Nested },
        en: "{name:?} cannot be used as an alias: {reason}",
        zh: "{name:?} 不能用作别名：{reason}";
    EmptyAliasPath { name: String },
        en: "alias {name:?} points at an empty path",
        zh: "别名 {name:?} 指向空路径";
    InstanceDefault { name: String },
        en: "{name} cannot have a global default: it belongs to a single instance, so set it in that instance's config.toml",
        zh: "{name} 属于单个实例，不能设置全局默认值；请在该实例的 config.toml 中设置";
    TomlValue { key: String },
        en: "{key} has a value that a config key cannot hold (null, or a number out of range)",
        zh: "{key} 的值不能写入配置（值为 null 或数字超出范围）";
    AliasEmpty,
        en: "it is empty",
        zh: "别名不能为空";
    AliasNumeric,
        en: "a number is read as a port",
        zh: "别名不能是纯数字（会与端口号解析冲突）";
    AliasAll,
        en: "it is a reserved target of stop and status",
        zh: "\"all\" 是 stop 的保留目标（停止全部实例）";
    AliasIdle,
        en: "it is a reserved target of stop and status",
        zh: "\"idle\" 是 stop/status 的保留目标（空闲实例筛选）";
    AliasDefault,
        en: "it is reserved for the default config",
        zh: "\"default\" 指代默认配置文件（settings 的 default_config），是保留字";
    AliasTypo,
        en: "it is reserved for the default config",
        zh: "\"defult\" 是 \"default\" 的常见笔误，两者都保留以免混淆";
    AliasReserved { why: Nested },
        en: "{why}",
        zh: "{why}，不能用作别名";
    MissingBase,
        en: "base_url is not set: it is the upstream API this instance forwards to",
        zh: "未设置 base_url：这是此实例转发请求的上游 API 地址";
    BaseScheme { base: String },
        en: "base_url must start with http:// or https://, not {base}",
        zh: "base_url 必须以 http:// 或 https:// 开头，不能为 {base}";
    BaseQuery { base: String },
        en: "base_url must not contain ? or #: the request path is appended to it, so the path would end up in the query ({base})",
        zh: "base_url 不能包含 ? 或 #；请求路径会追加到地址末尾，否则会落入查询部分（{base}）";
    ListenPort { addr: String },
        en: "listen_addr has no port or the port is not valid: {addr} (example: 127.0.0.1:12345)",
        zh: "listen_addr 缺少端口或端口无效：{addr}（例如：127.0.0.1:12345）";
    ProxyUrl { shown: String, error: String },
        en: "proxy is not a URL ({shown}): {error}",
        zh: "proxy 不是 URL（{shown}）：{error}";
    ProxyScheme { shown: String },
        en: "proxy must be an http, https, socks4 or socks5 URL, not {shown}",
        zh: "proxy 必须为 http、https、socks4 或 socks5 URL，不能为 {shown}";
    ProxyHost { shown: String },
        en: "proxy has no host: {shown}",
        zh: "proxy 缺少主机名：{shown}";
    ProxyMissing,
        en: "proxy_username or proxy_password is set but proxy is not",
        zh: "已设置 proxy_username 或 proxy_password，但未设置 proxy";
    KeyHeader,
        en: "api_key contains characters that cannot be sent in an HTTP header",
        zh: "api_key 含有无法在 HTTP 请求头中发送的字符";
    HeaderName { name: String },
        en: "{name:?} is not a valid HTTP header name",
        zh: "{name:?} 不是有效的 HTTP 头名称";
    HeaderValue { name: String },
        en: "the value of header {name} contains characters that cannot be sent in an HTTP header",
        zh: "头 {name} 的值含有无法在 HTTP 头中发送的字符";
    EmptyHeartbeat,
        en: "keepalive.heartbeat must not be empty (set keepalive.interval_secs = 0 to turn heartbeats off)",
        zh: "keepalive.heartbeat 不能为空（要关闭心跳，请设 keepalive.interval_secs = 0）";
    HeartbeatLine,
        en: "keepalive.heartbeat must end with a line break, or the half line joins the first replayed line",
        zh: "keepalive.heartbeat 必须以换行符结尾，否则最后一行会与回放的第一行相连";
    HeartbeatEvent,
        en: "keepalive.heartbeat must end with a blank line when it has field lines such as event: or data:, or it merges with the first replayed event",
        zh: "含有 event: 或 data: 等字段行时，keepalive.heartbeat 必须以空行结尾，否则会与回放的第一个事件合并";
    Override { message: Nested },
        en: "{message} (given on the command line)",
        zh: "{message}（来自命令行）";
    NotSet,
        en: "not set",
        zh: "未设置";
    Verbatim { text: String },
        en: "{text}",
        zh: "{text}";
    Withheld { key: String, what: String, default: String, target: Nested, file: String },
        en: "the default {key} was not inherited: it belongs to the default {what} ({default}), and this config's {what} is {target}. This config has no {key} of its own; set one in {file} if the target needs it",
        zh: "未继承默认的 {key}：它属于默认的 {what}（{default}），而本配置的 {what} 为 {target}。本配置没有自己的 {key}；如果目标需要，请在 {file} 中设置";
    NoStoredSecret { key: String },
        en: "{key} was written back as a secret placeholder, but there is no stored value to keep there; enter the secret again",
        zh: "{key} 被写成了密钥占位符，但没有可保留的已存值；请重新输入密钥";
    ReadFile { error: String },
        en: "cannot read the file: {error}",
        zh: "无法读取文件：{error}";
    UnreadableRecord,
        en: "record cannot be read; it is left in place",
        zh: "无法读取记录；文件已原样保留";
    SharedHeader { name: String },
        en: "header {name} is sent to every target; if it carries a credential, move it into the configs that need it",
        zh: "头 {name} 会发送给每个目标；如果它携带凭据，请将它移入需要此凭据的配置";
    HangDelay { n: u64, delay: u128 },
        en: "watchdog.heartbeat_secs is {n}: a hung instance is detected only after about {delay} seconds",
        zh: "watchdog.heartbeat_secs 为 {n}：约 {delay} 秒后才能发现实例无响应";
    RestartsDisabled,
        en: "watchdog.max_restarts is 0: crashed or hung instances are not restarted",
        zh: "watchdog.max_restarts 为 0：不会重启崩溃或无响应的实例";
    MissingDir { path: String },
        en: "config directory {path} does not exist",
        zh: "配置目录 {path} 不存在";
    MissingConfig,
        en: "config file does not exist",
        zh: "配置文件不存在";
    UnreadableConfig { error: String },
        en: "config file cannot be read: {error}",
        zh: "无法读取配置文件：{error}";
    FilePermissions { who: String },
        en: "the group or others can access this file ({who})",
        zh: "同组或其他用户可以访问此文件（{who}）";
    SharedPort { port: u16, files: String },
        en: "port {port} is also used by {files}; these configs cannot run at the same time",
        zh: "{files} 也使用端口 {port}；这些配置不能同时运行";
    WithAliases { message: Nested, aliases: String },
        en: "{message} (alias {aliases})",
        zh: "{message}（别名 {aliases}）";
    OwnedHome { who: String },
        en: "the group or others can access the home ({who}); aproxy tightens it on its next write",
        zh: "同组或其他用户可以访问主目录（{who}）；aproxy 将在下次写入时收紧权限";
    OtherHome { who: String },
        en: "the group or others can access the home ({who}); it belongs to another user, so aproxy leaves it alone",
        zh: "同组或其他用户可以访问主目录（{who}）；目录属于其他用户，aproxy 不会修改其权限";
    ExposedKey { addr: String },
        en: "listen_addr {addr} is not a loopback address: any host that can reach this port can use the proxy, and the API key it injects",
        zh: "listen_addr {addr} 不是回环地址：任何能访问此端口的主机都能使用代理及其注入的 API 密钥";
    ExposedProxy { addr: String },
        en: "listen_addr {addr} is not a loopback address: any host that can reach this port can use the proxy",
        zh: "listen_addr {addr} 不是回环地址：任何能访问此端口的主机都能使用代理";
    RecordRead { error: String },
        en: "cannot read: {error}",
        zh: "无法读取：{error}";
    RecordParse { error: String },
        en: "not a valid record: {error}",
        zh: "记录无效：{error}";
    RecordSchema { schema: u64 },
        en: "written by a newer aproxy (schema {schema})",
        zh: "由较新版本的 aproxy 写入（schema {schema}）";
    InstallMissing,
        en: "The configuration file does not exist.",
        zh: "配置文件不存在。";
    InstallRead { error: String },
        en: "Cannot read the configuration file: {error}",
        zh: "无法读取配置文件：{error}";
}

impl Reason {
    /// 公共诊断只携带英文句子，不能泄露私有的排版标识或改变字段类型。
    pub(crate) fn serialize_english<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}
