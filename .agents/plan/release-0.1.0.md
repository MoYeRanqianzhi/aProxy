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
- Claude Code 三层流超时已黑盒实测，结论与实验方法见
  [[claude-code-stream-watchdogs]]：首字节约 360s、字节级空闲 300s（注释心跳可
  覆盖）、事件级空闲 600s（注释与 ping 都不算，只能由客户端
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS` 解除）。实验方法：python mock 上游记录请求到达
  与连接重置时刻，隔离 `CLAUDE_CONFIG_DIR` + 假 key + `NO_PROXY` 驱动 `claude -p`。

## WS-1b 设计定稿（依据上述实测）

1. 新增 `keepalive_trigger = accept | body_stream | any`（settings 全局默认 +
   toml 覆盖），默认 `any`：Accept 含 text/event-stream，或客户端原始请求体顶层
   `"stream": true`（在 request_transform 之前判定；磁盘 spool 的大请求体要流式
   只看顶层键，不能整体物化）。
2. 首轮提交：保活适用的请求，上游返回 2xx 且 content-type 为 SSE 时立即把真实
   status/响应头转给客户端（去掉 content-length 与 hop-by-hop），之后照旧缓冲完整
   流、校验无误再回放；流中途出错就在同一响应里继续重试（客户端只见过心跳）。
   上游在 keepalive 间隔内仍没回响应头，就先发骨架头（200 + text/event-stream），
   与既有保活通道同一取舍。配置了 response_transform 时不提交上游真实头，只走
   骨架。
3. 心跳全程覆盖：提交之后无论是在等上游、缓冲上游流还是退避，都按 keepalive
   间隔发注释（修 proxy-core-02 的在途空窗）。
4. 客户端在长等待后断开时（接近 600s），日志给出「若客户端是 Claude Code，请设置
   CLAUDE_STREAM_IDLE_TIMEOUT_MS」的提示。
5. 文档与 aproxy-cli skill：接入 Claude Code 时写入 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`
   （推荐值以实测为准）；非流式请求无法心跳，同时建议调大 `API_TIMEOUT_MS`。
6. 顺带：WS-2 建议的转换失败 502 文案（保留「不重试」字样，测试断言依赖它）。

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
- [x] Claude Code 看门狗黑盒实验（7 组，结论见 [[claude-code-stream-watchdogs]]）
- [ ] 第 1 波：WS-5b 已合并（2e06dce + 主代理补修 069e001：grep -E、.gitattributes
  让 install.cmd 以 CRLF 入库）；WS-2 已合并（e60de76）；WS-1a、WS-3、WS-4 进行中
  （WS-4 曾因账户并发上限 409 中断，已续跑）
- [x] WS-1a 已合并（64aeaf2；真实 Claude Code 不发 Origin、Host 为 127.0.0.1:端口，已核对抓包）；WS-3 已合并（5c71f35，进程身份改为 pid + 创建时间，旧记录保守不误杀）
- [x] 第 2 波：WS-4（a171cdf）、WS-5a（d5f56b9）、WS-1b（b9b1f7d）已合并；主代理收尾项
  （--force 帮助文字、Linux clippy 两处、identity-no-name 记忆）已完成。stash@{0} 草稿
  仍待用户决定是否丢弃
- [x] 选版只认项目版本号文法（0dc9440）：历史测试 tag alpha.12t3 按 semver 胜过 alpha.17，
  Rust 侧与三脚本统一修复；脚本在 gawk / dash+mawk / busybox / PS 5.1 / pwsh 7 / cmd 实测
- [x] install.ps1 不能加 BOM（688bbbd）：BOM 让 irm | iex 的 param 块失效（5.1/7 实测），
  5.1 本地 -File 解析失败作为已知限制写入文档
- [x] 第 3 波文档：WS-6 两轮已合并（d481ba7、9c0ba4b），主代理修正选版措辞、macOS 状态、
  开发命令（b45a399）
- [x] Windows `cargo test --workspace --locked` 全绿（2026-10-04）：日志脱敏测试并行跑
  捕获为空是 tracing-core 单 Dispatch 快速路径所致，WS-1a 时就存在，已修（cf81486）
- [ ] WS-1b 独立对抗审查（ws1b-review 代理进行中）→ 按结论修复
- [x] 真实 Claude Code 无限重试验收通过（2026-10-04，master b9b1f7d 的 debug 构建，
  默认配置，隔离 APROXY_HOME；claude 2.1.288，`CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000`，
  白名单 `env -i` + 隔离 CLAUDE_CONFIG_DIR + 假 key）。mock 按请求序号注入：529 → 断连 →
  200+error JSON → SSE 半截断开 → SSE error 事件 → 500 → 挂起 120s → 503 → 529 → 529 →
  第 11 次（首请求后 757s）成功并慢速生成 140s。结果：claude 退出码 0、耗时 15 分钟，
  输出完整且不含第 4 次半截流的标记文本；mock 收到 11 次主请求全部 retry-count=0
  （客户端从未自行重发）；aProxy 只记录 1 条 POST /v1/messages 的「代理请求」，末行
  「上游成功，回放到已提交的响应（保活通道） attempt=11」。退避节奏与 retry.rs 一致
  （0/0/0/5/10/20/40/80/160/320s）。审查若改动保活路径，需按同一时间线重跑
- [x] ssh remote 全新目录全量（2026-10-04，6216f4b，Ubuntu glibc 2.39）：14 个测试二进制 427 项全过，fmt/clippy 干净。WS-1b 修复轮合并后需在新目录重跑
- [ ] Linux gnu 产物 glibc 下限：remote 无 zig，改由 rc 演练时 release.yml 的构建断言验证
- [ ] 合并后审查与修复
- 待办：aproxy-format 需发 format 新版本才能让 transform-03 生效；doctor 测试不隔离
  主目录（未进 0.1.0，见 TODO）
