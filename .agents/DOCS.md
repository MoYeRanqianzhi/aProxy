# 文档索引

## 面向人类（./docs/）

- `docs/architecture.md` — 架构总览：模块职责、重试/流式回放、守护模型、IPC、restore、日志治理

## 面向 agent（./.agents/）

- `AGENTS.md`（仓库根）/ `CLAUDE.md` — 工作约定
- `.agents/MEMORY.md` — 关键记忆索引
- `.agents/memory/` — 长期记忆（架构决策、审查结论、环境事实）
- `.agents/TODO.md` — 共享待办
- `.agents/docs/` — 开发文档（本文档树）

## .agents/docs/ 内容规划

- `daemon-model.md` — 守护进程运行模型决策记录（为什么命名管道而非端口、
  实例键=端口号、restore 记录的生命周期、.restore 不被 status 清理的原因）
- `review-history.md` — 两轮审查（66 项首轮 + 40 项守护轮 + 本轮）的发现与
  修复分布、有意跳过项及其理由
- `environment.md` — 开发环境事实（本机代理环境变量污染与 NO_PROXY 隔离、
  生产实例保护约定、测试端口派生规则）
