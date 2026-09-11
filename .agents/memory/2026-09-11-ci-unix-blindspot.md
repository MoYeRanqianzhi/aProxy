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

- **每次 push 后 `gh run list --branch main --limit 2` 确认 CI 绿，才算收尾。**
- unix-only 代码改动后，本地无法编译验证时（交叉缺 gcc），交给 WSL/远端/CI
  三者之一裁决，绝不凭「看起来对」就提交。
- `.github/workflows/ci.yml` 的 unix job 升级为 `cargo test` 是 TODO 项
  （行为面实测），编译面 check 至少要盯住。

相关：[[2026-09-11-never-kill-aproxy-by-name]]、[[2026-09-11-install-pitfalls]]
