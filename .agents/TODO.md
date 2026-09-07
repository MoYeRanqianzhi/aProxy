# TODO（团队共享，定期清理；完成项删除并留 git 历史）

> 2026-09-05 据进度盘点建立。2026-09-07 全部近期收口完成（v0.1.0-alpha.4 已 tag）。
> 2026-09-08 补录 tag 之后一轮（性能优化 + 磁盘缓存 + 重构 + skill），全部已完成。

## 近期收口（全部完成）

- [x] **A. alpha.4 收口**：bump + tag v0.1.0-alpha.4；release 构建于独立 `CARGO_TARGET_DIR=target-rel`（target/release/aproxy.exe 被生产实例锁定不可覆盖）——用户自行替换部署
- [x] **B. logs 前台实例修复**（d378e30）：限时 5 秒明确报错
- [x] **C. 日志治理**（6a6f137）：status 清孤儿日志（保 .restore 待恢复实例）+ 运行期 8MiB 每小时轮转
- [x] **D. CI**（624c6a6）：windows 全量（fmt --check/clippy -D warnings/test）+ ubuntu/macos cargo check
- [x] **E. 文档树**（624c6a6/382dbec）：README + docs/architecture + .agents/docs 三篇
- [x] **别名系统** [G]（3e5e1a1）：settings.json 内部配置层 + aproxy start/stop <别名> + alias 管理
- [x] **三项收口** [G]（2b88808）：settings default_config 指定默认配置 + max_retry_backoff_secs（0=零延迟）+ 控制台 UTF-8 乱码修复
- [x] **审查**（fab2bad）：主会话直接审查（后台 workflow agent 连续被外部中断）——修复 4 项（退避指数写死/默认配置失效误导/verbatim 前缀/测试期望），跳过 3 项有据

## alpha.4 之后（2026-09-07~08，全部完成）

- [x] **零拷贝回放**（0fb38d8）：流式回放/keepalive 转发 slice_ref 共享原分配，50MB 响应峰值 64.7→~33MB
- [x] **release profile**（cbe3e1b/1f3a0ae）：fat LTO + opt-level=z + strip，6.78→4.06MB（-40%）；panic 保持 unwind（理由入注释，abort 实测数据入记忆）
- [x] **磁盘缓存双模缓冲**（1fe69ca→0ee93c3）：请求体/响应超 1MiB 溢写 spool，内存与负载解耦——每并发 10.5→~2.4MB（-77%），1GB 预算并发容量 80→320+，吞吐持平；压测报告 docs/benchmark-memory.md，原始数据 .agents/bench/
- [x] **max_body_mb 分层配置**（随磁盘缓存）：默认 128MB（0=不限），settings.json 全局默认 + toml 覆盖，解锁旧 10MiB 硬上限
- [x] **main.rs 拆分**（70e103f）：1719→114 行；cli.rs + commands/ 九命令 + server.rs + util.rs
- [x] **aproxy-cli skill**（f62d837）：.claude/skills/ 渐进式全量参考；版本留存策略见 memory/2026-09-07-aproxy-cli-skill-versioning（小版本直改 latest，大版本才留存）；评测经用户决定放弃（agent 越界读源码，验证靠逐条源码核对）
- [x] **alpha.5 收口**（2026-09-08）：bump v0.1.0-alpha.5 + tag；architecture.md 文档同步（模块分工/双模缓冲/版本路线）；版本路线定调——0.1.x 全程预发布后缀，0.2.0 起才引入 UI（远期）；生产实例（12345/12349）仍跑旧二进制，新功能需用户替换部署后生效

## 中期功能（对齐「无限重试、不中断」使命）

- [ ] **正式发布：GitHub 构建指令集多版本**（必然项，2026-09-07 定调）：CI 矩阵 baseline + `RUSTFLAGS="-C target-cpu=x86-64-v3"`（AVX2），产物命名区分，发布页两者都放；详见 memory/release-engineering
- [ ] **F. IPC 观测扩展**：ping 响应带请求计数/重试计数/最近错误，`status` 展示（走管道不碰代理端口，符合铁律）
- [ ] **G2. 运行期看门狗** [G]：守护崩溃自动拉起（与「分离无父进程」模型有张力，需小型监督进程，先讨论再动）。2026-09-07 定调：这是通向「绝对不间断」的缺失一环——unwind 管小故障隔离，看门狗兜底进程级死亡；也是未来若采用 panic=abort 的前置配套（见 memory/panic-abort-outlook）
- [ ] **H.（可选）配置热重载**：IPC reload，避免 stop/start 断流
- [ ] unix 分支实测（UDS IPC / unix spawn 路径从未在类 Unix 环境运行过；CI ubuntu/macos 只 cargo check）

## 已结案（有意跳过，见记忆/审查记录）

- CREATE_BREAKAWAY_FROM_JOB（作业对象不允许时 CreateProcess 直接失败）
- 命名管道 ACL/冒名校验（tokio 不暴露，误判方向 fail-safe）
- stop 退出码差异、status/stop/logs 忽略 --config（有意设计）
- settings.json 并发 add 丢更新（本地单用户 CLI，last-write-wins 可接受）
