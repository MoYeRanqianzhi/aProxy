//! 可编辑的场景模板；两种语言只改变 TOML 注释。
use aproxy_ipc::message;

message!(openrouter(), en:r#"# Upstream URL for OpenRouter. If you use your own gateway, also update route.toml.
base_url = 'https://openrouter.ai'
"#, zh:r#"# OpenRouter 的上游地址；使用自建网关时同步修改 route.toml。
base_url = 'https://openrouter.ai'
"#);

message!(relay(), en:r#"# Set the relay URL. If you use a key pool, also update origin in keys.toml.
base_url = 'https://relay.example.com'
"#, zh:r#"# 替换为实际中继地址；使用密钥池时也要修改 keys.toml 的 origin。
base_url = 'https://relay.example.com'
"#);

message!(timeouts(), en:r#"
[timeouts]
# For slow models, increase the first-byte timeout in seconds.
first_byte_secs = 600

[[retry.rules]]
# Limit attempts only for the token-count endpoint. Other requests use the instance policy.
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3
"#, zh:r#"
[timeouts]
# 慢模型可适当延长首字节时限，单位为秒。
first_byte_secs = 600

[[retry.rules]]
# 只限制计数接口的尝试次数，其他请求沿用实例策略。
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3
"#);

message!(threshold(), en:r#"# Switch delivery mode after this many consecutive failures. Adjust for the reliability of your relay.
failure_threshold = 2
"#, zh:r#"# 连续失败达到次数后切换交付模式；按中继稳定性调整。
failure_threshold = 2
"#);

message!(adaptive(path:&str), en:r#"
# Choose delivery mode based on recent failures. Set the threshold in adaptive.toml.
[[format]]
name = 'adaptive'
command = 'aproxy-format'
args = ['adaptive', '--config', {path}]
process = 'persistent'
stages = ['request', 'decide', 'notify']
required = false
pool_max = 1
worker_idle_secs = 0
"#, zh:r#"
# 根据近期失败情况选择交付模式；阈值在 adaptive.toml 中设置。
[[format]]
name = 'adaptive'
command = 'aproxy-format'
args = ['adaptive', '--config', {path}]
process = 'persistent'
stages = ['request', 'decide', 'notify']
required = false
pool_max = 1
worker_idle_secs = 0
"#);

message!(channel(), en:r#"[[channel]]
# Name this channel to distinguish it from other upstreams.
id = 'openrouter'
# Set the full upstream URL that accepts Chat Completions.
url = 'https://openrouter.ai/api/v1/chat/completions'
# Enter the name of the credential environment variable, not the key itself.
credential_env = 'OPENROUTER_API_KEY'
"#, zh:r#"[[channel]]
# 渠道标识用来区分候选上游，可自行命名。
id = 'openrouter'
# 替换为接收 Chat Completions 的完整上游地址。
url = 'https://openrouter.ai/api/v1/chat/completions'
# 只写存放凭据的环境变量名，不要填入密钥本身。
credential_env = 'OPENROUTER_API_KEY'
"#);

message!(route(path:&str), en:r#"
# Select a channel from route.toml and select again on retries.
[[format]]
name = 'route'
command = 'aproxy-format'
args = ['route', '--config', {path}]
process = 'persistent'
stages = ['request', 'retry']
"#, zh:r#"
# 选用 route.toml 中的渠道；重试时重新选择。
[[format]]
name = 'route'
command = 'aproxy-format'
args = ['route', '--config', {path}]
process = 'persistent'
stages = ['request', 'retry']
"#);

message!(convert(from:&str, stages:&str), en:r#"
# Convert between the client and relay protocols. Set --from and --to for these endpoints.
[[format]]
name = 'convert'
command = 'aproxy-format'
args = ['convert', '--from', '{from}', '--to', 'openai_chat']
process = 'persistent'
stages = [{stages}]
required = true
"#, zh:r#"
# 在客户端与中继的协议间转换；按两端协议修改 --from 和 --to。
[[format]]
name = 'convert'
command = 'aproxy-format'
args = ['convert', '--from', '{from}', '--to', 'openai_chat']
process = 'persistent'
stages = [{stages}]
required = true
"#);

message!(pool(), en:r#"[[pool]]
# Set the origin allowed to receive these keys. Its scheme, host, and port must match the relay.
origin = 'https://relay.example.com'
# Set the authentication header and prefix required by the relay.
header = 'authorization'
scheme = 'Bearer'
keys = [
    # First key: edit the ID and variable name. Set the value in the environment.
    {{ id = 'a', env = 'RELAY_KEY_A' }},
    # Second key: add or remove entries as needed. Do not put keys in this file.
    {{ id = 'b', env = 'RELAY_KEY_B' }},
]
"#, zh:r#"[[pool]]
# 替换为允许接收这些密钥的来源，须与实际中继的协议、主机和端口一致。
origin = 'https://relay.example.com'
# 按中继要求选择鉴权头和前缀。
header = 'authorization'
scheme = 'Bearer'
keys = [
    # 第一把密钥：可修改标识和变量名，变量值在环境中设置。
    {{ id = 'a', env = 'RELAY_KEY_A' }},
    # 第二把密钥：按需增删条目，不在文件里写入密钥。
    {{ id = 'b', env = 'RELAY_KEY_B' }},
]
"#);

message!(keys(path:&str), en:r#"
# Rotate between keys for the same relay. Set the origin and variable names in keys.toml.
[[format]]
name = 'keys'
command = 'aproxy-format'
args = ['keys', '--config', {path}]
process = 'persistent'
stages = ['request', 'retry']
pool_max = 1
worker_idle_secs = 0
"#, zh:r#"
# 在同一中继的密钥之间轮换；在 keys.toml 中设置来源和环境变量名。
[[format]]
name = 'keys'
command = 'aproxy-format'
args = ['keys', '--config', {path}]
process = 'persistent'
stages = ['request', 'retry']
pool_max = 1
worker_idle_secs = 0
"#);
