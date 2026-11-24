---
name: migration-validation-boundary
description: Recall when recognizing 0.1.x files, adding migration rows, or deciding what a dry-run may promise.
metadata:
  type: project
  scope: the explicit 0.1.x settings and config migration
  status: active
  last_verified: 2026-11-25
---

A file's shape and a launch's validity answer different questions. Use the strict file reader to identify legacy names, but use the startup layers to decide whether to publish a rewrite. A read-only command can suggest migration; it does not run it.

**Why:** An instance can obtain its upstream from a global default or a start override. A missing effective value is not by itself evidence of an old file. Conversely, `settings::read` intentionally withholds *all* settings after an error; using that fallback for discovery would hide the aliases and directories the user needs to migrate. Reading the on-disk inventory is not permission to run an instance with invalid settings.

**How to apply:** Keep the compatibility tables as the source of mechanical renames. Report ambiguous values in the original file's coordinates. Preview against the proposed settings layer, without assuming caller-specific overrides. A preview is a plan, not a write-permission check. The home lock does not exclude editors: stage and sync before the last comparison, for rollback as well as publication. An existing backup may be an earlier original: reuse it only when its bytes match and it can be made as private as a fresh backup.

**Evidence:** `migrate_preview_apply_and_repeat_in_a_mixed_home`, `migrate_never_overwrites_a_different_backup` and `migrate_limits_explicit_targets_and_discovers_readable_directory_entries` in `tests/json_docs.rs`; `guarded_publication_and_revalidation_preserve_the_right_bytes` in `src/migrate/files.rs`; strict-reader behavior in `src/settings/mod.rs` and `src/config/resolve.rs`.

**Recheck when:** The compatibility tables, strict-reader fallback, discovery scope, global credential binding or startup layers change. See [[config-settings-shape-0-2-0]].
