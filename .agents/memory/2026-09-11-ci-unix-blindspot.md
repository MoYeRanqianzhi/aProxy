# 2026-09-11 CI unix 编译盲区——推送后必须确认 CI 状态

## 事故

install 功能收尾汇报「204 项测试全绿 + 已推送」时，`main` 分支 CI 的 unix job
（ubuntu/macos `cargo check --all-targets`）**其实是红的**——两个 unix 编译错误
在 CI 里躺着没人看：

1. `src/install/announce.rs` E0596：unix 心跳写 `&mut &a._keepalive` 对共享
   引用再取 `&mut`。unix 分支在 Windows 开发机上从未被编译过。
   修复：`_keepalive: File` → `Mutex<File>`（保持 Send+Sync）。
2. `src/install/download/mod.rs`：`std::arch::is_x86_feature_detected!("avx2")`
   在 aarch64/mac 目标编译失败（「This macro cannot be used on the current
   target」）。修复：`target_arch` 门控，非 x86 恒 baseline（1b98662）。

## 根因

- CI 配置有 unix check job（ci.yml `unix` matrix），但收尾时只看本地测试绿就
  汇报完成，没有 `gh run list` 确认远端 CI 结论。
- Windows 开发机写 unix 分支 = 写从未执行的代码，本地全绿不代表编译面正确。

## 纪律

- **每次 push 后确认 CI 绿才算收尾——且必须按 workflow 名过滤**：
  `gh run list --workflow=CI --limit 3`。同一 push 会同时触发 CI 与 Review
  两个 workflow，`gh run list --limit 1` 可能抓到 Review 的 success 而误判
  CI 绿（2026-09-11 夜与 09-12 两次实际踩中，见下节）。
- unix-only 代码改动后，本地无法编译验证时（交叉缺 gcc），交给 WSL/远端/CI
  三者之一裁决，绝不凭「看起来对」就提交。

## 复发（2026-09-12 发现，事故升级为 macOS 未支持）

`gh run list --limit 1` 抓到并发 Review run 的 success，据此两次误报「CI 全绿」。
实测真相：**macOS job 自 unix job 升级为 `cargo test`（bd36e10）起每次都红**，
从未绿过一次。3 个失败：

1. `install::announce` 宣告节创建失败
2. `watchdog` 心跳节创建失败
3. `watchdog` 本进程创建时间查不到

根因：`#[cfg(unix)]` 的共享内存与进程查询实现是 **Linux 专用**——
`/dev/shm`（宣告节、心跳节）与 `/proc`（进程创建时间/镜像路径/退出判定）
在 macOS 都不存在。影响面比测试失败更大：macOS 上看护者心跳、install 宣告、
进程身份（选举/收养/处决验证）全部失效，且 `is_aproxy_process` 对活进程
返回 false（readlink ENOENT 被当作「进程已死」）——**静默失效不报错**。

教训：编译面 check 通过 ≠ 平台可用；把 job 从 check 升级为 test 必须当次
就盯结论，不能默认「升完就绿」。

## 现行 CI 结构（2026-10-04，WS-5a 之后）

上文的「unix matrix」已拆掉。`.github/workflows/ci.yml`（workflow 名仍为 CI）现有
三个独立 job：`windows`（clippy + test）、`ubuntu`（clippy + test）、`macos`
（只跑 test，已知红，没有设 continue-on-error，红色就表示 macOS 支持未完成）。
test 与 clippy 一律带 `--workspace`。判断能否收尾时按 job 看结论：`windows` 与
`ubuntu` 是门禁，必须绿；`macos` 红属预期，但要确认失败的仍是已知的那一类——依赖 `/dev/shm`
或 `/proc` 的测试（2026-10-06 为 5 个：survey 无响应实例、两个 announce 测试、claim 身份、心跳读写），
没有新增别的失败（`gh run view <id> --log-failed | grep FAILED`）。2026-10-06 起另有
`upgrade-from-0-1-0` 任务（windows + ubuntu）跑真实 v0.1.0 驱动的升级。
Recheck when：ci.yml 的 job 结构变化，或 macOS 支持完成。

## 复发（2026-10-06，IPC 提交 B faf551d 后 ubuntu 连红三次）

本机仍无法编译 Linux（WSL 工具链坏、无交叉 gcc）。三个坑，每个都是「Windows 上不存在的路径」：
1. 只被 `cfg(windows)` 代码与测试调用的函数，在 unix 非测试构建里是死代码——clippy `-D warnings`
   报错。写跨平台模块时，凡调用方带平台门控，被调函数也要 `#[cfg(any(windows, test))]` 之类。
2. 调整启动顺序（控制端点先于注册表）后，unix socket 所在的 run 目录可能还没被任何人创建。
   Windows 管道不落盘，这类顺序依赖只在 unix 暴露。
3. 对 Linux errno 的假设（残留 socket 的 connect 只会是 ConnectionRefused）没有验证手段时，
   判定条件要写成「只在确有证据时拒绝，其余一律按安全默认处理」，并让测试把实际错误打出来。

**How to apply**：改了 unix 路径（`cfg(unix)`、socket、/proc、/dev/shm、文件顺序）的提交推送后，
先等 ubuntu job 结论再叠加下一步；手上有半成品时用 `git stash` 先单独提交修复（本次三次都这样处理，
未丢工作）。

相关：[[2026-09-11-never-kill-aproxy-by-name]]、[[2026-09-11-install-pitfalls]]、
[[2026-09-12-install-online-deep-test]]
