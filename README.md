# aProxy

> 「飘风不终朝，骤雨不终日。」——《道德经》

**本地 API 代理，为 agent 软件的每一次请求护道。**

上游限流、断流、超时、五百年一遇的抖动——aProxy 在本地把请求完整接住：失败**无限重试**（指数退避，封顶可配），流式请求（`Accept` 含 `text/event-stream` 或请求体 `"stream": true`）期间注入 SSE 心跳保活，成功后按原字节回放。客户端零感知：它以为的那次请求，只是慢了一点。（两个显式例外：`delivery = "forward"` 模式显式放弃重试保障，换取真流式直通；`[[retry.rules]]` 命中的请求达到 `max_attempts` 即透传真实响应、响应头已被保活心跳提交的流式请求则以终态 SSE error 事件收场——均见「配置文件」。）

[English](README_EN.md)

## 其术

- **无限重试** —— 夫唯不争，故天下莫能与之争。网络错误、4xx/5xx、错误 JSON（200 携带 error）皆触发重试；客户端断开即中止上游请求（计费保护）。两个**显式出口**：`delivery = "forward"` 模式（显式放弃重试，换取请求体与响应的流式直通）与 `[[retry.rules]]`（对确定性报错的上游端点，达到 `max_attempts` 即透传真实响应，不再无限等待；仅当流式请求的响应头已被保活心跳提交时，改以终态 SSE error 事件告知）。
- **完全透传** —— 大音希声，大象无形。路径、查询、请求头原样转发；控制通道走独立命名管道，代理端口只做透传一件事。
- **format 程序（可选）** —— 化而欲作，吾将镇之以无名之朴。请求、响应、重试的决定与心跳都可以交给外部 format 程序（一行 JSON 一问一答，任何能读写 stdin/stdout 的程序都行），多个程序按配置顺序串成一条链。本体只做编排，转换逻辑全部外置；官方的 `aproxy-format` 开箱即用：OpenAI Chat、OpenAI Responses 与 Anthropic Messages 之间的协议转换（**流式与非流式**），多 key 轮换与冷却，按模型的多渠道路由与故障切换，重试策略，按请求在真流式与无限重试之间选择。程序坏了有熔断：判断类程序被跳过，请求回到无限重试。
- **守护进程** —— `aproxy start` 后台启动（按拉起规格分离子进程，关终端不掉；密钥只在拉起规格与期望状态记录里，不出现在实例与看护者的进程命令行上）；`status` / `stop` / `logs` / `restore` 全套实例管理。
- **看门狗** —— 天网恢恢，疏而不失。全局看护进程自动重拉崩溃/挂死的实例（默认开启；实测 +2.4% 体积、+2.9MB 常驻、转发热路径零损耗）。身份按进程 ID + 创建时间核验，与二进制文件名无关。连续重拉失败不放弃：转入每 300 秒一次的慢速重试，直到实例起来或被 `aproxy stop` 删掉期望状态记录；重拉出的实例存活满 60 秒才清空之前的失败计数。每个扫描周期重读 settings，关掉 `watchdog.enabled` 即退出。
- **多开** —— 万物并育而不相害。每份 config.toml 一实例，端口各自独立，并存不扰。
- **自愈** —— 复命曰常，知常曰明。崩溃/断电/系统重启后 `aproxy restore` 一键恢复，空清单静默成功——可配开机自启。实例的期望状态记录（拉起规格：配置文件、覆盖参数、钉住的端口、启动时的代理环境与 PATH、日志路径）只有 `stop` 删除；`restart` 中途失败也留着记录，修好原因后 `restore` 或看门狗可把它拉回。配置文件暂时找不到时记录同样保留。

## 入门

**交给 agent 一句话即可**——把下面这行发给你的 agent（Claude Code 等），它会读文档并完成安装与配置：

```text
Read https://raw.githubusercontent.com/MoYeRanQianzhi/aProxy/main/docs/INSTALL_AGENT.md and install (or upgrade) aProxy on this machine exactly as it says.
```

**人类自装**（首装二进制 + skill 文档，SHA256 校验，落位 `~/.aproxy/`）。默认安装最新**稳定版**；仓库里一个稳定版都没有时，回退到版本号最大的预发布并给出提示；加 `--pre` 则预发布也参与选择，取版本号最大者（PowerShell 为 `-Pre`，也可设环境变量 `APROXY_PRE=1`）。只认 `vX.Y.Z` 与 `vX.Y.Z-(alpha|beta|rc).N` 格式的 tag。

> **0.2.0 目前只有预发布版本。** 各渠道不特别指定时装到的都是稳定版 0.1.0，它没有下文说的桌面界面、两个变体与 `install --force`。要装 0.2.0：一键脚本加 `--pre`，npm 带 `@next`，cargo 写明版本号。0.1.x 不能原地升级到 0.2.0，见下文「从 0.1.x 升级」。

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
- **两个变体**：每个平台都有带桌面界面的 desktop（`aproxy desktop` 打开窗口）与无界面的 headless 两个版本，代理与命令行完全相同；musl 只有 headless。Windows 与 macOS 装 desktop。Linux 上带界面的版本动态链接 GTK 与 WebKitGTK，缺库或库太旧时连 `--version` 都起不来，所以 sh 脚本先看本机：musl、glibc 低于 2.34、找不到 WebKitGTK 4.1（Debian/Ubuntu 的 `libwebkit2gtk-4.1-0`）的装 headless 并说明原因，带界面的版本下载后自检跑不起来（例如 WebKitGTK 低于 2.40）也改装 headless。要指定变体：sh 脚本加 `--variant desktop|headless`（或设 `APROXY_VARIANT`）；装好之后换变体用 `aproxy install --variant desktop`（或 `headless`）。
- 首次安装最后把 `<home>/bin` 加进当前用户的 PATH，带界面的版本再建一个打开窗口的入口（Windows 开始菜单、macOS 的「应用程序」、Linux 的应用菜单）；不想改 PATH：sh 脚本加 `--no-path`，PowerShell 脚本加 `-NoPath`，或设 `APROXY_NO_PATH=1`。
- `irm | iex` 无法传参，PowerShell 脚本可用环境变量代替：`APROXY_PRE`、`APROXY_NO_SKILLS`、`APROXY_DL_PROXY`（下载代理，与上游请求代理无关）、`APROXY_NO_PATH`。
- Windows PowerShell 5.1 用 `-File` 本地运行 `install.ps1` 会解析失败（文件为 UTF-8 无 BOM）；请用上面的 `irm | iex`，或 `pwsh -File`。

其他渠道（装好后用 `aproxy install --adopt` 收编到标准位置：复制进 `<home>/bin`、加 PATH、建窗口入口，`--no-path` 与 `--no-shortcut` 分别跳过后两步）：

```sh
npm install -g @meowo/aproxy@next            # 带桌面界面，按平台自动选二进制
npm install -g @meowo/aproxy-headless@next   # 无界面（服务器、缺 WebKitGTK 的 Linux）；两个包都提供 aproxy 命令，只装其一
cargo install aproxy --locked --version 0.2.0-alpha.2   # 源码编译，需 rustc 1.90+
```

预发布发在 npm 的 `next` 标签，`latest` 标签留给正式版；cargo 不写版本号时也只取稳定版。0.2.0 现在只有预发布，所以上面的命令都带着标签或版本号（`@meowo/aproxy-headless` 的 `latest` 只是占位版本，不能用）。0.2.0 正式版发布后，去掉 `@next` 与 `--version` 即可。

`cargo install` 默认编带桌面界面的版本，Linux 上要先装 WebKitGTK 4.1 的开发包（Debian/Ubuntu：`libwebkit2gtk-4.1-dev`）；Linux 服务器上用 `cargo install aproxy --locked --no-default-features`（同样要写明 `--version`）编无界面的 headless 版本。`--locked` 让依赖取包里 Cargo.lock 记的、发布前测过的版本；不加时 cargo 会改用各依赖当下的最新兼容版本。`cargo binstall aproxy` 取带界面的预编译产物（musl 系统上取 headless）；要无界面的版本，指定资产地址：

```sh
cargo binstall aproxy --pkg-url '{ repo }/releases/download/v{ version }/{ name }-headless-{ target }{ binary-ext }' --pkg-fmt bin
```

手动下载：带界面的是 `aproxy-<target>[-v3]`，无界面的是 `aproxy-headless-<target>[-v3]`，Releases 页的 `.sha256` 与发布清单 `aproxy-manifest.json` 可供核对。npm 装的带界面版本在 Linux 上缺库起不来时，启动器会提示改装 `@meowo/aproxy-headless`。

升级：`aproxy install`（在线下载链条自动滚动重启，逐实例无感；`--from <路径>` 本地安装、`--adopt` 收编既有安装）。
- **更新通道**：`install` 默认的 `latest` 在「通道」内取版本号最大者。当前是正式版则只取正式版（不会被带到预发布）；当前是预发布（如 alpha）则默认就在预发布通道；`--pre` 显式让预发布参与选择。只认 `vX.Y.Z` / `vX.Y.Z-(alpha|beta|rc).N`，按版本号而非创建时间比较，`format-v*` 与历史测试 tag 不参与。通道内没有更新版本时提示「已是最新」或「暂无可用版本」并保持现状，不会偷偷换通道。指定具体版本号（`aproxy install 0.2.0`）不受通道影响；低于当前版本的目标默认拒绝，需 `--allow-downgrade`；低于 0.2.0 的版本一律不装。
- **从 0.1.x 升级**：0.1.x 与 0.2.0 起的版本互相控制不了对方的实例，0.1.x 的 `aproxy install` 装不上 0.2.0（下载后报 `--version 输出无版本号` 退出，什么都不改）。按 [CHANGELOG](CHANGELOG.md) 的「从 0.1.x 迁移」停掉实例、重新安装，再用 `aproxy migrate` 把 settings.json 与配置文件转换成 0.2.0 的写法（先 `--dry-run` 预览，原文留 `.0.1.bak` 备份）；0.2.0 现在只有预发布，重装时一键脚本要加 `--pre`，否则装回的还是 0.1.x。
- **下载渠道与代理**：在线安装按 github → npm → cargo-binstall → cargo 的顺序尝试，settings.json 的 `update.download_chain` 可以调整次序、加镜像（url 模板）；下载走代理用 `--download-proxy <URL>` 或 settings.json 的 `update.download_proxy`，与上游请求代理无关。字段说明见 skill 文档 [settings-json.md](.claude/skills/aproxy-cli/references/latest/settings-json.md)。强制安装不读 settings.json，只用命令行的 `--download-proxy`（没给就走系统环境代理）。
- **变体**：升级保持已装的变体（`aproxy --version` 括号里的第一段）；`aproxy install --variant desktop|headless` 换变体，通道里没有更新的版本时重装当前版本的另一个变体。安装器不预先猜本机缺什么，只看下载下来的二进制自检（`--version`）能不能跑：没写 `--variant` 时，带界面的版本跑不起来（Linux 上多半是缺 WebKitGTK）就改装同一目标的 headless 并说明，Linux 上再不行换 musl 产物；写了 `--variant desktop` 就报错退出，什么都不改。`install --force` 也一样。
- **失败回滚**：滚动重启时某个实例在新版本下起不来，`install` 会用旧二进制按同一份拉起规格把它拉回、中止滚动（其余实例不动）并以非零退出；排除原因后重新执行 `aproxy install` 继续。
- **强制安装**：普通安装失败、卡住，或装着的版本坏到跑不起来时，用 `aproxy install --force [版本|latest]`（或 `--from <下载好的二进制>`）。它不走交接：按进程台账结束这个 home 下当前用户的全部 aproxy 进程，换上新二进制，再用新版本的 `restore` 按期望状态记录里的拉起规格拉回实例。这会立即中断所有经 aproxy 的在途请求，所以先确认（非交互环境加 `--yes`）。装着的版本连运行都不行时，手动下载新版本，执行 `<新版本> install --force --from <新版本>`。退出码：0 完成；10 一个进程都没结束、二进制没换；11 结束过进程、二进制没换（再执行一次即可）；12 二进制已换、有实例没回来（输出里按端口列出启动命令）。它不更新 skill 文档，之后执行 `aproxy install --skills-only`。

源码构建：

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy/aproxy-desktop/ui
npm ci
npm run build          # 桌面界面的前端，输出到 aproxy-desktop/dist；需要 Node.js 20.19+ 或 22.12+
cd ../..
cargo build --release
```

桌面界面的前端编进二进制，所以带界面的版本要先构建前端，缺了它编译会报错并说明做法。不要界面时跳过前端，用 `cargo build --release --no-default-features` 编无界面的 headless 版本，不需要 Node.js。Linux 编带界面的版本还要 WebKitGTK 4.1 的开发包（见上）。

## 平台支持

| 平台 | 状态 |
|---|---|
| Windows（x64 / x86 / arm64） | 全功能 |
| Linux（x86_64 / aarch64，glibc 与 musl） | 全功能；headless 的 gnu 产物要求 glibc ≥ 2.28，带界面的 gnu 产物要求 glibc ≥ 2.34 与 WebKitGTK ≥ 2.40（glibc 下限由发布流程构建时断言）；更低版本的 glibc 与 musl 系统使用静态链接的 musl 产物（只有 headless） |
| macOS（Apple Silicon / Intel） | 全功能；每次提交都在 Apple Silicon 的 macOS 上跑全部测试，看门狗、安装升级与桌面窗口也在上面实测过。Intel 版交叉编译，没有在 Intel Mac 上运行过 |

## 起手

新装机没有上游地址，直接运行 `aproxy start` 会因 `base_url` 为空而启动失败——先配置再启动：

```powershell
# 1. 配置上游（写入 ~/.aproxy/config.toml；每次改一个键）
aproxy config set base_url https://api.anthropic.com
aproxy config set api_key sk-ant-...
#    密钥不想留在 shell 历史里时：aproxy config set api_key -

# 2. 启动（默认后台）
aproxy start

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
| `aproxy start` | 后台启动代理（默认配置，默认端口 127.0.0.1:12345） |
| `aproxy start <别名\|路径>` | 按别名或配置文件路径启动 |
| `aproxy start --foreground` | 前台运行（日志走控制台，Ctrl+C 停止） |
| `aproxy status` | 列出 home 里的实例与看护者：运行中、无响应、等待恢复的实例与读不懂的记录分块列出；`--idle`/`--busy` 只筛运行中的实例 |
| `aproxy stop [PORT\|all\|别名]` | 停止实例；多实例必须指定端口、`all` 或别名；`--force` 立即强杀（不会被看门狗复活）；目标已崩溃时删掉它留下的期望状态记录 |
| `aproxy restart [PORT\|all\|别名]` | 重启运行中的实例（只重启不启动；改配置生效用）；停旧实例前先预检新配置，不通过则旧实例保持运行；失败时记录保留，`restore` 或看门狗可拉回；`--force` 立即强杀重启；`--format all\|<名字>` 只重启 format 程序，实例与在途请求不受影响 |
| `aproxy logs [PORT\|别名]` | 打印实例日志最近 200 行后退出；`-n N` 改行数，`-f` 持续跟随；不支持 `all` |
| `aproxy restore` | 恢复崩溃/重启前在运行的实例（按期望状态记录）；配置文件暂时找不到时保留记录并说明；无实例则静默结束（开机自启友好） |
| `aproxy alias add\|remove\|list` | 管理配置别名（存于 settings.json） |
| `aproxy doctor` | 配置体检：settings.json 与每份配置都按 `start` 的规则检查，另查 home 与配置文件的权限、读不懂的记录；有 error 时以 3 结束 |
| `aproxy find [关键字]` | 从配置目录发现全部配置；`--aliased/--unaliased --port PORT` 过滤 |
| `aproxy config show\|get\|set\|unset` | 查看或修改配置：`show` 打印文件里写的值，`--effective` 连全局默认与内置默认一起打印并注明来源；`set`/`unset` 每次改一个键 |
| `aproxy settings show\|get\|set\|unset` | 查看或修改全局设置（`~/.aproxy/settings.json`）：别名、`defaults` 全局默认、看护者、日志、安装更新；settings.json 有错误时也能用，用来修复它 |
| `aproxy install [版本]` | 安装或升级（别名 `upgrade`），逐实例滚动重启，动第一个实例之前先预检；`--check` 只查询有没有新版本，`--variant` 换变体，`--force` 强制安装；见「入门」里的升级说明 |
| `aproxy desktop` | 打开桌面界面（见下） |

`start` 的参数：`<别名|路径>` 或 `--config <PATH>`（指定配置文件，多开用；两者等价）、`--listen-addr <ADDR>`、`--base-url <URL>`、`--proxy <URL>`、`--api-key <KEY|->`（只记录进这次启动的拉起规格，`restart`/`restore` 沿用，不写进 config.toml；`-` 表示从 stdin 读一行）、`--log-file <PATH>`、`--foreground`。不带命令的 `aproxy` 只打印帮助并以退出码 2 结束。

## 桌面界面

`aproxy desktop` 打开桌面界面。它是一个独立的窗口进程，关掉窗口就退出，实例照常运行，与窗口开没开无关。和其余 aproxy 命令一样，它发现上次的安装没做完时会在后台把安装续完，续作可能逐个重启实例。

- **能做什么**：总览这个 home 下的全部实例（状态、在途请求、重试与救回、近一小时的走势）；查看请求日志与单条请求的详情（请求与响应的正文、每次尝试为什么失败，密钥已打码），把要留着的请求固定下来；启动、停止、重启实例，或只重启某个 format 程序；编辑配置（保存前按 `start` 的规则检查，重启实例后生效）；开关看护者与开机自启；检查并安装更新；切换语言、深浅色、Mica 与吉祥物。界面偏好存在 `~/.aproxy/desktop/desktop.toml`。
- **主题与扩展**：在设置页可以装主题包（换颜色，可以带样式表、图片与字体）与扩展包（在窗口里运行的 JavaScript，可以加页面与面板、读写实例与配置）。包只从本地装：选一个 .zip 文件，或把 .zip 与解开的目录拖进窗口。扩展不在沙箱里，能做窗口能做的一切，所以安装之前窗口会把适用的风险逐条讲清（闭源、与 GitHub release 的摘要对不上、仓库星数很少、核对不了），每一条都要你确认。拿不准时，先让你的 agent 用随 aProxy 发布的审查 skill（`aproxy-audit-extension`、`aproxy-audit-theme`）审一遍。自己写主题或扩展：见 [docs/extensions.md](docs/extensions.md)（英文）。
- Windows 需要 Microsoft Edge WebView2 运行时（Windows 11 自带，Windows 10 没有时按提示安装）。从终端运行时命令立即返回，窗口独立运行。
- Linux 需要图形会话与 WebKitGTK 4.1（2.40 或更新）。带界面的版本动态链接 WebKitGTK，缺这些库时整个程序都起不来（连 `aproxy --version` 也不行），没有图形界面的机器请用无界面的 headless 版本——一键安装脚本与 `aproxy install` 在缺库的机器上会自动装它；musl 构建只有 headless 版本。
- macOS 保留系统的红黄绿三个窗口按钮，页面一直延伸到标题栏里。
- 同一个 home 只开一个窗口，再次运行会提示已经在运行。
- headless 版本（`aproxy --version` 括号里的第一段是 `headless`）没有界面，运行 `aproxy desktop` 会说明怎样换装带界面的版本（`aproxy install --variant desktop`）。

## 多开与别名

```powershell
aproxy start ~/.aproxy/work.toml      # listen_addr = "127.0.0.1:12345"
aproxy start ~/.aproxy/personal.toml  # listen_addr = "127.0.0.1:12346"
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

别名存于 `~/.aproxy/settings.json`（整个 home 唯一的程序状态；用 `aproxy alias` 管理，也可以 `aproxy settings` 直接读写）。config.toml 人类可读可写、可多份平行并存。

## 开机自启（可选）

任务计划程序创建「登录时运行」任务，操作指向 `aproxy.exe`，参数 `restore`。之前在运行的实例自动恢复；之前全关了则静默结束，无副作用。

## 配置文件

`~/.aproxy/config.toml`（多开时各配置文件独立）：

```toml
base_url = "https://api.anthropic.com"   # 上游地址
listen_addr = "127.0.0.1:12345"          # 本地监听（端口 0 = 系统分配；首次拿到的端口会被钉住，重启/恢复回到同一端口）
# api_key = "sk-..."                     # 快捷鉴权（等效覆盖 Authorization: Bearer）
# proxy = "http://127.0.0.1:7890"        # 上游经代理转发（支持 socks5，可配用户名密码）
# extra_headers / override_headers       # 追加/覆盖请求头（表：extra_headers = { "x-title" = "..." }）
# max_request_mb = 128                   # 请求体上限（MB；0 = 不设限。未设时取 settings.json 的 defaults.max_request_mb）
# max_response_mb = 256                  # 上游响应缓冲上限（MB；0 按 1 处理）
# disk_cache = true                      # 磁盘缓存：大请求体/响应溢写磁盘，内存与负载解耦
# delivery = "retry"                     # 请求起始的投递方式："retry"（默认，缓冲并重试）/ "forward"（仅转发：流式直通，不重试、不发心跳）；format 程序可以按请求改
# log_file = "D:/aproxy-logs/a.log"      # 自定义日志文件（~ 展开，相对路径相对 APROXY_HOME；缺省按启动随机命名，地址经 IPC 获取）
# allowed_hosts = ["myproxy.local"]      # 额外放行的 Host（防 DNS 重绑定；追加到内置名单，"*" 关闭校验）
# allowed_origins = ["http://localhost:5173"] # 放行的浏览器 Origin（默认拒绝一切带 Origin 的请求，"*" 关闭校验）

[timeouts]
# connect_secs = 30                     # 上游连接建立超时（0 = 不设限）
# first_byte_secs = 300                 # 从发出请求到收到响应头的时限（0 = 不设限）
# read_idle_secs = 300                  # 两次读到响应体数据之间的间隔上限（0 = 不设限）

[keepalive]
# interval_secs = 15                    # SSE 心跳间隔，0 关闭保活
# trigger = "any"                       # accept（Accept 含 SSE）/ body_stream（请求体 "stream": true）/ any（任一，默认）
# heartbeat = ": keepalive\n\n"         # 每拍写给客户端的心跳字节（默认 SSE 注释；如客户端容不下注释可改成 "\n"）

[retry]
# max_backoff_secs = 320                # 重试退避封顶（0 = 所有重试零延迟）
# [[retry.rules]]                      # 第 N 次带响应的失败命中规则且 N >= max_attempts 即透传（0 = 不限，可作豁免）
# path = '/v1/messages/count_tokens(\?.*)?'   # 对「路径?查询串」整体的正则，自动锚定两端
# max_attempts = 3

[history]
# body_budget_mb = 64                   # 请求历史里保留的请求体与响应体合计上限（MB，占内存；0 = 不保留体）

# [[format]]                             # format 程序，可以有多条；列表顺序就是请求方向的顺序（见下文「format 程序」）
# name = "keys"
# command = "aproxy-format"              # 裸命令名先找 ~/.aproxy/bin/，再找 PATH
# args = ["keys", "--config", "keys.toml"]   # 相对路径相对 ~/.aproxy
# process = "persistent"                 # spawn（默认）/ persistent（stream、notify 与有状态的程序必须用）
# stages = ["request", "retry"]          # request / retry / decide / response / stream / heartbeat / notify
# env = { RELAY_KEY_A = "sk-a", RELAY_KEY_B = "sk-b" }   # 追加的环境变量（值不在 config show 中显示）
```


运行数据在 `~/.aproxy/`：`run/`（实例记录 `<端口>.instance.json`、期望状态记录 `<端口>.restore.json`、看护者记录 `watchdog.json`；启动瞬间还有短暂存在的拉起规格交接文件）、`logs/`（守护日志，按启动随机命名、自动清理与轮转，地址经 IPC 向实例询问；可用 `log_file` 自定义去向）、
`spool/<pid>-<短哈希>/`（磁盘缓存临时文件，每个实例进程一个目录；实例停止时删掉，崩溃留下的由下一个启动的实例或看门狗清理）、
`settings.json`（整个 home 的程序状态：`aliases`、`default_config`、`config_dirs`，`defaults`（config.toml 键的全局默认），以及 `watchdog`、`logs`、`instances`、`update` 各段；`ui` 段保留不用，桌面界面的偏好存在 `desktop/` 下）。

`settings.json` 由 `aproxy settings show|get|set|unset` 读写（键是点号路径，例如 `watchdog.enabled`、`defaults.max_request_mb`）；也可以 `aproxy alias add|remove` 与 `aproxy settings set|unset default_config` 改各自的键。手工编辑只在文件无法解析时作为兜底：它被严格解析，不认识的键、类型不对的值都是错误，此时 `status`、`stop`、`install` 会按内置默认运行并告警（别名不可用），其余命令以退出码 3 结束并打印修法。`defaults` 里的凭据（`api_key`、密钥头、代理口令）只随它的目标继承：实例的 `base_url`/`proxy` 指向别处时不会带上它，`aproxy config show --effective` 会列出被扣下的项。字段说明见 skill 文档 [settings-json.md](.claude/skills/aproxy-cli/references/latest/settings-json.md)。

### 保活（SSE 心跳）

保活适用的请求（`keepalive.interval_secs` > 0、`delivery` 不是 `"forward"`，且按 `keepalive.trigger` 命中：`Accept` 含 `text/event-stream`，或请求体顶层 `"stream": true`；默认 `any` 任一即可）从**首轮**起就走保活通道：

- 任一次尝试上游回 2xx + 未压缩的 `text/event-stream` 时，立即把上游真实的状态码与响应头转给客户端（没有 format 程序声明 `response`、`stream` 或 `decide` 时）；否则约一个保活间隔后先提交骨架头（200 + SSE）。「需要重试」本身不提交，重试几次后很快成功的请求拿到的仍是上游真实响应。
- `"stream": true` 的非 SSE 流（如 Ollama 原生 API 的 NDJSON）在不重试时原样直通；若该上游常需重试（重试拖过一个保活间隔会提交 SSE 骨架头，改写 content-type 并混入心跳），建议该实例设 `[keepalive] trigger = "accept"`。
- 提交之后，无论是在等首字节、上游在途、缓冲上游流还是退避，都按 `keepalive.interval_secs` 发 SSE 注释心跳；响应体仍是缓冲完整、校验无误后才回放，失败尝试的数据不会混入。
- 为避免往压缩流里插入明文心跳，保活适用的请求发往上游时 `accept-encoding` 一律改为 `identity`——这是对「完全透传」的一个有意例外，代价只是上游到本机这一段多传一些字节；不适用保活的请求不改写。
- `"stream": false` 的普通请求没有保活通道；以 `forward` 方式投递的请求与 `keepalive.interval_secs = 0` 同样没有。

### format 程序与官方 aproxy-format

`[[format]]` 条目让 aProxy 在请求的几个固定时刻调用一个外部程序：发往上游之前改写请求（协议转换、选 key、按模型选渠道），返回客户端之前改写响应，一次尝试失败之后决定要不要重试，等待期间生成心跳，请求结束之后得到通知。aProxy 本体不含任何转换逻辑；它与程序之间走 stdin/stdout，一行 JSON 一问一答，任何语言都能写。

**列表顺序就是请求方向的顺序**：写在最前面的离客户端最近，最后面的离上游最近；响应按相反的顺序经过它们。

| 键 | 默认 | 说明 |
|---|---|---|
| `name` | `command` 的文件名主干 | 列表内唯一，1 到 64 字节；出现在日志、`status` 与错误响应的 `details.format` 里 |
| `command` | 必填 | 直接执行，不经过 shell。相对路径相对 `~/.aproxy`；裸命令名先找 `~/.aproxy/bin/`，再找 `PATH`。Windows 上不接受 `.cmd` 与 `.bat`：要用脚本，就把解释器写在 `command`、脚本写在 `args` |
| `args` | `[]` | 原样传给程序；worker 的工作目录是 `~/.aproxy`，相对路径相对它 |
| `env` | `{}` | 追加给程序的环境变量，密钥放这里；`config show` 只列名字 |
| `process` | `"spawn"` | `spawn` 每条消息起一个进程；`persistent` 是常驻进程池，`stream`、`notify` 与要跨请求记状态的程序必须用它 |
| `stages` | 必填 | 这个程序参与的阶段，见下 |
| `required` | `true` | 程序自己出错（崩溃、超时、回信不合法、已熔断）时怎么办：`true` 时请求侧的这次尝试失败并照常重试，连续出错到熔断后以 503 `format_unavailable` 结束；`false` 时跳过这一层 |
| `pool_max` | `4` | `persistent` 进程池的上限；正在处理流的 worker 不计入。状态要在所有请求之间共享的程序配 `1` |
| `worker_idle_secs` | `300` | 空闲多久回收一个 worker；`0` 表示不回收 |
| `timeout_secs` | `30` | 一条消息等回信的时限；`0` 表示不限 |
| `decide_timeout_secs` | `2` | `decide` 阶段的时限；超时按「不表态」处理，不会因此结束请求 |
| `breaker_failures` | `5` | 连续这么多次进程级失败就熔断；`0` 表示不熔断 |
| `breaker_cooldown_secs` | `30` | 熔断之后过多久放一条消息试探；不能为 `0` |

七个阶段：

| 阶段 | 什么时候 | 用来做什么 |
|---|---|---|
| `request` | 每个请求一次，第一次尝试之前 | 转换协议、选 key 与渠道、为这个请求选投递方式 |
| `retry` | 重试之前要重做请求时，带上一次的失败原因与上游的状态、响应头 | 失败后换 key、换渠道 |
| `decide` | 一次尝试失败之后 | 重试还是停止、等多久、要不要重做请求 |
| `response` | 成功的响应交付之前，整体处理 | 转换非流式响应 |
| `stream` | 成功的响应逐块经过 | 边收边转换流式响应 |
| `heartbeat` | 每一拍保活心跳 | 发出客户端协议认得的心跳 |
| `notify` | 请求结束之后，aProxy 不等回信 | 按结果积累判断 |

下面三种写法在启动时报 `config_conflict`：

- 同一个程序同时声明 `response` 与 `stream`；
- 声明 `stream` 或 `notify` 的程序不是 `persistent`；
- 请求方向上，排在声明了 `retry` 的程序之后的程序没有声明 `retry`（前面的程序在重试时给出了新的请求，后面的不重做，发出去的就是上一次的结果）。

**投递方式与 format**：`delivery` 只是每个请求的起点。`request` 阶段的程序可以把单个请求改成 `forward` 或 `retry`；`forward` 请求在一个字节都没写给客户端之前失败时，声明了 `decide` 的程序可以让它当场转为 `retry`。所以推荐保持默认的 `retry`，由官方的 `adaptive` 把没失败过的请求改成真流式的 `forward`：它不可用时，一切回到无限重试。把 `delivery` 配成 `"forward"` 时，请求体直接流向上游（链上有程序要请求体或声明了 `decide` 时除外，那时先读完），响应经过声明了 `stream` 的程序边收边转；声明 `response` 的程序要整个响应，那一层会失去真流式。只配了请求侧的转换、没有配 `stream` 或 `response` 时，客户端收到的仍是上游协议的响应。

**官方 `aproxy-format`** 把常用的功能做成角色，一个子命令一个角色，各占一个 `[[format]]` 条目，也能与自己写的程序混用：

| 角色 | 做什么 | `args` 的最小写法 | `stages` |
|---|---|---|---|
| `convert` | Anthropic Messages、OpenAI Chat、OpenAI Responses 三种协议互转，流式与非流式都支持 | `["convert", "--from", "anthropic_messages", "--to", "openai_chat"]` | `request`、`stream`（排在 `route` 之后时加 `retry`） |
| `keys` | 多个 key 轮换：轮询、加权或随机，失败的 key 冷却（尊重 `Retry-After`），可按对话粘性 | `["keys", "--config", "keys.toml"]` | `request`、`retry` |
| `route` | 按模型选渠道：别名、优先级、权重、故障切换，每个渠道可带自己的凭据与超时覆盖 | `["route", "--config", "route.toml"]` | `request`、`retry` |
| `policy` | 按状态码、失败原因与响应体的规则决定重试或停止，停止时可以回一个客户端协议的错误 | `["policy", "--config", "policy.toml"]` | `decide` |
| `adaptive` | 按请求在 `forward` 与 `retry` 之间选：没失败过的走真流式，失败过的固定用重试 | `["adaptive", "--config", "adaptive.toml"]` | `request`、`decide`、`notify` |
| `rewrite` | 增删请求头，用 JSON Pointer 改请求体，给模型改名 | `["rewrite", "--config", "rewrite.toml"]` | `request`（排在 `route` 之后时加 `retry`） |
| `heartbeat` | 把保活心跳换成客户端协议的事件（Anthropic Messages 的 `ping`） | `["heartbeat"]` | `heartbeat` |

一条完整的链（Claude Code 接到一个只有 OpenAI Chat 接口、有两个 key 的中转）：

```toml
base_url = "https://relay.example.com"

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false          # 判断类程序：坏了就跳过，请求照常无限重试
pool_max = 1              # 它的判断记在一个进程里
worker_idle_secs = 0

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1              # key 的冷却记在一个进程里
worker_idle_secs = 0
```

不必手写：四个工具子命令负责生成与验证。

```powershell
aproxy-format init adaptive-relay --output ~/.aproxy   # 生成一套起步配置并列出下一步；场景还有 claude-relay、key-rotation、openrouter
aproxy-format check ~/.aproxy/config.toml              # 校验整条链与各角色的配置，不执行配置里的命令
aproxy-format try ~/.aproxy/config.toml --request sample.json   # 拿本地样例干跑整条链，不连上游
aproxy-format translate --from anthropic_messages --to openai_chat < request.json   # 一次性转换一个请求
```

`init` 不写密钥（配置里只有环境变量的名字），也不覆盖已有的文件。`check` 通过时会说明查了什么、没查什么：环境变量有没有设置、实例的其余配置都不在它的范围内，后者用 `aproxy doctor` 检查。`try` 默认只在进程内模拟官方角色，链上的第三方程序显示为跳过；加 `--allow-external` 才会启动它们，这时样例与环境变量会交给那个程序。`check` 与 `try` 可以用 `--home <目录>` 指明实例的 home，相对路径就按实例的方式解析。每个子命令都有 `--help`。

已知的限制：

- **Chat 上游的流式工具调用**：把 OpenAI Chat 的流转成 Messages 或 Responses 时，工具调用的参数要到这次调用结束才一起交付，生成很长的参数期间客户端看不到进度；内容不丢。
- **Messages 上游转给 Chat 客户端的 thinking**：Chat 协议没有地方放 thinking 的签名。带着工具结果续接时，没有签名的 thinking 块被丢掉，开着 thinking 的上游可能拒绝这次续接。
- **Chat 与 Responses 路径上的心跳只是传输层保活**：`heartbeat` 角色在这两种协议上原样保留 aProxy 的心跳字节（SSE 注释），不造事件，所以连接不会断，但喂不住按事件计时的客户端看门狗。Anthropic Messages 路径上发的是原生的 `ping` 事件。
- 多个 worker 各记各的状态，所以 `keys` 与 `adaptive` 要配 `pool_max = 1`、`worker_idle_secs = 0`；实例重启后冷却与判断记录从头开始。

协议的逐字段说明、各角色配置文件的写法、怎样自己写一个 format 程序，见 skill 文档 [aproxy-format](.claude/skills/aproxy-format/SKILL.md)；`[[format]]` 在 aProxy 一侧的全部行为见 [config-toml.md](.claude/skills/aproxy-cli/references/latest/config-toml.md)。

### 入站来源校验

代理默认只服务本机 CLI 类客户端，并对网页类来源设防（详见 [SECURITY.md](SECURITY.md)）：

- **Host**（防 DNS 重绑定）：监听回环地址时，只放行 `localhost` / `127.0.0.1` / `[::1]` 与监听地址自身的主机名；`allowed_hosts` 的条目在此基础上**追加**（忽略端口、不区分大小写）。监听非回环地址且 `allowed_hosts` 为空时不做 Host 校验。
- **Origin**：任何携带 `Origin` 头的请求默认拒绝（只有浏览器、Electron/WebView 类客户端会发），除非精确匹配 `allowed_origins`（不区分大小写，忽略末尾 `/`）；与监听地址无关。
- 两项都可写 `"*"` 关闭；`[]` 等同未配置。每实例的 `config.toml` 与 settings.json 的 `defaults.allowed_hosts`/`defaults.allowed_origins` 均可配置（toml 优先）。
- 被拒请求在本地返回 **403**，错误文案点名对应配置项；它从未转发上游、不注入 `api_key`、不进入重试，对被放行的请求无任何影响。
- 受影响的客户端：Cherry Studio、Open WebUI 等会发 `Origin` 的应用，需把其 Origin 加入 `allowed_origins`。Claude Code 等 CLI 客户端不发 `Origin`、Host 为 `127.0.0.1:端口`，不受影响。

## 安全须知

aProxy 是本机单用户代理：它持有并向每个转发请求注入上游密钥，代理端口本身没有鉴权。

- 默认只监听回环地址（`127.0.0.1`）。**不要把 `listen_addr` 改成 `0.0.0.0` 或局域网地址**——那等于让同网段的任何主机免鉴权使用你的密钥；确需对外开放时，启动、`startup.log` 与 `aproxy doctor` 会给出警告。
- `api_key` 以明文存放在 `config.toml`。aProxy 的主目录（默认 `~/.aproxy`）只让当前用户访问：新建时就是这样，已有而权限过宽的会在下一次写入时被收紧，`aproxy doctor` 会指出别的用户读得到的主目录。放在主目录之外的配置文件（`--config` 指向别处）请自己收紧权限。
- 控制通道（`status`、`stop` 等命令与实例之间）只接受启动实例的那个用户（unix 上另外放行 root），不开网络端口。
- format 程序（`[[format]]` 条目）会按配置执行任意命令；只配置你信任的程序，且不要让他人能修改你的 `config.toml`。
- 日志与 `status` 对 URL 内嵌凭据与查询串的值打码（`?key=***`），可安全粘贴分享。

完整的威胁模型与漏洞报告方式见 [SECURITY.md](SECURITY.md)。

## 开发

```powershell
cargo test --workspace --locked           # 全量测试（含 aproxy-envelope / aproxy-format）
cargo clippy --workspace --all-targets --locked -- -D warnings   # 必须零警告（项目纪律）
cargo fmt --all -- --check
```

默认特性带桌面界面，编译之前先构建一次前端（见「源码构建」）。无界面的 headless 版本同样要过一遍，排除 `aproxy-desktop`（它是 workspace 成员，`--no-default-features` 管不到它）：

```powershell
cargo test --workspace --exclude aproxy-desktop --no-default-features --locked
cargo clippy --workspace --exclude aproxy-desktop --all-targets --no-default-features --locked -- -D warnings
```

界面开发：在 `aproxy-desktop/ui` 里 `npm run dev`，浏览器里打开的是带审核工具条与模拟窗口的原型；改完 `npm run build`（先跑 svelte-check，零警告），再编译 aproxy。

架构与开发文档见 [`docs/`](docs/)；面向 agent 的开发文档在 `.agents/docs/`。

## License

MIT
