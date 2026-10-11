---
name: shared-worktree-commit-pathspec
description: 多个代理（或主代理与子代理）在同一个工作区里各自提交时读——提交必须带路径，否则会把别人已暂存的改动一起提交
metadata:
  type: feedback
  scope: 主代理与子代理共用同一个 git 工作区的任何时候
  status: active
  last_verified: 2026-10-12
---

在共用的工作区里提交，一律用 `git commit -m "…" -- <路径…>`（只提交列出的路径），不要先 `git add` 再裸 `git commit`。
裸 `git commit` 提交的是整个暂存区，别的代理已 `git add` 但还没提交的改动会被一并带进来。

**Why:** 2026-10-11 主代理提交 .agents 记录时用了裸 `git commit`，把 W1 子代理已暂存的「删除 0.1.0 兼容层」整批改动
（10 个文件、约 1000 行）提交进了一个 docs 提交；事后用 `git reset --soft` + `git reset` 拆回（未推送，无外部影响）。

**How to apply:** 主代理与子代理都按带路径的写法提交；新加的未跟踪文件先 `git add <路径>` 再带同样的路径提交。派发会提交
代码的子代理时，在任务说明里写明这一条。需要并行改代码时，优先给子代理开独立 worktree（各用独立的 CARGO_TARGET_DIR，
见 [[shared-target-dir-stale-tests]]）。

**Evidence:** 本地提交 d81f1e0、85871b1（已拆为 a92486d、1f23ff2，W1 的改动退回工作区）。

**Recheck when:** 改为每个写代码的子代理都在独立 worktree 里工作之后，此条可降级为一般提醒。
