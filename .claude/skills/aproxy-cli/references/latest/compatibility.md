# Version compatibility

How to tell which aProxy version is actually running, which versions these references describe, and
what to do when they differ. Read it when the version you see is not one these docs describe, when
output does not match what these docs quote, or before moving an installation from 0.1.x to 0.2.0,
which cannot be done in place.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `compatibility.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [compatibility/versions.md](compatibility/versions.md) | Which version these docs describe and how older versions differ. | [What these docs describe](compatibility/versions.md#what-these-docs-describe) |
| [compatibility/alpha.md](compatibility/alpha.md) | Moving from a 0.2.0 alpha build. | [Upgrading from 0.2.0-alpha.1 or 0.2.0-alpha.2](compatibility/alpha.md#upgrading-from-020-alpha1-or-020-alpha2) · [Alpha builds](compatibility/alpha.md#alpha-builds) |
| [compatibility/from-0.1.md](compatibility/from-0.1.md) | Moving a 0.1.x installation to 0.2.0: the steps, the commands that changed, and going back. | [Moving from 0.1.x to 0.2.0](compatibility/from-0.1.md#moving-from-01x-to-020) · [Commands changed in 0.2.0](compatibility/from-0.1.md#commands-changed-in-020) · [Going back to 0.1.x](compatibility/from-0.1.md#going-back-to-01x) |
| [compatibility/migrate.md](compatibility/migrate.md) | Converting 0.1.x settings.json and config files with `aproxy migrate`, and settling with the user what it leaves for review. | [Converting the files with aproxy migrate](compatibility/migrate.md#converting-the-files-with-aproxy-migrate) |
| [compatibility/renamed-keys.md](compatibility/renamed-keys.md) | The old-to-new key tables for settings.json and config.toml, for editing by hand. | [settings.json keys renamed in 0.2.0](compatibility/renamed-keys.md#settingsjson-keys-renamed-in-020) · [Config keys renamed in 0.2.0](compatibility/renamed-keys.md#config-keys-renamed-in-020) |

## Find the version in use

Two versions matter, and they can differ:

| Command | Reports |
|---|---|
| `aproxy --version` | the CLI binary that `aproxy` resolves to |
| `aproxy status` | `v<version>` per running instance: the binary that instance was started from |

From 0.2.0 on, pre-releases included, `aproxy --version` (or `-V`) prints one line in a format that
every later version keeps: `aproxy v<version> (<variant>; <target>[; x86-64-v3])`, for example
`aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)`. The variant is `desktop` for builds
with the desktop interface and `headless` for builds without it; the target is the platform the
binary was built for, such as `x86_64-unknown-linux-musl`; `x86-64-v3` marks a build for CPUs with
AVX2. The version is the text after `aproxy v` up to the first space. 0.1.x prints
`aproxy <version>` instead. `--version` reads and writes nothing under `<home>` and starts no
process, so it is safe to run at any time, even in the middle of an install.

- A running instance keeps its version until it restarts; replacing the binary on disk changes
  only the CLI. `status` then adds
  `Instance v<x> differs from CLI v<y>. After replacing the binary, run aproxy restart <port> to finish the upgrade.`
- Judge runtime behavior (retries, heartbeats, log messages) by the instance's version, and
  command syntax by the CLI's.
- Several copies may be installed: `aproxy install` manages `<home>/bin/` (`<home>` is
  `~/.aproxy` unless `APROXY_HOME` is set), while npm, cargo or a package manager put theirs
  elsewhere. `where aproxy` (Windows) or `command -v aproxy` shows which one runs. `restart`
  launches the copy you invoke, so restarting from an older copy downgrades the instance. Run
  file-writing commands (`config` setters, `alias`, `settings`) with the newest copy too: an older
  binary may drop the keys it does not know when it rewrites config.toml or settings.json, while
  0.2.0 and later keep them (and report an unknown key as an error in the first place). A 0.1.x copy
  and a 0.2.0 or later copy cannot control each other's instances at all (see
  [Moving from 0.1.x to 0.2.0](compatibility/from-0.1.md#moving-from-01x-to-020)).
- An `aproxy install` that halts mid-way leaves the new binary in place while some instances still
  run the old one; commands.md, install / upgrade, explains how to finish.
