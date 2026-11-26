# The aProxy extension standard

What a conforming extension looks like. Each departure is a finding (SKILL.md, step 2); how severe
depends on what it hides or enables.

## Contents

- [Package](#package)
- [manifest.json](#manifestjson)
- [Entry module](#entry-module)
- [The extension API](#the-extension-api)
- [Settings and data](#settings-and-data)
- [Not part of the standard](#not-part-of-the-standard)
- [Releases](#releases)

## Package

A zip archive, or the same files unpacked into a directory, with these entries at its root:

| Entry | Required | What it is |
|---|---|---|
| `manifest.json` | yes | metadata, below |
| `main.mjs` | yes | the entry module |
| `assets/` | no | files the module uses (images, fonts, data, further modules), addressed by relative URL or `api.assets.url` |
| `README*`, `LICENSE*` | no | documentation |

Nothing else: no executables, no scripts in other languages, no archives inside the archive, no
files whose content is not what their extension says.

## manifest.json

UTF-8 JSON, one object:

| Field | Required | Type | Meaning |
|---|---|---|---|
| `schema` | yes | integer | `1` |
| `kind` | yes | string | `"extension"` |
| `id` | yes | string | lowercase letters, digits, `.` and `-`, 3 to 100 characters, in reverse-domain style such as `io.github.alice.keys-panel`; also the name of the install directory |
| `name` | yes | string or object | display name; an object maps language tags (`en`, `zh-CN`) to strings |
| `version` | yes | string | semantic version, `MAJOR.MINOR.PATCH` with an optional pre-release |
| `author` | yes | string | |
| `description` | yes | string or object | what it does, in a sentence or two |
| `repository` | no | string | `https://github.com/<owner>/<repo>`; leaving it out declares the extension closed source |
| `license` | no | string | SPDX identifier, e.g. `MIT` |
| `formats` | no | array of strings | format programs this extension is a UI for, by the `name` in their hello reply |
| `network` | no | array of strings | every host the extension contacts |
| `writes` | no | array of strings | what it changes: any of `config`, `instances`, `updates`, `formats` |

`network` and `writes` are disclosures: aProxy shows them in the install warnings and enforces
neither. An extension that contacts hosts or makes changes it does not disclose does not conform.
Fields not listed here do not conform either.

## Entry module

`main.mjs` is an ES module that exports `activate(api)` and may export `deactivate()`. aProxy loads
it from the package's own address (the `aproxy-pkg` scheme, `http://aproxy-pkg.localhost/<id>/` on
Windows), so it may import further modules from `assets/` by relative path, and
`new URL('./assets/…', import.meta.url)` resolves inside the package. Every script in the package is
part of the review, not only `main.mjs`. Code should ship unminified: minifying saves little in a
local app and costs every reviewer. `deactivate()` undoes what `activate` set up outside what the
API tracks: timers, listeners, DOM it added outside its own pages and mount points (aProxy removes
pages, mount points, mascots and subscriptions made through `api` by itself).

## The extension API

`api` is everything a conforming extension uses:

| Group | What it gives |
|---|---|
| `api.instances` | instance cards; start, stop and restart instances |
| `api.requests` | request records, their details and bodies, with credentials masked |
| `api.config` | read, validate and write an instance's configuration file |
| `api.updates` | check for and install aProxy updates |
| `api.formats` | the format programs of an instance; call their control methods; restart them |
| `api.ui` | pages in the navigation (`addPage`), panels in fixed mount points (`mount`: `overview.top`, `instance.top`), mascots (`registerMascot`), the theme's color tokens, interface state (current page, language, mascot mood, retrying and in-flight counts), toasts |
| `api.settings` | the extension's own table in `desktop.toml` |
| `api.storage` | JSON key-value data in the extension's data directory |
| `api.files` | files in the extension's data directory, by name without a path |
| `api.assets` | the address of a file in the package (`url('assets/…')`) |

The groups that change things (`api.config`, `api.instances`, `api.updates`, `api.formats`) are
the ones `writes` discloses. The groups that read request content (`api.requests`) are the most
sensitive reads.

## Settings and data

| What | Where | Through |
|---|---|---|
| the package | `<home>/desktop/extensions/<id>/` | read-only once installed; replaced whole on upgrade |
| settings people edit | `<home>/desktop/desktop.toml`, table `[extensions.settings."<id>"]` | `api.settings` |
| data the extension keeps | `<home>/desktop/extension-data/<id>/` | `api.storage`, `api.files` |

Whether an extension is enabled is decided by the `enabled` list under `[extensions]` in
`desktop.toml`, which belongs to the user; an extension does not change it. The home is
`~/.aproxy` unless `APROXY_HOME` points elsewhere.

## Not part of the standard

Each of these is a finding; SKILL.md's rules say how severe:

- writing into its own package directory, or anywhere outside its data directory;
- `localStorage`, `sessionStorage`, IndexedDB, cookies, Cache Storage, service workers;
- calling the window's internal commands, or importing the app's own modules;
- changing the page outside the extension's own mount points and pages, aProxy's install warnings
  above all;
- loading code from outside the package at run time;
- contacting hosts missing from `network`, making changes missing from `writes`;
- calling the control methods of formats missing from `formats`.

## Releases

A release on GitHub is tagged `v<version>` or `<version>` and carries the package file as an asset.
Before installing a file, aProxy looks for its sha256 among the digests of all the release's
assets. With no match it warns: of a mismatch when an asset of the same file name has a different
digest, otherwise that the release lacks the file or gives no digests; you can do the same
(SKILL.md, step 3).
