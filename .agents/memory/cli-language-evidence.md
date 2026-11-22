---
name: cli-language-evidence
description: Recall before localizing a reason another process recorded, or changing a machine-facing English sentence.
metadata:
  type: project
  scope: CLI and worker language boundaries in 0.2.0 and later
  status: active
  last_verified: 2026-11-22
---

The process that detects a failure is not necessarily the one whose user will read it. In particular, a missing or unfamiliar private render record is not an invitation to translate forensic evidence by matching prose.

**Why:** Starting a Chinese worker must not change what an English reader, a JSON consumer, or an older binary observes on the same wire. A quoted foreign reason can look exactly like a familiar local reason. A truncated public detail can look plausible while describing a different cause.

**How to apply:** Keep the public sentence in English. Prefer typed, sanitized evidence when a later terminal renders its own language; otherwise show what was actually recorded. Localize the log reader's notices, not the log it copies. See [the contributor note](../plan/w3-i18n.md) for the permitted private evidence slots and frozen exceptions.

**Evidence:** `tests/startup_compat.rs` exercises the readiness reader and writer in temporary homes. Against the original pre-release reader the private result was ignored and the English startup log was preserved; against the original writer the current reader retained its log. `tests/worker_language.rs` compares two real language environments and separately checks the language of a later reader.

**Recheck when:** A wire or persistent format gains a versioned reason contract, or its oldest supported reader changes. Recheck the actual old reading path, not just whether a serde struct accepts unknown members. See [[ipc-v1-upgrade-contract]].
