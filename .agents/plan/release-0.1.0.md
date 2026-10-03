# 0.1.0 正式发布修复计划

> 交接笔记：下一个接手的会话（可能是没有今天上下文的你）从这里恢复。动手前先对照
> `git log`、`git worktree list` 与本文「进度」节核对现状——笔记可能落后于代码。

## 目标与验收

把 2026-10-04 审查（`.agents/review/2026-10-04-0.1.0发布前-13维审查.md`）确认的
0.1.0 发布阻断与发布前应修项全部闭环，达到可打 `v0.1.0` 的状态。

**唯一硬验收（用户 2026-10-04 定调）**：「你只需确保最终结果符合无限重试要求即可」。
落到可检验的标准：

1. 默认配置下，任何上游失败（网络错误、4xx/5xx、200 携带 error、流中断）都无限
   重试，直到成功或客户端主动断开；新增的任何出口（Host/Origin 拒绝、确定性错误
   透传等）要么默认关闭，要么只作用于**从未转发**的请求。
2. 对主力客户端（真实 Claude Code，当前 2.1.288）端到端成立：上游持续失败或长时间
   生成期间，客户端不得因首字节/空闲超时自行放弃。验收方式是真实 Claude Code 经
   隔离实例打到可控故障 mock 的实测，不是单测。

## 范围决定（用户 2026-10-04「同意你的建议」）

采纳审查记录「需用户拍板的决策」节的推荐选项，例外与补充如下：

- **macOS 不处理**（用户：没有测试机器，后续交给其他贡献者）——阻断 7、决策 1 与
  决策 2 的 macOS 部分移出范围，详见 [[2026-09-12-macos-support-required]]。
- Linux 可在本机 WSL（Debian 12，glibc 2.36，无 curl/git/python）、`ssh router`、
  `ssh remote`（均 Ubuntu 24.04，glibc 2.39，有 cargo/git/curl/python3）实测。
- 推送分支、打 tag、改 GitHub 仓库设置（environment 保护规则、分支保护、移动
  CARGO_REGISTRY_TOKEN）属于对外操作，执行前仍需逐次向用户确认。

## 已核实的新证据（2026-10-04，主代理实测）

- 真实 Claude Code 2.1.288 的主请求：`POST /v1/messages?beta=true`，
  `Accept: application/json`，请求体 `"stream": true`，`x-stainless-timeout: 600`。
  → 保活通道只看 Accept（`src/proxy.rs:878`），Claude Code 的流式请求永远进不了
  保活通道：审查项 proxy-core-03 实锤，升级为发布阻断。
- Claude Code 二进制内含三层流看门狗：`CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS`、
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS`（报错「no chunks received」）、
  `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS`；报错文案直指「A proxy or gateway that
  buffers streaming responses can cause this」。默认值与「SSE 注释心跳能否重置
  它们」由黑盒实验测定（见下方进度），结论决定 WS-1b 的设计。

## 工作流与文件归属

并行代理各用 `.worktrees/<名>`（基于本地 master）独立 worktree + 独立 target，
互不可见半成品；主代理负责合并、全量测试与 Linux 复验。总并发 ≤5。
各代理不改 README/docs/skill/.agents（文档统一放 WS-6，避免冲突），在报告里列出
文档影响。

| 代号 | 内容（审查 id） | 文件归属 |
|---|---|---|
| WS-1a | Host/Origin 校验（security-01/02）、凭据统一脱敏（config-01/02、security-03/04、perf-12）、spool flush（perf-01）、ubuntu CI 测试修复 F1（tests-01） | src/proxy.rs、config.rs、settings.rs、util.rs、doctor.rs、commands/{start,config}.rs、tests/proxy_integration.rs |
| WS-1b | 保活触发条件 + 首轮提交 + 在途心跳（proxy-core-01/02/03，按实验结论设计）、转换失败 502 文案 | src/proxy.rs、config.rs、settings.rs、tests/proxy_integration.rs（待 WS-1a 合并后开工） |
| WS-2 | 转换器进程池加固（transform-01/05/09）、aproxy-format auto 模式（transform-03/07） | src/transform.rs、examples/format-echo.rs、tests/transform_integration.rs、aproxy-format/** |
| WS-3 | 身份判定去名称化（watchdog-04，参考 stash@{0} 草稿）、重拉换端口残留（watchdog-01）、restart 预检与 stop --force 清理（daemon-ipc-01/02） | src/watchdog.rs、daemon.rs、server.rs、commands/{stop,restart,restore}.rs、tests/restart_integration.rs |
| WS-4 | 更新通道与选版（install-02/03、transform-04 客户端侧）、滚动升级失败回滚（install-01） | src/install/**、commands/install.rs、cli.rs、tests/install_*.rs |
| WS-5a | 质量门禁（release-01/02/03、tests-02/03/04）、发布幂等（release-05/06/11）、GLIBC 构建（install-04 构建侧）、workflow 权限（security-06、release-10）、format 线 make_latest、npm dist-tag | .github/workflows/*、npm/build-and-publish.sh、Cargo.toml |
| WS-5b | 一键脚本批量修复（install-10/11/14、docs-01/02、security-12）、选版过滤（脚本侧）、glibc 探测与 musl 回退（install-04、release-12） | scripts/install.*、npm/aproxy/bin/aproxy.js |
| WS-6 | 文档与 skill 全量对齐（docs-03/04/05/07/08、release-15 及各 WS 的文档影响）、SECURITY.md、发布说明草稿 | README*、docs/**、.claude/skills/** |

## 验证计划

1. 每个代理：fmt --check、clippy -D warnings、`cargo test --lib` + 自身相关的定向
   集成测试，全绿才在自己分支提交。
2. 主代理合并后：Windows `cargo test --workspace --locked` 全量（串行守护测试）；
   `ssh remote` 全新目录全量；WSL 验证 gnu 产物的 glibc 下限。
3. 无限重试验收：真实 Claude Code → 隔离 aProxy → 故障 mock（持续 529 若干分钟后
   成功、长生成、流中途断开三种），客户端必须拿到最终成功结果。
4. 合并后独立审查一轮（≤5 并发），再进入 rc 演练（需用户确认后打 tag）。

## 进度

- [x] 审查记录、CLAUDE.md/AGENTS.md、.gitignore、macOS 决定已提交（04deb86…c188508）
- [x] Claude Code 请求头实测（见上）
- [ ] Claude Code 看门狗黑盒实验（静默 / 注释心跳 / ping 事件三组，进行中）
- [ ] 第 1 波：WS-1a、WS-2、WS-3、WS-4、WS-5b
- [ ] 第 2 波：WS-1b、WS-5a
- [ ] 第 3 波：WS-6 + 全量验证 + 无限重试验收
- [ ] 合并后审查与修复
