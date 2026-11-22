# The aProxy theme standard

What a conforming theme looks like. Each departure is a finding (SKILL.md, step 2); how severe
depends on what it hides or enables.

## Contents

- [Package](#package)
- [manifest.json](#manifestjson)
- [theme.json](#themejson)
- [theme.css](#themecss)
- [Assets](#assets)
- [Where it lives](#where-it-lives)
- [Releases](#releases)

## Package

A zip archive, or the same files unpacked into a directory, with these entries at its root:

| Entry | Required | What it is |
|---|---|---|
| `manifest.json` | yes | metadata, below |
| `theme.json` | yes | colors and shapes, below |
| `theme.css` | no | a stylesheet, below |
| `assets/` | no | images and fonts the stylesheet uses |
| `README*`, `LICENSE*` | no | documentation |

Nothing else.

## manifest.json

UTF-8 JSON, one object:

| Field | Required | Type | Meaning |
|---|---|---|---|
| `schema` | yes | integer | `1` |
| `kind` | yes | string | `"theme"` |
| `id` | yes | string | lowercase letters, digits, `.` and `-`, 3 to 100 characters, in reverse-domain style such as `io.github.alice.night-owl`; also the name of the install directory |
| `name` | yes | string or object | display name; an object maps language tags (`en`, `zh-CN`) to strings |
| `version` | yes | string | semantic version, `MAJOR.MINOR.PATCH` with an optional pre-release |
| `author` | yes | string | |
| `description` | yes | string or object | a sentence or two |
| `repository` | no | string | `https://github.com/<owner>/<repo>`; leaving it out declares the theme closed source |
| `license` | no | string | SPDX identifier, e.g. `CC-BY-4.0` or `MIT` |

The extension-only fields (`formats`, `network`, `writes`) do not belong in a theme: a theme
contacts nothing and changes nothing. Neither do fields not listed here.

## theme.json

UTF-8 JSON, one object. Only `name` is required:

| Field | Type | Allowed values |
|---|---|---|
| `name` | string | the theme's name |
| `author` | string | |
| `contrast` | string | `standard`, `more` |
| `radius` | number | 0 to 1.5, a multiple of the built-in corner radius |
| `font` | string | `rounded`, `system`, `mono`: the window's own fonts |
| `motion` | number | 0 to 1.5, a multiple of the built-in motion; 0 keeps only fades |
| `titlebarMin` | number | 36 to 64, the title bar's minimum height in CSS pixels |
| `light`, `dark` | object | colors for each mode; a missing mode keeps the built-in colors |

A color object has any of `primary`, `text`, `surface`, `ok`, `retry`, `error` and `warn`, each
`#rgb`, `#rrggbb` or `oklch(L C H)`. Colors set the hue; the window derives lightness from its own
steps, so text contrast stays as in the built-in theme. `ok`, `retry`, `error` and `warn` mean
success, retrying, failure and attention: they must stay distinct.

## theme.css

An optional stylesheet, applied after the window's own styles and kept in plain readable CSS.
It refers to the theme's files by paths relative to the package root (`url("assets/sky.webp")`)
and to nothing outside the package. It changes appearance only: it does not hide, cover or reword
content, and it reads nothing.

## Assets

Under `assets/`, files whose content matches their extension:

| Kind | Extensions | First bytes |
|---|---|---|
| PNG | `.png` | `89 50 4E 47 0D 0A 1A 0A` |
| JPEG | `.jpg`, `.jpeg` | `FF D8 FF` |
| GIF | `.gif` | `47 49 46 38` (`GIF8`) |
| WebP | `.webp` | `52 49 46 46` (`RIFF`), then `57 45 42 50` (`WEBP`) at offset 8 |
| AVIF | `.avif` | `66 74 79 70 61 76 69 66` (`ftypavif`) at offset 4 |
| SVG | `.svg` | text: `<svg` or `<?xml` |
| WOFF2 | `.woff2` | `77 4F 46 32` (`wOF2`) |
| WOFF | `.woff` | `77 4F 46 46` (`wOFF`) |
| TrueType | `.ttf` | `00 01 00 00` |
| OpenType | `.otf` | `4F 54 54 4F` (`OTTO`) |

An SVG is a picture: no scripts, no event handlers, no `<foreignObject>`, no external references,
no `<!DOCTYPE>` or entities. No file carries anything after its format's end.

## Where it lives

Installed at `<home>/desktop/extensions/<id>/`, read-only, replaced whole on upgrade. The theme in
use is the `theme` key in `<home>/desktop/desktop.toml` (`builtin` or a theme's id). A theme has
no settings and keeps no data.

## Releases

A release on GitHub is tagged `v<version>` or `<version>` and carries the package file as an
asset. aProxy compares the sha256 of the file being installed with the digest of the asset that
has the same file name, and warns when they differ or the asset is missing.
