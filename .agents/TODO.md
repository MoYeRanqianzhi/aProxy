# TODO（团队共享，定期清理；完成项删除并留 git 历史）

> 2026-09-05 据进度盘点建立。优先级从上到下，标注 [G] 为用户点名需求。

## 近期收口（工程债）

- [x] **B. logs 前台实例修复**：`--foreground` 实例不写日志文件导致 `aproxy logs` 无限等待——限时 5 秒后明确报错（d378e30）
- [x] **G1. restore 命令** [G]：崩溃/断电/系统重启后一键恢复实例；空清单静默 exit 0（开机自启友好）；幂等跳过已在运行（d378e30）
- [ ] **A. alpha.4 收口**：bump + tag + `cargo build --release`，提示用户替换 aproxy-using.exe（生产 12349 实例仍在跑无 status/stop/logs/restore 的旧版）
- [ ] **C. 日志治理**：死实例日志随 status/restore 枚举顺带清理（`~/.aproxy/logs/` 已堆 57 个测试残留）；运行期大小轮转（follower 已支持截断检测）
- [ ] **D. CI**：windows-latest 跑 `cargo fmt --check` / `clippy -D warnings` / 全量 test；ubuntu 至少 `cargo check` 保 unix 分支可编译
- [ ] **E. 文档树落地**：README（安装/命令/多开/守护/restore）+ `./docs/`（人类文档）+ `.agents/docs/`（agent 开发文档，沉淀两轮审查的架构决策）

## 中期功能（对齐「无限重试、不中断」使命）

- [ ] **F. IPC 观测扩展**：ping 响应带请求计数/重试计数/最近错误，`status` 展示（走管道不碰代理端口，符合铁律）
- [ ] **G2. 运行期看门狗** [G]：守护崩溃自动拉起（与「分离无父进程」模型有张力，需小型监督进程，先讨论再动）
- [ ] **H.（可选）配置热重载**：IPC reload，避免 stop/start 断流
- [ ] unix 分支实测（UDS IPC / unix spawn 路径从未在类 Unix 环境运行过）

## 已结案（有意跳过，见记忆/审查记录）

- CREATE_BREAKAWAY_FROM_JOB（作业对象不允许时 CreateProcess 直接失败）
- 命名管道 ACL/冒名校验（tokio 不暴露，误判方向 fail-safe）
- stop 退出码差异、status/stop 忽略 --config（有意设计）
