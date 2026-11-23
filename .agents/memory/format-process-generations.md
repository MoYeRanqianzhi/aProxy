---
name: format-process-generations
description: Recall when changing format restart, worker admission, or shutdown with old streams still running.
metadata:
  type: project
  scope: Format entries and their worker and control processes within one instance
  status: active
  last_verified: 2026-11-23
---

A format entry is a stable selector, not a single set of processes. A restart needs two different lifetimes: request capacity, negotiation and breaker state belong to a generation; accountability for live children lasts until those children have actually exited.

**Why:** If a replacement shares the old semaphore, a stream or busy worker can block the first message of the replacement. If it instead forgets the old children, a still-running old stream can collide with a reused worker ID or outlive instance shutdown. Control failure must not consume the sole data permit or trip the required entry's breaker.

**How to apply:** In a restart test, hold old work open, observe the replacement doing real work before releasing it, then check old stream continuity and eventual retirement. Compare process identities and file contents observed by the programs, not elapsed time. Preserve these lifetime boundaries even if pool ownership or the desktop projection changes; the [page contract](../plan/w5e-extensions.md#format-control-and-restart-as-built) describes what observers may rely on.

**Evidence:** `src/transform/tests.rs` (`restarting_preserves_busy_request_and_stream`, `control_faults_do_not_touch_request_workers`, `control_waiting_capacity_is_independent`) runs local child programs; `tests/contract_v0_2.rs` (`format_restart_operation_contract`) edits a role file and observes a different version on the same instance, then a failed handshake without losing the instance.

**Recheck when:** Changing worker identity assignment, admission, draining, or where process shutdown is owned; a passing snapshot serialization test alone cannot establish any of these lifetimes.
