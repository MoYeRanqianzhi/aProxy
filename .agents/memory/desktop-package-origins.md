---
name: desktop-package-origins
description: Recall when changing package URLs, style CSP or presenting repository verification in the desktop window.
metadata:
  type: project
  scope: Tauri 2.12 desktop scheme and GitHub release-asset facts for local packages
  status: active
  last_verified: 2026-11-23
---

Keep package resources at hierarchical URLs. In Tauri 2.12.3 `scripts/core.js` applies `encodeURIComponent` to the entire `convertFileSrc` argument. Pass the package ID alone and append the individually encoded path segments: a percent-encoded slash is not a segment separator. With this app's default protocol options the Windows origin is `http://aproxy-pkg.localhost`; macOS/Linux use `aproxy-pkg://localhost`. The query distinguishes successive ES module activations in the webview, not files in the installer.

**Why:** Browsers resolve CSS URLs and module imports against their parent resource. They also ignore style `'unsafe-inline'` when a nonce or hash is present. Tauri's manager tests the script and style CSP-modification switches independently; disabling the style modification alone leaves script handling intact and permits the page's injected styles. Neither the declared resource URLs nor a successful Rust response prove all engines loaded a font, a stylesheet or a module.

**How to apply:** Preserve hierarchy and check computed style, actual module activation and relative requests in each real engine when changing the host. For provenance, bind the *original ZIP* to a release asset by sha256, not filename. A renamed download can still match. Stars, license and other repository facts without a matching digest describe a repository anyone could name, not the bytes selected by the user. See [[desktop-extension-trust]] and the [page contract](../plan/w5e-extensions.md) for the decisions the page owns.

**Evidence:** Tauri 2.12.3 `scripts/core.js` (`convertFileSrc`) and `src/manager/mod.rs` (per-directive nonce/hash processing); `tests/desktop_packages.rs` exercises hierarchical requests and a local GitHub fake including renamed downloads, mismatches and absent digests. A read-only GET of `repos/MoYeRanqianzhi/aProxy/releases/latest` showed `sha256:` on uploaded release assets. Native engine qualification is separate.

**Recheck when:** Upgrading Tauri/Wry, changing HTTPS scheme options, changing CSP injection, or when GitHub changes the release asset digest contract.
