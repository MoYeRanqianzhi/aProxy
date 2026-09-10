# Unix 分支首次实机测试报告

> 2026-09-09，Ubuntu 24.04.4 LTS（kernel 6.8.0-124，x86_64，10 核 / 7.8GB RAM / 108G 盘），
> ssh remote（RackNerd VPS）。rustc 1.98.1 stable。
> 测试对象：master @ 7ab84aa（v0.1.0-alpha.6）。修复分支 `fix/unix-first-test`
>（worktree `.worktree/unix-fixes`，4 个提交：9bb8d2e / 35003b3 / bf3be2e / +本文档）。
> 测试执行：专职测试窗口；只测与修本报告所列问题，不含任何其他改动。

## 一、结论摘要

unix 分支（UDS IPC / unix spawn / /dev/shm 心跳 / kill 信号）在此前**从未实机运行过**
（CI ubuntu/macos 仅 cargo check）。本次实测：

- **产品核心功能全部可用**：代理转发、无限重试（含 SSE keepalive 通道）、流式回放、
  磁盘缓存 spool、守护生命周期、restart、alias、status/stop/logs、restore、doctor、
  看门狗（修复后）。
- **修复 5 个 unix 专属缺陷**（2 个编译/链接阻断 + 2 个看门狗功能性缺陷 + 1 个测试
  平台假设），详见第二节。
- **性能、内存、体积数据齐全**，与 Windows 已知数据量级一致，无回归迹象。第五节。
- 测试基础设施问题（mock 脚本 Content-Length 错标）一度误导排查方向，
  已记录于第六节教训。

## 二、发现并修复的缺陷（按严重度）

### P0-1 编译阻断：unix 分支缺 `Duration` 导入
- `src/watchdog.rs` `#[cfg(unix)] mod imp::wait_blocking` 使用 `Duration::from_secs`
  但模块未导入 `std::time::Duration`。Windows 下 cfg 屏蔽不可见。
- **教训**：TODO 里「unix 分支实测」防线缺失的第一次兑现——仓库无 remote，
  CI ubuntu/macos cargo check 从未运行（见 TODO 既有条目）。

### P0-2 链接阻断：extern "C" 符号名写错
- `src/daemon.rs` unix `terminate_process` 经 `unsafe extern "C" { fn libc_kill(...) }`
  调用——libc 无 `libc_kill` 符号（作者把 `libc` crate 的 `libc::kill` 命名习惯带进
  裸 extern 声明），Linux 链接即失败：`undefined symbol: libc_kill`。
- 修复：符号改为 `kill`，附注释说明 unix 分支不引 libc crate 的约束。

### P1-1 看门狗死亡检测从不工作（unix）
- `open_sync_handle` unix 版恒返 `None`（占位）→ `adopt_scan` 里
  `let Some(handle) = ... else { continue }` → **任何实例都收养不进** →
  health_scan 只扫 `watched` 表（空转）、死亡 watcher 从未挂上 →
  实例死亡后看门狗零响应、respawn 永不触发。
- **这是 G2 看门狗在 unix 上的整体失效**，非边角。
- 修复：按注释声明的设计意图「死亡检测退化为轮询」落地——句柄值存 pid
  （0 会被 adopt_scan 视作句柄缺失反复补挂），死亡等待轮询进程态。

### P1-2 死亡等待误判 zombie、挂死处决空操作（unix）
- `wait_blocking` 若用 `kill(pid,0)` 探活：对 zombie（已死未收割）返回成功——
  守护死后成 zombie 期间死亡事件永不触发（父进程不 waitpid 场景常见：
  start 命令已退出后由 init 收割前的窗口、测试进程持有 Child 等）。
- `terminate_handle` 原为空操作：health_scan 判挂死后实例杀不掉，
  死亡事件也无从产生。
- 修复：wait_blocking 读 `/proc/<pid>/stat` 排除 Z 态（macOS 无 /proc，
  回退 `kill(pid,0)` 且 EPERM 视为存活）；terminate_handle 实装为对句柄内
  pid 的 SIGKILL。

### P2-1 测试平台假设（3 处）
- `settings.rs` 大小写变体去重断言：Linux 大小写敏感，`/ROOT` 与 `/root`
  本就不该去重（`path_match_key` unix 分支不做 lowercase 是正确平台语义）。
  拆为 `#[cfg(windows)]` 独立测试 + 跨平台保留分隔符变体断言。
- `tests/proxy_integration.rs` `process_alive` unix 版 `ps -p` 对 zombie 返回
  成功 → `daemon_second_instance_on_same_port_exits`、`restore_recovers_...`
  两例误报失败。改为 `ps -o stat=` 排除 Z 态。
- restore 测试无条件用 `taskkill`（Windows-only）→ Linux 直接 panic。
  换统一 `kill_pid` 并补 unix SIGKILL 实现。

### P2-2 隔离 `APROXY_RUN_DIR` 场景的 IPC 寻址（测试侧修复 + 产品语义记录）
- `endpoint_for` unix 版 = `{run_dir}/{port}.sock`——IPC 寻址跟随 run_dir；
  Windows 命名管道是全局命名空间不受影响。后果：守护以隔离 `APROXY_RUN_DIR`
  运行时，不带同环境变量的调用方（测试进程、CLI）找不到 socket。
- 测试侧修复：`ipc_request_to`（daemon.rs 新增 pub，按显式端点发请求，
  `ipc_request` 薄封装行为不变）+ 测试 `ipc_ping_in_dir` 显式按隔离 socket
  寻址 + `DaemonGuard`/`RestartGuard` 携带 run_dir。
- **产品语义记录**：unix 上「设置了 APROXY_RUN_DIR 的实例只能被同样设置了
  APROXY_RUN_DIR 的 CLI 管理」。单用户自洽（环境变量传播链一致），但与
  Windows「管道名全局唯一」语义有平台差异。如需对齐（UDS 移到固定路径
  并处理 /tmp 符号链接攻击面），是独立的行为变更决策，不在本批修复。

### 解锁的测试（原 `#[cfg(windows)]`，实为编写时未验 Unix）
- restart 换端口回归 2 例（restart_integration.rs，2026-09-09 刚修的用户实测 bug）
- 看门狗 respawn / 选举唯一性 2 例（proxy_integration.rs）
- /dev/shm 心跳写读 1 例（watchdog.rs lib test）
- 全部在 Linux 实机通过。

## 三、编译与测试验证（修复后）

| 项 | Windows（本地） | Linux（remote） |
|---|---|---|
| cargo check --all-targets | ✅ 0 警告 | ✅ 0 警告 |
| cargo clippy --all-targets -D warnings | ✅ | ✅ |
| cargo fmt --check | ✅ | ✅ |
| lib tests | 113 ✅ | 111 ✅（2 个 Windows 专属） |
| 集成 tests（串行） | 36+2 ✅ | 36+2 ✅ |

- 注：Windows/Linux 集成测试均需 `--test-threads=1` 串行跑守护类用例
  （既有已知问题：并行时全局 status 断言互踩，environment.md 有记录）。
- remote 测试遗留进程清理：守护测试失败路径曾泄漏守护/看护进程（继承
  stdio 导致 ssh 会话挂起）。测试代码用 guard 兜底 stop，但断言失败的
  panic 路径上 `wd_child.kill()` 等手写清理会跳过。建议后续把看门狗测试的
  子进程也纳入 RAII guard（未修，测试基础设施范畴）。

## 四、功能实测清单（全部实机通过）

- 守护生命周期：start(--daemon-child) → status（v2 观测字段：请求/重试/最近
  错误/闲置时长）→ IPC stop；SIGTERM 优雅退出；stop 后注册清理。
- restart 换端口端到端（解锁的回归测试 + 手动复现）。
- alias start/stop（测试遗留孤儿看护进程问题已由 P1-1 修复间接解决——
  修复前看护者失控存活，修复后闲置自灭正常）。
- 无限重试：502 窗口期请求 → 指数退避重试（0/0/0/5s/10s/…，`max_retry_
  backoff_secs` 覆盖生效为 1s）→ 上游恢复后客户端拿到 200。日志链完整。
- SSE keepalive 通道：502 窗口期 SSE 请求立即收到 `: keepalive` 注释流，
  上游恢复后无缝接续 `data: tick` 帧。修复 mock 脚本后验证通过。
- 流式回放：chunked SSE 透传、字节保真（集成测试覆盖）。
- 磁盘缓存 spool：>1MiB 溢写验证——5 并发 32MiB 响应瞬时 spool 峰值
  ~160MB（≈5×32MiB），回放完成后目录归零（`ls | wc -l` = 0），
  无残留泄漏。
- 看门狗：收养 → kill 守护 → 轮询检测死亡 → 立即重拉 → 新 pid 就绪 →
  优雅 stop 后不复活 → 看护者闲置自灭。选举唯一性（claim 抢占）。
- /dev/shm 心跳：写读往返、10s 周期 beat 实测精确（delta=10000ms）、
  freshness 判定。
- doctor / logs / status --idle / stop idle 语义（日志轮转阈值未单独实测，
  Windows 侧已有覆盖）。

## 五、性能 / 内存 / 体积数据

### 5.1 吞吐与延迟（NODELAY mock 上游，127.0.0.1 回环）

| 场景 | 直连上游 | 经 aproxy | 代理开销 |
|---|---|---|---|
| c8 非流式 RPS | 2506 | 2269~2333 | ~8% |
| c8 非流式延迟 | 3.72ms avg | 3.73ms avg | ≈0 |
| c64 RPS | 2514 | 2060 | ~18% |
| c8 流式 SSE RPS | 2658 | 2162 | ~19% |
| 8MiB body c4 | — | 50 RPS | — |
| 512KiB body c8 | — | 890 RPS | — |
| c1 裸 socket p50 | 114us | 927us | +813us |
| c1 裸 socket p99 | 251us | 2955us | +2.7ms |

- **重要环境修正**：首版 mock（python http.server）未设 TCP_NODELAY 且响应
  分两次 write，Nagle×delayed-ACK 咬合产生 42ms 假延迟——曾误判「经代理
  188 RPS vs 直连 191 RPS 持平」。修正 mock 后真实开销如上表。
  **对照数据（docs/benchmark-memory.md 等）若使用同类 python mock，
  其绝对值同样受此影响，横向比较时需注意**。
- 代理单请求开销 ~0.8ms（c1 短请求），高并发下吞吐开销来自多一跳 +
  hyper 客户端/服务端两套栈。
- 上游连接 hyper 默认未开 TCP_NODELAY（`ss -tni` 实测 ato:40），
  与无 NODELAY 的上游配合时会出现同样的 40ms 咬合。**可评估在构建
  upstream client 时开启 nodelay**（潜在优化项，未改，仅记录）。

### 5.2 内存

| 场景 | RSS |
|---|---|
| 空闲基线 | ~6MB（debug）/ ~8MB（release，11 线程） |
| 5 分钟小请求浸泡（2273 RPS） | 79.4~79.5MB 恒定（60 样本零漂移） |
| 8MiB body c4 压测中 | 65~71MB |
| 5×32MiB 并发拉流中 | 峰值 ~37MB RSS + spool 磁盘 ~160MB |
| 32MiB 响应后空闲 | 27MB（分配器保留页，未归还属正常） |
| valgrind memcheck 短跑 | **0 bytes definitely lost，0 errors** |
|  | still reachable 66KB（tokio 运行时常驻，正常） |

- 与 Windows 已知数据（每并发 ~2.4MB 磁盘缓存模式）量级一致。
- 502 重试循环长时间运行（attempt 60+）无内存增长迹象。

### 5.3 perf 热点（15s c8 压测采样）

- 采样头部为 malloc/free 族（~6%）与 memmove/memcmp——字节流转发场景
  的正常形态；单函数最高 2.18%（malloc）。
- perf stat：1.45 CPU、3.8K ctx-switches/s、IPC 0.20（IO 密集特征，
  符合代理角色）、cache-miss 3.6%。
- 结论：无异常热点，瓶颈在网络 IO 而非 CPU。strip 后无符号导致
  perf 符号显示为地址（正常，发布二进制特征）。

### 5.4 二进制体积

| 项 | Windows | Linux |
|---|---|---|
| release 文件 | 4.06 MiB（记忆值） | **3.97 MiB（4,166,832 B）** |
| .text | — | 2545.5 KiB |
| .rodata | — | 556.7 KiB |
| unwind 相关（.eh_frame+.hdr+.gcc_except_table） | — | 653.8 KiB（≈16%） |
| 动态依赖 | — | libgcc_s / libm / libc（基础三件） |

- 与 Windows 4.06MiB 同量级（unwind 占比与记忆中 abort 可省 32% 的
  数据自洽），profile 配置跨平台生效正常。

## 六、测试基础设施教训（非产品缺陷，已修正）

1. **mock 脚本 Content-Length 错标**：502 body 20 字节声明 21 字节 →
   hyper 按声明等待，连接污染 → aproxy 502 路径「卡死」假象。
   排查耗费约一小时，一度误判为产品缺陷。**教训：mock 的
   Content-Length 必须程序化生成（len(body)），严禁手写数字**。
2. **python http.server 无 NODELAY + 分段 write** → 42ms Nagle 咬合
   假延迟（见 5.1 环境修正）。
3. **ssh + 测试遗留守护**：守护继承测试 stdio 导致 ssh 等 EOF 挂起。
   远程测试一律输出重定向到 remote 文件再回读。
4. **`pkill -f` 模式自匹配**：ssh 远端命令行包含模式文本时把自己的
   shell 杀掉（exit 255）。模式用 `[g]` 方括号规避。

## 七、遗留事项（如实记录，未处理）

1. **产品语义**：unix `APROXY_RUN_DIR` 影响 IPC 寻址（P2-2）——是否对齐
   Windows 语义待决策。
2. **优化候选**：upstream client 开启 TCP_NODELAY（5.1）。
3. **测试基建**：看门狗测试子进程清理的 RAII 化（第三节末）；
   CI 挂 remote 后 ubuntu/macos cargo check 真正运行（TODO 既有条目），
   本次 P0 两项正是它该拦下的。
4. **IPC roundtrip 的 unix 单测缺口**：`ipc_ping_and_shutdown_roundtrip`
   仍为 Windows-only（UDS 版未写）。
5. **/dev/shm 心跳文件残留**：进程死亡不清理（Windows 共享内存节随进程
   消失，unix 文件永存）。104 个残留文件仅占 ~8KB，`endpoint_for` 与
   create 均按端口名覆盖写，无功能影响；「status 清孤儿」类治理可顺带
   清理，未改。
6. remote 上的测试副本 `~/aproxy-test`（含临时 sed 补丁）与 `~/aproxy-fix`
   （正式修复源码）保留待复查，确认合并后可删。

## 八、提交清单

分支 `fix/unix-first-test`（自 master 7ab84aa）：
1. `9bb8d2e` fix: unix 分支首次实机修复——编译/链接/测试平台适配
   （P0-1、P0-2、P2-1、P2-2 测试侧、解锁测试）
2. `35003b3` fix: restart 集成测试隔离 run_dir 显式寻址（unix 必需）
3. `bf3be2e` fix: unix 看门狗死亡检测从不工作改为轮询退化（P1-1、P1-2）
4. 本报告（`.agents/docs/unix-testing.md`）

验证链：Windows fmt/clippy/test 全绿 → Linux fmt/clippy/lib 111/集成 36+2
全绿 → 功能实测 + 压测 + 浸泡 + perf + valgrind + 体积分析完成。
