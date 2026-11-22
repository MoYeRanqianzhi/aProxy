//! 离线试跑的终端诊断，与标准输出的结构化事件分开。

use aproxy_ipc::message;

message!(write(), en: "Cannot write the dry-run result.", zh: "无法写出试跑结果");

message!(restart(), en: "Cannot restart the role.", zh: "角色不能重新启动");

message!(frame(), en: "Cannot build a message for this stage from the sample.", zh: "样例不能构成该阶段的消息");

message!(reply_size(), en: "Dry-run reply exceeds the instance limit.", zh: "试跑回信超过实例上限");

message!(disconnected(), en: "External program is not connected.", zh: "外部程序未连接");

message!(invalid_peer(), en: "Invalid external program reply.", zh: "外部程序回信无效");

message!(sequence(), en: "Role reply sequence number does not match.", zh: "角色回信序号不符");

message!(reply(), en: "Invalid HTTP metadata or state in the role reply.", zh: "角色回信的 HTTP 元数据或状态无效");

message!(headers_limit(), en: "Reply headers exceed the limit.", zh: "回信头表超过限制");

message!(control(), en: "record the failure and continue using this control stage's fallback rules", zh: "记录故障，按此控制阶段的回退规则继续");

message!(mandatory(), en: "record the required program's failure and retry or end the request according to the delivery stage", zh: "记录必需程序故障，按交付阶段重试或结束请求");

message!(optional(), en: "record the optional program's failure and continue using this stage's bypass rules", zh: "记录可选程序故障并按该阶段的旁路规则继续");

message!(worker(stage: &str, behavior: &str), en: "Worker failure, replacement, and circuit breaking at {stage}; a live instance would {behavior}.", zh: "{stage} 阶段的工作进程故障、替换与熔断；真实实例会{behavior}");

message!(body(), en: "Cannot encode the sample body.", zh: "样例正文无法编码");

message!(preview(), en: "Decoded preview over 8 MiB; a live instance would scan the full entity as a stream.", zh: "超过 8 MiB 的解码预览；真实实例会流式扫描完整实体");

message!(multistream(), en: "Interruption or failure in a multi-layer stream; a live instance would propagate an abort, fall back, or end delivery according to its open stream leases.", zh: "多层流的中断或失败；真实实例会按已打开的流租约传递中止、回退或终止交付");

message!(rejected(), en: "The stream role rejected the sample data.", zh: "流角色拒绝样例数据");

message!(encoded(), en: "Encoded terminal response after a heartbeat was committed; a live instance would strictly decode the response before choosing the terminal event.", zh: "已提交保活后的编码终态；真实实例会先严格解码再决定终态事件");

message!(external(), en: "The dry-run tool itself does not open network connections. External programs receive the sample and environment variables and may use them freely, including over the network. Run only programs you trust.", zh: "注意：试跑工具自身不打开网络连接。外部程序会收到样例和环境变量，能任意使用它们，包括访问网络；只运行你信任的程序。");

message!(sample(), en: "The dry-run sample is not a valid request description.", zh: "试跑样例不是有效的请求描述");

message!(base(), en: "The dry run requires base_url.", zh: "试跑需要 base_url");

message!(path(), en: "Invalid dry-run request path.", zh: "试跑请求路径无效");

message!(request_size(), en: "Sample request body exceeds the instance max_request_mb limit (413).", zh: "样例请求正文超过实例的 max_request_mb（413）");

message!(method(), en: "Invalid sample HTTP method.", zh: "样例的 HTTP 方法无效");

message!(header_name(), en: "Invalid sample header name.", zh: "样例头名称无效");

message!(header_value(), en: "Invalid sample header value.", zh: "样例头值无效");

message!(relative(), en: "Running a relative external command requires --home. No program was started.", zh: "运行外部相对命令需要 --home；未启动程序");

message!(incompatible(), en: "Incompatible dry-run handshake. Check the protocol and stages.", zh: "试跑握手不兼容，请检查协议和阶段");

message!(handshake(), en: "Dry-run handshake failed.", zh: "试跑握手失败");

message!(attempt(), en: "Invalid status, failure, or entity in the simulated attempt.", zh: "模拟尝试的状态、失败或实体无效");

message!(duplicate_header(), en: "Sample response headers contain a case-insensitive duplicate.", zh: "样例存在大小写重复的响应头");

message!(unfinished(), en: "A buffered layer read an incomplete pass-through entity; a live instance would end delivery according to the actual stream failure.", zh: "整包层读取未完成的直通实体；真实实例会按实际流故障结束交付");

message!(mixed(), en: "A mixed return chain was interrupted before commitment; a live instance would release its open streams before returning control to the decision stage.", zh: "混合回程链在提交前中断；真实实例会先释放已打开的流，再交回决定层");

message!(clock(), en: "The logical clock exceeded its range; a live instance would wait for the configured backoff.", zh: "逻辑时钟超出可表示范围；真实实例会按配置的退避等待");

message!(timeout(), en: "External program did not reply in time.", zh: "外部程序未按时回答");

message!(locate(), en: "Cannot locate the external program.", zh: "无法定位外部程序");

message!(spawn(), en: "Cannot start the external program.", zh: "无法启动外部程序");

message!(unsimulated(reason:&str),en:"Not simulated: {reason}",zh:"未模拟：{reason}");
