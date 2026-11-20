# TODO（团队共享，定期清理；完成项删除并留 git 历史）

> 2026-11-20 整理：做完的条目与全部做完的小节已删（见 git 历史）；现在分三部分：0.2.0、0.2.0 之后、更早留下的未排期条目。

## 0.1.0 已发布（2026-10-04）

v0.1.0 已发布：GitHub Latest、npm `latest`、crates.io 均为 0.1.0（rc.1 演练在先）。硬验收「默认配置下无限重试对真实
Claude Code 端到端成立」通过三次（含 rc.1 正式 release 构建）；Codex 经正反黑盒实测确认需调 `stream_idle_timeout_ms`。
发布计划已结束并删除，最后版本见 `git show 58e62cd:.agents/plan/release-0.1.0.md`；审查依据见
`.agents/review/2026-10-04-0.1.0发布前-13维审查.md`。

## 0.2.0（2026-10-11 用户下令）

计划：[plan/release-0.2.0.md](plan/release-0.2.0.md)，执行顺序、tag 节点与进度在它的「执行顺序与进度」一节（2026-11-11 用户：
做完 0.2.0、过程中持续提交与打 tag）；定调：[0.2.0-direction](memory/2026-10-11-0.2.0-direction.md)。
**接下来做什么、谁在做，看 [plan/codex-tasks-0.2.0.md](plan/codex-tasks-0.2.0.md)**（交给 Codex 的任务清单，按先后排好）。
已发：alpha.1（2026-10-13）、alpha.2（2026-11-17）、alpha.3（2026-11-19）。下一个：alpha.4（format 一线做完）。

- [ ] **W2 契约实现**（按设计稿 §14）：P1–P5 与 B0–B9 已做完（alpha.3）；剩 B10a–B10d、B11 与 W2c，即 format 协议 1 与官方
  aproxy-format，任务 t06 在做，做完打 alpha.4。实现落地后重写永久接口清单
- [ ] **性能**（用户 2026-11-20）：在服务器上严格测出基线，功能都接入之后重测，rc.1 之前做优化。见
  [plan/perf-0.2.0.md](plan/perf-0.2.0.md)
- [ ] **已知缺口**（B9 留下，归 W6）：`start_failed` 的 `details.code` 没填；没有路径产生 `instance_incompatible`；
  `EditError::Io` 没有操作码；桌面界面拉起强制安装时 stdio 为 null（归 W5）
- [ ] **协议转换的两条已知限制**（出在 `switchyard-translation` 里，记在 [format-roles](plan/format-roles.md)）：写进给用户的文档
  （`aproxy-format` 技能与 README），并报给上游。Messages 的 thinking 签名过不了 Chat 这一条，以后可以考虑在 convert 里
  按工具调用 ID 暂存签名、下一轮请求转回 Messages 时补上（跨请求的进程内状态，worker 重启就丢），0.2.0 不做。
- [ ] **format-v* 线在 aproxy 0.2.0 正式版之前发布前，先定 envelope 怎么办**：envelope 现在随主线同号发预发布版
  （align-version.py），release-format.yml 只对齐 aproxy-format 自己，于是会按仓库里的开发版本号（例如
  0.2.0-alpha.0，不对应任何 aproxy 发布）把 envelope 发出去，aproxy-format 也依赖一个预发布版。现在不发 format
  就不用解决。另外 0.2.0 正式版之后 envelope 是继续随主线同号，还是改回只在改动时升版本，届时再定（改回的话，
  改公开 API 必须手动升版本，见记忆 transform-format）
- [ ] **W3 i18n**（给用户看的 en/zh-CN，给 agent 或不可见的固定英文）
- [ ] **W4 `aproxy skill` 命令**（install/upgrade/list/show，show 的安全设计，`--help` 末尾提示行）
- [ ] **W5 UI**（Tauri 2 编进本体、`desktop` feature 默认开、headless 版本；自定义标题栏；Mica 默认关可开）。第 3 步留下的：
  从快捷方式启动会闪一下终端（要 GUI 子系统的启动器，或清单的 consoleAllocationPolicy=detached——后者作用于整个 exe，
  守护进程拉起的 worker 会不会弹窗未验证）；Linux 上「跟随系统」读不到深浅色（要开 tauri 的 dbus 特性、CI 装
  libdbus-1-dev）；界面偏好（含 Mica 开关）只在内存里，重启恢复默认（随 W2 的 settings `ui` 段持久化）。
  接入真实数据的设计与分段见 [w5-real-data](plan/w5-real-data.md)（开机自启：三个平台各一种登记；扩展包与写授权：0.2.0 做，沙箱 iframe 加 Rust 侧授权）
- [ ] **README 截图**（用户 2026-10-12）：UI 完成后构造演示数据，拍中英两套截图，中文版用于 README.md，英文版用于
  README_EN.md。演示数据里不能出现真实的 key、上游地址或个人信息
- [ ] **W6 审查发现的修复与重构**；另含 W1 留下的：`install --from` 不做降级检查（审查 CI-06）；以及
  `tests/restart_integration.rs:170`（`restart_after_port_change_reports_new_port`）断言失败时打印 restart 的 stdout 与
  stderr，再查 2026-10-12 演练 38291736162 的 test (windows-latest, desktop) 里那次偶发（现在只有「restart 应成功，实际
  exit 非零」，查不了原因；同一提交 458900f 的 ci.yml 里 windows 是绿的）
  同类的又一次（2026-11-15，CI run 43745240586 的 windows (desktop)，提交 1607caa）：`tests/proxy_integration.rs` 的
  `port_zero_restore_record_uses_actual_port` 里 `aproxy stop <端口>` 退出非零、stdout 为空（当时没打印 stderr，现在
  断言失败时两者都打印）；同一提交的 windows (headless) 与下一个提交都是绿的。stdout 为空说明走的是只写 stderr 的
  分支——`stop_instance` 的失败提示都写 stdout，所以更像是选目标阶段的 `Unresponsive`（ping 不通而进程还在），
  这是推测，等下一次带 stderr 的失败来证实
- [ ] **W7 发布 0.2.0**。另外两件：
  - 撤回预发布期的措辞（`--pre` / `@next` / `--version`、「目前只有预发布」）。2026-10-13 文档终审列的位置：README.md 31、60–63，
    README_EN.md 81、135–142、183，docs/INSTALL_AGENT.md 54–58、156，compatibility.md 步骤 3，CHANGELOG.md 24；
    带「alpha.1」字样、等有真实数据要改的：README.md 206，README_EN.md 380，SKILL.md 88，commands.md 476，troubleshooting.md 145，
    architecture.md 513、678，aproxy-format 的 guide.md（envelope 钉 `=0.2.0-alpha.1` 改回 `"0.2"`）。行号会漂，按内容找。
  - 重写 SECURITY.md：支持的版本改为 0.2.x，威胁模型补上桌面窗口与强制安装。
  - **0.1.x 的 settings.json 怎么过渡**（B3 留下，rc.1 之前要定并做完）：0.2.0 只认分节的新形状，0.1.x 的平铺文件会让
    多数命令以退出码 3 结束。现在只有技能 compatibility.md 里的手工对照表。倾向：`install` 换版时把平铺文件迁成新形状
    （原文件留备份），并让 `upgrade_from_0_1_0` 的 CI 测试在升级后跑一条要读 settings 的命令。见计划 B3 记录第 4 条。

### 0.2.0 之后紧接着（近期，用户 2026-10-12）
- [ ] **并发限制与排队**：aproxy 的排队放行 + 官方 aproxy-format 的并发/限速/额度规则（协议形状在 0.2.0 定稿，见计划 W2 的 format 定稿补充）
- [ ] **用量与费用统计**、**按 key 限速**、**预算上限**（建在生命周期通知上）
- [ ] **Gemini 协议**（官方 aproxy-format 的协议转换）

### 以后可能会做（低优先级，暂不做）
- [ ] embeddings、图片等非对话接口的转换

## 更早留下的（没排进 0.2.0；按当时的来源分组，动手之前先核对是否仍然成立）

用户原则（2026-10-06）：发布前每一部分都要做到足够优秀，好的设计能避免发布后才发现问题、再改动带来的连锁问题。

### 0.1.0 发布前留下（2026-10-06）

- [ ] **format 扩展的开放问题**（低优先）：（1）`state` 不设大小上限，文档只建议保持很小——若有人塞大对象，它随该请求的每行信封往返；（2）format 生成的协议事件当心跳时，真实客户端（Claude Code 等）的反应未实测；（3）心跳信封没有「最近一次上游失败」摘要（every_attempt 的 retry 信封有）

### 外部转换器（format）+ aproxy-format（2026-09-23 下令，2026-09-30 完成）

- [ ] **其余全局 run 目录测试的隔离化**（低优先）：仍有少数测试走全局
  run 目录（依赖真实环境语义），生产看门狗活跃时的竞争面已大幅收窄，
  逐个评估是否需要同款隔离模式。

### 大审查轮（2026-09-22，用户下令「多角度审查 + 全量实测，主代理进行」）

- [ ] **测试隔离债务：restore_recovers 测试写真实 run 目录**（本轮全量测试
  首跑偶发失败的根因定性）：该测试有意用全局真实 `~/.aproxy/run`（restore
  命令当时无隔离手段的妥协，注释已写明），守护被**生产看护者收养**
  （adopt_scan 每 tick 扫描 run_dir 全部 .restore），行为受生产环境干扰；
  单跑 5/5 过、全量并行首跑失败、复跑全绿。修法：改 APROXY_HOME 子进程级
  注入隔离（`Command.env` + `restore_file_path_in` 断言，参照
  port_zero_restore_record_uses_actual_port 的写法），幂等断言在隔离目录内
  验证。端口 0 新测试已是隔离写法，可作模板
- [ ] **优化候选（审查观察，非缺陷）**：`commands/mod.rs` 的
  resolve_config_target 在 stop/logs/start 三处各有同构展开（别名分支约 20
  行 ×3）；若第四处出现再抽公共函数（现在抽会破坏各命令报错文案的差异化）

### 日志随机命名 + IPC 上报（2026-09-22，用户定调三点）

- [ ] **集成测试补强**：随机命名日志的 e2e（自定义 log_file 经 CLI+IPC、
  换端口 restart 后 log_path 经 IPC 更新、stop 后日志清理/kill -9 后 .restore
  引用保留）——代码已落地，集成测试待补（Windows 侧 e2e + router 实测）

### 仅转发模式 forward_only（2026-09-14）

- [ ] **仅转发模式在 status/find/doctor 里没有模式标识**（审查 items，产品决定）：
  「重试 0」与正常实例的「无需重试」读数完全同形，运维时看不出这个实例已放弃
  重试保障。要做需经 IPC 暴露模式 = bump `IPC_PROTO_VERSION` 并处理混版本。
- [ ] **判定侧与回放侧的 is_streaming 可能分叉**（审查 item，low）：判定侧已按
  解码副本选 `is_streaming`，回放侧三处仍看原始字节。可观察后果仅为分块 vs
  定长分帧的差异（字节不受影响），且只在「压缩体 + content-type 非 SSE + 解码后
  含 data: 行」这种组合下出现。对齐需要二次解码或有代价，暂记。
- [ ] **仅转发模式（forward_only）落地确认**：config.toml 每实例字段 +
  settings.json 全局默认（内置 false），与 `max_body_mb`/`disk_cache` 同款分层。
  开启后**放弃重试/缓冲/心跳**，请求体与响应流式直通——可信上游 + 要真流式的
  **显式取舍**（产品最核心的重试保障让位于真流式）。`max_body_mb` 仍强制；
  上游失败 502 不重试并记 `note_upstream_failure`；响应流中断直接截断 +
  `tracing::warn`。分支点在 `read_request_body` 之前、`requests_total.fetch_add`
  之后；消费统一走 `forward_only_enabled()`（**不得 unwrap**）。文档与 skill 已
  同步（README/README_EN/docs/architecture.md + skill 四份 references + SKILL.md）；
  **代码实现与测试由并行 agent 完成，落地后按上列约束逐条核对本条目**。
  详见 `.agents/memory/2026-09-14-forward-only.md`

### 压缩响应体检查（2026-09-14，用户实测暴露）

- [ ] **测试补强：mock 上游要覆盖压缩**：既有集成测试的 mock 从不发
  `content-encoding`，这正是 220 项全绿却漏掉上述 bug 的直接原因。补一组
  「压缩响应体」集成测试（gzip/br 各一，覆盖判定与预览两条路径）
- [ ] **磁盘模式的压缩体判定（已知边界）**：响应 > 1 MiB 时判定仍走原始字节的
  增量扫描，压缩体不解码（仅预览解头部快照）。现实中 >1 MiB 的压缩错误体不存在，
  故不做流式解码；若将来出现真实场景再补

### alpha.6 之后（2026-09-09~10，unix 实测轮）

- [ ] **产品语义决策**（实测发现）：unix 上 APROXY_RUN_DIR 影响 IPC 寻址
  （UDS 路径在 run_dir 内，Windows 管道全局名不受影响）——是否对齐待定。
  2026-09-22 router 实测补充：logs/status 的注册表读取与 IPC ping 在
  RUN_DIR 重定向下行为一致（`logs <别名>` 正常连接），当前语义对使用者
  无坑；对齐与否降为低优先级设计口味问题
- [ ] **优化候选**：upstream client（reqwest/hyper）开启 TCP_NODELAY——实测与
  无 NODELAY 上游配合时有 40ms Nagle×delayed-ACK 咬合
- [ ] **测试基建**：看门狗测试子进程清理 RAII 化（panic 路径手写 kill 会跳过）；
  daemon.rs 的 UDS IPC roundtrip 单测（现为 Windows-only）
- [ ] **测试守护泄漏治理**（2026-09-14 现场实测；2026-09-21 再度实证并扩大影响面）：
  机器上累积过 46 个 aproxy 进程（2026-09-14），2026-09-21 又累积 60+
  （32 守护 + 32 看护者，tempdir 已删但进程存活）。危害升级实证：**残留
  看护者/守护会干扰并行全量测试**——两轮稳定复现 `logs_via_alias_…` 与
  `watchdog_respawns_killed_daemon` 双双失败（前者「当前未在运行」、后者
  「守护未就绪」），清理残留后同款并行组合 9.9s 全过。清理须遵铁则：
  **绝不按名批量杀**，只走产品优雅路径：`Get-NetTCPConnection` 按
  Temp 路径 pid 反查监听端口 → 逐端口 `aproxy stop <端口>`（IPC shutdown
  删 .restore，看护者不 respawn）→ 看护者等 `watchdog_idle_exit_secs`
  空闲自灭。根因是测试收尾未覆盖 panic/超时路径，与上一条 RAII 化同源

### 修复审查轮遗留（2026-09-10，独立复验后记录，详录 .agents/docs/unix-stress-review.md 复验节）

- [ ] **F2（产品决策）：is_aproxy_process 二进制名精确耦合**——改名运行的二进制
  （如生产 aproxy-using.exe 部署形态）被判非 aProxy 进程：不被收养（失去看门狗
  自动恢复，remote 实证：改名副本运行时看护者收养日志 0 条）、不计入选举、
  处决被关卡拒绝（防误杀方向正确）、claim 接管不杀前任。
  Windows 侧 merge-base 前已有语义，unix 实装后两端一致。待决策：接受「改名
  二进制不受看护」为约定，或放宽比对（会同时削弱防冒名闸门）

### 中期功能（对齐「无限重试、不中断」使命）

- [ ] **正式发布：GitHub 构建指令集多版本**（必然项，2026-09-07 定调）：CI 矩阵 baseline + `RUSTFLAGS="-C target-cpu=x86-64-v3"`（AVX2），产物命名区分，发布页两者都放；详见 memory/release-engineering
- [ ] **H.（可选）配置热重载**：IPC reload，避免 restart 断流（改配置生效目前用 restart，已有单命令路径）
- [ ] **测试稳定性**：proxy_integration 的 alias_start_and_stop_roundtrip
  偶发并行失败（单跑必过——端口/时序竞争，本轮全量跑撞上一次）
- [ ] **看门狗二期（可选）**：挂死不杀进程原地救（scoped runtime 注入 spike）、实例数极大时线程池死亡等待（unix 死亡等待已实装为轮询，见 unix-testing.md）

## macOS 支持（必须完成——用户 2026-09-12 定调）

- [ ] **macOS 平台支持修复**（后续协作者在 macOS 真机上完成实现与验证）：
  当前 macOS 上代理转发可用，但**看护者与 install 的核心保障是坏的**——
  `#[cfg(unix)]` 的实现依赖 Linux 专用原语：`/dev/shm`（install 宣告节、
  看护者心跳节）与 `/proc`（进程创建时间 / 镜像路径 / 退出判定 / zombie），
  macOS 两者皆无。
  - 症状：CI macos job（`cargo test`）自 `bd36e10` 起**每次红、从未绿过**
    （3 个 lib 测试失败：宣告节创建 / 心跳节创建 / 本进程创建时间）
  - 更严重的静默失效：`is_aproxy_process` 对**活**进程返回 false
    （readlink ENOENT 被当作「进程已死」）→ 选举、收养、处决验证全部失效
    且不报错；`process_image_path` 恒 None → install 管辖检查全部跳过
  - 实现方向：介质换 `<APROXY_HOME>/run`（或 temp_dir）并注释语义差异；
    进程查询走 libproc（`proc_pidpath`/`proc_pidinfo`）+ sysctl（zombie）
  - 验收：CI macos job 全绿 + 真机行为面实测（看护者选举/收养/心跳/清理、
    进程身份判定、install 宣告与滚动升级）
  - 影响面逐行清单、实现细节与验收标准详见
    `.agents/memory/2026-09-12-macos-support-required.md`
- [ ] **CI 现状处理（待用户定）**：macos job 红着会让每次 push 的 CI 结论
  成为 failure，长期会掩盖新失败。可选：(a) 加 `continue-on-error: true`
  并注释指向本工作项；(b) 保持红作显式提示。**回退到 `cargo check` 不可取**
  ——check 通过正是这次问题被藏住的原因

## 已结案（有意跳过，见记忆/审查记录）

- CREATE_BREAKAWAY_FROM_JOB（作业对象不允许时 CreateProcess 直接失败）
- 命名管道 ACL/冒名校验（tokio 不暴露，误判方向 fail-safe）
- stop 退出码差异、status/stop/logs 忽略 --config（有意设计）
- settings.json 并发 add 丢更新（本地单用户 CLI，last-write-wins 可接受）
- ~~unix 分支实测~~ → 已完成（2026-09-10，见上节与 .agents/docs/unix-testing.md）
