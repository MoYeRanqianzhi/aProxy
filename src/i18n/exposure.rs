//! 监听地址和注入凭据决定暴露风险；终端与日志各自决定使用哪种语言。

use aproxy_ipc::message;

message!(credentials(addr: &str, port: &str), en: "Listening address {addr} is not a loopback address. This instance injects an api_key or an Authorization / x-api-key header into every incoming request. Any host that can reach this port can use your key at your expense. For local use, listen on 127.0.0.1:{port}.", zh: "监听地址 {addr} 不是回环地址，且本实例配置了 api_key（或 Authorization / x-api-key 鉴权头）：代理会把它注入每一个入站请求——任何能连到该端口的主机都能用你的 key 调用上游、费用记在你的账上，等于把 key 共享给整个局域网。只给本机用请改回 127.0.0.1:{port}");

message!(exposed(addr: &str, port: &str), en: "Listening address {addr} is not a loopback address. Any host that can reach this port can access the upstream through this proxy. For local use, listen on 127.0.0.1:{port}. Only expose it on a trusted network.", zh: "监听地址 {addr} 不是回环地址：局域网/同网段内任何能连到该端口的主机都能经本代理访问上游。只给本机用请改回 127.0.0.1:{port}；确需对外开放请只在可信网络中使用");
