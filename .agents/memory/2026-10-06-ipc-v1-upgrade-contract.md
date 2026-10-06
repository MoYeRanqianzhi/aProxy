---
name: ipc-v1-upgrade-contract
description: 改控制协议、.pid/.restore/claim/install.state 格式、install 流程或 CLI 参数之前读——哪些是跨版本永久接口、0.1.x 兼容窗口何时结束、为什么
metadata:
  type: project
  scope: src/daemon.rs、src/compat_0_1_0.rs、src/install/、src/watchdog.rs 的 claim、src/cli.rs 的隐藏参数
  status: active
  last_verified: 2026-10-06
---

原地升级时新旧两个版本的代码同时在跑：旧安装器、旧 CLI、旧看护者读新版本写下的文件、联系新守护，
新版本也要读旧版本留下的现场。所以下面这些东西不是内部实现，改了就是破坏升级。

**永久接口（没有兼容代码，靠「不改」维持）**
- `run/<端口>.pid` 的必填字段集（`InstanceRecord`）；0.1.0 的普查会删掉解析不了的 `.pid`。
- `.restore` 的字段名 `args`、`log_path`：0.1.0 两个字段都带 serde default，改名会被读成空参数，
  看护者按默认配置重拉实例。
- `watchdog.claim` 的三个字段（`version` 可选）；0.1.0 解析不了就覆盖 claim，两个看护者同时在任。
- `install.state` 的阶段词表与字段名冻结，新字段一律可选：交棒后等待结局的是较旧的安装者，它读接手者
  写下的状态报告结局（读不出来就干等到 600 秒超时）。
- unix 控制 socket 路径 `run/<端口>.sock`；控制协议 v1（`{"v":1,"op"}`，未知状态读作 unknown）。
- CLI 面：`--version` 输出、`install --continue`、`install --continue --handover-from <pid>`，以及
  `.restore` 里可能出现的每个启动参数。
- R1：守护启动时退役比自己旧的看护者（claim 没有 version 视为 0.1.0），新版本胜出。
- 早交接：备料校验后，安装由目标版本的二进制执行；降级由较新的安装者自己执行。

**Why:** 早交接之前，升级由旧安装器驱动，新版本必须让旧代码看得懂自己（旧懂新），而且旧安装器的缺陷
新版本修不了（0.1.0 在 unix 上不停旧看护者，只能靠 R1 从新版本这边补）。从这一版起由目标版本驱动，
兼容只需要「新读旧」，旧版本只需读懂 install.state。

**0.1.x 兼容窗口**：0.1.0 的安装器能直接跳到任何后续版本，且那一跳由 0.1.0 的代码驱动，所以
`src/compat_0_1_0.rs` 的 S1–S5 与 `continue_install` 里的 relaying 豁免要保留整个 0.1.x 线，0.2.0 一起删，
CHANGELOG 写明升级下限（先装到 0.1.x 的某个版本）。

**How to apply:** 动上面任何一项前，先跑 `tests/compat_v0_1_0.rs`（冻结的 0.1.0 结构，双向解析），想清楚
更旧的读者会怎样；新字段用 serde default。改 install 流程要让 `tests/upgrade_from_0_1_0.rs`（CI 的
upgrade-from-0-1-0 任务）保持通过。设计全文与逐条依据（含 0.1.0 → 新版本每一步由谁执行的分析）在计划的最后版本：
`git show 9e91a95:.agents/plan/ipc-v1.md`。

**Evidence:** CI 88cf50c（2026-10-06）upgrade-from-0-1-0 在 windows 与 ubuntu 上用真实 v0.1.0 二进制驱动升级
通过；早交接由 install_flow 的 6d/6e 与 install_flow_lib 的 `only_the_named_successor_takes_over_a_live_install`
覆盖；变异验证：去掉 Windows 交换后的那一跳，快路径测试因 staging 删不掉而失败。

**Recheck when:** 规划 0.2.0（删兼容模块、写升级下限）；改动上述任一产物；upgrade-from-0-1-0 任务变红。

相关：[[install-pitfalls]]、[[ci-unix-blindspot]]、[[identity-no-name]]
