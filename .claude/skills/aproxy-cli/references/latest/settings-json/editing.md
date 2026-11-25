# settings.json reference: location, editing and errors

Part of [settings-json.md](../settings-json.md), which lists the other parts. Where settings.json lives, how to edit it, and what happens when it is invalid.

## Location and editing

- One file per aProxy home, `<APROXY_HOME>/settings.json` (`~/.aproxy/settings.json` unless
  `APROXY_HOME` is set), shared by every instance under that home. It may hold API keys and proxy
  passwords in plain text, so it is written with owner-only permissions: mode `0600` on Unix, and on
  Windows a DACL granting only the current user, SYSTEM and Administrators.
- **Edit it with `aproxy settings`.** Keys are dotted paths, exactly as they appear in the file:
  `watchdog.enabled`, `aliases.work`, `defaults.timeouts.connect_secs`. A segment made only of
  letters, digits, `_` and `-` is written bare; any other segment, such as a header name with a dot,
  is quoted: `defaults.extra_headers."x.foo"`.

  | Command | Effect |
  |---|---|
  | `aproxy settings show` | print every effective value: the file's keys over the built-in defaults, secrets masked. Exits 3 after printing if the file has errors |
  | `aproxy settings get KEY` | print one effective value as JSON; a key that is neither stored nor built in prints `null`. It reads the file as it is, so it answers even when the file has errors |
  | `aproxy settings set KEY VALUE` | set one key. Only keys this version knows can be set |
  | `aproxy settings unset KEY` | remove one key, back to its built-in default. Any key can be removed, including one this version does not know |

- **Values.** `set` takes a JSON value (`true`, `30`, `["a"]`, `{"k":1}`). For keys whose value is a
  string the raw text is accepted too, so `aproxy settings set language zh-CN` needs no quotes; a
  value starting with `"` is read as a JSON string literal, which is how to write a newline. A value
  of `-` is read from one line of stdin, which keeps a key out of the shell history:
  `printf '%s\n' "$KEY" | aproxy settings set defaults.api_key -`.
- **Secrets are never printed back.** `show` and `get` replace a secret with
  `{"secret": true, "hint": "sk-a…f3a9"}` (the hint appears when the value is at least 24 characters)
  and mask URL credentials (`https://***@host/…?key=***`). Writing that placeholder object back
  keeps the stored value; a placeholder with no stored value to keep is refused.
- **Every write is checked before it lands.** A change that would introduce an error is refused and
  the file is left as it was; errors already in the file that this change does not touch do not block
  it, so a broken file can be repaired one key at a time. Writes take a lock shared with the other
  aproxy writers, keep every other key and its order, and create the file (with `"schema": 1` first)
  when it does not exist. The file is re-serialized as pretty JSON, so hand-made spacing and line
  breaks are not preserved. Two commands never lose each other's updates.
- `aproxy alias add|remove` and `aproxy settings set|unset default_config` edit only their own key
  through the same writer. They no longer rewrite the file wholesale, so keys they
  do not know are no longer dropped; a key this version does not know is an error until you remove it.
- **Hand editing is the fallback, and it is strict.** Read by hand only when the file cannot be parsed
  at all: a JSON syntax error, a file that is not a JSON object, or a `schema` newer than this
  version. In those cases no command can rewrite it, and `settings` says so instead of guessing.
  Otherwise prefer `aproxy settings set`, which stores the value in the shape this version expects.
  After any hand edit run `aproxy settings show` or `aproxy doctor`.
- Any key not documented here is an error (`config_unknown_key`), not a value that is ignored. That
  includes the flat field names of older releases; compatibility.md maps them to these keys.

The settings command's own exit codes: 0 stored, unchanged, or printed; 2 a malformed key path or a
value that is not valid JSON; 3 the change would make the file invalid, or the file cannot be read
(syntax error, not an object, newer schema); 1 the file could not be written (I/O or lock conflict).

## When settings.json is invalid

settings.json is read strictly, because a silently ignored key would leave a user believing a setting
is in effect. Unknown keys, wrong types, invalid values and a higher `schema` are errors. The policy
for a command that meets them depends on what the command is for:

| Command | Behavior with an invalid settings.json |
|---|---|
| `aproxy settings …`, `aproxy doctor` | Run normally: they are how the file is inspected and repaired. `doctor` lists the errors; `settings show` still prints the effective values, then the errors, and exits 3 |
| `aproxy status`, `aproxy stop`, `aproxy install` / `upgrade`, `aproxy skill …` | Run on built-in defaults and print the errors as `[WARN]` lines. Aliases are unavailable: address instances by port or `all`. Exit with the command's normal code |
| `aproxy migrate`, `aproxy install --force`, `aproxy desktop` | Dispatched before this check, so they print none of these lines. `migrate` reads the files itself, because converting them is its job; `install --force` never reads settings.json, so a broken file cannot block the repair; `desktop` opens the window, which reads the home on its own and shows the problem there |
| every other command: `start`, `restart`, `logs`, `find`, `restore`, `config`, `alias` | Print the errors as `[ERROR]` lines with repair hints and exit 3, without doing anything. A started instance never runs with settings it did not get |

The warning form ends with:

```
[WARN] Using built-in defaults for this command. Aliases are unavailable; select instances by port or all. Inspect with aproxy settings show and repair with aproxy settings unset <key>.
```

The error form adds a repair command for each bad key and one way out of the whole file:

```
settings.json has errors:
  <file>: watchdog: watchdog must be an object [config_type]
  <file>: max_body_mb: unknown key max_body_mb [config_unknown_key]

Remove the invalid keys below or correct their values with aproxy settings set <key> <value>:
  aproxy settings unset 'watchdog'
  aproxy settings unset 'max_body_mb'
To return to built-in defaults, delete the file. This also removes aliases and global defaults: <file>
```

Prefer the narrowest repair: `settings set` to correct a value, `settings unset` for a key that has
no place in this version. Before unsetting, look at what the key holds. A key under `defaults` or an
old 0.1.x key may carry a credential (an API key, a proxy password) the user still needs; do not
print its value, tell the user which key it is and ask where it belongs (`aproxy migrate` does this
for the 0.1.x keys). Deleting the whole file loses `aliases`, `default_config` and `defaults` along
with any credential in them: do it only when the user agrees, and first copy the file somewhere
private (only the user can read it) so it can be recovered.

- **A file that cannot be parsed at all** — a JSON syntax error, or content that is not a JSON object
  — can only be repaired by hand or deleted. `settings show` prints
  `settings.json is not a valid JSON object. Repair it manually or delete it to return to built-in defaults:`
  and exits 3, and every write
  command refuses and leaves the file untouched (never "rewrite it with the defaults", which would
  lose the aliases).
- A file that is valid JSON with bad keys is repaired one key at a time; `settings unset` accepts
  the unknown key, and each later edit is checked against the file as it stands.
- A `schema` from a newer aProxy is a separate case: no key can be fixed, and the error says to
  upgrade aproxy, or to go back to the version you need with `aproxy install --force <version>`. Run
  that command only after the user agrees: it stops every aProxy process of the home.
