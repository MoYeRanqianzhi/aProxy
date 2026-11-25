---
name: launch-provenance
description: Where to carry information about one start across mixed-version spawners and instances.
metadata:
  type: project
  scope: instance startup and recovery, including older __serve readers
  status: active
  last_verified: 2026-11-26
---

Treat the temporary handoff and the desired-state record as different lifetimes even though both contain the same launch spec.

**Why:** An old receiver cannot clear an environment variable it does not know about: its format programs could inherit a note even if every current receiver clears it. A field inside the reusable spec would survive a restore. A sibling of the spec fields in the private handoff has neither problem. Do not infer the initiator from the parent process or the presence of install state: a frozen force-install invokes the public `restore` command just as a user could.

**How to apply:** Keep replayable configuration in `InstanceSpec` and per-start evidence in the handoff only. Redact using the receiving instance's effective configuration before exposing that evidence. A caller invoking an older instance cannot make it advertise provenance it does not implement.

**Evidence:** `src/spec.rs` (`write_spec_file`, `read_spec_file`); `tests/json_docs.rs` (`launch_metadata_follows_each_launch_not_the_desired_state`); the plain `restore` invocation in `src/install/force.rs` (`restore_and_check`).

**Recheck when:** The spec reader, handoff lifetime, or cross-version startup contract changes. See [[ipc-v1-upgrade-contract]].
