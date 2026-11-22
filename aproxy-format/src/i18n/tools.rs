//! 离线检查、模板生成后的终端说明。

use aproxy_ipc::message;

message!(official(), en: "The official command requires a valid role.", zh: "官方命令缺少有效角色");

message!(base(), en: "Invalid base_url.", zh: "base_url 无效");

message!(directory_hint(), en: "Note: --home was not specified. Relative paths use the proxy configuration directory.", zh: "提示：未指定 --home，相对路径以代理配置所在目录为准。");

message!(workdir(), en: "Cannot determine the role working directory.", zh: "无法确定角色工作目录");

message!(encoding(), en: "Proxy configuration is not UTF-8.", zh: "代理配置不是 UTF-8");

message!(invalid(code: &str, line: u32, column: u32), en: "Invalid proxy configuration: {code} (line {line}, column {column})", zh: "代理配置无效：{code}（行 {line}，列 {column}）");

message!(issue(index: usize, name: &str, reason: &str), en: "Entry {index}, {name}: {reason}", zh: "第 {index} 项的 {name}：{reason}");

message!(stages(index: usize, name: &str), en: "Entry {index}, {name} does not support the declared stages.", zh: "第 {index} 项的 {name} 不支持所声明的阶段");

message!(retry(), en: "Request conversion or rewriting after routing must declare retry.", zh: "路由后的请求转换或改写必须声明 retry");

message!(request(), en: "Automatic protocol conversion on the return path requires the request stage.", zh: "自动协议转换的回程需要声明 request");

message!(require_convert(), en: "Cross-protocol conversion requires required=true.", zh: "跨协议转换必须设置 required=true");

message!(request_only(), en: "Conversion applies only to the request. The response keeps the upstream protocol.", zh: "转换仅作用于请求，响应仍是上游协议");

message!(scenario(), en: "Unknown scenario. Use claude-relay, key-rotation, openrouter, or adaptive-relay.", zh: "未知场景，可用 claude-relay、key-rotation、openrouter、adaptive-relay");

message!(mkdir(), en: "Cannot create the output directory.", zh: "无法创建输出目录");

message!(directory(), en: "Invalid output directory.", zh: "输出目录无效");

message!(role_path(), en: "Invalid role configuration path.", zh: "角色配置路径无效");

message!(exists(), en: "Output file already exists; not overwriting it.", zh: "输出文件已存在，不覆盖");

message!(create(), en: "Cannot create the output file.", zh: "无法新建输出文件");

message!(write(), en: "Cannot write the configuration.", zh: "写入配置失败");

message!(generated(path: std::path::Display<'_>), en: "Generated in {path}:", zh: "已生成（{path}）：");

message!(proxy_file(), en: "[[format]] chain for the instance configuration", zh: "实例配置里的 [[format]] 链");

message!(keys_file(), en: "keys configuration", zh: "keys 的配置");

message!(route_file(), en: "route configuration", zh: "route 的配置");

message!(adaptive_file(), en: "adaptive configuration", zh: "adaptive 的配置");

message!(next(), en: "Next steps:", zh: "下一步：");

message!(set_router(), en: "  1. Set OPENROUTER_API_KEY in the environment. Do not write the key to a file.", zh: "  1. 设置环境变量 OPENROUTER_API_KEY（密钥不写进文件）");

message!(set_base(), en: "  1. Set base_url to the relay URL.", zh: "  1. 把 base_url 改为实际中继地址");

message!(set_keys(), en: "  1. Set RELAY_KEY_A and RELAY_KEY_B in the environment. Do not write keys to a file.", zh: "  1. 设置环境变量 RELAY_KEY_A、RELAY_KEY_B（密钥不写进文件）");

message!(check_file(path: std::path::Display<'_>), en: "  2. Check: aproxy-format check \"{path}\"", zh: "  2. 检查：aproxy-format check \"{path}\"");

message!(merge(), en: "  3. Merge config.toml into the instance configuration, then start or restart the instance.", zh: "  3. 把 config.toml 的内容并入实例的配置，然后启动或重启实例");

message!(unchecked(), en: "Not checked:", zh: "没有检查：");

message!(separator(), en: ", ", zh: "、");

message!(external(), en: "  - Third-party program capabilities and configuration (programs were not started)", zh: "  - 第三方程序的能力与配置（未启动程序）");

message!(rest(), en: "  - Other instance settings, including the listen address and timeouts. Run aproxy doctor to check them.", zh: "  - 实例的其余配置（监听地址、超时等），由 aproxy doctor 检查");

message!(home_arg(), en: "--home requires a directory.", zh: "--home 缺少目录");

message!(duplicate_home(), en: "--home was specified more than once.", zh: "--home 重复");

message!(note(n: &str), en: "Note: {n}", zh: "提示：{n}");

message!(valid_role(r: &str), en: "The {r} role configuration is structurally valid.", zh: "{r} 角色配置结构有效。");

message!(usage(), en: "Usage: init SCENARIO --output DIRECTORY; check PROXY_CONFIG [--home DIRECTORY] or check ROLE --config FILE; try PROXY_CONFIG --request SAMPLE [--home DIRECTORY] [--allow-external]", zh: "用法：init 场景 --output 目录；check 代理配置 [--home 目录] 或 check 角色 --config 文件；try 代理配置 --request 样例 [--home 目录] [--allow-external]");

message!(variables(names: &str), en: "  - Whether {names} are set in the environment (needed when starting a key role or selecting a channel)", zh: "  - 环境变量 {names} 是否已设置（启动密钥角色或选用渠道时需要）");

message!(valid_chain(n: usize), en_one: "The format chain and official role configuration are valid ({n} program).", en_other: "The format chain and official role configuration are valid ({n} programs).", zh: "format 链与官方角色配置有效（{n} 个程序）。");
