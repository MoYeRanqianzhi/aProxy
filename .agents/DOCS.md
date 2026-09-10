# 文档索引

## 面向人类（./docs/）

- `docs/architecture.md` — 架构总览：模块职责、重试/流式回放、守护模型、IPC、restore、日志治理
  （**待同步**：main.rs 拆分后的模块分工、磁盘缓存双模缓冲）
- `docs/benchmark-memory.md` — 高并发压测与磁盘缓存优化报告（before/after 实证）

## 面向 AI 用户（.claude/skills/）

- `aproxy-cli/` — aProxy CLI 全量参考（SKILL.md 导航 + references/latest/ 五分文件）；
  版本留存策略与持续更新义务见 `.agents/memory/2026-09-07-aproxy-cli-skill-versioning.md`

## 面向 agent（./.agents/）

- `AGENTS.md`（仓库根）/ `CLAUDE.md` — 工作约定
- `.agents/MEMORY.md` — 关键记忆索引
- `.agents/memory/` — 长期记忆（架构决策、审查结论、环境事实）
- `.agents/TODO.md` — 共享待办
- `.agents/bench/` — 压测编排脚本与原始采样数据（before/after CSV）
- `.agents/docs/` — 开发文档（本文档树）

## .agents/docs/ 实际内容

- `daemon-model.md` — 守护进程运行模型决策记录（为什么命名管道而非端口、
  实例键=端口号、restore 记录的生命周期、.restore 不被 status 清理的原因）
- `review-history.md` — 各轮审查的发现与修复分布、有意跳过项及其理由
- `environment.md` — 开发环境事实（本机代理环境变量污染与 NO_PROXY 隔离、
  生产实例保护约定、测试端口派生规则）
- `unix-testing.md` — unix 分支首次实机测试报告（2026-09-09，Ubuntu 实机）：
  5 个 unix 专属缺陷的发现与修复、功能/并发/内存/perf/体积全套数据、
  mock 脚本教训、遗留事项清单
- `unix-stress-review.md` — unix 分支压力实测审查报告（2026-09-10 第二轮）：
  代码逐行审查 + 10 项压力场景实测的问题归档（S1 respawn 竞态/S2 claim 覆写/
  S3-S6 遗留占位与多端差异），含根因链、实测证据、修复方向与探针复现指引；
  探针资产 `.agents/bench/probe/`
