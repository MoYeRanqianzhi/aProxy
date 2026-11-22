//! 角色自己的静态故障。不能把解析器的原始输入、配置值或凭据拼进这些句子。
use aproxy_ipc::{i18n::Lang, message};
macro_rules! faults {
    ($($id:ident,$name:ident,en:$en:literal,zh:$zh:literal;)+)=>{
        $(message!($name(),en:$en,zh:$zh);)+
        /// 角色运行时的故障分类；流终止和容量判断按类别而非按英语句子分支。
        #[derive(Debug,Clone,Copy,PartialEq,Eq)]
        pub(crate) enum Fault {$(#[doc=$zh] $id,)+}
        impl Fault {
            /// 内置试跑可以用终端语言解释，真正的角色工作进程始终使用英文。
            pub(crate) const fn render(self,lang:Lang)->&'static str{match self{$(Self::$id=>$name(lang),)+}}
            /// 回复帧和转换出的错误事件都沿用同一份协议英文。
            pub(crate) const fn wire(self)->&'static str{self.render(Lang::En)}
        }
    };
}
faults! {
CredentialName, credential_name, en:"invalid credential environment variable name", zh:"密钥环境变量名无效";
CredentialMissing, credential_missing, en:"credential environment variable is unset", zh:"密钥环境变量未设置";
AuthValue, auth_value, en:"invalid authentication header value", zh:"鉴权头的值无效";
RequestJson, request_json, en:"request body is not valid JSON", zh:"请求体不是 JSON";
IdentifyClient, identify_client, en:"unknown client protocol; specify --from", zh:"无法识别客户端协议，请指定 --from";
ClientProtocol, client_protocol, en:"invalid client protocol", zh:"未知协议名称";
UpstreamUrl, upstream_url, en:"invalid upstream URL", zh:"上游地址无效";
IdentifyUpstream, identify_upstream, en:"unknown upstream protocol; configure a destination mapping", zh:"无法识别上游协议，请配置目的端映射";
UpstreamProtocol, upstream_protocol, en:"invalid upstream protocol", zh:"未知协议名称";
RequestConversion, request_conversion, en:"request protocol conversion failed", zh:"请求协议转换失败";
ProtocolState, protocol_state, en:"invalid protocol state", zh:"协议状态无效";
ClientState, client_state, en:"client protocol state is missing", zh:"响应缺少客户端协议状态";
CrossStream, cross_stream, en:"cross-protocol SSE requires the stream stage", zh:"跨协议事件流需要 stream 阶段";
ResponseJson, response_json, en:"response body is not valid JSON", zh:"响应体不是 JSON";
ResponseFields, response_fields, en:"response does not match the upstream protocol", zh:"响应缺少上游协议的必要字段";
ResponseConversion, response_conversion, en:"response protocol conversion failed", zh:"响应协议转换失败";
OpenStream, open_stream, en:"stream is not open", zh:"流未打开";
StreamEvent, stream_event, en:"unsupported stream event", zh:"不支持的流事件";
UsageCounts, usage_counts, en:"invalid upstream usage counts", zh:"上游用量计数无效";
EventSize, event_size, en:"stream event exceeds 8 MiB", zh:"单个流事件超过 8 MiB";
UpstreamStream, upstream_stream, en:"upstream stream failed", zh:"上游事件流报告失败";
StreamEnd, stream_end, en:"incomplete upstream event stream", zh:"事件流没有完整结束";
MalformedEvent, malformed_event, en:"invalid stream event", zh:"无法转换流事件";
Budget, budget, en:"response exceeds the configured limit", zh:"转换响应超过配置的限制";
NoPool, no_pool, en:"no credential pool matches the upstream origin", zh:"上游没有对应的密钥池";
Stage, stage, en:"unsupported stage", zh:"阶段不受支持";
Pointer, pointer, en:"invalid JSON Pointer target", zh:"JSON Pointer 的目标无效";
Path, path, en:"invalid preserved client path segment", zh:"保留的客户端路径包含无效路径段";
MissingModel, missing_model, en:"request body has no model", zh:"请求体缺少 model";
NoChannel, no_channel, en:"no channel matches the model", zh:"模型未命中任何渠道";
ChannelUrl, channel_url, en:"invalid channel URL", zh:"上游地址无效";
}
