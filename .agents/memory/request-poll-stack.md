---
name: request-poll-stack
description: Diagnose a Windows HTTP reset after successful startup when format workers have already processed the request.
metadata:
  type: project
  scope: The proxy's async request path, especially the Windows debug build on a current-thread runtime
  status: active
  last_verified: 2026-11-26
---

A reset on the first GET is not by itself a readiness failure. Distinguish a socket error from the serving process exiting before it can send the response.

**Why:** The clock fixture polls production request tasks on the executable's main thread. Large futures passed by value through nested wait, select and heartbeat drivers can exhaust its 1 MiB Windows stack in an unoptimized build. Having Tokio store the spawned task on the heap does not remove the stack frames of its nested `poll` calls. The normal CLI schedules these tasks on worker threads instead.

**How to apply:** Capture the instance PID **and a handle while it is alive**, then inspect the handle on failure before home cleanup. The `stop` commands' exit codes are not the instance's exit code. For `0xc00000fd`, match the faulting main thread against the same executable and PDB. When a driver only polls an operation, pin it once and pass a borrow through the nested drivers rather than moving the full future into each one.

**Evidence:** `tests/format_protocol.rs` (`startup_probes_run_in_parallel_and_respect_worker_lifetimes`), `examples/format-start-clock.rs` and `src/proxy/attempts.rs` (`Responder::wait`, `wait_after_head`). A local Windows debug reproduction recorded both request frames and an accepted upstream connection, then the instance handle signaled `0xc00000fd`. The symbolized stack went from the request task through `run_attempts`, `deliver` and `wait_after_head` into the response transform stage check. Pinning reduced the combined fixed stack allocations of the first four of those frames by 334,960 bytes on rustc 1.100.0 x64 MSVC. In a separate control, the ordinary CLI with 1 MiB worker stacks also reset a request through formats before launch provenance existed (`reports_land_in_the_request_record`); the same test passed after pinning. These are measurements, not a portable size limit.

**Recheck when:** The runtime, compiler profile, wait/heartbeat drivers, or composition of the request and response futures changes.
