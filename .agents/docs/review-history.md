# 审查历史

> 三轮审查的发现分布与处置。新审查者先读「有意跳过项」避免重复报告。

## 第一轮（2026-08-29，代理核心）：确认 66 项

范围：retry/proxy/config。代表性发现：总超时掐断长流、keepalive 保真缺陷群、
生命周期泄漏。全部修复（见 git log「审查修复」），记录见
`.agents/memory/2026-08-29-review-findings.md`。

## 第二轮（2026-08-31，守护进程模型 3948cad）：43 候选 → 40 确认

方法：8 维度并行 + 每项双对抗验证（验证者强制只读）。HIGH 两项：
1. `spawn_detached` 句柄泄漏（bInheritHandles=TRUE）——「测试环境 start
   挂起之谜」根因，改手写 CreateProcessW
2. 测试固定端口可能杀到用户实例——改 pid 派生动态端口 + DaemonGuard

修复于 b31461c（40 项），记录见 `.agents/memory/2026-08-30-daemon-model.md`。

## 第三轮（2026-09-05，restore/logs/警告清理/日志治理 b31461c..HEAD）

范围：31a0b7e（logs）、682906c（clippy 清零+rustfmt）、d378e30（restore）、
6a6f137（日志治理）。5 维度：restore 生命周期、日志治理、logs 命令、
回归面（rustfmt/let-chains/flatten）、测试覆盖缺口。结果见本轮 workflow
报告（运行于 wf_c3ef2aa0-144）。

## 有意跳过项汇总（勿重复报告）

- `CREATE_BREAKAWAY_FROM_JOB`：作业不允许 breakaway 时 CreateProcess 直接失败
- 命名管道 ACL/冒名校验：tokio 不暴露 SECURITY_ATTRIBUTES；误判方向 fail-safe
- stop 退出码差异、status/stop/logs 忽略 `--config`：有意设计
- CLI `--api-key` 明文可见性：help 文本已提示建议改用配置文件（本地单用户低危）
- `ForwardResult` `large_enum_variant`：Bytes 已引用计数，装箱无收益
  （`#[allow]` + 理由注释在 proxy.rs）
