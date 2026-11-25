---
name: parallel-codex-tasks
description: 派发 Codex 任务、决定排队还是并行之前回想：用户允许没有冲突的任务并行，各用独立 worktree 与编译目录。
metadata:
  type: feedback
  scope: 主代理给 Codex（或其他实现代理）派任务的方式，2026-11-26 起
  status: active
  last_verified: 2026-11-26
---

用户 2026-11-26：「这类没有冲突的任务可以并行」。改动范围不重叠的任务不必等前一个做完：

- 每个并行任务一个独立 worktree（`.worktrees/codex-<任务>`）、一个独立分支、一个独立的 `CARGO_TARGET_DIR`
  （`launch.ps1 -WorkDir ... -TargetDir ...`），不与其他任务共用目标目录（共用的坑见 [[shared-target-dir-stale-tests]]）。
- 简报里写明另一个任务在哪个 worktree、不得触碰；「一次只跑一个 cargo」只约束任务自己的命令，不再约束任务之间。
- 改同一批文件、或一个依赖另一个结果的任务，仍然排队。

**Why:** 用户嫌串行等待拖慢进度；此前「同一时刻只跑一个 cargo」源自本机可用内存约 4.4 GB（release-0.2.0 计划
2026-11-11 的记录），是保守做法而非用户要求。

**How to apply:** 派任务前先判断文件范围是否重叠；不重叠就开独立 worktree 并行。并行时若出现内存不足或构建被杀，
先降回串行并告诉用户，不要自行重启被系统杀掉的后台任务。

**Evidence:** 用户 2026-11-26 的消息；t16（`.worktrees/codex`）与 t17（`.worktrees/codex-t17`）当天并行启动。
**Recheck when:** 本机内存条件变化，或并行任务出现互相干扰的构建失败。
