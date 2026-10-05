# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
0.1.0 是第一个正式版本；此前的 `0.1.0-alpha.*` 预发布版本不再维护，也不在此记录。

## [Unreleased]

### 新增

- **可自定义心跳**：config.toml 的 `keepalive_heartbeat` 指定每个保活间隔写给客户端的字节，默认仍是 SSE 注释 `: keepalive`。aProxy 不认识协议，心跳的形态由用户按客户端决定，例如解析器容不下注释的客户端可改成空行 `"\n"`。启动与 `aproxy doctor` 校验它写完后客户端的 SSE 解析器停在事件边界（非空、以换行结尾、含 `event:`/`data:` 等字段行时以空行收尾），否则心跳会和回放的第一个上游事件拼在一起。

### 变更

- **skill 文档改为英文并重写**：aproxy-cli 与 aproxy-format 两个 skill 按 skill-creator 标准重写，面向操作 aProxy 的 agent。每条事实都对照源码核实；程序输出的中文原文照录，并附英文释义。aproxy-cli 新增 `clients.md`（接入各 agent 客户端）与 `troubleshooting.md`（症状 → 原因 → 处理），`compatibility.md` 只讲版本判定以及 0.1.0 与开发线的差异。
- **断开提示覆盖所有客户端**：客户端在「只收到心跳」的等待中（响应已提交、尚未收到任何上游真实字节）等了至少 60 秒才断开时，日志提示检查客户端的流空闲超时，并带上已等秒数。原来只在等满 590 秒后提示 Claude Code 的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`，Codex（默认 300 秒）、Qwen Code（240 秒）到点断开时没有任何提示。

### 移除

- 去掉只为 0.1.0 之前的预发布版本保留的兼容：config.toml 不再把 `upstream_url` 当作 `base_url` 读取（仍在用旧名的配置请改名，否则启动报 `base_url` 为空）；不再读取预发布版本写下的纯数组格式恢复记录（`run/<端口>.restore`）。

### 修复

- 相对路径的 `--config` 在命令行入口展开 `~` 并转成绝对路径后再转发给守护进程、写入恢复记录；此前原样保存，`aproxy restore` 若在别的目录运行会把该实例的恢复记录当失效清掉。
- `aproxy install --skills-only` 可以同时指定版本（`aproxy install <版本> --skills-only`），与 latest 查询失败时的提示一致。
- `aproxy stop` 等不到实例退出时，提示改为跨平台、会核验进程身份的 `aproxy stop <端口> --force`（此前在所有平台都建议 `taskkill`）。
- `settings.json` 的 `default_config` 指向的文件不存在时，只有要读配置的命令（启动、`aproxy config` 查看或修改内容）报错退出；`status`、`stop`、`doctor`、自动拉起的 `install --continue` 等不再被拦，报错信息推荐的 `aproxy config --clear-default` 也能直接执行（此前所有不带 `--config` 的命令都在分发前退出 1）。
- `settings.json` 解析失败时，`aproxy alias add/remove` 与 `aproxy config --set-default/--clear-default` 拒绝修改并提示先修复或删除该文件，不再用默认值写回、冲掉原有的全部别名。
- 相对路径的 `log_file` / `--log-file` 按文档相对 `APROXY_HOME` 解析（此前实际相对当前工作目录）。
- **外部转换器纳入心跳节拍**：保活适用的请求，请求转换、响应转换与回放前解码期间照常提交骨架头并发心跳。此前 format 慢（`timeout_secs = 0` 时没有上界）时，客户端可能在收到任何字节之前一直干等。请求转换若在骨架头提交之后才失败，以 `proxy_transform_failed` 终态 error 事件收场（提交前失败仍是 502）。
- `override_headers` 里配置的 `accept-encoding` 对保活适用的请求不生效（这些请求一律以 `identity` 发往上游），启动时 warn 一次说明，不再静默覆盖。
- 客户端上传大请求体（超过 1 MiB、已溢写到磁盘）途中断开时，`req-*.spooltmp` 临时文件不再残留到下次启动。
- **挂死实例不再从视野里消失**：进程还在、却不应答控制通道的实例（按 pid + 创建时间核验），`aproxy status` 不再删掉它的注册记录与日志，而是单列为「无响应」并提示 `aproxy stop <端口> --force`；`stop`/`restart` 的 `--force` 经注册记录找到它并核验后终止，`stop all --force` 一并处理。此前一次 status 就会删掉记录，`--force` 随即找不到它，尚未被看门狗收养的挂死实例也失去了被收养、处决重拉的依据。
- `aproxy stop` 有目标未能确认停止（等不到退出、`--force` 失败、目标挂死而未加 `--force`）时以 1 退出，脚本可据此判断；此前一律退出 0。
- **看门狗即时重拉崩溃实例**：进程退出的事件一到就处理，崩溃实例在启动耗时内（通常不到 1 秒）回来；此前要等下一个扫描周期，默认最长约 30 秒断流。`stop --force` / `restart --force` 相应改为终止前先摘掉恢复记录（终止失败再放回），强停的实例不会被看门狗拉回；`restart --force` 也不再与看门狗同时拉起新实例争抢端口。

## [0.1.0] - 2026-10-04

0.1.0 是首个稳定版，在 `0.1.0-alpha.17` 的基础上收口。

### 新增

- **入站来源校验**：新增配置字段 `allowed_hosts`、`allowed_origins`（每份 `config.toml` 与 `settings.json` 全局默认均可配置，toml 优先）。Host 校验防 DNS 重绑定，Origin 校验防网页借本机代理调用上游；被拒请求在本地返回 403，不转发上游、不注入 `api_key`、不进入重试。用法见 README「入站来源校验」。
- **保活触发条件 `keepalive_trigger`**：新增配置字段（`config.toml` 与 `settings.json` 全局默认均可配置，toml 优先），取值 `accept`（`Accept` 含 `text/event-stream`）/ `body_stream`（请求体顶层 `"stream": true`）/ `any`（任一，默认）；非法值启动报错、`aproxy doctor` 报 settings 里的非法值，`config --show` 展示。`forward_only` 下无效。
- **保活首轮提交与全程心跳**：保活适用的请求从首轮起保活——任一次尝试拿到上游 2xx + 未压缩的 `text/event-stream` 头时立即转发上游真实状态码与响应头，否则约一个保活间隔后提交骨架头（200 + SSE）；「需要重试」本身不提交，重试几次后很快成功的请求仍拿到上游真实头。上游回 2xx 但非 SSE（如 `"stream": true` 的 NDJSON）时该次尝试不提交骨架，成功即原样直通（若该上游常需重试，建议该实例设 `keepalive_trigger = "accept"`）。等首字节、上游在途、缓冲、退避全程发 SSE 注释心跳，响应体仍缓冲完整后回放。已提交的响应等待 590 秒以上后客户端断开时，日志会提示 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`。
- **`aproxy install --pre` 与更新通道**：`install` 的 `latest` 分通道——当前是正式版只取正式版，当前是预发布默认含预发布，`--pre` 显式含预发布；只认 `vX.Y.Z[-(alpha|beta|rc).N]`、按版本号取最大，通道为空时保持现状。
- **滚动升级失败回滚**：滚动重启时某实例在新版本下起不来，`install` 用旧二进制按原参数把它拉回、中止滚动并以非零退出；unix 交换前保留旧二进制为 `bin/aproxy.old`。
- **接入 Claude Code 说明**：README、安装指南与 skill 补充 `ANTHROPIC_BASE_URL` 与必设的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`（原因与实测依据见 README「接入 Claude Code」）。
- **非回环监听告警**：`listen_addr` 不是回环地址时，`start`、`startup.log` 与 `aproxy doctor` 都会告警；配置了 `api_key` 时措辞更重。
- **安装脚本预发布开关**：`install.sh --pre`（`curl … | sh -s -- --pre`）、`install.ps1 -Pre`、`install.cmd --pre`/`-Pre`，三者都支持环境变量 `APROXY_PRE=1`；`install.ps1` 另支持 `APROXY_DL_PROXY`、`APROXY_NO_SKILLS`（`irm | iex` 无法传参）。
- **SECURITY.md**：威胁模型与漏洞报告方式。
- **README 平台支持矩阵**与 npm / cargo 安装渠道说明；README_EN 与中文版逐节对齐。

### 变更

- **Origin 默认拒绝**：任何携带 `Origin` 头的请求默认被拒绝（只有浏览器、Electron/WebView 类客户端会发），需要时用 `allowed_origins` 放行。
- **凭据脱敏统一**：所有日志与 `status` 的「最近错误」里，URL 的 userinfo（用户名或密码）整体打成 `***@`，查询串保留键名、值变 `***`（`?beta=true` 显示为 `?beta=***`），片段整体遮掉；`config --show` 对转换器 `args` 中密钥旗标之后的值与 `extra` 打码。依赖旧日志格式做文本匹配的脚本需要调整。
- **进程身份判定去名称化**：实例与看护者的身份按 pid + 进程创建时间核验，不再依赖二进制文件名——改名部署的实例同样受看门狗看护，`stop --force` 也不再比对镜像名。注册表与 IPC 新增 `process_start` 字段。
- **restart 先预检后停止**：停旧实例前先校验新配置，预检失败不动旧实例并报告原因；`restart all` 逐个预检、失败的跳过、最后汇总并以非零退出；新实例启动即退出时展示 `startup.log` 的新增内容。
- **安装脚本选版规则**：只认 `vX.Y.Z` 与 `vX.Y.Z-(alpha|beta|rc).N` 格式的 tag（排除 `format-v*` 与 `v0.1.0-alpha.12t3` 这类历史测试 tag）并跳过 draft；默认安装版本号最大的稳定版，仓库尚无稳定版时（0.1.0 发布前）回退到版本号最大的预发布并提示；`--pre` 让预发布也参与，取版本号最大者（按版本号而非创建时间）。
- **保活适用的请求发往上游时 `accept-encoding` 改为 `identity`**：往压缩流里插入明文心跳会让客户端解压失败，这是对「完全透传」的有意例外；不适用保活的请求不改写。上游无视该要求仍回压缩体、而响应头已以骨架提交时，回放前完整解码（截断/损坏则以终态 SSE `event: error` 事件收场）。受限重试路径（`bounded_retry_paths`）在保活请求上达上限时：响应头尚未提交（常态）照常透传真实失败响应；只有已被保活节拍提交了骨架头的，才改以终态 SSE `event: error` 事件收场。
- **`npm` 预发布发 `next` 标签**：预发布发在 `next`，正式版发 `latest`；正式版发布后 `npm i -g @meowo/aproxy` 只会装到正式版，预发布需 `@meowo/aproxy@next`。
- **外部转换器错误按成因表述**：进程/管道层故障、format 自报错误、协议违规的 502 文案各不相同；官方 `aproxy-format` 的 `client_format = "auto"` 改为只放行同协议，跨协议路由在请求侧直接报错并提示显式声明（该项随 `aproxy-format` 新版本生效）。
- 新装机的安装收尾提示改为先 `aproxy config --baseurl … --api-key …` 再 `aproxy`。

### 修复

- **转换器串包**：format 的输出校验为恰好一行合法信封后才归还 worker；多输出、非法输出的 worker 被剔除（此前下一个请求可能读到上一个请求的输出）。复用到已死的空闲 worker 时自动换新 worker 重试一次。
- **Claude Code 的流式请求进不了保活通道**：Claude Code 的主请求是 `Accept: application/json` + 请求体 `"stream": true`，alpha 只看 `Accept` 的判定让它永远得不到心跳；同时保活通道此前不覆盖「上游在途」的等待。现在默认 `keepalive_trigger = any` 并全程发心跳。
- **本地磁盘缓存收尾写失败不再静默**：响应 spool 的最后一块写失败现在返回 502（保活通道为 SSE error 事件），不再把被截断的响应当作成功回放。
- **看门狗重拉换端口残留**：配置换了端口后重拉，旧端口的恢复记录被清理；`restore` 报告实际端口。
- **`stop --force` 不再被复活**：强杀成功后清理该实例的恢复记录，看门狗与 `aproxy restore` 不会再把它拉回来。
- **安装脚本**：`install.cmd` 无参运行时的 PowerShell 语法错误；`install.ps1` 在 `irm | iex` 下会关闭用户的 PowerShell 会话、并改写全局 TLS 设置；`install.sh` 在下载被截断时可能执行半截脚本、skill 解压失败会中止已成功的二进制安装；三个脚本都不再错选 `format-v*` 发版线的 release。
- **Linux gnu 产物的 glibc 下限**：alpha.17 的 gnu 产物需要 glibc 2.39，Debian 12、Ubuntu 22.04 等系统启动即失败；现在发布流程用 cargo-zigbuild 按 glibc 2.28 下限构建并断言。低于下限的系统与 musl 系统自动改用静态链接的 musl 产物，落位前 `--version` 自检，gnu 产物跑不起来时自动改拉 musl；npm 转发器在 glibc 过低时给出清晰报错并优先使用已装的 musl 平台包。
- **发布与 CI**：测试与 clippy 一律带 `--workspace`（此前 aproxy-envelope 与 aproxy-format 的测试从未进入门禁），ubuntu 增加 clippy；发布前在 windows 与 ubuntu 上复跑全量门禁；npm 与 crates.io 发布幂等，任一渠道失败后可单独重跑。
- **文档**：转换器示例补全响应侧 `extra` 并改用不依赖守护进程工作目录的路径写法；删除不存在的 `aproxy watchdog` 子命令与 `status` 看护者告警的描述；aproxy-format skill 补充协议违规排障。

### 安全

- 入站 Host / Origin 校验（见上），挡住 DNS 重绑定与网页跨站调用。
- 凭据脱敏统一出口（见上），日志与 `status` 可安全粘贴分享。
- 非回环监听告警（见上）。

### 已知限制

- **macOS**：提供预构建二进制，但未经真机验证；看门狗、`aproxy install` 与依赖进程查询的实例管理用到 Linux 专有接口（`/dev/shm`、`/proc`），在 macOS 上不可用或退化，需要在真机上补齐。
- **`aproxy-format` 的跨协议转换只支持非流式**：跨协议的 SSE 流式响应不支持（响应侧转换报错后透传上游原始响应）；同协议 SSE 原样直通。
- **Windows PowerShell 5.1** 用 `-File` 本地运行 `install.ps1` 会解析失败（文件为 UTF-8 无 BOM），请用 `irm | iex` 或 `pwsh -File`。
- **`"stream": false` 的请求没有保活通道**：没有可注入心跳的响应流，首字节延迟等于完整生成时长，客户端需自行调大超时。
- **CI 的 macOS job 已知为红**（`/dev/shm`、`/proc` 依赖），不进发布门禁，也不设 `continue-on-error`。
- `aproxy status` 目前不展示看护者的运行状态。

### 从 alpha 升级的注意事项

- **会发 `Origin` 的客户端需要配置**：Cherry Studio、Open WebUI 等基于浏览器/Electron 的客户端升级后会收到本地 403，需把它们的 Origin 加入 `allowed_origins`。Claude Code 等 CLI 客户端不发 `Origin`、Host 为 `127.0.0.1:端口`，不受影响。
- **接入 Claude Code 必须设 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`**（推荐 `86400000`）：aProxy 的注释心跳覆盖不了 Claude Code 的事件级空闲超时（默认 600 秒），不设则超过 10 分钟的重试期或长生成会被客户端断开重发；`API_TIMEOUT_MS` 不控制这道闸。
- **日志里的查询串值被打码**：用日志调试 `bounded_retry_paths` 时，日志显示 `?beta=***`，匹配仍按真实查询串。
- **升级后需重启实例**才会采用新行为（`aproxy install` 会逐实例滚动重启；手动升级用 `aproxy restart all` 或 `stop all` + `restore`）。旧版本实例的注册记录没有 `process_start`，看护者对它们按保守策略处理（挂死的旧实例不被收养，`--force` 需 IPC 确认）。
- **`aproxy install` 的 `latest` 现在分通道**：从 alpha 升级时默认仍在预发布通道；0.1.0 正式版之后，正式版用户不会再被带到预发布，需要时加 `--pre`。
- **npm 渠道的旧二进制**更新 skill 会失败（skill 包由单包改为多 skill 总包），不影响安装，下次 `aproxy install` 自愈。
- 配置文件向前兼容：新版本读旧配置取默认值；旧二进制读到新字段（如 `allowed_hosts`）静默忽略。
