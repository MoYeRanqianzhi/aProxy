//! 转换程序的握手诊断；程序名已脱敏，协议故障不附上不受信的回复正文。

use aproxy_ipc::message;

message!(open(), en: "circuit breaker open: calls to this program are blocked for now", zh: "熔断器已打开");

message!(spawn(), en: "could not start the program", zh: "无法启动程序");

message!(io(), en: "worker pipe I/O failed", zh: "工作进程管道读写失败");

message!(died(), en: "worker exited before replying", zh: "工作进程在应答前退出");

message!(timeout(), en: "worker reply timed out", zh: "等待工作进程应答超时");

message!(protocol(), en: "invalid protocol version, framing or sequence number", zh: "协议版本、帧边界或序号无效");

message!(invalid(), en: "invalid worker reply", zh: "工作进程应答无效");

message!(stages(), en: "worker does not support the configured stages", zh: "工作进程不支持配置的阶段");

message!(handshake(name: &str, cause: &str, code: &str), en: "format {name:?}: {cause} [{code}]", zh: "格式程序 {name:?}：{cause} [{code}]");
