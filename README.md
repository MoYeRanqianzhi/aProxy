# aProxy

> 「飘风不终朝，骤雨不终日。」——《道德经》

**本地 API 代理，为 agent 软件的每一次请求护道。**

上游限流、断流、超时、五百年一遇的抖动——aProxy 在本地把请求完整接住：失败**无限重试**（指数退避，封顶可配），流式请求（`Accept` 含 `text/event-stream` 或请求体 `"stream": true`）期间注入 SSE 心跳保活，成功后按原字节回放。客户端零感知：它以为的那次请求，只是慢了一点。（两个显式例外：`delivery = "forward"` 模式显式放弃重试保障，换取真流式直通；`[[retry.rules]]` 命中的请求达到 `max_attempts` 即透传真实响应、响应头已被保活心跳提交的流式请求则以终态 SSE error 事件收场——均见「配置文件」。）

[English](README_EN.md)

## 其术

- **无限重试** —— 夫唯不争，故天下莫能与之争。网络错误、4xx/5xx、错误 JSON（200 携带 error）皆触发重试；客户端断开即中止上游请求（计费保护）。两个**显式出口**：`delivery = "forward"` 模式（显式放弃重试，换取请求体与响应的流式直通）与 `[[retry.rules]]`（对确定性报错的上游端点，达到 `max_attempts` 即透传真实响应，不再无限等待；仅当流式请求的响应头已被保活心跳提交时，改以终态 SSE error 事件告知）。
- **完全透传** —— 大音希声，大象无形。路径、查询、请求头原样转发；控制通道走独立命名管道，代理端口只做透传一件事。
- **外部转换器（可选）** —— 化而欲作，吾将镇之以无名之朴。请求/响应可整流交给外部 format 程序改写（一行 JSON 信封进出，任何能读写 stdin/stdout 的程序都行）：OpenAI ↔ Anthropic 协议转换、多 key 轮换、多模型多渠道聚合。本体只加编排代码，转换逻辑全部外置；官方示例 `aproxy-format` 二进制开箱即用（独立发版）。常驻进程池复用到已死的空闲 worker 时，会自动换新 worker 重试一次。**限制**：`aproxy-format` 的跨协议转换（如 Anthropic ↔ OpenAI）只支持**非流式**，跨协议的 SSE 流式响应不支持——响应侧转换报错后透传上游原始响应，客户端会收到渠道协议格式的流；同协议 SSE 原样直通，多 key 轮换与同协议聚合不受影响。
- **守护进程** —— `aproxy` 后台启动（按拉起规格分离子进程，关终端不掉；密钥只在拉起规格与期望状态记录里，不出现在实例与看护者的进程命令行上）；`status` / `stop` / `logs` / `restore` 全套实例管理。
- **看门狗** —— 天网恢恢，疏而不失。全局看护进程自动重拉崩溃/挂死的实例（默认开启；实测 +2.4% 体积、+2.9MB 常驻、转发热路径零损耗）。身份按进程 ID + 创建时间核验，与二进制文件名无关。连续重拉失败不放弃：转入每 300 秒一次的慢速重试，直到实例起来或被 `aproxy stop` 删掉期望状态记录；重拉出的实例存活满 60 秒才清空之前的失败计数。每个扫描周期重读 settings，关掉 `watchdog.enabled` 即退出。macOS 暂不支持，见「平台支持」。
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
- `irm | iex` 无法传参，PowerShell 脚本可用环境变量代替：`APROXY_PRE`、`APROXY_NO_SKILLS`、`APROXY_DL_PROXY`（下载代理，与上游请求代理无关）。
- Windows PowerShell 5.1 用 `-File` 本地运行 `install.ps1` 会解析失败（文件为 UTF-8 无 BOM）；请用上面的 `irm | iex`，或 `pwsh -File`。

其他渠道（装好后可用 `aproxy install --adopt` 收编到标准位置）：

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
- **从 0.1.x 升级**：0.1.x 与 0.2.0 起的版本互相控制不了对方的实例，0.1.x 的 `aproxy install` 装不上 0.2.0（下载后报 `--version 输出无版本号` 退出，什么都不改）。按 [CHANGELOG](CHANGELOG.md) 的「从 0.1.x 迁移」停掉实例、重新安装；0.2.0 现在只有预发布，重装时一键脚本要加 `--pre`，否则装回的还是 0.1.x。
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
| macOS（Apple Silicon / Intel） | 提供预构建二进制，但未经真机验证；看门狗、`aproxy install` 与依赖进程查询的实例管理用到 Linux 专有接口（`/proc`），在 macOS 上不可用或退化（需真机补齐，欢迎贡献） |

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
| `aproxy stop [PORT\|all\|别名]` | 停止实例；多实例必须指定端口、`all` 或别名；`--force` 立即强杀（不会被看门狗复活）；目标已崩溃时删掉它留下的期望状态记录 |
| `aproxy restart [PORT\|all\|别名]` | 重启运行中的实例（只重启不启动；改配置生效用）；停旧实例前先预检新配置，不通过则旧实例保持运行；失败时记录保留，`restore` 或看门狗可拉回；`--force` 立即强杀重启 |
| `aproxy logs [PORT\|别名]` | 连接实例实时输出日志；不支持 `all` |
| `aproxy restore` | 恢复崩溃/重启前在运行的实例（按期望状态记录）；配置文件暂时找不到时保留记录并说明；无实例则静默结束（开机自启友好） |
| `aproxy alias add\|remove\|list` | 管理配置别名（存于 settings.json） |
| `aproxy doctor` | 配置体检：settings.json error 级检查 + 别名配置与目录 toml 审查（warning） |
| `aproxy find [关键字]` | 从配置目录发现全部配置；`--aliased/--unaliased --port PORT` 过滤 |
| `aproxy config [选项]` | 查看或修改配置（`--show` 打印当前配置） |
| `aproxy settings show\|get\|set\|unset` | 查看或修改全局设置（`~/.aproxy/settings.json`）：别名、`defaults` 全局默认、看护者、日志、安装更新；settings.json 有错误时也能用，用来修复它 |
| `aproxy install [版本]` | 安装或升级（别名 `upgrade`），逐实例滚动重启；`--variant` 换变体，`--force` 强制安装；见「入门」里的升级说明 |
| `aproxy desktop` | 打开桌面界面（预览版，显示的是演示数据，见下） |

通用参数：`--config <PATH>`（指定配置文件，多开用）、`--listen <ADDR>`、`--proxy <URL>`、`--api-key <KEY>`（只记录进这次启动的拉起规格，`restart`/`restore` 沿用，不写进 config.toml）。

## 桌面界面（预览）

`aproxy desktop` 打开桌面界面。它是一个独立的窗口进程，关掉窗口就退出。窗口本身不启停、不改动任何实例；和其余 aproxy 命令一样，它发现上次的安装没做完时会在后台把安装续完，续作可能逐个重启实例。

- **0.2.0 各 alpha 版的界面是预览版**：里面的实例、请求日志与统计都是演示数据，不是这台电脑上正在运行的 aProxy，界面顶部常驻「预览版 · 演示数据」标签。真实数据在之后的版本接入，在那之前以 `aproxy status`、`aproxy logs` 为准。界面偏好（主题、语言、Mica 开关）这个版本也不保存：关掉窗口再打开，恢复默认。
- Windows 需要 Microsoft Edge WebView2 运行时（Windows 11 自带，Windows 10 没有时按提示安装）。从终端运行时命令立即返回，窗口独立运行。
- Linux 需要图形会话与 WebKitGTK 4.1（2.40 或更新）。带界面的版本动态链接 WebKitGTK，缺这些库时整个程序都起不来（连 `aproxy --version` 也不行），没有图形界面的机器请用无界面的 headless 版本——一键安装脚本与 `aproxy install` 在缺库的机器上会自动装它；musl 构建只有 headless 版本。
- macOS 的带界面版本只保证编得过，窗口未经真机验证（和整个 macOS 支持一样，见「平台支持」）。
- 同一个 home 只开一个窗口，再次运行会提示已经在运行。
- headless 版本（`aproxy --version` 括号里的第一段是 `headless`）没有界面，运行 `aproxy desktop` 会说明怎样换装带界面的版本（`aproxy install --variant desktop`）。

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
# delivery = "retry"                     # "retry"（默认，缓冲并重试）/ "forward"（仅转发：流式直通，无重试/缓冲/心跳）
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
# body_budget_mb = 8                    # 已解析，本版本尚无效果

# [[format]]                             # 外部转换器：请求/响应/心跳每阶段至多一个
# name = "convert"
# command = "~/.aproxy/bin/aproxy-format"
# args = ["run", "--config", "~/.aproxy/agg.toml"]   # 不再有 extra 键，配置路径走 args 或 env
# process = "persistent"                 # spawn（默认）/ persistent（有状态程序必须用）
# stages = ["request", "retry"]          # request / response / heartbeat；加 retry = 每次重试前重新转换
# env = { KEYS = '{"keys": ["sk-a", "sk-b"]}' }  # 额外环境变量（值不在 --show 中显示）
```

多 key 轮换默认只在**请求之间**生效：同一请求的重试沿用转换后的同一个 key（请求只转换一次，重试重放转换产物）。需要每次重试都重新转换时，在请求方的 `stages` 里加上 `"retry"`。

运行数据在 `~/.aproxy/`：`run/`（实例记录 `<端口>.instance.json`、期望状态记录 `<端口>.restore.json`、看护者记录 `watchdog.json`；启动瞬间还有短暂存在的拉起规格交接文件）、`logs/`（守护日志，按启动随机命名、自动清理与轮转，地址经 IPC 向实例询问；可用 `log_file` 自定义去向）、
`spool/<pid>-<短哈希>/`（磁盘缓存临时文件，每个实例进程一个目录；实例停止时删掉，崩溃留下的由下一个启动的实例或看门狗清理）、
`settings.json`（整个 home 的程序状态：`aliases`、`default_config`、`config_dirs`，`defaults`（config.toml 键的全局默认），以及 `watchdog`、`logs`、`instances`、`update`、`ui` 各段）。

`settings.json` 由 `aproxy settings show|get|set|unset` 读写（键是点号路径，例如 `watchdog.enabled`、`defaults.max_request_mb`）；也可以 `aproxy alias add|remove` 与 `aproxy config --set-default|--clear-default` 改各自的键。手工编辑只在文件无法解析时作为兜底：它被严格解析，不认识的键、类型不对的值都是错误，此时 `status`、`stop`、`install` 会按内置默认运行并告警（别名不可用），其余命令以退出码 3 结束并打印修法。`defaults` 里的凭据（`api_key`、密钥头、代理口令）只随它的目标继承：实例的 `base_url`/`proxy` 指向别处时不会带上它，`aproxy config --show` 会列出被扣下的项。字段说明见 skill 文档 [settings-json.md](.claude/skills/aproxy-cli/references/latest/settings-json.md)。

### 保活（SSE 心跳）

保活适用的请求（`keepalive.interval_secs` > 0、`delivery` 不是 `"forward"`，且按 `keepalive.trigger` 命中：`Accept` 含 `text/event-stream`，或请求体顶层 `"stream": true`；默认 `any` 任一即可）从**首轮**起就走保活通道：

- 任一次尝试上游回 2xx + 未压缩的 `text/event-stream` 时，立即把上游真实的状态码与响应头转给客户端（未配 `response` 阶段的 format 时）；否则约一个保活间隔后先提交骨架头（200 + SSE）。「需要重试」本身不提交，重试几次后很快成功的请求拿到的仍是上游真实响应。
- `"stream": true` 的非 SSE 流（如 Ollama 原生 API 的 NDJSON）在不重试时原样直通；若该上游常需重试（重试拖过一个保活间隔会提交 SSE 骨架头，改写 content-type 并混入心跳），建议该实例设 `[keepalive] trigger = "accept"`。
- 提交之后，无论是在等首字节、上游在途、缓冲上游流还是退避，都按 `keepalive.interval_secs` 发 SSE 注释心跳；响应体仍是缓冲完整、校验无误后才回放，失败尝试的数据不会混入。
- 为避免往压缩流里插入明文心跳，保活适用的请求发往上游时 `accept-encoding` 一律改为 `identity`——这是对「完全透传」的一个有意例外，代价只是上游到本机这一段多传一些字节；不适用保活的请求不改写。
- `"stream": false` 的普通请求没有保活通道；`delivery = "forward"` 实例与 `keepalive.interval_secs = 0` 同样没有。

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
- `api_key` 以明文存放在 `config.toml`，请收紧文件权限（仅当前用户可读）。
- 外部转换器（`[[format]]` 条目）会按配置执行任意命令；只配置你信任的程序，且不要让他人能修改你的 `config.toml`。
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
