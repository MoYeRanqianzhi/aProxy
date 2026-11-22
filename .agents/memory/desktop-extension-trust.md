---
name: desktop-extension-trust
description: Where extension identity, permission to read, and authority to write come from
metadata:
  type: project
  scope: Official desktop extension packages and the shipped host, starting in 0.2.0
  status: active
  last_verified: 2026-11-22
---

**Why:** Copying a directory, enabling its contents, and approving an operation answer different questions. The package name, version and permission declarations are claims by its author. The user enables particular bytes to read what the page can read; redaction of credentials does not make request text public. Consent to read is not consent to start a process, stop an instance or install an update.

**How to apply:** Keep the three checks independent:

- **Which bytes?** Read a bounded snapshot from checked file handles and compare its digest with the user's opt-in. Names, versions and the timer's metadata hint are not identities. Changing one asset invalidates trust even if the manifest is untouched.
- **Which caller?** The shipped page owns the Rust-issued lease. The host contract requires it to bind one opaque frame to a private message port and translate only the published methods, never an extension-supplied command, identity or consent token. An opaque origin isolates the parent DOM; it does not by itself prove that native IPC cannot dispatch. On Windows the key-bearing sender, fixed shell response and native WebView2 hooks are a separate boundary. Do not infer qualification from a successful build. DNS, speculative loads and WebRTC remain residual disclosure paths.
- **Which operation?** Rust derives the confirmation target and rechecks the lease, package, authorization and exact target at admission to the side effect. A remembered instance verb applies only in that home to those bytes; install always asks. The private audit records execution before the effect and does not replay it after a crash. Neither an enabled flag, an old dialog nor a successful prior write substitutes for this check.

**Evidence:** [The host contract, trust boundaries and real-window checks](../plan/w5e-extensions.md) distinguish what belongs to the page from what Rust can enforce. See `src/desktop/extensions/tree.rs` and `authority.rs`, `aproxy-desktop/src/top-sender.js` and `extension_frame.rs`, and `tests/desktop_extensions.rs`. The Rust tests exercise a temporary home, including changed bytes, stale answers and changed targets. The transport analysis is for Tauri 2.12.3 and Wry 0.57.0; no window was qualified by those tests.

**Recheck when:** Changing renderer or IPC versions, shell build/headers, window navigation or native hooks, host port dispatch, or what a resolved launch or update plan means. Recheck the page's disclosure when adding readable data. The trusted-page assumption does not cover code injected into that same page.
