---
name: config-settings-shape-0-2-0
description: 0.2.0 起 config.toml 与 settings.json 的形状、严格解析、分层与写回在哪里实现；读到用旧键名（max_body_mb、forward_only、bounded_retry_paths、[request_transform]、平铺的 watchdog_* 等）的旧记忆或旧代码印象时先看这里
metadata:
  type: project
  scope: aProxy 0.2.0 起的配置与设置（W2 契约块 B2、B3、B4）
  status: active
  last_verified: 2026-11-19
---

0.2.0 把两个配置文件都换了形状，并从「不认识的键静默忽略」改成「严格解析，出错指出文件、键、行列」。
2026-11-16 之前写的记忆（alias-settings、forward-only、bounded-retry-paths、transform-format 等）里的键名是 0.1.x 的叫法，
机制仍然成立，名字要按这里换。

**config.toml**（设计稿 [w2-contracts](../plan/w2-contracts.md) §7）：`max_request_mb`、`max_response_mb`、`delivery = "retry"|"forward"`、
`[timeouts]`（`connect_secs`、`first_byte_secs`、`read_idle_secs`）、`[keepalive]`（`interval_secs`、`trigger`、`heartbeat`）、
`[retry]`（`max_backoff_secs`、`[[retry.rules]]` 的 `path`、`status`、`max_attempts`）、`[[format]]`（`command`、`args`、`env`、
`process`、`stages`、`worker_idle_secs`）。旧名与新名的完整对照在技能的
`.claude/skills/aproxy-cli/references/latest/compatibility.md` 与 CHANGELOG 的「破坏性变更」。

**settings.json**（§8）：`schema`、`language`、`aliases`、`default_config`、`config_dirs`，以及 `watchdog`、`logs`、`instances`、
`update`、`ui`（不透明）、`defaults` 各节。`defaults` 里写的就是 config.toml 的键，是全局默认那一层。

**代码在哪**：`src/config/{file,resolve,edit,mod}.rs` 与 `src/settings/{file,edit,mod}.rs`；诊断类型 `aproxy_ipc::diag`。
运行时结构 `Config` 与 `Settings` 的**字段名没有跟着改**（`max_body_mb`、`forward_only`、`log_rotate_mb` 等仍是旧叫法），
对应关系写在 `src/config/mod.rs` 的模块文档里——对外的只有文件里的键名。

**Why:** 旧行为下写错一个字母的键等于没写，用户看到的是一个行为不对的实例；0.2.0 是今后所有版本要面对的最老读者，
键名与严格程度要在这一版定下来（用户 2026-10-11 定调：去掉全部兼容、前瞻稳定优先，见 [[0.2.0-direction]]）。

**How to apply:**
- 加新配置键：在 `config/file.rs` 加解析与测试，在 `resolve.rs` 接进分层；settings 的 `defaults` 自动跟着认识它
  （它把 JSON 转成 TOML 交给同一个解析器），只有 `listen_addr`、`log_file`、`format` 在 `defaults` 里被拒。
- 写回一律走 `config::edit` / `settings::edit`（只改目标键、同一把 `run/config-write.lock`、拒绝引入新错误），
  不要再整份序列化重写。
- 全局默认里的凭据只随它的目标继承（D-16）：`defaults.api_key` 与密钥头只在实例的 `base_url` 与 `defaults.base_url`
  规范化后相同时继承，代理口令跟着 `defaults.proxy`；扣下的项在 `Resolved.withheld`。
- settings.json 无效时各命令的分工在 `src/main.rs` 开头那一段：`settings` 与 `doctor` 自己报告，`status`、`stop`、`install`
  告警后按内置默认运行，其余退出 3。新增命令时要想清楚它归哪一类。
- 0.1.x 的平铺 settings.json 目前没有自动迁移（见 TODO 的 W7 一条），接手时先看那条有没有定。
- For a read-only inventory, distinguish a missing file from an empty config layer. `config::resolve` intentionally
  accepts a missing file: startup may have enough information in overrides and defaults. A successful resolve is
  therefore not evidence that an alias target exists. Likewise, inspect settings with `settings::read`; `load_from`
  can return the last good settings, which say nothing about whether the file on disk is valid now.

**Evidence:** 提交 5518090（B2）、dc5ee49（B4）、26883f1（B3）；集成测试 `unknown_config_keys_fail_start_naming_file_and_key`、
`invalid_settings_block_start_but_not_stop_status_or_repair`、`settings_defaults_reach_config_and_credentials_follow_their_target`
（`tests/proxy_integration.rs`）；计划 [release-0.2.0](../plan/release-0.2.0.md) 的 B2、B3 记录列了定下的事与同设计稿的出入。

The missing-file distinction is exercised by `doctor_json_document` (`tests/json_docs.rs`) and
`instance_without_a_config_file_restarts_and_restores` (`tests/restart_integration.rs`); the settings distinction
by `an_invalid_file_falls_back_to_the_last_valid_settings` (`src/settings/mod.rs`).

**Recheck when:** Either reader changes its missing/invalid-file policy, the runtime config or settings fields
are renamed, or the migration strategy for 0.1.x settings is decided.
