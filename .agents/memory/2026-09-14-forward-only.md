---
name: forward-only
description: 仅转发模式（forward_only，alpha.12）：放弃重试/缓冲/心跳换真流式直通；config.toml + settings.json 分层；分支点、流式透传与「不得 unwrap」消费点约束
metadata:
  type: project
---

2026-09-14 新增：**仅转发模式 `forward_only`**（config.toml 每实例字段 +
settings.json 全局默认，与 `max_body_mb`/`disk_cache` 完全同款分层）。

**它是什么**：开启后实例走一条极简路径——请求体边收边发上游、响应边收边回
客户端，**不缓冲、不重试、不落盘、不发心跳**。这是给「上游可信 + 客户端要真
流式」场景的**显式取舍**：它放弃了本产品最核心的重试保障。产品文案（README/
architecture/skill）每一处都必须写明这一点，**不得**把它写成无害的普通开关。

**关键实现约束**（改动或审查本模式时逐条核对）：

- 分支点在 `read_request_body` **之前**（否则缓冲已发生，模式失去意义），且在
  `requests_total.fetch_add` **之后**（否则 status 的请求数恒为 0）
- 请求体一律流式（`req.into_body().into_data_stream()` 经计数适配器 →
  `reqwest::Body::wrap_stream`），**不做空 body 特判**
- 响应 `resp.bytes_stream()` 直回客户端，status 与响应头原样透传；hop-by-hop
  照旧过滤，但 **`content-length` 必须保留**（字节未经变换，上游声明仍精确）
- 超 `max_body_mb`：流式途中计数超限 → 让请求体流产出 `Err`（令 reqwest 中止
  上游请求）→ 回 413，复用现有文案（`max_body_mb` 是本模式唯一仍强制的限制）
- 上游请求失败 → 502 + 原因，不重试，并调 `state.note_upstream_failure`——
  否则 status 的「最近错误」对这类实例永久显示「无」
- 响应流中断 → **直接截断**（不注入任何上游未发出的字节）+ `tracing::warn!`
  记录错误与已转发字节数 + `note_upstream_failure`
- 客户端断开 → 响应 Body 被 drop，reqwest 连接随之关闭（计费保护天然成立，
  写注释说明即可，见 [[disconnect-billing-protection]]）
- 不进入：重试循环、SSE 保活骨架、错误内容拦截（`is_error_body`/
  `is_stream_error_body`）、spool、`client_wants_sse` 判定；不生效：`disk_cache`/
  `spool_limit_mb`/`keepalive_interval_secs`/`max_retry_backoff_secs`
- 消费点统一 `Config::forward_only_enabled()`（`unwrap_or(DEFAULT_FORWARD_ONLY)`），
  **不得 `unwrap()`**——doctor 的 `parse_config_file`、`find::discover` 与约 20 处
  测试的 `AppState::new` 都不经 settings 注入；`proxy::router(AppState)` 签名不变
- `resolve_runtime_config` 里 `cfg.forward_only.get_or_insert(s.forward_only)`
  紧邻 `max_body_mb`/`disk_cache` 的注入；不新增 CLI 旗标，仅 `config --show` 展示

**Why:** 存在「上游可信 + 要真流式（不缓冲、不重放）」的场景，重试保障在此反成
负担；同时产品必须诚实标注这是放弃核心保障的取舍。分层方式复用既有
[[alias-settings]] 的 config.toml/settings.json 两层制；文档同步的纪律见
[[aproxy-cli-skill-versioning]]（skill 与代码不一致 = 文档缺陷，与测试红灯同级）。

**How to apply:** 改动本模式时保持上述分支点/流式/透传约束；新增消费点走
`forward_only_enabled()` 而非 `Option::unwrap()`。文档同步范围为人类侧
（README.md/README_EN.md/docs/architecture.md）与 skill 侧
（SKILL.md、references/latest/ 的 config-toml/settings-json/behaviors/
compatibility 四份）。相关 [[alias-settings]] [[disconnect-billing-protection]]
[[aproxy-cli-skill-versioning]]。
