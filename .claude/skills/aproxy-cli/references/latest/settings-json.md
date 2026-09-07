# settings.json 配置参考

位置：`~/.aproxy/settings.json`。程序管理的**全局唯一内部配置**，与可多份并存、
人类手写的 config.toml 平行分工。格式与字段随版本演进，**不建议手改**——
一切修改走命令：别名用 `aproxy alias add/remove`，默认配置用
`aproxy config --set-default/--clear-default`；数值字段（本文件）通常保持默认。

文件损坏（JSON 语法错误）不会阻断启动：回退为默认空配置并向 stderr 报 error
（别名丢失可重新 add）。

## 目录

- [字段总表](#字段总表)
- [各字段语义](#各字段语义)
- [与 config.toml 的分层关系](#与-configtoml-的分层关系)

---

## 字段总表

| 字段 | 类型 | 默认值 | 管理方式 |
|---|---|---|---|
| `aliases` | map\<name, path\> | 空 | `aproxy alias` 命令 |
| `default_config` | string? | 无 | `aproxy config --set-default/--clear-default` |
| `config_dirs` | string[] | `[]` | 手编（进阶） |
| `log_rotate_mb` | u64 | 8 | 手编（进阶） |
| `idle_timeout_secs` | u64 | 1800 | 手编（进阶） |
| `max_body_mb` | u64 | 128 | 手编（进阶；toml 可按实例覆盖） |
| `disk_cache` | bool | true | 手编（进阶；toml 可按实例覆盖） |

## 各字段语义

### aliases

别名 → config.toml 绝对路径 的映射。供 `aproxy start/stop <别名>` 快捷定位。
保存时路径已绝对化（`~` 展开）；运行实例匹配按归一化路径键（大小写/分隔符
不敏感，剥 Windows verbatim 前缀），端口改变不影响别名有效性。
非法名（纯数字、`all`/`idle`/`default`/`defult`）被 add 拒绝；手写进去的非法名
会在每次运行时与 doctor 报 error。

### default_config

默认配置文件路径（支持 `~`，存绝对路径）。未指定时回退 `~/.aproxy/config.toml`。
指向的文件被删/移动时：启动明确报错退出 1（不静默回退，防在错误配置上排障）；
`default` 保留字（start/stop target）也解析到它。

### config_dirs

配置目录列表：`aproxy find`/`aproxy doctor` 扫描这些目录下的 `*.toml`
（只查该层，不递归）。**默认两个目录 `~/.aproxy/` 与 `~/.aproxy/configs/`
始终参与**（手写重复也静默去重）；此字段用于追加第三方的配置存放目录。
路径支持 `~` 展开。

### log_rotate_mb

守护日志运行期轮转阈值（MB）：每小时检查，端口日志超过即**截断清空**
（非 rename 轮转，不产生轮转文件堆；`aproxy logs` 跟随器检测到变小会自动从头
重跟）。0 = 不轮转。启动时另有独立的 2 MiB 检查（与旧实例兼容，不受此项控制）。
全局治理项，所有实例共用，故放 settings 而非各 toml。

### idle_timeout_secs

实例「空闲」判定阈值（秒）：距最近一次收到客户端请求的时长。作用于
`aproxy status --idle/--busy` 与 `aproxy stop idle [SECS]`（后者显式给秒数时
临时覆盖此值）。默认 1800（30 分钟）。

### max_body_mb（全局默认层）

`config.toml` 未显式写 `max_body_mb` 的实例取此值（内置默认 128）。toml 显式值
优先——分层实现于启动时 get_or_insert 注入。0 = 不设限。

### disk_cache（全局默认层）

`config.toml` 未显式写 `disk_cache` 的实例取此值（内置默认 true）。toml 显式值
优先。语义见 config-toml.md。

## 与 config.toml 的分层关系

只有 `max_body_mb` 与 `disk_cache` 存在三层优先级：

```
config.toml 显式值  >  settings.json 值  >  内置默认 (128 / true)
```

其余字段是「toml 显式值 > 内置默认」或完全归 settings 管理（本文件全部字段）。
`aproxy config --show` 会注明哪些值「未在 toml 设置，运行时取 settings.json
全局默认」。

`aproxy doctor` 的 error 级检查覆盖本文件：JSON 语法错误、别名非法、别名指向
不存在的文件。这些检查在每次 aproxy 运行时都执行（stderr 报出，不退出——
status/stop 等管理命令不能因内部配置损坏而不可用）。
