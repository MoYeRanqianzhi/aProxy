# 命令参考（aProxy CLI）

适用版本见 [compatibility.md](compatibility.md)。所有命令均为 `aproxy` 单一二进制；
平台差异处已注明（当前主力平台 Windows）。

## 目录

- [通用参数](#通用参数)
- [启动：aproxy / aproxy start](#启动)
- [status](#status)
- [stop](#stop)
- [logs](#logs)
- [restore](#restore)
- [alias](#alias)
- [doctor](#doctor)
- [find](#find)
- [config](#config)
- [退出码与输出约定](#退出码与输出约定)

---

## 通用参数

| 参数 | 作用 | 备注 |
|---|---|---|
| `--config <PATH>` | 指定配置文件 | global 参数（可放任意位置）；默认 `~/.aproxy/settings.json` 的 default_config，再回退 `~/.aproxy/config.toml`；多开时各指定一份 |
| `--foreground` | 前台运行 | 日志走控制台，Ctrl+C 停止；省略 = 后台守护 |
| `--baseurl <URL>` | 覆盖 base_url | 仅本次运行生效，不写入任何配置文件 |
| `--listen <ADDR>` | 覆盖监听地址 | 同上 |
| `--proxy <URL>` | 覆盖上游代理 | 同上 |
| `--api-key <KEY>` | 覆盖快捷鉴权 | 同上；命令行参数可被本机其他进程枚举，长期使用应写进 toml |
| `--daemon-child` | 内部标记 | 隐藏参数，父进程 spawn 守护子进程时附加；勿手动使用 |

`--config` 指向的文件若不存在：启动路径直接报错退出（不静默回退默认配置）。

## 启动

```
aproxy                      # 等价于 aproxy start（无 target）：后台启动
aproxy --foreground         # 前台运行
aproxy start [ALIAS|PATH]   # 按别名或配置文件路径启动
```

target 解析顺序：别名（settings.json）→ `default` 保留字 → 配置文件路径；
都落空报「未知的别名或配置文件」并退出 1。

启动流程与判定：

1. 父进程预检 1：IPC ping 同端口——已有实例且监听地址相同 → 提示「已在运行」退出 0
   （幂等，不重复启动）；监听地址不同 → 拒绝（实例按端口号区分，无法并存）。
2. 预检 2：TCP bind 探测（随即释放）→ 失败再 ping 一次（排除自家正在启动的
   竞态窗口），仍失败按错误类别报错退出 1。
3. spawn 分离子进程（命令行转发 + `--daemon-child`），父进程轮询 IPC ping
   至多 8 秒判定就绪；超时报「未就绪」并指出 startup.log 与端口日志两个位置。
4. `--listen` 端口为 0 时跳过就绪等待，提示用 status 查看实际端口。

就绪判定用 IPC ping 而非 TCP connect（管道名 `aproxy-<port>` 由守护进程独占创建，
connect 成功无法区分自家子进程与第三方监听者）。

## status

```
aproxy status               # 列出全部运行中实例
aproxy status --idle        # 只列闲置实例
aproxy status --busy        # 只列活跃实例（--idle 与 --busy 互斥）
```

数据来自实例注册表（run/ 目录），**存活以 IPC 探测为准**（注册表死记录会显示但
ping 不通即已死）。每行含：端口、pid、版本号（`v<semver>`）、已运行时长、闲置时长、
监听地址、上游（打码）、配置文件路径。闲置/活跃阈值 = settings.json 的
`idle_timeout_secs`（默认 1800 秒）。

## stop / restart

```
aproxy stop                 # 恰好 1 个实例时可省略参数
aproxy stop <PORT>          # 按端口号停（不依赖注册表，IPC 直接定位）
aproxy stop all             # 停全部
aproxy stop <ALIAS>         # 按别名停（按配置路径匹配运行实例，端口变了依然有效）
aproxy stop idle            # 停全部空闲超阈值（idle_timeout_secs）的实例
aproxy stop idle <SECS>     # 同上，阈值临时改为 SECS 秒
aproxy stop --force ...     # 任意 target 组合加 --force：立即 TerminateProcess，零等待
```

`restart` 的 target 语义与 stop **完全一致**（把上面命令里的 `stop` 换成
`restart` 即可）。两者的差异：

- `stop` 停止后结束；`restart` 停止后用**原始启动参数**立即拉起并等 IPC
  就绪（8 秒判定），报出新 pid。
- **改配置（toml，含换端口）后重启生效**：就绪判定按新实例 pid 定位，配置
  改了端口也能正确确认，成功输出新监听地址。
- **restart 只负责重启，不负责启动**：target 未在运行时提示「端口 X 上没有
  运行中的 aProxy 实例」并退出 1——不会顺手把没启动的实例拉起来。重启后想
  首次启动请用 `aproxy start <别名|路径>`。
- 重启参数来源：`.restore` 记录的原始启动参数（保 `--api-key`/`--listen` 等
  仅本次参数）；缺失时回退注册表的配置文件路径。
- `--force`（stop/restart 通用）：跳过 IPC 优雅关闭，立即 `TerminateProcess`
  ——零等待。终止前验证进程镜像名，非 aProxy 进程（PID 复用）拒绝执行。
  在途请求会立即中断，仅用于优雅停止失效或需要瞬间重启的场景。

语义细节：

- 多实例时省略参数是**错误**（退出 1，并列出每个实例的 stop 命令）——防止误停。
- 别名命中但该配置的实例未运行：报「未在运行」退出 1。
- 优雅停止流程：IPC shutdown → 轮询至多 12 秒确认退出；未退出则提示
  `taskkill /PID <pid> /F`（`--force` 则由 aProxy 自己完成且零等待）。
- 优雅停止时守护进程有 10 秒宽限强退兜底（在途请求会中断）。

## logs

```
aproxy logs                 # 恰好 1 个实例时可省略
aproxy logs <PORT>          # 多实例必须指定端口
```

- **不支持 `all`**（一次只能跟随一个实例，传 all 按错误退出）。
- tail -f 语义：先输出末尾约 30 行，再增量输出；200ms 轮询读文件，
  约每秒 IPC 探活一次，实例停止后自动结束。
- 日志文件被截断（超过轮转阈值）时自动从头重新跟随。
- `--foreground` 实例没有日志文件：跟随器等 5 秒后报「日志输出在它的控制台」。

## restore

```
aproxy restore              # 恢复崩溃/断电/重启前在运行的实例
```

依据 run/<端口>.restore 恢复记录（守护 bind 成功时写入，优雅退出时删除）。
幂等：已在运行的跳过；配置文件已删除的记录清理掉。空清单时**静默成功退出 0**
（专为开机自启设计——任务计划程序登录时运行 `aproxy restore` 即可实现自愈）。

## alias

```
aproxy alias add <NAME> [PATH]    # PATH 省略 = 默认 ~/.aproxy/config.toml
aproxy alias remove <NAME>
aproxy alias list
```

- 别名存于 settings.json；非法名（空、纯数字、`all`/`idle`/`default`/`defult`
  大小写变体）拒绝并说明原因。
- add 指向的配置文件必须存在（支持 `~` 展开）；重复 add 是**覆盖更新**（提示
  「已更新」）。
- remove 不存在的别名报错退出 1。

## doctor

```
aproxy doctor
```

汇总配置体检：settings.json error 级检查（别名非法/指向缺失/JSON 损坏）+
别名配置与配置目录 toml 深入审查（warning 级），均含端口冲突检测。
settings.json 的 error 级检查在**每次 aproxy 运行时**都会输出（doctor 之外
不重复汇总）。输出「全部配置正常」退出；有问题列出清单，error 级建议先修。

## find

```
aproxy find [QUERY] [--aliased|--unaliased] [--port <PORT>]
```

从配置目录列表（settings.json 的 config_dirs + 默认 `~/.aproxy/` 与
`~/.aproxy/configs/`）发现全部 *.toml（不递归）。QUERY 大小写不敏感匹配路径
片段或别名；`--aliased`/`--unaliased` 过滤别名归属；`--port` 过滤监听端口。

## config

```
aproxy config --show                          # 打印当前配置（敏感值打码）
aproxy config --baseurl <URL>                 # 设置上游（末尾 / 自动去除）
aproxy config --listen <ADDR>
aproxy config --api-key <KEY> | --clear-api-key
aproxy config --extra-header K=V              # 可重复；仅当请求未携带时追加
aproxy config --override-header K=V           # 可重复；无条件覆盖
aproxy config --clear-headers
aproxy config --keepalive-secs <SECS>         # 0 = 关闭心跳
aproxy config --proxy <URL> [--proxy-username U] [--proxy-password P]
aproxy config --clear-proxy                   # 清 URL+用户名+密码三者
aproxy config --set-default <PATH>            # 写 settings.json 的 default_config
aproxy config --clear-default
```

写入的对象是**当前生效的配置文件**（--config 指定 > default_config > 默认路径）。
安全守卫：现有 toml 解析失败时拒绝修改（防止用回退默认值整个覆盖用户的坏文件）；
`--set-default` 与 `--clear-default` 互斥；api_key/proxy 及代理凭据不允许空串
（清空用对应 --clear-*）。保存前校验 base_url 格式（http/https 开头、无 ? #）。

注意：修改 toml 的 config 命令**不会重启已运行实例**——用 `aproxy restart <端口>`
使修改生效。

## install / upgrade

`aproxy install`（`upgrade` 为别名）：把 aProxy 安全安装/升级到规范位置
`~/.aproxy/bin/aproxy.exe`，全程对客户端 ≈ 无感（逐实例滚动重启，任一时刻
至多一个实例在重启，status 中该实例显示「二进制更换中」）。

```
aproxy install [latest|版本]        # 在线安装：下载链条 github→npm→cargo-binstall→cargo
aproxy install --from <二进制路径>   # 从本地文件安装（--version 自报版本即目标）
aproxy install --adopt              # 收编：包管理器/npm 装的 aProxy 迁到标准位置
aproxy install --abort              # 中止进行中的安装（仅交换开始前可回滚）
aproxy install --skills-only        # 只更新 skill 文档，不动二进制
```

参数：
- `--variant v3|baseline`：手动指定指令集变体（默认运行时检测 AVX2）
- `--allow-downgrade`：目标版本低于当前时默认拒绝，此开关放行
- `--download-proxy <URL>`：仅本次下载用的代理（**与 config.toml 的请求代理
  绝对分离**——后者管上游转发，前者只管 install 下载）
- `--no-skills`：本次跳过 skill 文档更新（全局开关 settings 的 skill_auto_update）

版本语义：`latest` = GitHub Releases 最新（<1.0 时代含 prerelease）。目标版本
经下载链条获取后先 `--version` 试跑自证（自报必须等于目标），再走交换；
github 渠道另有 `.sha256` 强校验、npm 渠道有 registry integrity 校验。

下载链条（settings `download_chain` 可配，**严格数组语义**：配置后完全按
数组执行，不自动补默认项，建议写全）：
```json
"download_chain": ["github", "npm", {"url": "https://mirror.example/{version}/{asset}"}]
```
url 模板占位符：`{version}/{asset}/{target}/{variant}`（jsDelivr 等国内可达
CDN 可自行填入；代码不内置任何 CDN 域名）。未配置 = 内置默认链。

## 恢复自动化（install 中断）

安装每一步都先写进度（`~/.aproxy/run/install.state`）再执行——崩溃/断电/强杀
后**自动续作，无人工询问**：
- 主责：看护者启动时与每日一次发现残留 → 自动拉起 `install --continue`
- 兜底：任何 aproxy 命令入口读到残留 → 静默拉起续作（本命令照常执行）
- Windows 接力：交换后旧镜像进程自动退出，新二进制进程接管剩余阶段
- `install.state` 残留 ≠ 完成残留——done/aborted 时文件即删除

`--abort` 仅在二进制交换**开始前**可完全回滚；此后只进不退（实例可能已开始
滚动），中断的安装会自动续作完成。

**手动修复（最后防线，仅 CLI 全失效时）**——swapping 空窗（bin 里只剩
`aproxy.old.exe`）时所有 aproxy 命令无处可落，按序尝试（全部是文件操作）：
1. `~/.aproxy/staging/<版本>/aproxy.exe` 仍在 → **复制**到 `~/.aproxy/bin/aproxy.exe`
2. staging 也没了 → `bin/aproxy.old.exe` 改名回 `aproxy.exe`（旧版本可用优先），重跑 `aproxy install`
3. 两者皆失 → GitHub Releases 重新下载或 `--from` 任意可用二进制

**手动更新兜底（install 反复失败时，有服务中断，仅作最后手段）**：
`aproxy stop all` → 替换 `~/.aproxy/bin/aproxy.exe` → `aproxy restore`。

## 退出码与输出约定

- 成功 0；用户可修复的错误（配置错误、未知别名、多实例未指定 target、端口
  占用等）1。
- 面向用户的输出全部简体中文；stderr 报错、stdout 出结果。
- 所有展示输出对 api_key/头值/代理密码/base_url 内嵌凭据打码（前 6 字符 + `***`
  或 `user:***@host`），日志同理——粘贴分享日志不泄露凭据。
- Windows 控制台代码页在进程入口自动切 UTF-8（65001），守护日志为 UTF-8（无 BOM）。
