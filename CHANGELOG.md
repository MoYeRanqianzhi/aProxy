# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
`0.1.0-alpha.*` 各预发布版本的行为差异记录在 aproxy-cli skill 的
`.claude/skills/aproxy-cli/references/latest/compatibility.md`。

## [0.1.0] - 未发布

0.1.0 是首个稳定版，在 `0.1.0-alpha.17` 的基础上收口。

### 新增

- **入站来源校验**：新增配置字段 `allowed_hosts`、`allowed_origins`（每份 `config.toml` 与 `settings.json` 全局默认均可配置，toml 优先）。Host 校验防 DNS 重绑定，Origin 校验防网页借本机代理调用上游；被拒请求在本地返回 403，不转发上游、不注入 `api_key`、不进入重试。用法见 README「入站来源校验」。
- **非回环监听告警**：`listen_addr` 不是回环地址时，`start`、`startup.log` 与 `aproxy doctor` 都会告警；配置了 `api_key` 时措辞更重。
- **安装脚本预发布开关**：`install.sh --pre`（`curl … | sh -s -- --pre`）、`install.ps1 -Pre`、`install.cmd --pre`/`-Pre`，三者都支持环境变量 `APROXY_PRE=1`；`install.ps1` 另支持 `APROXY_DL_PROXY`、`APROXY_NO_SKILLS`（`irm | iex` 无法传参）。
- **SECURITY.md**：威胁模型与漏洞报告方式。
- **README 平台支持矩阵**与 npm / cargo 安装渠道说明；README_EN 与中文版逐节对齐。

### 变更

- **Origin 默认拒绝**：任何携带 `Origin` 头的请求默认被拒绝（只有浏览器、Electron/WebView 类客户端会发），需要时用 `allowed_origins` 放行。
- **凭据脱敏统一**：所有日志与 `status` 的「最近错误」里，URL 的 userinfo（用户名或密码）整体打成 `***@`，查询串保留键名、值变 `***`（`?beta=true` 显示为 `?beta=***`），片段整体遮掉；`config --show` 对转换器 `args` 中密钥旗标之后的值与 `extra` 打码。依赖旧日志格式做文本匹配的脚本需要调整。
- **进程身份判定去名称化**：实例与看护者的身份按 pid + 进程创建时间核验，不再依赖二进制文件名——改名部署的实例同样受看门狗看护，`stop --force` 也不再比对镜像名。注册表与 IPC 新增 `process_start` 字段。
- **restart 先预检后停止**：停旧实例前先校验新配置，预检失败不动旧实例并报告原因；`restart all` 逐个预检、失败的跳过、最后汇总并以非零退出；新实例启动即退出时展示 `startup.log` 的新增内容。
- **安装脚本选版规则**：只认 `v*` tag 并跳过 draft；默认安装最新稳定版，仓库尚无稳定版时（0.1.0 发布前）回退到最新预发布并提示；`--pre` 取最新创建的 `v*` release。
- **外部转换器错误按成因表述**：进程/管道层故障、format 自报错误、协议违规的 502 文案各不相同；官方 `aproxy-format` 的 `client_format = "auto"` 改为只放行同协议，跨协议路由在请求侧直接报错并提示显式声明（该项随 `aproxy-format` 新版本生效）。
- 新装机的安装收尾提示改为先 `aproxy config --baseurl … --api-key …` 再 `aproxy`。

### 修复

- **转换器串包**：format 的输出校验为恰好一行合法信封后才归还 worker；多输出、非法输出的 worker 被剔除（此前下一个请求可能读到上一个请求的输出）。复用到已死的空闲 worker 时自动换新 worker 重试一次。
- **本地磁盘缓存收尾写失败不再静默**：响应 spool 的最后一块写失败现在返回 502（保活通道为 SSE error 事件），不再把被截断的响应当作成功回放。
- **看门狗重拉换端口残留**：配置换了端口后重拉，旧端口的恢复记录被清理；`restore` 报告实际端口。
- **`stop --force` 不再被复活**：强杀成功后清理该实例的恢复记录，看门狗与 `aproxy restore` 不会再把它拉回来。
- **安装脚本**：`install.cmd` 无参运行时的 PowerShell 语法错误；`install.ps1` 在 `irm | iex` 下会关闭用户的 PowerShell 会话、并改写全局 TLS 设置；`install.sh` 在下载被截断时可能执行半截脚本、skill 解压失败会中止已成功的二进制安装；三个脚本都不再错选 `format-v*` 发版线的 release。
- **Linux gnu 产物**：glibc 过低或 musl 系统自动改用静态链接的 musl 产物，落位前 `--version` 自检，gnu 产物跑不起来时自动改拉 musl；npm 转发器在 glibc 过低时给出清晰报错并优先使用已装的 musl 平台包。
- **文档**：转换器示例补全响应侧 `extra` 并改用不依赖守护进程工作目录的路径写法；删除不存在的 `aproxy watchdog` 子命令与 `status` 看护者告警的描述；aproxy-format skill 补充协议违规排障。

### 安全

- 入站 Host / Origin 校验（见上），挡住 DNS 重绑定与网页跨站调用。
- 凭据脱敏统一出口（见上），日志与 `status` 可安全粘贴分享。
- 非回环监听告警（见上）。

### 已知限制

- **macOS**：代理转发可用；看门狗与 `aproxy install` 暂不支持，需要在真机上补齐。
- **`aproxy-format` 的跨协议转换只支持非流式**：跨协议的 SSE 流式响应不支持（响应侧转换报错后透传上游原始响应）；同协议 SSE 原样直通。
- **Windows PowerShell 5.1** 用 `-File` 本地运行 `install.ps1` 会解析失败（文件为 UTF-8 无 BOM），请用 `irm | iex` 或 `pwsh -File`。
- `aproxy status` 目前不展示看护者的运行状态。

### 从 alpha 升级的注意事项

- **会发 `Origin` 的客户端需要配置**：Cherry Studio、Open WebUI 等基于浏览器/Electron 的客户端升级后会收到本地 403，需把它们的 Origin 加入 `allowed_origins`。Claude Code 等 CLI 客户端不发 `Origin`、Host 为 `127.0.0.1:端口`，不受影响。
- **日志里的查询串值被打码**：用日志调试 `bounded_retry_paths` 时，日志显示 `?beta=***`，匹配仍按真实查询串。
- **升级后需重启实例**才会采用新行为（`aproxy install` 会逐实例滚动重启；手动升级用 `aproxy restart all` 或 `stop all` + `restore`）。旧版本实例的注册记录没有 `process_start`，看护者对它们按保守策略处理（挂死的旧实例不被收养，`--force` 需 IPC 确认）。
- **npm 渠道的旧二进制**更新 skill 会失败（skill 包由单包改为多 skill 总包），不影响安装，下次 `aproxy install` 自愈。
- 配置文件向前兼容：新版本读旧配置取默认值；旧二进制读到新字段（如 `allowed_hosts`）静默忽略。
