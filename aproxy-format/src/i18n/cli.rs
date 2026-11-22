//! 面向终端的用法与安全的配置诊断；共享程序路径由调用端明确选语言。
use aproxy_ipc::message;

message!(file(),en:"Offline input must be a readable regular local file.",zh:"离线输入必须是可读的本地普通文件");

message!(size(),en:"Offline input exceeds 8 MiB.",zh:"离线输入超过 8 MiB");

message!(utf8(),en:"Role configuration is not UTF-8.",zh:"角色配置不是 UTF-8");

message!(invalid_role(),en:"Invalid role configuration.",zh:"角色配置无效");

message!(field(),en:"Invalid role configuration field: ",zh:"角色配置字段无效：");

message!(missing_value(),en:"Missing option value.",zh:"参数缺少值");

message!(unknown_option(),en:"Unknown option.",zh:"未知参数");

message!(duplicate(),en:"An option is duplicated or empty.",zh:"参数重复或为空");

message!(stdout(),en:"Cannot write to stdout.",zh:"无法写入标准输出");

message!(translate_usage(),en:"Usage: aproxy-format translate --from <protocol> --to <protocol>",zh:"用法: aproxy-format translate --from <协议> --to <协议>");

message!(stdin(),en:"Cannot read stdin.",zh:"无法读取标准输入");

message!(unknown_protocol(),en:"Unknown protocol name.",zh:"未知协议名称");

message!(usage(),en:"Usage: aproxy-format <convert|keys|route|policy|adaptive|rewrite|heartbeat|init|check|try|translate> [arguments]",zh:"用法: aproxy-format <convert|keys|route|policy|adaptive|rewrite|heartbeat|init|check|try|translate> [参数]");

message!(help(name:&str,usage:&str,options:&str,example:&str),en:"Usage: aproxy-format {name} {usage}\n{options}\n-h, --help  Show this help.\nExample: aproxy-format {name} {example}",zh:"用法：aproxy-format {name} {usage}\n{options}\n-h、--help 显示本说明。\n示例：aproxy-format {name} {example}");

message!(convert_usage(),en:"(--from CLIENT_PROTOCOL --to UPSTREAM_PROTOCOL | --config FILE)",zh:"(--from 客户端协议 --to 上游协议 | --config 文件)");

message!(convert_help(),en:"Use --from and --to to choose protocols, including auto. Use --config for a TOML file with protocols and conversion limits.",zh:"--from、--to 选择两端协议，可用 auto；--config 改用 TOML 指定协议与转换预算。");

message!(translate_args(),en:"--from SOURCE_PROTOCOL --to TARGET_PROTOCOL",zh:"--from 源协议 --to 目标协议");

message!(translate_help(),en:"Use anthropic_messages, openai_chat, or openai_responses. Read one JSON request from stdin and write the converted result to stdout. Use convert in a proxy chain, check to validate it, and try for a dry run.",zh:"--from、--to 可用 anthropic_messages、openai_chat、openai_responses。从标准输入读取一个 JSON 请求，向标准输出写入转换结果。代理链使用 convert，验证用 check，试跑用 try。");

message!(config_file(),en:"--config FILE",zh:"--config 文件");

message!(keys_help(),en:"--config specifies credential pools, origins, and environment variable names.",zh:"--config 指定密钥池、来源及环境变量名。");

message!(route_help(),en:"--config specifies channels, model matching, and routing rules for retries.",zh:"--config 指定渠道、模型匹配与重试选路规则。");

message!(config_optional(),en:"[--config FILE]",zh:"[--config 文件]");

message!(rewrite_help(),en:"--config specifies rules for rewriting models, headers, and JSON fields.",zh:"--config 指定模型、请求头和 JSON 字段的改写规则。");

message!(heartbeat_help(),en:"--config specifies the heartbeat frame format.",zh:"--config 指定保活帧的样式。");

message!(adaptive_help(),en:"--config specifies failure thresholds and recovery conditions.",zh:"--config 指定故障阈值与恢复条件。");

message!(policy_help(),en:"--config specifies rules for deciding what to do after each attempt.",zh:"--config 指定尝试结束时的判定规则。");

message!(init_args(),en:"SCENARIO --output DIRECTORY",zh:"场景 --output 目录");

message!(init_help(),en:"--output selects the output directory. Existing files are not overwritten. Scenarios: claude-relay, key-rotation, openrouter, adaptive-relay.",zh:"--output 指定输出目录，不覆盖已有文件。场景：claude-relay、key-rotation、openrouter、adaptive-relay。");

message!(check_args(),en:"PROXY_CONFIG [--home DIRECTORY] or check ROLE --config FILE [--home DIRECTORY]",zh:"代理配置 [--home 目录] 或 check 角色 --config 文件 [--home 目录]");

message!(check_help(),en:"--config checks one role file. --home sets the instance working directory; otherwise relative chain paths use the proxy configuration directory. This checks structure, not credential availability.",zh:"--config 指定单个角色文件；--home 指定实例工作目录。未指定时，链内相对路径以代理配置所在目录为准。只检查结构，不验证密钥是否可用。");

message!(try_args(),en:"PROXY_CONFIG --request SAMPLE [--home DIRECTORY] [--allow-external]",zh:"代理配置 --request 样例 [--home 目录] [--allow-external]");

message!(try_help(),en:"--request supplies a JSON request and simulated upstream results; --home sets the instance working directory. The dry run does not open network connections. --allow-external starts third-party programs with access to the sample, environment, and network. An unsimulated step exits with code 2 without guessing the outcome.",zh:"--request 指定 JSON 请求与模拟上游结果；--home 指定实例工作目录。试跑工具自身不打开网络连接。--allow-external 启动第三方程序，它会收到样例和环境变量，能任意使用它们，包括访问网络。遇到未模拟的步骤时退出码为 2，不继续输出推测结果。");

message!(no_protocol_options(),en:"this role does not accept protocol options",zh:"此角色不接受协议参数");
