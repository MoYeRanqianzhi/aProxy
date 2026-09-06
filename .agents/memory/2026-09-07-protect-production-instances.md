---
name: protect-production-instances
description: 铁律+教训：绝不 stop/kill 用户生产实例（曾 stop default 命中 12345 生产实例并险断会话）；测试前必须准备好即时恢复手段
metadata:
  type: feedback
---

2026-09-07 严重事故：冒烟 `aproxy stop default` 时，`default` 解析到的是**用户真实默认配置**（12345 生产实例），直接停掉了它。更危险的是：会话内的 agent 工作流本身也依赖该代理——停掉实例可能同时中断自己的会话，事后无法汇报、无法恢复。

**用户明确要求**：测试 stop 类破坏性命令前，必须提前准备好启动（恢复命令备好、或先起一个可牺牲的测试实例验证），测试后立刻重新启动被停实例，避免中断的同时完成测试。

**Why:** stop/kill 是不可逆动作，且 aProxy 本身是会话的依赖项——测试它等于测试自己的生命线。`default`/`idle` 这类「作用于用户真实配置」的保留字测试绝不能直接执行。

**How to apply:**
1. 任何 `stop <保留字/别名/default>` 冒烟前：先确认目标端口是否用户实例（`aproxy status` 对照 base_url/config_path），或干脆不起实例直接验证「无实例时的输出」
2. idle 语义测试：用**测试实例 + 极小阈值（如 1 秒）**，绝不针对用户实例跑 idle
3. 测试会停实例的场景：把恢复命令写在同一条 shell 里（`stop ... ; start ...` 连跑），或先验证 start 路径可用再测 stop
4. 铁律不变：绝不 stop/kill 用户实例（[[daemon-model]] 中已有，此条是它的实操细则）
