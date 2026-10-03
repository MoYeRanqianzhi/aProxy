---
name: shared-target-dir-stale-tests
description: 在 worktree 与主工作区之间切换跑 cargo test 时回想——共用 CARGO_TARGET_DIR 会让 cargo 把旧构建当成最新，测到的不是当前代码
metadata:
  type: feedback
  scope: 本仓库任何用 git worktree 做对照/并行开发并共用同一个 CARGO_TARGET_DIR 的场景
  status: active
  last_verified: 2026-10-04
---

# worktree 与主工作区不要共用 CARGO_TARGET_DIR

**规则**：每个 worktree 用自己的 `CARGO_TARGET_DIR`。如果已经共用过，回到另一份
源码前先 `cargo clean -p aproxy`（目标目录不变），并核对测试数与预期一致。

**Evidence**（2026-10-04）：在 `.worktrees/pre-ws1b`（6721714）里用
`CARGO_TARGET_DIR=target-review` 跑完 proxy_integration 后，回到主工作区
（b9b1f7d，多了 WS-1b 的 13 个测试）用同一目标目录再跑，cargo 没有重编，
直接执行了旧二进制：结果显示 73 项而非 86 项。`cargo clean -p aproxy` 后重编，
恢复为 86 项。

**Why**（机制为推断，未读 cargo 源码核实）：dep-info 里记的是相对包根的源文件
路径，cargo 按 mtime 判断新鲜度。主工作区的源文件比刚在 worktree 里构建出的产物
旧，于是被判为「无需重编」。表面现象是测试照常通过或失败，没有任何报错，很容易
把旧代码的结论当成当前代码的结论。

**How to apply**：并行代理与对照实验各配一个独立目标目录（例如
`target-<工作项>`）；汇报测试结果时附上测试数，与预期对不上就先怀疑构建陈旧。

**Recheck when**：cargo 改变指纹的路径或新鲜度算法。

相关：[[unix-stress-review]]（并行 cargo 抢锁导致的假编译错误，另一类目标目录共享问题）
