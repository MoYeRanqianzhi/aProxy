# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
0.1.0 是第一个正式版本；此前的 `0.1.0-alpha.*` 预发布版本不再维护，也不在此记录。

## [Unreleased]

### 破坏性变更

0.2.0 不能从 0.1.x 原地升级，请按下面的「从 0.1.x 迁移」重新安装。

- **0.1.x 的 `aproxy install` / `aproxy upgrade` 装不上 0.2.0**：它下载 0.2.0 之后先运行一次新二进制的 `--version` 自证版本，这一步报 `--version 输出无版本号` 并以 1 退出（下载本身是成功的，随后那句下载代理的建议不用理会）。它不建安装状态、不动实例、不换二进制，也不更新 skill 文档；在线安装只在 `~/.aproxy/staging/` 下留着下载的文件，可以删掉。`--from` 同样在这一步失败。
- **删除与 0.1.0 互通的兼容层**：0.1.0 的 CLI 与安装器联系不上 0.2.0 的实例，0.2.0 的 CLI 也联系不上 0.1.0 的实例。Windows 上不再在 0.1.0 的控制管道名 `\\.\pipe\aproxy-<端口>` 上应答，控制协议只认 v1（不带 `v` 的请求按 `bad_request` 拒绝），安装宣告只用本 home 的名字，安装状态机去掉只由 0.1.0 写入的 `relaying` 阶段。0.2.0 的实例启动时仍会退役比自己旧的看门狗，迁移时没来得及退出的 0.1.0 看门狗不会留着。
- **`--version` 输出改为永久格式** `aproxy v<版本> (<变体>; <目标三元组>[; x86-64-v3])`，例如 `aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)`：变体是 `desktop`（带桌面界面的版本）或 `headless`（无界面版本），最后一段只在 x86-64-v3 构建上出现。之后的每个版本都按这一格式输出、安装器也按它认版本，格式不再改动。解析过 `aproxy <版本>` 的脚本请改为取首行 `aproxy v` 之后、到空格为止的部分。`aproxy --version` 与 `aproxy -V` 不读写 `APROXY_HOME`、不联网、不拉起任何进程，输出写不出去（例如接到已经关闭的管道）时退出码仍是 0。
- **安装状态挪到 `<home>/install/`**：状态文件由 `run/install.state` 改为 `install/state.json`，备料与下载暂存由 `staging/<版本>/` 改为 `install/staging/<版本>/`，一键安装脚本的下载暂存由 `staging/bootstrap` 改为 `install/bootstrap`。0.1.0 留下的 `run/install.state` 与 `staging/` 不再被读取。
- **`aproxy install` 拒绝低于 0.2.0 的版本**：在线安装指定的版本号与 `--from` 的二进制都一样，`--allow-downgrade` 也不行，报 `目标版本 <版本> 低于 0.2.0，不能安装：…` 并说明怎样回到 0.1.x。0.2.0 的预发布（alpha、beta、rc）不受影响。
- **`aproxy install` 的指令集档参数改名为 `--cpu`**：原来的 `--variant v3|baseline` 改为 `--cpu x86-64-v3|baseline`（与 `--version` 末段的写法一致）。`--variant` 改为选变体，取 `desktop|headless`（见「新增」里的两个版本）。
- **`APROXY_HOME` 必须是绝对路径**：设成相对路径时，除 `--version`、`--help` 外的命令一律报错退出，`install --force` 以 10 结束（此前相对当前工作目录解析，在不同目录下执行的命令会用到不同的 home）。值原样使用、不再去掉首尾空白，为空或只含空白视同未设；不是合法 UTF-8 的值也照用（此前会静默退回 `~/.aproxy`）。未设时的用户目录与一键安装脚本一致：Windows 取 `USERPROFILE`，其他平台取 `HOME`，为空时才向系统查询；命令行路径参数、配置（`log_file`、变换的 `command`）里的 `~` 与读 `~/.npmrc` 也都用这个用户目录。三个一键安装脚本按同一规则处理 `APROXY_HOME`：只含空白视同未设，不是绝对路径就报错退出、什么都不装。

#### 从 0.1.x 迁移

1. 用 0.1.0 记下现有实例：`aproxy status`（端口与配置文件）、`aproxy alias list`；备份 `~/.aproxy/` 下的 `*.toml` 与 `settings.json`。
2. 用 0.1.0 停掉全部实例：`aproxy stop all`，再执行 `aproxy status` 确认已经没有实例。必须在换二进制之前做：之后的 0.2.0 联系不上 0.1.0 的实例。0.1.0 的看门狗在实例清零后默认 5 分钟自行退出，0.2.0 的实例启动时也会退役它。
3. 换二进制：
   - 用一键脚本或 `aproxy install` 装的：删除 `~/.aproxy/bin/` 下的 `aproxy`（Windows 为 `aproxy.exe`，以及可能残留的 `aproxy.old.exe`），再重新运行一键安装脚本（已有二进制时脚本不覆盖）。Windows 上删不掉，是 0.1.0 的看门狗还在用这个文件，等它退出再删。
   - npm 装的：`npm i -g @meowo/aproxy@latest`（预发布用 `@next`）。
   - cargo 装的：`cargo install aproxy --force`（预发布加 `--version <版本>`）。
4. 清理 0.1.0 的残留：`~/.aproxy/run/install.state`（如果有）、`~/.aproxy/staging/`，以及 `~/.aproxy/run/*.restore`（第 2 步之前崩溃过的实例会留下，留着的话 `aproxy restore` 会按 0.1.0 记下的参数拉起它们）。
5. 配置文件 `config.toml` 与 `settings.json` 沿用，按本节其余条目修改用到的旧写法（例如 `upstream_url` 须改名为 `base_url`），然后运行 `aproxy doctor` 检查。
6. 用 0.2.0 逐个启动：`aproxy start <别名|配置路径>`。端口不变，agent 客户端的 base URL 不用改。
7. 更新 skill 文档：`aproxy install --skills-only`，再确认 agent 的 skills 目录链接或复制的是 `~/.aproxy/skills/` 里的新文档。

要回到 0.1.x：先用 0.2.0 执行 `aproxy stop all`，删除 `~/.aproxy/bin/` 下的 aproxy 二进制，再用 0.1.x 的一键安装脚本指定版本重新安装（例如 `sh install.sh v0.1.0`、`install.ps1 -Version v0.1.0`）；0.2.0 的 `aproxy install` 不会装回 0.1.x。

### 新增

- **每个平台两个版本：带桌面界面的 desktop 与无界面的 headless**，代理与命令行完全相同。GitHub 资产里带界面的沿用 `aproxy-<target>[-v3]`，无界面的是 `aproxy-headless-<target>[-v3]`；musl 只有 headless，共 20 个二进制。npm 新增 `@meowo/aproxy-headless` 一组（与 `@meowo/aproxy` 都提供 `aproxy` 命令，只装其一；`@meowo/aproxy` 的两个 musl 平台包装的也是无界面的二进制）；cargo 编无界面版本用 `--no-default-features`；`cargo binstall aproxy` 在 musl 系统上取 headless。每个 Release 附发布清单 `aproxy-manifest.json`（每个二进制的目标、变体、指令集档与 SHA256）。`aproxy install` 升级保持已装的变体，`--variant desktop|headless` 换变体，没有更新的版本时重装当前版本的另一个变体；与 `--from` 同用时是核对。Linux 上带界面的版本动态链接 GTK 与 WebKitGTK，缺库时连 `--version` 都起不来，所以在线安装（含 `--force`）要装它之前先看本机：musl、glibc 低于 2.35、缺 WebKitGTK 4.1 或低于 2.40 时，没写 `--variant` 就改装 headless 并说明原因，写了 `--variant desktop` 就报错；下载后带界面的版本跑不起来时同样改装 headless。一键安装脚本 install.sh 在 Linux 上按同样的规则自动选择，可用 `--variant desktop|headless`（或 `APROXY_VARIANT`）指定。npm 装的带界面版本在 Linux 上缺库起不来时，启动器提示改装 `@meowo/aproxy-headless`。
- **桌面界面预览 `aproxy desktop`**：带界面的版本（`--version` 的变体段为 `desktop`）可以打开桌面窗口。这一版的界面是预览版：实例、请求日志与统计都是演示数据，不是这台电脑上正在运行的 aProxy，界面顶部常驻「预览版 · 演示数据」标签；真实数据在之后的版本接入。窗口是独立的进程，关掉就退出，不启停、不改动任何实例；同一个 home 只开一个（再次运行提示已经在运行，以 0 退出），登记在进程台账里（角色 `desktop`），webview 的数据放在 `<home>/desktop/`。Windows 需要 WebView2 运行时：从终端运行时命令立即返回、窗口独立运行，从快捷方式启动不留控制台窗口、说明改用对话框；Windows 11 上可以在设置里打开 Mica 背景。Linux 需要图形会话与 WebKitGTK 4.1。打不开时（没有 WebView2、没有图形会话）说明原因并以 1 退出，代理与其余命令不受影响。无界面的 headless 版本也认得这个命令（帮助里不列出），执行时说明怎样换装带界面的版本（`aproxy install --variant desktop`），以 1 退出。
- **强制安装 `aproxy install --force`**：普通安装失败、卡住，或装着的版本坏到跑不起来时使用。它不走交接：先取得并核对目标（在线时读 release 附带的发布清单 `aproxy-manifest.json` 选资产、比对 sha256，或用 `--from <文件>`），在本机跑通它的 `--version`；确认之后按进程台账结束这个 home 下当前用户的全部 aproxy 进程（安装者、看护者、实例；属于别的用户的跳过并报告），清空 `<home>/install/`，换上新二进制，再用新版本的 `aproxy restore` 按原参数拉回实例。这会立即中断所有经 aproxy 的在途请求，所以先确认；标准输入不是终端时必须加 `--yes`。明确写出的版本号可以降级，但不装低于 0.2.0 的版本；`latest` 解析出的版本低于正在运行的版本时（新版本的发布漏了清单）以 10 结束，不悄悄降级。下载只走 HTTPS，60 秒收不到数据才放弃，清单与资产各有大小上限；`--variant desktop|headless` 选变体，默认与当前相同（与 `--from` 同用时是核对：文件必须正是这个变体）；与 `--force` 冲突的 `--no-skills`、`--allow-downgrade` 等选项按用法错误拒绝（退出码 2，什么都不做）；`--cpu` 显式指定的指令集档必须正好存在，不指定时按本机 CPU 选 x86-64-v3 或基线档。清单里没有这个平台对应的变体或指令集档时，错误里列出该平台实际有的组合，以 10 结束。它不读配置与 `settings.json`（损坏的配置挡不住它），也不更新 skill 文档（之后执行 `aproxy install --skills-only`）。它只修已经装着的 home：home 不存在（多半是 `APROXY_HOME` 写错了）时以 10 结束。退出码：0 完成；10 一个进程都没结束、二进制没换；11 结束过进程、二进制没换（再执行一次即可，或 `aproxy restore` 用原来的二进制拉回实例）；12 二进制已换、有实例没回来（按端口列出启动命令）。输出写不出去（例如接到已经关闭的管道）不改变退出码；同一个 home 上后来的强制安装会结束先来的，先来的那个以 1（Windows）或 137（Linux、macOS）结束。装着的版本连运行都不行时，手动下载新版本，执行 `<新版本> install --force --from <新版本>`。从 0.2.0 起，任何版本的强制安装都能安装任何版本、结束任何版本启动的进程，这套约定（目录布局、台账、发布清单、命令行与退出码）不再改动。
- **进程台账 `<home>/ledger/`**：实例（后台与前台）、看护者与安装者在占用端口或拉起子进程之前写一条 `<pid>.json`（pid、进程启动标记、角色、版本，实例另记端口与配置文件），正常退出时删除；写失败只告警、照常运行。强制安装只按台账找进程，结束前核验启动标记，pid 已被别的进程复用时只删条目、不结束。0.1.x 不写台账，它启动的进程由强制安装列出、不结束。
- **可自定义心跳**：config.toml 的 `keepalive_heartbeat` 指定每个保活间隔写给客户端的字节，默认仍是 SSE 注释 `: keepalive`。aProxy 不认识协议，心跳的形态由用户按客户端决定，例如解析器容不下注释的客户端可改成空行 `"\n"`。启动与 `aproxy doctor` 校验它写完后客户端的 SSE 解析器停在事件边界（非空、以换行结尾、含 `event:`/`data:` 等字段行时以空行收尾），否则心跳会和回放的第一个上游事件拼在一起。
- **转换器信封加 `stage`、`request_id`、`state`**：同一客户端请求的请求转换与响应转换收到同一个 `request_id`；请求转换回信里的 `state`（不透明字符串）由 aProxy 按请求保存、交给响应转换，两个转换器不必再只靠 `url` 反查当初的决定。字段都是可选的，不读它们的 format 程序不受影响。信封契约 crate `aproxy-envelope` 随之升到 0.2.0。
- **心跳转换器 `heartbeat_transform`**：保活响应等待期间，每一拍交给 format 程序按请求内容生成心跳（信封 `stage = "heartbeat"`，首拍带请求体，另附第几拍、已等待毫秒数与当前第几次上游尝试），能按协议发出客户端认可的内容。它不会拖住心跳：上一拍没回来、调用失败或回信不合事件边界规则时，这一拍改发固定心跳（每个请求只告警一次）；回放开始后才回来的心跳直接丢弃。心跳回信的 `state` 同样交给之后的响应转换。
- **重试前重新执行请求转换（`every_attempt`）**：`[request_transform]` 设 `every_attempt = true` 后，每次重试之前 aProxy 用客户端的原始请求再调用一次请求转换（信封 `stage = "retry"`，`retry` 字段带这是第几次尝试、上一次的上游状态码或失败原因），回信就是这次发往上游的请求。format 可以借此在失败后换 key、换渠道，无限重试不会一直打同一个坏掉的上游；官方 aproxy-format 在 persistent 模式下每次重试都会换到下一个 key。这一次的转换失败不会终止请求，沿用上一次发出的请求。默认关闭，行为不变。信封契约 crate `aproxy-envelope` 0.2.0 随之增加 `retry` 字段。

### 变更

- **交换二进制改用唯一的中间名，并持有安装锁**：旧二进制让位为 `aproxy.old-<纳秒时间戳>-<pid>[.exe]`，新文件先落在 `aproxy.tmp-<纳秒时间戳>-<pid>[.exe]`，名字已存在就换一个。此前的固定名 `aproxy.old(.exe)` 在短时间内连续安装两次、上一次的旧文件还被占着时必然改名失败。删得掉的 `aproxy.old-*`、`aproxy.tmp-*` 残留在每次交换前清掉，删不掉的（仍被占用）跳过，不再挡住安装。修改 `bin/` 期间持有操作系统级排他锁 `<home>/install/lock`（进程退出即释放）。unix 上让位的旧二进制同样以 `aproxy.old-…` 保留到终验之后。Windows 的入口兜底脚本 `bin/aproxy.bat` 在 `aproxy.exe` 缺席时运行最新的 `aproxy.old-*.exe`，改由 `aproxy install` 在每次交换前写入，一键安装脚本不再写它。
- 源码编译（`cargo install aproxy`）需要 rustc 1.90 或更新的版本（此前 1.88）：安装锁用到 1.89 起稳定的标准库文件锁，默认带上的桌面界面（Tauri 2）要 1.90。默认编带界面的版本，Linux 上要先装 WebKitGTK 4.1 的开发包（Debian/Ubuntu：`libwebkit2gtk-4.1-dev`）；加 `--no-default-features` 编无界面的 headless 版本。从源码仓库构建带界面的版本要先构建前端（Node.js 20.19+ 或 22.12+，步骤见 README）。
- **控制协议整理为 v1**：请求是 `{"v":1,"op":"ping|shutdown|prepare_swap"}`，成功应答是 `{"v":1,"ok":true,"result":{…}}`，`result` 含注册记录 `instance`、所属 `run_dir`、状态 `state`（`serving` / `swap_prepared` / `stopping`，取代原来的 `swap_phase` 布尔值）、观测值 `activity` 与支持的 `ops`；失败应答带稳定的错误码（`bad_request`、`unsupported_version`、`unknown_op`、`invalid_state`）与说明。注册记录 `run/<端口>.pid` 只保存实例身份字段，请求数、重试数、最近错误等实时值只经控制通道提供。0.1.0 不带 `v` 的请求与应答不再被认，见「破坏性变更」。
- **skill 文档改为英文并重写**：aproxy-cli 与 aproxy-format 两个 skill 按 skill-creator 标准重写，面向操作 aProxy 的 agent。每条事实都对照源码核实；程序输出的中文原文照录，并附英文释义。aproxy-cli 新增 `clients.md`（接入各 agent 客户端）与 `troubleshooting.md`（症状 → 原因 → 处理），`compatibility.md` 讲版本判定、0.1.0 与开发线的差异，以及怎样从 0.1.x 迁到 0.2.0、怎样退回 0.1.x。
- **安装由目标版本执行**：`aproxy install` 下载并校验目标版本后，把广播、交换、滚动重启、终验与清理交给这个新二进制执行（所有平台），发起安装的进程只等待并转告结局，输出 `新版本进程已接手安装（广播、交换、滚动重启、终验），等待其完成...`。从这一版起，每次升级都由新版本驱动，新版本能修掉旧安装器的缺陷。降级仍由发起安装的进程自己执行。新二进制没能接手时，安装以 `目标版本没有接手安装（…）；实例与 bin 里的二进制都未改动` 失败收场。
- **断开提示覆盖所有客户端**：客户端在「只收到心跳」的等待中（响应已提交、尚未收到任何上游真实字节）等了至少 60 秒才断开时，日志提示检查客户端的流空闲超时，并带上已等秒数。原来只在等满 590 秒后提示 Claude Code 的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`，Codex（默认 300 秒）、Qwen Code（240 秒）到点断开时没有任何提示。

### 移除

- 去掉只为 0.1.0 之前的预发布版本保留的兼容：config.toml 不再把 `upstream_url` 当作 `base_url` 读取（仍在用旧名的配置请改名，否则启动报 `base_url` 为空）；不再读取预发布版本写下的纯数组格式恢复记录（`run/<端口>.restore`）；不再为没有登记进程创建时间的旧注册记录退回「IPC 应答的 pid 一致」来证明身份——这类记录不再被看门狗收养，`stop --force` 也拒绝据它终止进程。

### 修复

- 相对路径的 `--config` 在命令行入口展开 `~` 并转成绝对路径后再转发给守护进程、写入恢复记录；此前原样保存，`aproxy restore` 若在别的目录运行会把该实例的恢复记录当失效清掉。
- `aproxy install --skills-only` 可以同时指定版本（`aproxy install <版本> --skills-only`），与 latest 查询失败时的提示一致。
- `aproxy stop` 等不到实例退出时，提示改为跨平台、会核验进程身份的 `aproxy stop <端口> --force`（此前在所有平台都建议 `taskkill`）。
- `settings.json` 的 `default_config` 指向的文件不存在时，只有要读配置的命令（启动、`aproxy config` 查看或修改内容）报错退出；`status`、`stop`、`doctor`、自动拉起的 `install --continue` 等不再被拦，报错信息推荐的 `aproxy config --clear-default` 也能直接执行（此前所有不带 `--config` 的命令都在分发前退出 1）。
- `settings.json` 解析失败时，`aproxy alias add/remove` 与 `aproxy config --set-default/--clear-default` 拒绝修改并提示先修复或删除该文件，不再用默认值写回、冲掉原有的全部别名。
- 相对路径的 `log_file` / `--log-file` 按文档相对 `APROXY_HOME` 解析（此前实际相对当前工作目录）。
- **外部转换器纳入心跳节拍**：保活适用的请求，请求转换、响应转换与回放前解码期间照常提交骨架头并发心跳。此前 format 慢（`timeout_secs = 0` 时没有上界）时，客户端可能在收到任何字节之前一直干等。请求转换若在骨架头提交之后才失败，以 `proxy_transform_failed` 终态 error 事件收场（提交前失败仍是 502）。
- `override_headers` 里配置的 `accept-encoding` 对保活适用的请求不生效（这些请求一律以 `identity` 发往上游），启动时 warn 一次说明，不再静默覆盖。
- 请求体超过 `max_body_mb` 时，客户端能收到 413 和调整上限的提示。此前代理一发现超限就回 413 并关闭连接，而客户端往往还在上传，结果只看到连接被中止（Windows 上必现）。现在先把剩余的请求体读掉丢弃（最多 10 秒，2 秒没有新数据即止）再回应；缓冲路径与 `forward_only` 都是如此。
- 客户端上传大请求体（超过 1 MiB、已溢写到磁盘）途中断开时，`req-*.spooltmp` 临时文件不再残留到下次启动。
- **挂死实例不再从视野里消失**：进程还在、却不应答控制通道的实例（按 pid + 创建时间核验），`aproxy status` 不再删掉它的注册记录与日志，而是单列为「无响应」并提示 `aproxy stop <端口> --force`；`stop`/`restart` 的 `--force` 经注册记录找到它并核验后终止，`stop all --force` 一并处理。此前一次 status 就会删掉记录，`--force` 随即找不到它，尚未被看门狗收养的挂死实例也失去了被收养、处决重拉的依据。
- `aproxy stop` 有目标未能确认停止（等不到退出、`--force` 失败、目标挂死而未加 `--force`）时以 1 退出，脚本可据此判断；此前一律退出 0。
- **控制通道按 home 隔离（Windows）**：控制管道名加上 run 目录的标识（`\\.\pipe\aproxy-<home_id>-<端口>`），`aproxy` 命令只能控制同一个 `APROXY_HOME` 下启动的实例。此前管道名只含端口、全机共享，在临时 home 里做实验时 `aproxy stop 12345` 会停掉用户正在用的 12345 实例。Linux/macOS 的控制 socket 本就在 run 目录里，行为不变。
- **控制通道建不成时实例拒绝启动**：实例先创建控制端点，再写注册表与恢复记录；端点创建失败（同一 home 里另一个实例以不同监听地址占着同一端口，或 unix 上 run 目录路径过长）时在 startup.log 写明 `控制通道 … 创建失败` 并以 1 退出。此前它不带控制通道继续代理，`stop`、`status` 都找不到它，它的注册记录还会盖掉同端口那个实例的记录。unix 上也不再删掉一个仍有进程在听的 socket（此前同端口的第二个守护会把第一个的控制通道抢走）。
- `aproxy stop`、`restart` 与 install 的滚动重启只在实例的控制端点消失后才确认它已退出。此前连续两次探测失败（包括超时、管道忙）就算已退出，挂死、仍占着端口的实例会被报成「已停止」。
- **控制通道更稳、更难冒充**：Windows 上一个连接在建立前断开，不再让整条控制管道停止服务；守护读请求限时 5 秒，客户端读应答限长 1 MiB；客户端核对应答者自报的 pid 与连接对端的进程，对不上即拒收，`status` 也不再把别的进程的应答算到一条注册记录名下。看门狗续写 claim 改为先写临时文件再改名，并发读者不会读到半截 claim、误以为没有看护者。
- **心跳节与安装宣告也按 home 隔离**：看门狗读的心跳节（`aproxy-<home_id>-heart-<端口>`）与 Windows 的安装宣告（`aproxy-<home_id>-install`）带上 run 目录的标识；unix 的安装宣告从 `/dev/shm` 挪进 `<home>/install/announce`，强制安装清空 `install/` 时一并清掉；安装者崩溃留下的过期宣告由下一次续作删掉，看门狗不再每轮拉起一个立即退出的续作。一个 home 的安装不再让别的 home 的看门狗进入安装模式，unix 上也不再与别的用户的 `/dev/shm` 文件撞名。看门狗处理实例死亡时不再删除它的控制 socket（此刻端口上可能已有新实例，删掉就是抢走它的控制通道），残留 socket 由下一个实例启动时清理；unix 上被删的心跳文件会在下一拍重建。
- unix 上实例的控制 socket 文件被别的进程删掉时，实例在一秒内于原路径重建，`stop`、`status` 照常找得到它。从 0.1.0 迁移时还没退出的 0.1.0 看门狗，处理实例退出时就会删掉同一路径上的 socket。
- **升级后不再留着旧版本的看门狗**：install 在所有平台上都先停掉旧看门狗（此前只在 Windows 上停；Linux 上留下的旧看门狗会用已被替换的二进制重拉崩溃实例，重拉必然失败）。看门狗的 claim 记录自己的版本，实例启动时退役比自己旧的看门狗（包括从 0.1.0 迁移时还没退出的 0.1.0 看门狗），再由实例的自检补种新的；同版本或更新的看门狗不受影响。
- 在线 `aproxy install` 也做管辖检查：有实例从 `<home>/bin` 之外的副本（npm、cargo、包管理器）启动时拒绝安装，并指向对应渠道或 `aproxy install --adopt`，检查在下载之前。此前只有 `--from` 检查，在线安装会把这些实例悄悄换到 `<home>/bin` 的二进制上。
- `install --continue` 遇到「实例级失败已中止」的现场时，若快照里的实例其实都已从 `<home>/bin` 的新二进制、以目标版本正常服务，就收尾清场，不再报错拒绝；此前这份现场会让看门狗每天拉起一次注定被拒的续作，而 `install latest` 又报「已是最新」。
- 安装进行中运行的 `aproxy` 命令（它们会顺带拉起 `install --continue`）不再可能与正在接手的进程同时推进同一个安装。此前在 Windows 交换后的接力窗口里，这样一个续作也能接手，两个进程同时滚动重启实例。
- `--no-skills` 对安装的后续进程同样有效：Windows 上接力的进程、以及安装中断后的续作不再自行下载 skill 文档（此前会忽略这个选项照常下载）。
- **看门狗即时重拉崩溃实例**：进程退出的事件一到就处理，崩溃实例在启动耗时内（通常不到 1 秒）回来；此前要等下一个扫描周期，默认最长约 30 秒断流。`stop --force` / `restart --force` 相应改为终止前先摘掉恢复记录（终止失败再放回），强停的实例不会被看门狗拉回；`restart --force` 也不再与看门狗同时拉起新实例争抢端口。
- Windows 的 cmd 一键安装脚本（`scripts/install.cmd`）在没有设 `APROXY_HOME` 时，把二进制装到 `%USERPROFILE%\bin`、skill 文档装到 `%USERPROFILE%\skills`，而不是 aproxy 自己默认的 `%USERPROFILE%\.aproxy` 下；之后 `aproxy install` 管理的是 `.aproxy\bin`，两份二进制各管各的。现在默认装到 `%USERPROFILE%\.aproxy`，与 `install.ps1`、`install.sh` 一致。

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
