---
name: desktop-login-carrier
description: Choosing and rechecking the Windows carrier for per-home login restore
metadata:
  type: project
  scope: Windows current-user login registration of the installed console binary
  status: active
  last_verified: 2026-11-22
---

**Why:** A Run value names a command; it does not request `CREATE_NO_WINDOW`. An invisible parent does not imply its console-subsystem child has no visible terminal. A process exit code from a headless console host is not necessarily its client’s exit code.

**Evidence:** On Windows build 26300, a detached GUI-subsystem parent with no console launched the actual restore command with default flags into temporary homes. Two warmed direct trials (47 and 48 window samples) saw a visible `PseudoConsoleWindow` owned by the child and a newly visible external Windows Terminal window. Two wrapped trials (53 and 57 samples) saw neither. The intentionally failing direct command exited 1; its headless wrapper exited 0. No login or real Run registration was necessary. Microsoft’s *CreatePseudoConsole* requirements put the API floor at Windows 10 version 1809; the Microsoft Terminal console host parser (`src/host/ConsoleArguments.cpp`, tag `v1.0.1401.0`) recognizes `--headless`. This is not a measurement of every supported build.

**How to apply:** `src/desktop/login.rs` uses the absolute system console host with `--headless` when the pseudoconsole API exists; otherwise it registers the direct executable. Measure using a no-console parent, fresh home, default creation flags, PID ancestry and independently appearing terminal host windows. A hidden parent that also passes special creation flags does not emulate a Run launch. For registry tests inject only a random test subkey.

**Recheck when:** The supported Windows baseline, default terminal, registration mechanism, or console subsystem of the installed client changes.
