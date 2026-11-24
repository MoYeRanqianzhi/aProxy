---
name: darwin-exit-evidence
description: Recall when a macOS start times out after its child already logged a failure, or when checking process identity or installer exit waits.
metadata:
  type: project
  scope: macOS libproc identity, readiness and native validation
  status: active
  last_verified: 2026-11-24
---

A zombie can simultaneously exist for signal 0 and appear absent to a normal BSD process-info query. Neither result alone answers whether the recorded generation can still serve requests.

**Why:** In a native arm64 probe, a live child returned 136 bytes and status 2 for both arguments to `proc_pidinfo(PROC_PIDTBSDINFO)`. After SIGKILL but before `wait`, argument 0 returned 0/ESRCH, while argument 1 returned the full structure with status 5 (`SZOMB`). After `wait`, both returned 0/ESRCH. This is why waiting for disappearance alone can mask a startup failure until the ready deadline.

**How to apply:** Exercise the wait *before* reaping a child you own. Check a wrong start marker against a still-live process as a separate case; program names are not identity. Do not rewrite the frozen ledger marker or treat its unpadded microseconds field as a decimal fraction.

**Evidence:** `src/process.rs` tests `record_identity_classifies_alive_reused_legacy_gone`, `macos_process_queries_agree_with_ledger_and_executable` and `terminate_verified_refuses_mismatch_and_kills_match`; native arm64 macOS 26.7.2 lifecycle probe, 2026-11-24. See [[identity-no-name]].

**Recheck when:** The Darwin syscall, query flavor or zombie inclusion argument changes, or the test fixture starts reaping the process before observing it.
