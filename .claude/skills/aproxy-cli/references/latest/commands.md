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

## stop

```
aproxy stop                 # 恰好 1 个实例时可省略参数
aproxy stop <PORT>          # 按端口号停（不依赖注册表，IPC 直接定位）
aproxy stop all             # 停全部
aproxy stop <ALIAS>         # 按别名停（按配置路径匹配运行实例，端口变了依然有效）
aproxy stop idle            # 停全部空闲超阈值（idle_timeout_secs）的实例
aproxy stop idle <SECS>     # 同上，阈值临时改为 SECS 秒
```

target 同样走「别名 → default → 路径」解析，但纯数字按端口。语义细节：

- 多实例时省略参数是**错误**（退出 1，并列出每个实例的 stop 命令）——防止误停。
- 别名命中但该配置的实例未运行：报「未在运行」退出 1。
- 单实例停止流程：IPC shutdown → 轮询至多 12 秒确认退出；未退出则提示
  `taskkill /PID <pid> /F`。
- 停止是优雅关闭：守护进程收到信号后有 10 秒宽限强退兜底（在途请求会中断）。

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

注意：修改 toml 的 config 命令**不会重启已运行实例**——需 stop 后重新 start。

## 退出码与输出约定

- 成功 0；用户可修复的错误（配置错误、未知别名、多实例未指定 target、端口
  占用等）1。
- 面向用户的输出全部简体中文；stderr 报错、stdout 出结果。
- 所有展示输出对 api_key/头值/代理密码/base_url 内嵌凭据打码（前 6 字符 + `***`
  或 `user:***@host`），日志同理——粘贴分享日志不泄露凭据。
- Windows 控制台代码页在进程入口自动切 UTF-8（65001），守护日志写 UTF-8 BOM。
