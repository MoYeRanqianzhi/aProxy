# TODO（团队共享，定期清理；完成项删除并留 git 历史）

> 2026-09-05 据进度盘点建立。2026-09-07 全部近期收口完成（v0.1.0-alpha.4 已 tag）。
> 2026-09-08 补录 tag 之后一轮（性能优化 + 磁盘缓存 + 重构 + skill），全部已完成。
> 2026-09-09 第四轮审查修复 + 实测 restart bug + skill 指引优化，全部已完成。

## 仅转发模式 forward_only（2026-09-14）

- [x] **落地 + 独立审查闭环**：代码/测试/文档全部落地（`d357915`/`6daedec`/
  `391a65a`/`fd168e4`/`8f55746`/`8a1cff5`）。一轮独立审查（4 维 + 逐条对抗式复核）
  报 29 条、复核成立 22 条、驳回 7 条；成立的已全部处置，其中：长流期间刷新
  活动时间戳（否则 `stop idle` 会误杀正在传输的实例）、客户端上传中断不再被
  归因为「上游请求失败」、CL 与 TE 并存时剥离 content-length（否则静默截断）、
  `identity` 层不再拖垮整条解码链、MSRV 声明为 1.88、brotli 编码器移出常规依赖。
  测试面补齐三处规格零覆盖并修掉一条恒真断言（241 项全绿）。
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

## 压缩响应体检查（2026-09-14，用户实测暴露）

- [x] **压缩错误体不再退化为 hex**（f37e813 + 9ed566f）：Cloudflare 的 brotli
  404 页在日志里只剩 hex。根因是**检查路径跑在压缩字节上**（reqwest 为保真透传
  刻意不解压），连带 `is_error_body` / `is_stream_error_body` **静默失效**——
  HTTP 200 携带 error JSON 不再重试、流尾 error 事件检测不到。修法：新增
  `src/decode.rs` 解一份副本供检查，**转发字节不变**；日志补
  content-type/content-encoding 字段。新依赖 brotli + ruzstd（均纯 Rust）。
  端到端复现验证 + `cargo test --locked` 220 项全绿 + clippy 零警告。
  详见 `.agents/memory/2026-09-14-compressed-body-inspection.md`
- [ ] **测试补强：mock 上游要覆盖压缩**：既有集成测试的 mock 从不发
  `content-encoding`，这正是 220 项全绿却漏掉上述 bug 的直接原因。补一组
  「压缩响应体」集成测试（gzip/br 各一，覆盖判定与预览两条路径）
- [ ] **磁盘模式的压缩体判定（已知边界）**：响应 > 1 MiB 时判定仍走原始字节的
  增量扫描，压缩体不解码（仅预览解头部快照）。现实中 >1 MiB 的压缩错误体不存在，
  故不做流式解码；若将来出现真实场景再补

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
- [x] **G2 看门狗**（W1-W7，2026-09-08 完成）：全局单看护进程 + 内核等待 + 选举规范 + settings 五字段 + IPC v2 观测；实测 +2.4% 体积/+2.9MB 常驻/热路径零损耗（docs/benchmark-watchdog.md）；计划偏差 3 项记录；107 lib + 36 集成全绿
- [x] **F. IPC 观测扩展**（随 G2 落地）：proto 版本化 + InstanceInfo 观测字段（请求数/重试数/最近错误）+ status 混版本检测——滚动升级地基齐备

## alpha.6 之后（2026-09-09，全部完成）

- [x] **第四轮审查修复**（7bef574/4ba5b5c/6492740/4c89ab9/e6f676b，详见
  review-history.md 第四轮修复节）：H1 重试队列 + M2 退避移出主循环 +
  H2 unix 字段 + M1 重试中不算闲置 + L3/L4/L1/M3 + 文档/skill 全批；
  配套修复用户实测「改端口 restart 误报未就绪」（就绪判定按新 pid 定位，
  restart 与看门狗 respawn 同款）与 skill「stop 后再 start」误导源三处
- [x] restart 换端口回归集成测试（tests/restart_integration.rs 2 例）

## alpha.6 之后（2026-09-09~10，unix 实测轮）

- [x] **unix 分支实测**（ssh remote Ubuntu 24.04 实机，报告 .agents/docs/unix-testing.md）：
  修复 5 项——P0 编译/链接阻断 2 项（watchdog Duration 导入、daemon extern 符号名
  libc_kill→kill）、P1 看门狗 unix 失效 2 项（adopt_scan 收养不进 + zombie 误判/
  terminate 空操作）、P2 测试平台假设 3 处；解锁 5 个 Windows-only 测试并在 Linux
  通过；功能/并发/内存/perf/valgrind/体积全套数据入报告。
  分支 fix/unix-first-test（.worktree/unix-fixes），待合并。
- [ ] **产品语义决策**（实测发现）：unix 上 APROXY_RUN_DIR 影响 IPC 寻址
  （UDS 路径在 run_dir 内，Windows 管道全局名不受影响）——是否对齐待定
- [ ] **优化候选**：upstream client（reqwest/hyper）开启 TCP_NODELAY——实测与
  无 NODELAY 上游配合时有 40ms Nagle×delayed-ACK 咬合
- [ ] **测试基建**：看门狗测试子进程清理 RAII 化（panic 路径手写 kill 会跳过）；
  daemon.rs 的 UDS IPC roundtrip 单测（现为 Windows-only）
- [ ] **测试守护泄漏治理**（2026-09-14 现场实测）：机器上累积过 46 个 aproxy 进程，
  绝大多数是历史测试遗留的守护（最老 70 小时），其中 `start alias-test-<端口>
  --daemon-watchdog` 这个**看护进程**锁住 `target/debug/aproxy.exe`，导致后续
  `cargo test` 无法重新链接（`failed to remove file … 拒绝访问`），并让
  `restart_integration` 并行跑失败 215 秒（**单跑 4.5 秒通过**，与既有
  `alias_start_and_stop_roundtrip` 同族的端口/时序竞争）。清理须遵铁则：
  **绝不按名批量杀**，只走测试守护的 `APROXY_HOME` + `stop <端口>`（看护进程
  无端口，等其 `watchdog_idle_exit_secs` 空闲自灭）。根因是测试收尾未覆盖
  panic/超时路径，与上一条 RAII 化同源

## 待修（2026-09-10 压力实测审查轮）——已全部修复（a389d40/01978b4/f13bc0a/9cc53c8，详录 .agents/docs/unix-stress-review.md 修复记录节）

- [x] **S1（产品级，两平台同病）：respawn 就绪判定竞态**——respawn A 的就绪等待
  （每 200ms 调 list_instances_in）顺带删掉同注册表里 B/C 死实例的 .pid → 死亡事件
  被误判优雅退出 → 永不 respawn。修复：respawn_instance 改 registry_contains_pid_in
  只读检索 + handle_death 以 .restore 为唯一优雅退出依据 + server.rs 退出清理先
  .restore 后 .pid。复跑 p6_storm：3/5 → PASS=5 FAIL=0（3 实例全部 respawn）。
- [x] **S2：refresh_claim 无条件覆写无运行期夺权检测**——被接管的前任恢复后与
  接管者互相翻转 claim、双看护者共存。修复：refresh_claim 覆写前校验 pid 归属，
  易主即让位退出；unix terminate_verified 实装 SIGKILL。复跑 p7_takeover：
  PASS=5 FAIL=0，claim 单一 pid，前任被真杀。
- [x] **S3：is_aproxy_process unix 占位恒 true**——/proc/<pid>/exe 比对 basename
  实装（ENOENT 判死对齐 Windows 拒收；权限失败 fail-open；「 (deleted)」后缀兼容
  swap 升级）。选举与收养链随判定生效自动修复。
- [x] **S4：health_scan「无 PID 复用风险」注释 unix 不成立**——注释按平台改写 +
  处决前加 is_aproxy_process 防误杀关卡（ starttime 字段方案未采纳：S3 的 exe
  比对已封住复用误杀面，字段增加无增量收益）。
- [x] **S5：/dev/shm 心跳文件与 UDS socket 无退出清理**——新增 remove_heartbeat_file
  与 daemon::remove_socket_file（Windows 均 no-op），守护优雅退出 + 看护者
  handle_death 两处调用。
- [x] **S6：macOS 回退分支对 zombie 失效**——注释补声明，health_scan 兜底
  （macOS 未实测平台，接受退化）。

## 修复审查轮遗留（2026-09-10，独立复验后记录，详录 .agents/docs/unix-stress-review.md 复验节）

- [x] **R1（测试覆盖）→ 已补（75afe6b）**：registry_contains_pid_in 四分支 +
  is_aproxy_process unix 版真实 zombie/ENOENT 判死单测 + 正名成功路径集成断言 +
  swap 覆盖替换 (deleted) 语义集成测试（unix）。实测纠正：rename 走开（mv）不产生
  「 (deleted)」后缀（exe 跟随新路径名）；fail-open 分支无法确定性构造，留人工路径。
- [x] **R2（探针维护）→ 已修（75afe6b）**：p7_takeover.sh 文案与断言对齐修复后
  语义（接管即杀前任 SIGCONT 不可恢复 / 接管者在任 / claim 单一 pid 三条硬断言），
  复跑 PASS=7 FAIL=0
- [ ] **F2（产品决策）：is_aproxy_process 二进制名精确耦合**——改名运行的二进制
  （如生产 aproxy-using.exe 部署形态）被判非 aProxy 进程：不被收养（失去看门狗
  自动恢复，remote 实证：改名副本运行时看护者收养日志 0 条）、不计入选举、
  处决被关卡拒绝（防误杀方向正确）、claim 接管不杀前任。
  Windows 侧 merge-base 前已有语义，unix 实装后两端一致。待决策：接受「改名
  二进制不受看护」为约定，或放宽比对（会同时削弱防冒名闸门）

## 中期功能（对齐「无限重试、不中断」使命）

- [ ] **正式发布：GitHub 构建指令集多版本**（必然项，2026-09-07 定调）：CI 矩阵 baseline + `RUSTFLAGS="-C target-cpu=x86-64-v3"`（AVX2），产物命名区分，发布页两者都放；详见 memory/release-engineering
- [ ] **H.（可选）配置热重载**：IPC reload，避免 restart 断流（改配置生效目前用 restart，已有单命令路径）
- [x] **install/upgrade 二进制安装升级**（2026-09-11 完成，alpha.10）：完整计划
  `.agents/plan/install-v1.md` 十步全部落地——APROXY_HOME 重定向/状态机/
  staging/交换原语（Windows 双 rename 舞 + PATHEXT fallback）/PrepareSwap
  广播 + 安装态宣告节 + 看护者差异化/滚动重启/恢复续作（崩溃注入测试
  全矩阵）/下载链条（github+sha256/npm+integrity/binstall/cargo + url 模板
  + 下载代理分离）/skill 支线（并行 + 原子落位 + 路径穿越防护）。
  CLI：`install [latest|版本]`、`--from/--adopt/--skills-only/--abort/
  --no-skills/--variant/--allow-downgrade/--download-proxy`。
  实测挖出并修复的深层 bug：守护优雅退出删 .restore（restart 须先取参数）、
  IPC 消失≠进程终止（新增 process_exited 退出码判死）、10s 宽限强退竞速、
  tokio runtime drop 等无限任务须硬退、fallback bat 空窗验证通过。
  坑与教训：.agents/memory/2026-09-11-install-pitfalls.md
- [x] **install 三平台深审查实测**（2026-09-11，090ea90 前后提交链）：Windows/
  Ubuntu(ssh remote)/WSL(Debian) 三平台全量测试 + CLI 级 e2e 全绿（Windows
  24/24、两 Linux 各 26/26）。实测挖出并修复 5 个真 bug：unix 编译面×2
  （announce E0596、is_x86_feature_detected aarch64）、平台硬编码测试×2、
  库层 IPC 环境依赖泄漏（run_dir 参数被全局 env 派生架空 + 误删注册表副作用）、
  有实例 Acked/Swapping 续作被状态机拒绝（相位守卫）。CI unix job 同步升级
  为 cargo test。CI 首次暴露流程教训：push 后必须 gh run list 确认绿
  （且要按 workflow 名过滤——同一次 push 会同时触发 CI 与 Review）。
  详录 .agents/memory/2026-09-11-install-three-platform-e2e.md
  **遗留**：CI 升级后 macos job 一直红（见下节 macOS 支持），当时误报为绿
- [x] **install 在线渠道深度实测**（2026-09-12，WSL 3 轮 × 16 场景全 PASS）：
  测试 tag（v…tN，用户授权）触发全链发布做真实下载测试。挖出并修复 5 个
  产品 bug + 2 个韧性改进（gnu→musl 自动回退、latest 查询 npm 兜底）+
  release 版本对齐机制（build/publish 双 job）。代价：crates.io 的
  alpha.10/alpha.11 正式号被测试发布抢注（对齐缺失时代），正式版顺延
  alpha.12（Cargo.toml 已 bump，待发布）。详录
  .agents/memory/2026-09-12-install-online-deep-test.md 与
  .agents/memory/2026-09-12-release-version-alignment.md
- [x] **正式发布 v0.1.0-alpha.12**（2026-09-14，用户下令）：annotated tag
  `v0.1.0-alpha.12` 打于 `50b94f2`，触发 Release workflow（test 门禁 → 11 变体
  构建 → GitHub/npm/crates.io 发布）。发版时 CI 状态：ubuntu job 绿、macOS 红
  （既有未支持平台）、windows 红于一条**既有竞态偶发**（见下条）。三平台验证：
  本地 Windows 241 项全绿、远端 Ubuntu 全绿；WSL 因无网络拉不下新依赖
  （brotli/ruzstd）未能跑，其环境事实见
  `.agents/docs/environment.md` / 本文件「WSL 无 git/curl/python3 且有 TLS 故障」
- [ ] **测试稳定性**：proxy_integration 的 alias_start_and_stop_roundtrip
  偶发并行失败（单跑必过——端口/时序竞争，本轮全量跑撞上一次）
- [ ] **测试稳定性（新，有本机复现）**：`install_flow_lib` 的
  `continue_from_swapping_with_live_instance_redoes_swap` 是**既有竞态偶发**——
  2026-09-14 本机隔离重复 5 次**复现 1 次失败**（第 2 次耗时 90.33 秒，撞测试
  内部 90 秒超时；CI 那次是同一测试的 `done 后状态文件应删除` 断言，症状不同但
  同一处）。与当轮改动无文件交集（该文件未被触碰），ubuntu CI 恒绿、远端 Ubuntu
  全绿。**注意它会卡住 Release 的 test 门禁（跑在 windows-latest）**，
  值得专项排查：疑似「活实例滚动重启 + 状态文件删除」这条路径在 Windows 上有
  时序/句柄竞争（Windows 上删除被打开的文件会失败）。
- [x] **渠道 P0 全配 + 可信发布自动化**（2026-09-11，alpha.7~9 三轮发布实测）：
  npm（@meowo/aproxy，esbuild 式多平台包：主包 JS 转发器 + 9 平台子包
  os/cpu/libc 装配）、crates.io（aproxy）、cargo-binstall（零配置命中内置
  模板）、GitHub（11 变体：win x64±v3/x86/arm64 + linux x64/arm64 gnu+musl
  + macOS arm64/x64）；tag 推送全自动发布（release.yml publish job，
  npm OIDC provenance SLSA v1 实证 + crates.io trusted publishing #19617）；
  种子发布走本地 token，token 退役（NPM_TOKEN secret 已删）。
  坑与边界：npm GAT 有 IP 允许列表（GitHub runner 必 404，token 路径在 CI
  不可行）；npm trust 网页绑定强制人类 2FA（AI 不可代办，crates.io 有
  API 可代办——POST /api/v1/trusted_publishing/github_configs）；
  npm 11 prerelease 强制 --tag latest（0.1.x 全程 alpha，latest=最新 alpha）
- [ ] **看门狗二期（可选）**：挂死不杀进程原地救（scoped runtime 注入 spike）、实例数极大时线程池死亡等待（unix 死亡等待已实装为轮询，见 unix-testing.md）
- [x] **CI 激活与首版发布**（2026-09-10）：仓库开源（MoYeRanqianzhi/aProxy）+
  workflows 首跑全绿（CI/Review）；首跑抓到并修复 windows-sys 解析漂移
  （^0.52 被 CI 解析为 0.61.2，HANDLE 类型不兼容）——精确钉死 =0.52.0 +
  全工作流 --locked；v0.1.0-alpha.7 发布全链实测通过（7 变体 + skill 双包 +
  SHA256 表 + 更新链文案 + prerelease 标记）；三平台引导脚本实测通过
  （scripts/install.ps1|sh|cmd，APROXY_HOME 隔离测试，均实测从 GitHub 拉取
  二进制与 skill 并通过校验/落位/--version/幂等全断言）；dependabot 运行中
  （windows-sys 升级建议已忽略并关 PR）

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
