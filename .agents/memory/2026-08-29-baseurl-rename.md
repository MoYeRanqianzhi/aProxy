---
name: baseurl-rename
description: upstream 术语已更名为 baseurl（CLI）/ base_url（配置字段），旧字段名经 serde alias 兼容
metadata:
  type: project
---

2026-08-29 提交 `0964d22`：应用户要求，upstream 术语统一更名为 baseurl。

- CLI：顶层与 `config` 子命令的 `--upstream` → `--baseurl`；顶层同时新增 `--api-key`（修复了此前只有声明未应用的半成品状态）与已有 `--proxy`/`--listen` 对称
- 配置字段：`upstream_url` → `base_url`，`#[serde(default, alias = "upstream_url")]` 保证旧 `~/.aproxy/config.toml` 无需迁移；保存时写新名
- 顶层覆盖参数族现为：`--baseurl`/`--listen`/`--proxy`/`--api-key`（均仅本次运行生效，不写配置）

同日实机验证（hub.oaifree.com，OpenAI 风格中转）：
- key `ah-…d93` 有效，deepseek 模型为 `deepseek-v4-flash-0731` / `deepseek-v4-pro-0813`（v4 是 reasoning 模型，内容在 `choices[0].message.reasoning_content`，`content` 为空时先看 reasoning；max_tokens 太小会被推理耗尽导致 content 空）
- 全链路：12347 临时实例 `--baseurl https://hub.oaifree.com --api-key … --proxy http://127.0.0.1:7890`，客户端不带鉴权头，HTTP 200 正常回复

**Why:** 用户明确要求统一 baseurl 术语；实测中发现顶层 `--api-key` 未接线的缺口。

**How to apply:** 新代码一律用 `base_url` 字段与 `--baseurl` 参数；遇到用户旧文档/旧命令示例中的 `--upstream`/`upstream_url` 应指出已更名。运行中 12345 实例锁 `target/release/aproxy.exe`，rebuild 前确认实例已切换为 `aproxy-using.exe`（用户日常以 `aproxy-using.exe` 跑生产实例）。相关 [[proxy-config]]。
