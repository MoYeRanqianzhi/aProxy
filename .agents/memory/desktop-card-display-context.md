---
name: desktop-card-display-context
description: Recall when adding instance-record text to desktop cards or changing how a snapshot is sampled.
metadata:
  type: project
  scope: HomeView instance summaries joined to the desktop survey
  status: active
  last_verified: 2026-11-26
---

The window reads its display secrets and its instance summaries separately. Even with one discovery worker, a snapshot can pair a summary with the preceding run's survey on a reused port.

**Why:** The instance scrubs its own effective credentials. The desktop also knows local values, including overridden ones. Neither source-side redaction nor the previous run's secrets can stand in for that second check.

**How to apply:** Treat run-specific display text like the version: use the matching run and PID's display context, or omit the text. Test the mismatched pair as well as the normal snapshot; filtering optional metadata alone does not protect strings on the card itself. See [[launch-provenance]] for the source record's lifetime.

**Evidence:** `aproxy-desktop/src/bridge.rs` (`snapshot`), `src/desktop.rs` (`DesktopHome::inspect`), and `aproxy-ipc/src/view/tests.rs` (`card_launch_provenance_follows_the_sampled_run`, including a successor on the same port).

**Recheck when:** Discovery publishes summaries and display facts as one atomic sample, or ownership of desktop display redaction changes.
