# Writing themes and extensions for the aProxy window

The aProxy desktop window (`aproxy desktop`) can load two kinds of packages:

- a **theme** changes how the window looks: colors for light and dark mode, corner radius, motion,
  and optionally a stylesheet with its own images and fonts;
- an **extension** is a JavaScript module that runs inside the window. It can add pages and panels,
  a mascot, read the same instances, request history and configs the window shows, and do what the
  window can do: start and stop instances, edit configs, restart format programs, install updates.

Users install packages from a local `.zip` file or an unpacked folder (choose a file on the
settings page, or drag either into the window). There is no store.

## Trust model: no sandbox, an explicit install

An enabled extension runs in the main window with the window's full power. aProxy does not
restrict what it can call or which hosts it can reach. Instead, before anything is installed, the
window shows what the package is and asks the user to confirm each risk that applies:

- the package can run code, use the network and change instances and configs;
- the source is closed (no `repository` in the manifest);
- the ZIP's SHA-256 does not match any asset of the GitHub release for this version, or the release
  has no digest to compare;
- the repository has very few stars, or GitHub could not be checked.

Repository facts (stars, license, age) are shown as facts about the package only when the ZIP's
digest matched a release asset: anyone can name a popular repository in a manifest. The window
never asks GitHub with the user's credentials.

Write your package so that this review goes well: publish the source, attach the exact ZIP you
expect users to install to a GitHub release, and declare what you use in the manifest. Users and
their agents may review a package with the audit skills that ship with aProxy
(`aproxy-audit-extension`, `aproxy-audit-theme`); the signals those skills look for are listed in
`.claude/skills/aproxy-audit-*/references/signals.md`.

## Package layout

A package is a folder, or a ZIP of it (stored or deflate). `manifest.json` sits at the root; a ZIP
may instead contain one folder that holds the whole package. `__MACOSX`, `.DS_Store`, `Thumbs.db`
and `desktop.ini` are ignored.

```
my-theme/                     my-extension/
  manifest.json                 manifest.json
  theme.json                    main.mjs
  theme.css        (optional)   assets/          (optional, any files)
  assets/          (optional)     lib.mjs
    paper.webp                    icon.svg
    Inter.woff2                 README.md        (optional)
  README.md        (optional)   LICENSE          (optional)
  LICENSE          (optional)
```

At the root, besides the files above, only documentation files whose names start with `README`,
`LICENSE`, `LICENCE` or `COPYING` are allowed. A theme's `assets/` may hold only images (PNG, JPEG,
WebP, GIF, AVIF, SVG) and fonts (WOFF2, WOFF, TTF, OTF); an extension's `assets/` may hold anything.

Limits: a ZIP of at most 64 MiB, 2,000 files, 64 MiB per file and 256 MiB unpacked. Paths are
relative, use `/`, have at most eight components and 200 bytes, and each component avoids control
characters, `\ : * ? " < > |`, a trailing dot or space, and Windows device names (`con`, `nul`,
`com1`...). Two paths may not differ only in letter case. Links, junctions and encrypted entries
are refused.

### manifest.json

```json
{
  "schema": 1,
  "kind": "extension",
  "id": "example.key-pool",
  "name": { "en": "Key pool", "zh-CN": "密钥池" },
  "version": "1.2.0",
  "author": "Example Dev",
  "description": "Shows the keys of the keys format and lets you pause one.",
  "repository": "https://github.com/example/aproxy-key-pool",
  "license": "MIT",
  "formats": ["aproxy-format"],
  "network": [],
  "writes": ["formats"]
}
```

| Field | Rule |
|---|---|
| `schema` | `1`. |
| `kind` | `"theme"` or `"extension"`. An installed id cannot change kind. |
| `id` | 3-100 characters of `a-z 0-9 . -`, starting and ending with a letter or digit. It names the install folder and the data folder; keep it stable across versions. A reverse-domain or `author.name` form avoids collisions. |
| `name`, `description` | Text of 1-64 and 1-500 characters, or an object mapping 1-16 language tags (`en`, `zh-CN`...) to such text. Control and direction-override characters are removed. |
| `version` | SemVer 2.0.0 without a leading `v`. |
| `author` | 1-64 characters. |
| `repository` | Optional HTTPS URL of the source. For GitHub use `https://github.com/<owner>/<repo>`. Leave it out for closed source; the window says so. |
| `license` | Optional, e.g. an SPDX expression. |
| `formats` | Extensions only. Names of format programs this extension works with (the `name` a program reports in its hello). Up to 16. |
| `network` | Extensions only. Host names the extension connects to, optionally `*.`-prefixed. Up to 64. Shown to the user; not enforced. |
| `writes` | Extensions only. Which of `config`, `instances`, `updates`, `formats` the extension changes. Shown to the user; not enforced. |

The file is UTF-8 JSON, at most 64 KiB, with no repeated keys. Declare `network` and `writes`
honestly: they are what the user reads before confirming, and an audit compares them with the code.

## Themes

`theme.json` describes the colors; the window derives its whole palette from a few seed colors, so
a theme stays readable and consistent with every part of the window.

```json
{
  "name": "Paper",
  "author": "Example Dev",
  "contrast": "standard",
  "radius": 0.6,
  "motion": 1,
  "font": "system",
  "light": { "primary": "#7a5af8", "surface": "#f6f1e7", "text": "#2b2a33" },
  "dark":  { "primary": "oklch(0.72 0.14 290)", "surface": "#1d1b22" }
}
```

| Field | Meaning |
|---|---|
| `name` | Required, at most 40 characters. |
| `author` | Optional, at most 40 characters. |
| `contrast` | `"standard"` or `"more"` (darker secondary text, clearer lines). |
| `radius` | Corner radius multiplier, 0 (square) to 1.5; 1 is the built-in look. |
| `motion` | Motion strength, 0 (fades only) to 1.5. |
| `font` | One of the window's own fonts: `"rounded"`, `"system"`, `"mono"`. |
| `titlebarMin` | Minimum title bar height in CSS pixels, 36-64. |
| `light`, `dark` | Seed colors per mode for the roles `primary`, `text`, `surface`, `ok`, `retry`, `error`, `warn`, each `#rgb`, `#rrggbb` or `oklch(L C H)`. Missing roles, or a missing mode, use the built-in colors. |

`theme.css` (optional, at most 1 MiB) is added after the window's own styles. It can use the
window's CSS custom properties (`--text`, `--surface`, `--card-bg`, `--line`, `--lilac`,
`--mint`, `--coral`... see `aproxy-desktop/ui/src/app.css`) and refer to its own files with
relative URLs:

```css
.panel { background-image: url("assets/paper.webp"); }
@font-face { font-family: "Inter"; src: url("assets/Inter.woff2") format("woff2"); }
```

Class names inside the window are not a stable interface and may change between versions; prefer
the custom properties. A stylesheet that hides, covers or rewords warnings, errors or install
prompts is exactly what the audit skills flag.

## Extensions

`main.mjs` is an ES module. It exports `activate(api)` and, optionally, `deactivate()`:

```js
export async function activate(api) {
  const off = api.ui.mount('instance.top', (el, ctx) => {
    el.textContent = `Instance on port ${ctx.port}`;
    return () => { el.textContent = ''; };
  });

  api.ui.addPage({
    id: 'keys',
    title: { en: 'Key pool', 'zh-CN': '密钥池' },
    icon: 'list',
    render(el) {
      const img = document.createElement('img');
      img.src = api.assets.url('assets/icon.svg');
      el.append(img);
    },
  });
}

export function deactivate() {}
```

- The window loads enabled extensions in the order the user enabled them. Everything an extension
  registered through `api` (pages, panels, mascots, subscriptions) is removed for it when it is
  disabled, upgraded or uninstalled; clean up anything else yourself in `deactivate`.
- Relative `import`s work (`import { x } from './assets/lib.mjs'`), and so do relative URLs built
  from `import.meta.url`. Each activation gets fresh URLs, so after an upgrade the new versions of
  all your files load without restarting the window.
- An error thrown by `activate` or by a render function is caught: the extension is marked as
  failed on the settings page with the message, and the rest of the window keeps working.
- The window's language, light or dark mode and colors change at run time: read them from
  `api.ui.state()` and `api.ui.tokens()`, and subscribe with `api.ui.onState`.

### The `api` object, version 1

`api.version` is `1`. Version 1 only grows: names, parameters and results that exist keep their
meaning, so an extension written against it keeps working. The authoritative types are in
`aproxy-desktop/ui/src/lib/extensions/api.svelte.ts` (the `ExtensionApi` interface) and the data
shapes in `aproxy-desktop/ui/src/lib/backend/contract.ts`.

| Group | Members |
|---|---|
| `self` | `{ id, version }` from your installed manifest. |
| `instances` | `list()` the instance cards, by port; `onChange(cb)`; `configs()` the configs that can be started; `start(target)`, `stop(run)`, `restart(run)`. |
| `requests` | `recent()` the 50 newest requests of the home; `onChange(cb)`; `query(run, cursor, limit)` pages through history (`run` null for the whole home); `open(ref)` returns `{ detail, read(part, offset, limit), close() }` for one request's details and bodies. Credentials are redacted before they reach the window. |
| `config` | `read(target)`, `validate(target, edits)`, `write(target, edits, acknowledgeExternalAcl?)`. A write is checked by the same rules as `aproxy start` and applies after the instance restarts. |
| `updates` | `check('auto' \| 'pre')`, `install(checkToken)`. |
| `formats` | `list(run)` the `[[format]]` entries of a running instance; `call(run, name, method, params?)` a control method of one entry; `restart(run, names \| null)` replaces only those format processes. See below. |
| `ui` | `state()` and `onState(cb)` (page, language, light/dark, mascot mood, retrying and in-flight counts); `tokens()` the current color tokens; `addPage(spec)`; `mount(point, render)` for `'overview.top'` and `'instance.top'` (ctx carries `port`); `registerMascot(def)`; `toast(text, tone?)`; `navigate(page)`. |
| `settings` | `get()`, `set(changes)` (a `null` value deletes a key), `onChange(cb)`. Stored in `desktop.toml` under `[extensions.settings."<id>"]`, where users can edit them by hand; up to 64 KiB. |
| `storage` | `get()`, `set(changes)`, `reset(values?)`: one JSON object in `<home>/desktop/extension-data/<id>/storage.json`, up to 4 MiB. |
| `files` | `list()`, `read(name)`, `readText(name)`, `write(name, bytes \| string)`, `delete(name)`: files in `<home>/desktop/extension-data/<id>/files/`, names of `[A-Za-z0-9._-]`, up to 64 MiB each, 1,000 files and 512 MiB with storage. |
| `assets` | `url(path)` the URL of a file in your package. |

Calls that fail reject with `{ code, details }`, the same faults the window itself handles
(`desktop_changed` when the instance you named is no longer the one on that port,
`desktop_unresponsive`, `desktop_package` with a reason, and so on; see `Fault` in
`contract.ts`). Settings, storage and files survive upgrades; they are removed only when the user
uninstalls the extension and chooses to remove its data.

`Run` is `{ port, instanceId }`: an action is refused rather than sent to a different instance
that took the same port. Take it from the card you showed the user (`card.run`).

### Working with a format program

An extension can be the interface of a format program (see the aproxy-format skill): list the
program's name in `formats`, find its entries with `api.formats.list(run)` (match
`FormatView.program`, but act on `FormatView.name`: two entries can run the same program), and
call the methods the program offers:

```js
const [entry] = (await api.formats.list(run)).filter((f) => f.program === 'my-router' && f.control);
const settings = await api.formats.call(run, entry.name, 'settings.read');
```

`call` resolves with the program's result. A program's own refusal rejects with
`desktop_format_fail` and the program's `code` and `reason`; host-side problems reject with
`desktop_format` and a reason such as `no-control`, `busy`, `timeout` or `unsupported`. The methods,
their parameters (up to 64 KiB of JSON) and when a change reaches request traffic are up to the
program. `api.formats.restart(run, [entry.name])` starts fresh processes for that entry without
restarting the instance; if the new process fails its handshake, nothing is replaced.

## Trying it locally

Drag the unpacked package folder into the window and confirm the install. To load a change, drag
the folder in again: the window offers to reinstall the same version, and the new files are used
as soon as you confirm. Disabling and enabling on the settings page also reloads the code. Keep the
version number unchanged while developing; bump it for a release.

An error thrown by `activate` appears on the settings page next to the extension; an error in a
page or panel's render function is shown in place of that page or panel.

## Publishing

1. Put the source in a public repository and set `repository` in the manifest.
2. Tag the release `v<version>` (or `<version>`), matching the manifest's `version`.
3. Build the ZIP once and attach that exact file to the GitHub release. GitHub records its SHA-256;
   the window compares the user's file with the digests of the release assets. A rebuild of the same
   files usually produces a different ZIP (timestamps, ordering), so do not re-zip after uploading.
4. Tell users to download the ZIP from the release and install it from the window. A matching
   digest lets the window show your repository's facts as facts about this package.

Upgrades keep the extension enabled and keep its settings, storage and files. A theme in use stays
selected.
