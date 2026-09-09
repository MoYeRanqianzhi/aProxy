# unix 分支压力实测审查报告（2026-09-10 第二轮）

> 审查对象：`fix/unix-first-test` 分支（HEAD 0b53a10）相对 merge-base 7ab84aa 的全部改动。
> 审查方式：逐行 diff 审查 + Windows/unix 语义对照 + ssh remote（Ubuntu 24.04，rustc 1.98.1）
> 实机全量测试与 10 项压力场景实测。压力探针资产：`.agents/bench/probe/`（可复跑）。
> 本文档是问题归档：**每项问题的根因、位置、实测证据、修复方向**，供修复窗口直接使用。
> 结论：分支可合并（Windows 零行为变化确认、unix 实装质量扎实），下述问题按严重度待修。

## 修复记录（2026-09-10，S1-S6 全部关闭）

六项问题经代码确认后全部修复，实测复跑验证：

| 项 | 提交 | 修复 | 复跑实证 |
|---|---|---|---|
| S1 | a389d40 | respawn 就绪判定改 `registry_contains_pid_in` 只读检索；`handle_death` 以 `.restore` 为优雅退出唯一判据；server.rs 退出清理先 `.restore` 后 `.pid` | p6_storm：修复前 3/5 → **PASS=5 FAIL=0**，3 实例全部 respawn（1s） |
| S2 | 01978b4 | `refresh_claim` 覆写前校验归属，易主即让位退出；unix `terminate_verified` 实装 SIGKILL（身份由调用方 exe+starttime 双重把关） | p7_takeover：修复前 claim 两 pid 翻转/双存活 → **PASS=5 FAIL=0**，接管即杀前任，claim 单一 pid，A=0 B=1 |
| S3 | f13bc0a | unix `is_aproxy_process` 实装：`/proc/<pid>/exe` 比对 basename（剥「 (deleted)」后缀）；ENOENT（含 zombie，remote 实证 errno 2）判死；权限类失败 fail-open | 集成测试全绿；选举/收养链随判定生效自动修复 |
| S4 | f13bc0a | health_scan 注释按平台改写 + 处决前加 `is_aproxy_process` 防误杀关卡（unix 裸 SIGKILL 的闸，Windows 侧冗余第二道） | 挂死处决探针（审查时 9/9）路径不变 |
| S5 | 9cc53c8 | 新增 `remove_heartbeat_file`（unix 删 /dev/shm，Windows no-op）与 `daemon::remove_socket_file`，守护优雅退出 + 看护者 `handle_death` 两处调用 | 全量测试后 /dev/shm 零 aproxy-heart 残留 |
| S6 | 9cc53c8 | `process_alive_for_wait` 注释声明 macOS 回退对 zombie 失效、由 health_scan 兜底 | 文档性修复 |

回归验证：Windows fmt/clippy -D warnings/lib 113/集成 36+2 全绿；remote lib 111/
集成 36+2 全绿。探针注意事项：p7 输出中「unix no-op 未杀」是探针按旧行为写的
文案，修复后实际已真杀（SIGCONT 时 A 已不存在即证据），探针文案未随行为更新。

## 修复审查轮（2026-09-10，独立复验记录）

修复窗口交付后由审查窗口独立复验（非复用修复方自报数据）：

- **代码审查**：4 个提交与 S1-S6 修复方向逐项对应；`handle_death` 判据与
  server.rs 清理顺序配套自洽（两 unlink 间隙判 GracefulExit，不误 respawn
  刚停的实例）；`refresh_claim` 读失败 fail-open、易主退出时勿动 claim 文件
  （动了会被新任按「心跳停滞」再夺权）的语义正确。
- **独立复验**：Windows fmt/clippy/lib 113/集成 36+2 全绿；remote 全绿
  （lib 111）。核心探针复验——P1 挂死处决 **9/9**（S4 关卡不挡真实处决）、
  P6 死亡风暴修复前 3/5 → **5/5**（S1）、P7 修复前 claim 双 pid 翻转 →
  **5/5，前任被真杀（A=0 B=1）、claim 单一 pid**（S2）。

### 复验遗留小项（不阻断合并，供后续窗口）——R1/R2 已关闭（75afe6b）

- **R1（测试覆盖）→ 已补（75afe6b）**：`registry_contains_pid_in` 四分支
  （pid 匹配 / 非 .pid 忽略 / 损坏记录跳过 / 目录缺失）+ `is_aproxy_process`
  unix 版真实 zombie 判死与 pid_max 外 ENOENT 判死（cfg(unix) 单测）+
  daemon_lifecycle 正名守护成功路径断言 + 新集成
  `identity_check_tolerates_swapped_binary`（unix，见下）。实机全绿：
  Windows lib 115/集成 36+2；remote lib 114/集成 37+2。
  **实测纠正一个假设**：rename 走开（mv）运行中二进制不产生「 (deleted)」
  后缀——exe 链接跟随新路径名（`aproxy.swapped`），basename 变化判异己；
  「 (deleted)」仅出现在新文件 rename 原子覆盖原路径（swap 升级真实形态）
  的场景。fail-open 分支（跨用户权限）无法确定性构造，留探针/人工路径。
- **R2（探针资产维护）→ 已修（75afe6b）**：p7_takeover.sh 头注/文案/尾部
  断言全部对齐修复后语义，「接管即杀前任（SIGCONT 不可恢复）/接管者在任/
  claim 单一 pid」三条硬断言，任一不满足即 FAIL。复跑 **PASS=7 FAIL=0**。
- **F2（产品决策，保持待决）**：事实已 remote 实证——副本改名
  `aproxy-renamed` 运行时看护者收养日志 0 条（正名对照组有收养），改名
  实例确实失去看门狗自动恢复。Windows 侧同语义 merge-base 前已有。
  决策选项与权衡见下方 F2 条目。

### 新发现：S3 实装的两个行为面（需产品决策/知悉）

- **F1（行为收紧，已知方向）**：unix 收养对齐 Windows 拒收死条目——看护者
  启动时注册表里的陈旧死条目（进程已死、.restore+.pid 都在）不再被收养
  respawn，`aproxy restore` 手工路径保留。看护者运行中实例崩溃 → respawn
  的核心语义不变（watched 表内，`.restore` 判据）。两端一致，防 PID 复用
  冒名收养的合理权衡。
- **F2（二进制名耦合，两端一致，生产影响面待确认）**：`is_aproxy_process`
  两平台都按镜像名精确比对（Windows `aproxy.exe` / unix basename `aproxy`）。
  **改名运行的二进制会被判为非 aProxy 进程**——已知生产实例曾以
  `aproxy-using.exe`（Windows，绕开 target/release 文件锁的部署方式）运行，
  该形态下：不被收养（失去看门狗自动恢复）、不计入选举 min_alive、
  health_scan 处决被关卡拒绝（防误杀方向正确）、claim 接管时前任判定为
  「非 aProxy」不杀直接接管。Windows 侧此语义 merge-base 前已有（非新引入）；
  unix 实装后两端行为一致。**待决策**：接受「改名二进制不受看护」为约定，
  或放宽比对（如前缀匹配 / exe 路径白名单）——放宽会同时削弱防冒名闸门，
  需要权衡。

## 审查与实测结论摘要

- 5 项 unix 修复（2×P0 编译/链接 + 2×P1 看门狗 + 测试平台假设）全部落实且无超范围改动；
  `ipc_request` → `ipc_request_to` 为纯委托重构，Windows 路径逐字不变。
- 服务器全量 `cargo test` 全绿（单元 + proxy_integration 36 + restart_integration 2）。
- 压力实测 10 场景：P1 挂死处决 9/9、P2/P3 give_up 3/3、P6 死亡风暴 3/5（**发现竞态缺陷**）、
  S 生命周期 5/5、R1 重试风暴 50/50 恢复、R2 长挂重试 3/3、R3 拒绝连接重试 10/10、
  R4 隔离性 11/11（p50=17ms）、M1 浸泡 3/3（RSS 3 分钟趋稳 5.9→7.3→7.4MB、fd 回落）、
  P7 claim 接管 6/6。
- 压力探针复跑方式见文末「复现指引」。

---

## 问题清单（按严重度，全部待修）

### S1（产品级缺陷）：respawn 就绪判定竞态——多实例同时崩溃时部分实例丢失自动恢复

**严重度：高**。触发条件：多实例同时崩溃（OOM killer / 断电恢复 / 系统重启后的看护者自愈）。
违背「无限重试、不中断」使命。**两平台同病（共享逻辑，无 cfg 差异），非本分支引入。**

**根因链（P6 死亡风暴场景实机复现）**：

1. 3 个实例同时被 SIGKILL，注册表（`<run_dir>/<port>.pid`，内容为 InstanceInfo JSON）
   留下 3 条死记录，3 个死亡 watcher 各自报死。
2. tick 消化死亡事件是串行的：先处理 A → `respawn_instance`（watchdog.rs:585-624）
   的就绪等待循环每 200ms 调用一次 `crate::daemon::list_instances_in(run_dir)`（watchdog.rs:612）。
3. `list_instances_in`（daemon.rs:529-561）对注册表**全部**条目逐个 `ipc_ping` 验活，
   **ping 失败即 `remove_file` 删除注册文件**（daemon.rs:555）——A 等待期间，
   B/C 的死实例注册文件被顺带清理。
4. A 就绪后处理 B 的死亡事件：`handle_death`（watchdog.rs:359-372）的优雅退出判定是
   `!restore_path.is_file() || !registry_path.is_file()`——B 的 `.restore` 还在，
   但 `.pid` 注册已被第 3 步删掉 → **混合态被误判为「优雅退出（无恢复记录）」，摘除看护**。
   C 同理。
5. B/C 从此不被 respawn：`.restore` 还在但 `adopt_scan` 收养依赖
   `read_registry_info`（watchdog.rs:569-573，读 `.pid` 文件）→ None → 永不收养。
   实例静默失去自动恢复，只能手工 `aproxy restore`。

**实测证据**（2026-09-10 Ubuntu 实机，p6_storm.sh，看护者日志）：

```
17:52:40.36 WARN  实例崩溃，立即重拉 port=59985
17:52:41.17 INFO  实例已重拉并就绪 port=59985 new_pid=2369281
17:52:41.17 INFO  实例已优雅退出（无恢复记录），摘除看护 port=59984   ← 误判（.restore 在、.pid 被顺带删）
17:52:41.17 INFO  实例已优雅退出（无恢复记录），摘除看护 port=59986   ← 同上
```

日志文案「无恢复记录」有误导性——实际是「无注册记录」（.restore 在）；GracefulExit
分支不检查 .restore 与 .pid 的组合语义。

**修复方向（两处配合）**：

- `respawn_instance` 的就绪判定改**只读检索**：用 `read_registry_info`（已有函数，watchdog.rs:569）
  或新增 `list_instances_in` 的无清理变体，按「新 pid 是否出现在注册表」判定就绪，
  不复用带清理副作用的 `list_instances_in`。
- `handle_death` 的优雅退出判定以 `.restore` 为唯一依据（daemon.rs:565-567 已声明
  「.restore 承载期望在运行」语义）：`!restore.is_file()` → GracefulExit；
  `.restore` 在而 `.pid` 缺 → 仍走崩溃 respawn 路径（respawn_instance 只读 .restore 的 args，
  不依赖注册表；新守护 bind 后自写注册表）。

**复现**：`.agents/bench/probe/p6_storm.sh`（3 实例同 run_dir，同时 SIGKILL，
观测看护者日志与注册表 pid 变化；修好后 3 端口应全部 respawn 为新 pid）。

### S2（并发语义缺陷）：`refresh_claim` 无条件覆写 claim 文件，无运行期夺权检测

**严重度：中**。两平台一致（共享主逻辑）。P7 场景实机实证。

**现状**：

- `serve` 入口的 claim 接管有完整验证链（watchdog.rs:628-662）：acquire 失败 →
  有效性检查（claim 心跳过期阈值 = 3×`watchdog_heartbeat_secs` = 90s）→ 无效则
  验证前任身份（`is_aproxy_process` + `verify_claim_identity` starttime 比对）→
  `terminate_verified` 杀假死前任 → remove_claim → 原子接管。**入口语义正确**。
- 但运行期的 `refresh_claim`（watchdog.rs:501-513）**每 tick 无条件覆写 claim 文件**，
  不检查当前 claim 是否仍属于自己。后果：
  - 被接管的前任（恢复后）继续覆写 claim，与接管者互相翻转 claim 内容；
  - 任意第三个进程从外部写入 claim 也能加入续写行列。

**实测证据**（p7_takeover.sh，PASS 6/6 同时记录了缺陷行为）：

```
B2 接管后 SIGCONT 恢复 A：
  10s 内 claim pid 在 2429193 ↔ 2429078 间翻转（双写竞争）
  存活: A=1 B=1（双看护者稳定共存，无夺权检测）
B2 日志：前任看护者仍在但心跳过期（假死），终止后接管 pid=2429078 ← 尝试终止，
  但 unix terminate_verified 是 no-op → A 未被杀
```

**修复方向**：

- `refresh_claim` 先读 claim 校验 pid 是否为自己，被夺权则安静退出（或走接管者
  同款收尾）；这样双写竞争转为确定性的让位。
- 配套：unix 实装 `terminate_verified`（watchdog.rs:886 no-op → kill(pid, 9)，
  身份已由 `verify_claim_identity` 的 /proc starttime 比对把关）——杀掉假死前任，
  从源头消除双看护者窗口。

**复现**：`.agents/bench/probe/p7_takeover.sh`（约 2.5 分钟：A SIGSTOP → B1 让位 →
等 90s claim 过期 → B2 接管 → SIGCONT A → 观察 claim 翻转与双存活）。

### S3（遗留占位）：`is_aproxy_process` unix 版恒返回 true

**严重度：中**（2026-09-08 引入，merge-base 之前已有，watchdog.rs:1096-1100，TODO 注释自述）。

影响面两个：

- **收养防冒名失效（D1）**：`adopt_scan`（watchdog.rs:300）对 unix 不验证进程身份 →
  unix 会收养「看护者启动前已死」的注册表条目并立即 respawn
  （`consecutive_failures` 从 0 起，watchdog.rs:311——不追问死了多久）；Windows 上
  `OpenProcess` 失败 → 拒收死条目，`.restore` 留给手工 restore。**多端行为差异**：
  unix 更积极（方向符合使命），但缺少身份验证闸门——PID 复用到无关进程时
  会进入 respawn → spawn 失败 → 退避 → give_up 的无谓循环。
- **选举可被死条目卡住（D2）**：`this_process_may_spawn_watchdog_in`（watchdog.rs:140-144）
  把注册表所有 pid（含死条目）计入 `min_alive`——注册表残留死条目且新守护 pid 较大时
  被剥夺发起权 → 无人拉看护者。Windows 真验证不受影响。

**修复方向**：unix 版读 `/proc/<pid>/exe`（readlink 后比对 basename 为 `aproxy`，
注意 Linux 下二进制无 .exe 后缀；权限不足时 readlink 失败保守放行——与现注释的
fail-open 语义一致）。选举链随之自动修复。

### S4（注释与实现不符）：health_scan 的「无 PID 复用风险」声明在 unix 不成立

**严重度：低**。`health_scan` 文档注释（watchdog.rs:329-330）写「杀（本方句柄绑定
原进程，无 PID 复用风险）」——Windows 的 `TerminateProcess(handle)` 成立；unix 的
`terminate_handle` 是裸 `kill(pid, 9)`（watchdog.rs:880-884），理论窗口 ≤1 个扫描周期 +
pid 回绕（概率极低，且 Watched 条目在死亡事件处理时已摘除，窗口内进程多为 zombie，
SIGKILL 打在 zombie 上无害）。**修复方向**：注释按平台区分；可选加固：unix 版
terminate_handle 前比对 `/proc/<pid>/stat` 的 starttime 与收养时记录值
（`Watched` 无 starttime 字段，需随 S3 一起加）。

### S5（资源生命周期差异）：/dev/shm 心跳文件与 UDS socket 文件无退出清理

**严重度：低**。多端差异：Windows 侧心跳为 `Local\` 节对象、管道由内核回收；
unix 侧 `/dev/shm/aproxy-heart-<port>` 文件（watchdog.rs:967-1007）与
`<run_dir>/<port>.sock`（daemon.rs:901-928）在守护死亡/退出后残留——
socket 靠下次 bind 前 `remove_file` 自愈（daemon.rs:909），心跳文件**无人删除**，
实测一轮 cargo test 留 13 个残留（8 字节 + 文件元数据，/dev/shm tmpfs 内存计费，
机器重启清空）。

**修复方向**：unix 守护退出路径删除心跳文件与 socket 文件；或看护者
`handle_death` 摘除时顺带清理对应端口的心跳文件。写侧 truncate→write 之间的
空窗口读侧已有 fail-safe（`data.len() < 8 → None`，按无心跳处理），无需加锁。

### S6（平台退化未声明）：macOS 回退分支对 zombie 失效

**严重度：低**（macOS 未实测平台）。`process_alive_for_wait`（watchdog.rs:862-876）
在无 /proc 平台回退 `kill(pid, 0)`——zombie（已死未收割）对该调用返回成功 →
watcher 永不触发死亡事件，只能靠健康检查兜底。注释声明了回退策略但未声明
此缺陷。**修复方向**：注释补「macOS 回退对 zombie 失效，靠 health_scan 兜底」；
若未来 macOS 实测轮有需求，可改用 `waitid(WNOHANG)` 或进程状态查询。

---

## 附：实测基线数据（修复回归对照用）

- 看门狗：挂死处决全链路 11s（SIGSTOP → 判定 → SIGKILL → respawn → 重新收养）；
  give_up 全程 58s（5 次重拉 + 立即/1/2/4/8s 退避）；respawn 失败每次留 1 个 zombie
  （看护者不收割子进程，退出后由 init 收割——固有行为，max_restarts=5 封顶）。
- 重试：50 并发 × 8s 502 窗口 → 50/50 恢复（wall 15.3s）；conn refused 8s → 10/10
  恢复；上游恒 502 下 90s 长挂不断流（守护 RSS 6MB）；重试风暴期健康实例
  p50=17ms / max=25ms 零失败。
- 资源：3 分钟混合浸泡（含周期性 5s 502 窗口）RSS 5908→7448KB 趋稳
  （60s 时已近峰值），fd 13→21→16 回落，全程存活。
- 生命周期：10 轮 start/stop 18s 无 pid 注册残留；并发 3×stop 幂等全停；
  并发 3×start 唯一存活（注册表 + 进程双验证）；100 并发 IPC status 后守护存活。
- 已知观察项（非缺陷）：`aproxy start` 拉起的守护自建看护者，argv 形如
  `aproxy start --config X --daemon-watchdog`（watchdog_spawn_args 原样保留子命令，
  分流正确）；守护 stop 后该看护者继续持有 claim 直到 idle_exit（默认 300s）自灭——
  期间 claim 唯一性由它维护，无危害，但「stop 后残留常驻进程最长 5 分钟」值得知悉。

## 复现指引

全部探针在 `.agents/bench/probe/`，服务器（ssh remote）用法：

```bash
# 打包上传（Windows 侧）
tar czf /tmp/probe.tar.gz -C .agents/bench/probe . && scp /tmp/probe.tar.gz remote:/tmp/
ssh remote 'mkdir -p /root/probe && tar xzf /tmp/probe.tar.gz -C /root/probe \
  && chmod +x /root/probe/*.sh'
# 服务器上需要 /root/probe/aproxy（release 二进制）与代码目录 /root/aproxy-unixfix
# 场景全部使用隔离 run_dir=/root/probe/run 与端口段 59980-59999，不触碰生产实例
bash /root/probe/p6_storm.sh     # S1 竞态复现
bash /root/probe/p7_takeover.sh  # S2 复现（约 2.5 分钟）
```

注意（探针编写教训，新场景可借鉴）：
- 注册表 `<port>.pid` 与 claim 文件内容均为 JSON：前者 pretty 多行、后者紧凑单行——
  提取 pid 字段须用 `grep -oE '"pid": ?[0-9]+'`，`grep -o '[0-9]*'` 对单行 JSON 会
  返回多个数字段。
- bash 编排里 `wait` 会等待常驻 job（守护/mock 进程）——spawn 后须 `disown`，
  否则脚本无限阻塞（S4/M1 曾因此卡死）。
- mock 脚本 Content-Length 必须程序化生成（见 unix-testing.md 第六节 mock 教训）。
