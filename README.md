# aProxy

> 「飘风不终朝，骤雨不终日。」——《道德经》

**本地 API 代理，为 agent 软件的每一次请求护道。**

上游限流、断流、超时、五百年一遇的抖动——aProxy 在本地把请求完整接住：失败**无限重试**（指数退避，封顶可配），流式请求（`Accept` 含 `text/event-stream` 或请求体 `"stream": true`）期间注入 SSE 心跳保活，成功后按原字节回放。客户端零感知：它以为的那次请求，只是慢了一点。（两个显式例外：`forward_only` 模式显式放弃重试保障，换取真流式直通；`bounded_retry_paths` 命中的请求失败 3 次即透传真实响应、响应头已被保活心跳提交的流式请求则以终态 SSE error 事件收场——均见「配置文件」。）

[English](README_EN.md)

## 其术

- **无限重试** —— 夫唯不争，故天下莫能与之争。网络错误、4xx/5xx、错误 JSON（200 携带 error）皆触发重试；客户端断开即中止上游请求（计费保护）。两个**显式出口**：`forward_only` 模式（显式放弃重试，换取请求体与响应的流式直通）与 `bounded_retry_paths`（对确定性报错的上游端点，失败 3 次即透传真实响应，不再无限等待；仅当流式请求的响应头已被保活心跳提交时，改以终态 SSE error 事件告知）。
- **完全透传** —— 大音希声，大象无形。路径、查询、请求头原样转发；控制通道走独立命名管道，代理端口只做透传一件事。
- **外部转换器（可选）** —— 化而欲作，吾将镇之以无名之朴。请求/响应可整流交给外部 format 程序改写（一行 JSON 信封进出，任何能读写 stdin/stdout 的程序都行）：OpenAI ↔ Anthropic 协议转换、多 key 轮换、多模型多渠道聚合。本体只加编排代码，转换逻辑全部外置；官方示例 `aproxy-format` 二进制开箱即用（独立发版）。常驻进程池复用到已死的空闲 worker 时，会自动换新 worker 重试一次。**限制**：`aproxy-format` 的跨协议转换（如 Anthropic ↔ OpenAI）只支持**非流式**，跨协议的 SSE 流式响应不支持——响应侧转换报错后透传上游原始响应，客户端会收到渠道协议格式的流；同协议 SSE 原样直通，多 key 轮换与同协议聚合不受影响。
- **守护进程** —— `aproxy` 后台启动（分离子进程，关终端不掉）；`status` / `stop` / `logs` / `restore` 全套实例管理。
- **看门狗** —— 天网恢恢，疏而不失。全局看护进程自动重拉崩溃/挂死的实例（默认开启；实测 +2.4% 体积、+2.9MB 常驻、转发热路径零损耗）。身份按进程 ID + 创建时间核验，与二进制文件名无关。macOS 暂不支持，见「平台支持」。
- **多开** —— 万物并育而不相害。每份 config.toml 一实例，端口各自独立，并存不扰。
- **自愈** —— 复命曰常，知常曰明。崩溃/断电/系统重启后 `aproxy restore` 一键恢复，空清单静默成功——可配开机自启。

## 入门

**交给 agent 一句话即可**——把下面这行发给你的 agent（Claude Code 等），它会读文档并完成安装与配置：

```text
Read https://raw.githubusercontent.com/MoYeRanQianzhi/aProxy/main/docs/INSTALL_AGENT.md and install (or upgrade) aProxy on this machine exactly as it says.
```

**人类自装**（首装二进制 + skill 文档，SHA256 校验，落位 `~/.aproxy/`）。默认安装最新**稳定版**；仓库还没有稳定版时（0.1.0 发布前）回退到版本号最大的预发布并给出提示；加 `--pre` 则预发布也参与选择，取版本号最大者（PowerShell 为 `-Pre`，也可设环境变量 `APROXY_PRE=1`）。只认 `vX.Y.Z` 与 `vX.Y.Z-(alpha|beta|rc).N` 格式的 tag：

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
```

```sh
# Linux / macOS / Git Bash（加预发布：curl ... | sh -s -- --pre）
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
```

```bat
rem Windows (cmd 兜底；下载委托系统自带的 Windows PowerShell，需其可用)
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.cmd -o install.cmd && install.cmd
```

脚本说明：
- 已安装则不覆盖，升级走 `aproxy install`；sh 脚本需要 `sha256sum`、`shasum` 或 `openssl` 之一做 SHA256 校验。
- Linux：glibc 过低或 musl 系统自动改用静态链接的 musl 产物；落位前先 `--version` 自检，gnu 产物在本机跑不起来会自动改拉 musl。
- `irm | iex` 无法传参，PowerShell 脚本可用环境变量代替：`APROXY_PRE`、`APROXY_NO_SKILLS`、`APROXY_DL_PROXY`（下载代理，与上游请求代理无关）。
- Windows PowerShell 5.1 用 `-File` 本地运行 `install.ps1` 会解析失败（文件为 UTF-8 无 BOM）；请用上面的 `irm | iex`，或 `pwsh -File`。

其他渠道（装好后可用 `aproxy install --adopt` 收编到标准位置）：

```sh
npm install -g @meowo/aproxy     # 按平台自动选二进制；正式版发在 latest 标签
npm install -g @meowo/aproxy@next  # 预发布发在 next 标签，需显式指定
cargo install aproxy             # 源码编译，需 rustc 1.88+
```

升级：`aproxy install`（在线下载链条自动滚动重启，逐实例无感；`--from <路径>` 本地安装、`--adopt` 收编既有安装）。
- **更新通道**：`install` 默认的 `latest` 在「通道」内取版本号最大者。当前是正式版则只取正式版（不会被带到预发布）；当前是预发布（如 alpha）则默认就在预发布通道；`--pre` 显式让预发布参与选择。只认 `vX.Y.Z` / `vX.Y.Z-(alpha|beta|rc).N`，按版本号而非创建时间比较，`format-v*` 与历史测试 tag 不参与。通道内没有更新版本时提示「已是最新」或「暂无可用版本」并保持现状，不会偷偷换通道。指定具体版本号（`aproxy install 0.1.0`）不受通道影响；低于当前版本的目标默认拒绝，需 `--allow-downgrade`。
- **失败回滚**：滚动重启时某个实例在新版本下起不来，`install` 会用旧二进制按原参数把它拉回、中止滚动（其余实例不动）并以非零退出；排除原因后重新执行 `aproxy install` 继续。
- 手动兜底：`aproxy stop all` → 覆盖二进制 → `aproxy restore`。

源码构建：

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy
cargo build --release
```

## 平台支持

| 平台 | 状态 |
|---|---|
| Windows（x64 / x86 / arm64） | 全功能 |
| Linux（x86_64 / aarch64，glibc 与 musl） | 全功能；gnu 产物要求 glibc ≥ 2.28（发布流程构建时断言），更低版本的 glibc 与 musl 系统使用静态链接的 musl 产物 |
| macOS（Apple Silicon / Intel） | 提供预构建二进制，但未经真机验证；看门狗、`aproxy install` 与依赖进程查询的实例管理用到 Linux 专有接口（`/dev/shm`、`/proc`），在 macOS 上不可用或退化（需真机补齐，欢迎贡献） |

## 起手

新装机没有上游地址，直接运行 `aproxy` 会因 `base_url` 为空而启动失败——先配置再启动：

```powershell
# 1. 配置上游（交互写入 ~/.aproxy/config.toml）
aproxy config --baseurl https://api.anthropic.com --api-key sk-ant-...

# 2. 启动（默认后台）
aproxy

# 3. 把 agent 软件的 API base URL 指向本地代理
#    https://api.anthropic.com  →  http://127.0.0.1:12345
```

## 接入 agent 客户端

为了让「流中途断开也能透明重试」，aProxy 会缓冲完整响应、校验无误后才回放，等待期间只向客户端发 SSE 注释心跳。注释心跳能续住客户端**按字节计时**的超时，续不住**按 SSE 事件计时**的超时（注释与 `ping` 都不算事件）。客户端若有后一种超时，就**必须**调大它，否则任何超过它的重试期或长生成都会被客户端断开重发。

### Claude Code

```powershell
# PowerShell
$env:ANTHROPIC_BASE_URL = "http://127.0.0.1:12345"
$env:CLAUDE_STREAM_IDLE_TIMEOUT_MS = "86400000"
claude
```

```sh
# sh / bash / zsh
export ANTHROPIC_BASE_URL=http://127.0.0.1:12345
export CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000
claude
```

也可以写进 Claude Code 的 `~/.claude/settings.json` 的 `env` 字段，对每个会话生效：

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:12345",
    "CLAUDE_STREAM_IDLE_TIMEOUT_MS": "86400000"
  }
}
```

Claude Code 的事件级空闲超时默认 600 秒，由 `CLAUDE_STREAM_IDLE_TIMEOUT_MS` 控制，`86400000`（24 小时）经实测可用；`API_TIMEOUT_MS` 不控制这道闸。首字节（约 360 秒）与字节级空闲（300 秒）两道超时由心跳覆盖。（Claude Code 2.1.288 黑盒实测）

### Codex

在 `~/.codex/config.toml` 里加一个指向 aProxy 的 provider，并**必须**调大 `stream_idle_timeout_ms`：

```toml
model = "gpt-5"                     # 换成上游提供的模型名
model_provider = "aproxy"

[model_providers.aproxy]
name = "aproxy"
base_url = "http://127.0.0.1:12345/v1"
env_key = "OPENAI_API_KEY"          # Codex 从该环境变量取 key；aProxy 配了 api_key 时填任意非空值
wire_api = "responses"
stream_idle_timeout_ms = 86400000   # 24 小时
```

Codex 的 `stream_idle_timeout_ms` 默认 300000（5 分钟），按 SSE 事件计时，注释心跳续不住；超时后自动重连，默认 5 次后本轮失败。实测：设为 60 秒时恰好 60 秒断开重连；设为 24 小时后，10 分钟以上的等待正常完成。（codex-cli 0.160.0 源码与黑盒实测）

### 其他客户端

| 客户端 | 接 aProxy 需要做什么 |
|---|---|
| Qwen Code | 设环境变量 `QWEN_STREAM_IDLE_TIMEOUT_MS=0` 与 `QWEN_STREAM_MAX_LIFETIME_MS=0`（事件级空闲默认 240 秒，另有 15 分钟流总时长上限，两个都要关） |
| dsh（DeepSeek Harness） | 经 `llm-pi-ai` 适配器（OpenAI / Anthropic 兼容网关）接入时，给该 provider 设 `streamIdleTimeoutMs`（如 `172800000`）；`llm-deepseek` 适配器默认即可 |
| Gemini CLI | 用 `GOOGLE_GEMINI_BASE_URL` 与 `GEMINI_API_KEY` 接入，结果正常（实测）。但它的流式请求不进保活通道，而它的响应头超时写死为 300 秒（新版更短），重试期加生成超过这个时长就会失败，没有配置可调 |
| pi、OpenCode、Aider、Cline、Roo Code、Kimi CLI | 默认即可（只有按字节计时的超时）；OpenCode 1.18 起不要设 `timeout`，它限制的是含等待在内的整请求时长 |

除 Claude Code、Codex、Gemini CLI 外，上表来自源码调研，未逐个实测。各客户端的调研版本、配置位置与依据见 skill 文档 [clients.md](.claude/skills/aproxy-cli/references/latest/clients.md)。

注意：请求体里 `"stream": false` 的普通请求没有保活通道（没有可注入心跳的响应流），首字节延迟等于完整生成时长，客户端需自行调大超时（Claude Code 为 `API_TIMEOUT_MS`）。

## 命令

| 命令 | 说明 |
|---|---|
| `aproxy [start]` | 后台启动代理（默认端口 127.0.0.1:12345） |
| `aproxy start <别名\|路径>` | 按别名或配置文件路径启动 |
| `aproxy --foreground` | 前台运行（日志走控制台，Ctrl+C 停止） |
| `aproxy status` | 列出运行中的实例（端口/pid/版本/上游/配置） |
| `aproxy stop [PORT\|all\|别名]` | 停止实例；多实例必须指定端口、`all` 或别名；`--force` 立即强杀（不会被看门狗复活） |
| `aproxy restart [PORT\|all\|别名]` | 重启运行中的实例（只重启不启动；改配置生效用）；停旧实例前先预检新配置，不通过则旧实例保持运行；`--force` 立即强杀重启 |
| `aproxy logs [PORT\|别名]` | 连接实例实时输出日志；不支持 `all` |
| `aproxy restore` | 恢复崩溃/重启前在运行的实例；无实例则静默结束（开机自启友好） |
| `aproxy alias add\|remove\|list` | 管理配置别名（存于 settings.json） |
| `aproxy doctor` | 配置体检：settings.json error 级检查 + 别名配置与目录 toml 审查（warning） |
| `aproxy find [关键字]` | 从配置目录发现全部配置；`--aliased/--unaliased --port PORT` 过滤 |
| `aproxy config [选项]` | 查看或修改配置（`--show` 打印当前配置） |

通用参数：`--config <PATH>`（指定配置文件，多开用）、`--listen <ADDR>`、`--proxy <URL>`、`--api-key <KEY>`（启动路径仅本次生效）。

## 多开与别名

```powershell
aproxy --config ~/.aproxy/work.toml      # listen_addr = "127.0.0.1:12345"
aproxy --config ~/.aproxy/personal.toml  # listen_addr = "127.0.0.1:12346"
aproxy status                            # 两个实例都可见
aproxy stop 12346                        # 按端口管理
```

```powershell
aproxy alias add openrouter ~/.aproxy/openrouter.toml
aproxy alias add anthropic               # 省略路径 = 默认 ~/.aproxy/config.toml
aproxy start openrouter                  # 按别名启动
aproxy stop openrouter                   # 按别名停止（端口变了依然有效）
aproxy alias list
```

别名存于 `~/.aproxy/settings.json`（程序管理的内部配置，唯一；用 `aproxy alias` 管理）。config.toml 人类可读可写、可多份平行并存。

## 开机自启（可选）

任务计划程序创建「登录时运行」任务，操作指向 `aproxy.exe`，参数 `restore`。之前在运行的实例自动恢复；之前全关了则静默结束，无副作用。

## 配置文件

`~/.aproxy/config.toml`（多开时各配置文件独立）：

```toml
base_url = "https://api.anthropic.com"   # 上游地址
listen_addr = "127.0.0.1:12345"          # 本地监听
# api_key = "sk-..."                     # 快捷鉴权（等效覆盖 Authorization: Bearer）
# keepalive_interval_secs = 15           # SSE 心跳间隔，0 关闭保活
# keepalive_trigger = "any"              # 哪些请求走保活：accept（Accept 含 SSE）/ body_stream（请求体 "stream": true）/ any（任一，默认）
# keepalive_heartbeat = ": keepalive\n\n" # 每拍写给客户端的心跳字节（默认 SSE 注释；如客户端容不下注释可改成 "\n"）
# proxy = "http://127.0.0.1:7890"        # 上游经代理转发（支持 socks5，可配用户名密码）
# extra_headers / override_headers       # 追加/覆盖请求头
# max_retry_backoff_secs = 320           # 重试退避封顶（0 = 所有重试零延迟）
# spool_limit_mb = 256                   # 上游响应缓冲上限（MB）
# max_body_mb = 128                      # 请求体上限（MB；0 = 不设限。未设时取 settings.json 全局默认）
# disk_cache = true                      # 磁盘缓存：大请求体/响应溢写磁盘，内存与负载解耦
# forward_only = false                   # 仅转发模式（开启即放弃重试）：请求体与响应流式直通，无重试/缓冲/心跳
# bounded_retry_paths = [ '/v1/x' ]      # 受限重试路径（正则）：命中者失败 3 次即透传，不再无限重试
# log_file = "D:/aproxy-logs/a.log"      # 自定义日志文件（~ 展开，相对路径相对 APROXY_HOME；缺省按启动随机命名，地址经 IPC 获取）
# connect_timeout_secs = 30              # 上游连接建立超时（0 = 不设限）
# read_timeout_secs = 300                # 两次读到数据间隔超时（0 = 不设限）
# allowed_hosts = ["myproxy.local"]      # 额外放行的 Host（防 DNS 重绑定；追加到内置名单，"*" 关闭校验）
# allowed_origins = ["http://localhost:5173"] # 放行的浏览器 Origin（默认拒绝一切带 Origin 的请求，"*" 关闭校验）
# request_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
                                         # 外部转换器（请求侧）：交给 format 程序改写 body/headers/url（协议转换、
                                         # 多 key 轮换、多渠道聚合；失败 502 不重试；与 forward_only 互斥）
# response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
                                         # 外部转换器（响应侧，须与请求侧配同一份 extra）：改写上游响应后回放（失败透传原样）
                                         # command/extra 用 ~/ 或绝对路径：守护进程的工作目录不可靠，相对路径会找不到
```

多 key 轮换只在**请求之间**生效：同一请求的重试沿用转换后的同一个 key（请求只转换一次，重试重放转换产物）。

运行数据在 `~/.aproxy/`：`run/`（实例注册与恢复记录、看护者 claim）、`logs/`（守护日志，按启动随机命名、自动清理与轮转，地址经 IPC 向实例询问；可用 `log_file` 自定义去向）、
`spool/<端口>/`（磁盘缓存临时文件，启动时自动清理）、
`settings.json`（内部配置：别名、默认配置文件、日志轮转阈值、看门狗五字段等，程序管理不建议手改）。

### 保活（SSE 心跳）

保活适用的请求（`keepalive_interval_secs` > 0、非 `forward_only`，且按 `keepalive_trigger` 命中：`Accept` 含 `text/event-stream`，或请求体顶层 `"stream": true`；默认 `any` 任一即可）从**首轮**起就走保活通道：

- 任一次尝试上游回 2xx + 未压缩的 `text/event-stream` 时，立即把上游真实的状态码与响应头转给客户端（未配 `response_transform` 时）；否则约一个保活间隔后先提交骨架头（200 + SSE）。「需要重试」本身不提交，重试几次后很快成功的请求拿到的仍是上游真实响应。
- `"stream": true` 的非 SSE 流（如 Ollama 原生 API 的 NDJSON）在不重试时原样直通；若该上游常需重试（重试拖过一个保活间隔会提交 SSE 骨架头，改写 content-type 并混入心跳），建议该实例设 `keepalive_trigger = "accept"`。
- 提交之后，无论是在等首字节、上游在途、缓冲上游流还是退避，都按 `keepalive_interval_secs` 发 SSE 注释心跳；响应体仍是缓冲完整、校验无误后才回放，失败尝试的数据不会混入。
- 为避免往压缩流里插入明文心跳，保活适用的请求发往上游时 `accept-encoding` 一律改为 `identity`——这是对「完全透传」的一个有意例外，代价只是上游到本机这一段多传一些字节；不适用保活的请求不改写。
- `"stream": false` 的普通请求没有保活通道；`forward_only` 实例与 `keepalive_interval_secs = 0` 同样没有。

### 入站来源校验

代理默认只服务本机 CLI 类客户端，并对网页类来源设防（详见 [SECURITY.md](SECURITY.md)）：

- **Host**（防 DNS 重绑定）：监听回环地址时，只放行 `localhost` / `127.0.0.1` / `[::1]` 与监听地址自身的主机名；`allowed_hosts` 的条目在此基础上**追加**（忽略端口、不区分大小写）。监听非回环地址且 `allowed_hosts` 为空时不做 Host 校验。
- **Origin**：任何携带 `Origin` 头的请求默认拒绝（只有浏览器、Electron/WebView 类客户端会发），除非精确匹配 `allowed_origins`（不区分大小写，忽略末尾 `/`）；与监听地址无关。
- 两项都可写 `"*"` 关闭；`[]` 等同未配置。每实例的 `config.toml` 与 `settings.json` 全局默认均可配置（toml 优先）。
- 被拒请求在本地返回 **403**，错误文案点名对应配置项；它从未转发上游、不注入 `api_key`、不进入重试，对被放行的请求无任何影响。
- 受影响的客户端：Cherry Studio、Open WebUI 等会发 `Origin` 的应用，需把其 Origin 加入 `allowed_origins`。Claude Code 等 CLI 客户端不发 `Origin`、Host 为 `127.0.0.1:端口`，不受影响。

## 安全须知

aProxy 是本机单用户代理：它持有并向每个转发请求注入上游密钥，代理端口本身没有鉴权。

- 默认只监听回环地址（`127.0.0.1`）。**不要把 `listen_addr` 改成 `0.0.0.0` 或局域网地址**——那等于让同网段的任何主机免鉴权使用你的密钥；确需对外开放时，启动、`startup.log` 与 `aproxy doctor` 会给出警告。
- `api_key` 以明文存放在 `config.toml`，请收紧文件权限（仅当前用户可读）。
- 外部转换器（`request_transform` / `response_transform`）会按配置执行任意命令；只配置你信任的程序，且不要让他人能修改你的 `config.toml`。
- 日志与 `status` 对 URL 内嵌凭据与查询串的值打码（`?key=***`），可安全粘贴分享。

完整的威胁模型与漏洞报告方式见 [SECURITY.md](SECURITY.md)。

## 开发

```powershell
cargo test --workspace --locked           # 全量测试（含 aproxy-envelope / aproxy-format）
cargo clippy --workspace --all-targets --locked -- -D warnings   # 必须零警告（项目纪律）
cargo fmt --all -- --check
```

架构与开发文档见 [`docs/`](docs/)；面向 agent 的开发文档在 `.agents/docs/`。

## License

MIT
