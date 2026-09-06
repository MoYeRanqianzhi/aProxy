---
name: alias-settings
description: 别名系统（3e5e1a1）：settings.json 内部配置层与 config.toml 分层；start/stop <别名>；--daemon-child 必须 global=true 的教训与 settings 测试污染修复
metadata:
  type: project
---

2026-09-05 落地（提交 3e5e1a1）：**配置别名**——`aproxy start/stop <别名>` 快捷启停多开配置。

**配置两层制（用户明确要求）**：
- `~/.aproxy/config.toml`：人类可读可写的代理配置，**可多份**（多开各自指定，`--config`）
- `~/.aproxy/settings.json`：程序管理的**内部配置，全局唯一**，JSON（对程序读写更精确），原子写（tmp+rename）；损坏回退空表不阻断启动。别名表存于此；后续其他内部状态也放这里

**别名语义**：
- `alias add <名> [路径]`（省略路径=默认 config.toml）、`alias remove/list`；存的是绝对路径
- `start <别名|路径>`：别名优先、其次路径；解析出的 `--config` **绝对路径注入守护子进程命令行**——别名解析只发生在父进程，不注入子进程会回退默认配置
- `stop <别名>`：按实例注册的 config_path 匹配（absolute+分隔符统一+Windows 小写归一），**端口变了别名依然有效**
- 保留字校验：别名不得为 `all`/纯数字（会与 stop 保留字、端口解析冲突）
- 用户期望后续：settings.json 里显式指定默认配置文件（未实现，见 [[daemon-model]]）

**两个教训**：
1. `--daemon-child` 曾未标 `global = true`——`start <别名>` 转发后该标记出现在子命令之后，clap 解析失败令守护子进程秒死（stderr 进 spawn 的 null，无声无息，表现为父进程「未在预期时间内就绪」且 startup.log 无记录）。凡会被转发到子命令之后的顶层参数必须 global。
2. settings.rs 单测曾直接读写真实 `~/.aproxy/settings.json` 污染用户数据（写入 openrouter/anthropic 假条目）——已加 `settings_path_in/load_from/save_to` 目录注入变体；**单测绝不碰真实用户目录**。

**Why:** 用户多开多个上游配置（anthropic/openai/openrouter 等），按端口记忆启停太繁琐；别名让配置有名字。

**How to apply:** 改 start/stop 的 target 解析时保持「别名优先→路径」顺序与注入 --config 的转发逻辑；新增内部状态进 settings.json 时走 load_from/save_to 注入版做测试。相关 [[daemon-model]] [[warning-zero-policy]]。
