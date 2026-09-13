# macOS 支持：必须完成（用户 2026-09-12 定调）

## 决定

**macOS 支持是必须项，不是可选项。** 对外承诺已经存在：README 安装引导
（`scripts/install.sh` 标注 "Linux / macOS / Git Bash"）与 release 矩阵
（`aarch64-apple-darwin`、`x86_64-apple-darwin` 两个二进制）都已发布。

**后续协作者需在 macOS 真机上完成实现与验证。** Windows 开发机 + 远程 CI
只能给编译面与测试面信号；本项目看护者/进程身份这类行为，必须真机复验
（与 2026-09-10 unix 首测同样的道理——那轮也是真机才挖出 P0）。

## 现状（2026-09-12）

CI 的 macos job 自 unix job 升级为 `cargo test`（`bd36e10`）起**每次都红，
从未绿过一次**（此前两次误报「CI 绿」的经过与教训见
[[2026-09-11-ci-unix-blindspot]]）。3 个 lib 测试失败：

| 测试 | panic 信息 |
|---|---|
| `install::announce::tests::announce_roundtrip_and_active_judgement` | 宣告节创建失败 |
| `watchdog::tests::heartbeat_write_and_read_same_process` | 心跳节创建 |
| `watchdog::tests::claim_identity_rejects_mismatch_and_dead` | 本进程创建时间必须可查 |

ubuntu 与 windows 两个 job 全绿（问题仅限 macOS）。

## 根因：`#[cfg(unix)]` 的实现是 **Linux 专用**

两族原语在 macOS 都不存在。

### A. `/dev/shm`（共享内存文件介质）

- `src/install/announce.rs:201` — `shm_path()` → `/dev/shm/aproxy-install`（宣告节）
- `src/watchdog.rs:1146` — `create_heartbeat` → `/dev/shm/aproxy-heart-<port>`
- `src/watchdog.rs:1162` — `heartbeat_store`（beat 写）
- `src/watchdog.rs:1171` — `heartbeat_load`（读）
- `src/watchdog.rs:1187` — `remove_heartbeat_file`（清理）
- 另见 `src/watchdog.rs:409`、`src/server.rs:223` 的注释（同样假设 /dev/shm）

### B. `/proc`（进程查询）

- `src/watchdog.rs:1309` — `process_start_time`（`/proc/<pid>/stat` starttime）**无任何回退**
- `src/watchdog.rs:1328` — `is_aproxy_process`（`/proc/<pid>/exe` readlink）
- `src/watchdog.rs:1342` — `process_exited`（`/proc/<pid>/stat` 进程态）
- `src/watchdog.rs:1361` — `process_image_path`（`/proc/<pid>/exe` readlink）
- `src/watchdog.rs:1026` — `process_alive_for_wait`（有 `kill(pid,0)` 回退，
  但 zombie 判定在 macOS 失效——注释已声明该已知退化）

## 影响面（比 3 个测试失败大得多，且多为静默失效）

- **`is_aproxy_process` 对活进程返回 false**：readlink `/proc/<pid>/exe` 在
  macOS 返回 ENOENT，而该分支的语义是「进程已死 → false」——于是**所有**
  进程都被判为非 aProxy 进程。选举、收养链、处决验证全部失效，且不报错。
- `process_image_path` 恒 None → install 的管辖检查（bin 外实例拒绝/`--adopt`
  提示）全部跳过。
- `process_start_time` 恒 None → 看护者死亡等待、pid 复用防护、claim 身份
  校验退化。
- 心跳节读写失败 → 看护者挂死判定与 install 保活续作失效。
- 宣告节创建失败 → install 差异化行为（实例死亡复查、守护补种抑制）失效。

结论：**macOS 上代理转发本身可用，但看护者与 install 的核心保障是坏的。**

## 实现方向（供实现者参考，非定稿）

### A. 共享内存介质 → 换路径

macOS 无 `/dev/shm`。两个候选：

1. **`<APROXY_HOME>/run` 下**（推荐）：与 `.pid`/`.restore`/`install.state`
   同处，语义自洽——本项目对这些文件的要求只是「同机同用户可读写 +
   崩溃残留靠心跳过期判定」，不要求真共享内存；且天然享受 `APROXY_HOME`
   重定向（测试隔离白送，CI 可直接测）。
2. `std::env::temp_dir()`（macOS 的 `$TMPDIR` = `/var/folders/.../T/`）：
   缺点是多用户/沙箱环境下路径随机，测试隔离与断言都更麻烦。

**语义差异必须在注释写明**：`/dev/shm` 是 tmpfs（内存计费、重启即失），
换到磁盘目录后文件会跨重启残留。本项目判定逻辑本就以「心跳过期」兜底
（unix 崩溃残留路径），故影响可控——但注释要交代清楚，否则后续读者会
误以为仍是易失介质。

### B. 进程查询 → libproc / sysctl

- 可执行路径：`proc_pidpath(pid, buf, len)`（`<libproc.h>`）
- 创建时间：`proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size)` →
  `pbi_start_tvsec`（与 Windows `created_at` 语义对齐，注意秒/微秒换算）
- 进程态（zombie 判定）：`sysctl(CTL_KERN, KERN_PROC, KERN_PROC_PID, pid, ...)`
  → `kinfo_proc.kp_proc.p_stat == SZOMB`
- 链接：`#[link(name = "proc")]`（`libproc`）与 `libc` 均为系统库，无需新增
  crate 依赖；注意 macOS 上 `proc_pidpath` 对已死进程的返回码语义
- 权限方向保持现状：跨用户不可查时 fail-open（误放行代价 < 误拒绝）

### C. 平台收口纪律（沿用既有设计原则）

新增 macOS 分支应收口在既有 imp 模块内（`imp`/`imp_heart`/`imp_process`），
不要散落到调用方——与「状态机/IPC/恢复平台无关，平台差异只收口在交换原语、
宣告节介质、可执行位」的既有原则一致。

## 验收标准

1. CI macos job（`cargo test`）全绿。
2. macOS 真机行为面实测，照 `.agents/docs/unix-testing.md` 的清单做法逐项：
   - **看护者**：启动 / 选举 / 收养 / 心跳写入与读取 / 优雅退出清理残留
   - **进程身份**：`is_aproxy_process` 对活进程 true、对僵尸 false；
     `process_image_path` 返回真实路径（install 管辖检查依赖它）
   - **install**：宣告节创建/beat/解除；`--from` 快速路径；
     有实例滚动升级（真实守护，pid 滚动）
   - **残留**：崩溃后心跳/宣告文件的残留与过期判定
3. 测试补强：现仅 Linux 具备的判定测试（zombie 判死、exe 比对）在 macOS
   跑通，或显式 skip 并在注释写明原因（不可默默变红）。

## 未决事项（待用户定）

- **CI 现状处理**：macos job 红着会让每次 push 的 CI 结论成为 failure，长期
  会掩盖新引入的失败。可选：(a) 给该 job 加 `continue-on-error: true` 并注释
  指向本工作项（保住 CI 信号可用性）；(b) 保持红作为「已知未完成」的显式提示。
  **临时回退到 `cargo check` 不可取**——check 通过恰恰是这次问题被藏住的原因。
- release 矩阵是否在 macOS 修复前继续发布 macOS 二进制（当前发布的是部分
  功能失效的产物）。

相关：[[2026-09-11-ci-unix-blindspot]]、[[unix-first-test]]、
[[2026-09-11-install-three-platform-e2e]]、[[2026-09-12-install-online-deep-test]]
