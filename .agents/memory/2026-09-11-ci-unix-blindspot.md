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

相关：[[2026-09-11-never-kill-aproxy-by-name]]、[[2026-09-11-install-pitfalls]]、
[[2026-09-12-install-online-deep-test]]
