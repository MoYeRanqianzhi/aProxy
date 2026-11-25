# aproxy command reference: install and upgrade

Part of [commands.md](../commands.md), which lists the other parts. Installing, upgrading, checking for an update, choosing the variant or CPU build, adopting a copy installed elsewhere (`--adopt`, PATH and the app menu entry).

## install / upgrade

```
aproxy install [latest|VERSION] [--pre] [--allow-downgrade] [--cpu x86-64-v3|baseline]
               [--variant desktop|headless] [--download-proxy URL] [--no-skills] [--json]
aproxy install --from PATH [--variant desktop|headless] [--no-skills] [--download-proxy URL] [--json]
aproxy install --adopt [--no-path] [--no-shortcut] [--no-skills] [--json]
aproxy install --check [--pre] [--variant desktop|headless] [--download-proxy URL] [--json]
aproxy install [latest|VERSION] --skills-only [--pre] [--download-proxy URL]
aproxy install --abort
aproxy install --force [latest|VERSION] [--pre] [--variant desktop|headless]
               [--cpu x86-64-v3|baseline] [--download-proxy URL] [--yes]
aproxy install --force --from PATH [--variant desktop|headless] [--yes]
```

`aproxy upgrade` is the same command. `install` downloads (or copies) the binary into
`<home>/install/staging/`, checks that it runs and reports the expected version, swaps it into
`<home>/bin/`, restarts running instances one at a time on the new binary, verifies that each
reports the new version, and cleans up; each instance is restarted once, one at a time. How the
swap and rollback work: behaviors.md, "Upgrades with aproxy install". Only `--adopt` (and the install
scripts' first install, which runs the same step) edits `PATH`; after any other install check that
`aproxy` resolves to `<home>/bin/` (`where aproxy`, `command -v aproxy`), or you keep running another
copy.
`--force` replaces the binary another way, for when this one fails: see
[Force install](force-install.md#force-install).

| Option | Effect |
|---|---|
| `VERSION` | a version without the `v`, e.g. `0.2.0`; default `latest` |
| `--pre` | resolve `latest` in the pre-release channel |
| `--allow-downgrade` | allow a VERSION lower than the running CLI's version, but never below 0.2.0. Not with `--force`, which installs a named VERSION regardless |
| `--cpu x86-64-v3\|baseline` | which x86-64 build to download; default `x86-64-v3` when the CPU has AVX2 and the target publishes one (x86_64 Windows, x86_64 Linux gnu and x86_64 macOS), baseline otherwise |
| `--download-proxy URL` | proxy for downloads only; default `update.download_proxy` in settings.json (not read by `--force`), then the environment's proxy settings. Unrelated to config.toml's `proxy` |
| `--no-skills` | skip the skill update this time (permanently: `update.skill_auto_update` in settings.json). Not with `--force`, which never updates skills |
| `--from PATH` | install a local binary; its `--version` output decides the version, with no downgrade check (below 0.2.0 is still refused) |
| `--adopt` | copy the binary you are running into `<home>/bin/`, move running instances onto it, then add `<home>/bin` to the user's `PATH` and, for the desktop build, an app menu entry that opens the window ([What --adopt sets up](#what---adopt-sets-up)) |
| `--no-path` | with `--adopt`: leave `PATH` alone |
| `--no-shortcut` | with `--adopt`: create no app menu entry |
| `--skills-only` | update only the skill documents, to VERSION or, by default, the channel's latest version (the running version if the channel is empty) |
| `--abort` | cancel an install that has not reached the swap |
| `--check` | only report which version `install` would install, and whether it is newer than the running one; downloads nothing and changes nothing ("Checking for an update" below). Combines only with `--pre`, `--variant`, `--download-proxy` and `--json` |
| `--json` | machine-readable output on stdout, human lines on stderr: one JSON object per line during an install, one document with `--check` ("Exit codes and `--json`" below). Not with `--abort` or `--skills-only` |
| `--force` | stop every aproxy process of this home and replace the binary without the hand-over ([Force install](force-install.md#force-install)). `--adopt`, `--abort`, `--skills-only`, `--no-skills`, `--allow-downgrade`, `--check` and `--json` cannot be combined with it: the command stops with a usage error (exit 2) and does nothing |
| `--yes` | with `--force`: do not ask for confirmation; required when stdin is not a terminal |
| `--variant desktop\|headless` | the build to install, with or without the desktop interface; default the build you run, so upgrades keep it ("Choosing the variant" below). With `--from` it is a check, not a choice: the file must be that build, or nothing is installed. Not with `--adopt`, `--abort` or `--skills-only` |

**Choosing the version.**

- `latest` is the highest version in a channel. The stable channel holds releases without a
  pre-release suffix that GitHub does not mark as pre-release; the pre channel holds everything,
  alpha, beta and rc included. The channel is stable when the running CLI is a stable release and
  pre when it is a pre-release; `--pre` forces pre. There is no `--stable`: to leave a pre-release
  for a stable release, name the version.
- Versions come from GitHub releases, or npm dist-tags when GitHub is unreachable. Only tags of the
  form `vX.Y.Z` or `vX.Y.Z-(alpha|beta|rc).N` with this platform's binary attached count.
- Nothing newer: `已是最新：当前 <v>，<channel> 通道最新为 <v>，无需安装。` ("already up to date"),
  exit 0, nothing restarted. Empty channel: `<channel> 通道暂无可用的 <variant> 版本，保持当前 <v> 不变。`
  ("no <variant> version in this channel; keeping the current one"), exit 0. With `--variant`
  naming the other build, neither message appears: the current version is reinstalled in that
  variant ("Choosing the variant" below).
- An explicit VERSION below the running CLI's version stops with
  `目标版本 <v> 低于当前 <v>。确认要降级请加 --allow-downgrade。` ("target below current; add
  --allow-downgrade to confirm").
- Nothing below 0.2.0 is installed, `--allow-downgrade` and `--from` included:
  `目标版本 <v> 低于 0.2.0，不能安装：0.1.x 与 0.2.0 起的版本互不兼容…` ("target below 0.2.0
  cannot be installed: 0.1.x and 0.2.0 or later are incompatible"). compatibility.md, "Going back to
  0.1.x", explains the way back. Likewise a 0.1.x `aproxy install` cannot reach 0.2.0; see
  "Moving from 0.1.x to 0.2.0" there.
- Downloads try the channels in `update.download_chain` (settings-json.md), by default github, npm,
  cargo-binstall, cargo. Each downloaded binary must run its `--version` on this machine and report
  the target version and variant. When it does not run: a desktop build is replaced by the headless
  build of the same target, unless you gave `--variant desktop`; on Linux a gnu build is then
  replaced by the musl build, which comes only as headless. A desktop build that no channel
  delivers is not replaced, because a release without that file and a passing network failure look
  the same to the channels: the install stops with exit 1 and, without `--variant`, suggests
  `--variant headless`, or `install --force`, which reads the release manifest and installs the
  headless build only when the desktop build is really missing.

**Checking for an update.** `aproxy install --check` asks the same sources in the same channel as
`install latest` and prints one of:

- `有新版本 <latest>（当前 <current>，<channel> 通道）。执行 aproxy install 升级。` ("version <latest>
  is available; run aproxy install to upgrade");
- `已是最新：当前 <current>，<channel> 通道最新为 <latest>。` ("up to date");
- `<channel> 通道暂无可用版本，当前 <current>。` ("no version in this channel").

In the stable channel the last two add `（如需预发布版本，可加 --pre）` ("add --pre for
pre-releases"). It exits 0 whenever the sources answered, update or not, and 1 when they did not
(`install_download_failed`; `--download-proxy` may help). It takes no lock and starts nothing, so it
is safe to run at any time. With `--json`:

```json
{"schema":1,"current":"0.2.0-alpha.3","latest":"0.2.0-alpha.4","update_available":true,"channel":"pre","variant":"desktop","published_at":"2026-11-25T08:00:00Z","url":"https://github.com/MoYeRanqianzhi/aProxy/releases/tag/v0.2.0-alpha.4"}
```

`latest` is null when the channel has no version; `variant` is the build `install` would install;
`published_at` and `url` are present when the answer came from GitHub.

**Choosing the variant.** Every platform has two builds with the same proxy and commands: `desktop`,
with the window of `aproxy desktop`, and `headless`, without it. The first word in the parentheses
of `aproxy --version` tells which one runs. Upgrades keep it; `--variant` switches. When nothing
newer is available (or the channel is empty), switching reinstalls the current version in the
other variant.

On Linux the desktop build links GTK and WebKitGTK and cannot even print `--version` without them.
aproxy does not inspect the machine beforehand; the downloaded binary's own `--version` decides,
because what a build needs changes from release to release. Without `--variant`, a desktop build
that does not run is replaced by the headless build of the same target, and the output says
`改装的是无界面的 headless 版本…` ("installing the headless build instead") with how to switch
back once the libraries are there. With `--variant desktop` nothing is replaced: the install stops,
exit 1, after the loader's error and, when the system loader refused the binary,
`带桌面界面的版本在本机起不来：动态加载器的报错见上…` ("the desktop build cannot start here; see
the loader's error above"). The loader's error names what is missing: a library such as
`libwebkit2gtk-4.1.so.0` (Debian and Ubuntu ship it in `libwebkit2gtk-4.1-0`), an undefined symbol
(the library is too old), or `GLIBC_2.xx not found` (glibc is too old, which only a newer system
fixes). Install what it names and run the command again, or use `--variant headless`.

**Which instances move.** Every running instance of the home is restarted on the new binary, so an
install first makes sure it can restart all of them, and refuses before it downloads, locks or
writes anything (exit 1) when it cannot:

- An instance started by an older version whose records this version cannot read (`status` lists it
  among the other aproxy processes):
  `[ERROR] 这个 home 里有旧版本启动、本次安装管不了的实例还在运行（pid <pids>）。…`
  ("instances started by an older version, which this install cannot manage, are running"), code
  `install_unmanaged_instances`. Stop them with the version that started them (its `aproxy stop all`),
  then install; or use `install --force`, which stops every aproxy process of the home.
  compatibility.md has the steps for the 0.2.0 alphas.
- A foreground instance (`start --foreground`), which has no record to restart it from:
  `[ERROR] 端口 <port> 上的实例在前台运行，安装无法替它重启。…` ("the instance on port <port> runs in
  the foreground; the install cannot restart it"), code `install_foreground_instances` with
  `details.ports`. Stop it (Ctrl+C in its terminal, `aproxy stop <port>`, or the service manager that
  runs it), install, then start it again the way it was started.
- Online installs and `--from` also refuse while a running instance's binary lives outside
  `<home>/bin/` (`实例 <port> 的二进制不在安装管辖目录（…）下`, "instance binary outside the managed
  directory"). Upgrade that copy through its own channel (npm, cargo, a package manager) or run
  `aproxy install --adopt`, which moves every running instance onto `<home>/bin/`.

Right before it touches the first instance, after the download, the install checks the first two
points again (an instance may have started meanwhile) and prechecks every instance it will restart:
the instance's start spec, resolved by the new version against the current settings.json. When an
instance fails it, nothing is changed and the command prints
`[ERROR] 安装前的预检没有通过：端口 <port> 的实例按新版本解析配置时出错。什么都没有改动：实例照常运行，二进制没换。`
("the precheck failed: the new version cannot resolve the configuration of the instance on port
<port>; nothing was changed"), the diagnostics, and `修正上面的问题后重新执行 aproxy install。` ("fix
the problems above and run aproxy install again"), exit 3, code `install_precheck_failed` with
`details.port` and `details.diagnostics`. An invalid settings.json is only a warning for an install
with no running instance, but fails this precheck when one runs. A failure at this point leaves
`<home>/install/state.json` marked as halted, which nothing resumes in the background; fix the cause
and run `aproxy install` again. An instance you stop while the install runs is skipped, not an error.

**Reading the result.**

- Exit 0 with `安装完成：<path>（二进制已落位，实例已滚动到新版本）` ("installed; binary in place,
  instances rolled to the new version") means every running instance was verified on the new
  version. Before it, `新版本进程已接手安装（广播、交换、滚动重启、终验），等待其完成...` ("the new
  version's process has taken over the install; waiting for it") appears: once the download is
  verified, the downloaded binary runs the rest in a separate process while the command waits (up
  to 600 s) and then reports. A downgrade is run by the command itself and skips this line.
  `<home>/install/state.json` may linger a moment afterwards while the old binary and the staging
  copy are deleted.
- While instances roll, `status` marks them `二进制更换中…`; do not stop or restart those.
- Skill documents are updated alongside into `<home>/skills/<name>/`; a skill failure does not
  fail the install. They are not linked into any agent's skill directory.
- `已有安装进行中（…）` ("an install is already running"): wait for it; a record whose installer died
  is taken over, a live one only after 600 s without progress.

**When it fails.** `[ERROR] 安装失败: …` is followed by one of two hints:

- `滚动已中止，不会自动重试…` ("rolling aborted, no automatic retry"): an instance did not start on
  the new version (`install_restart_failed`, with `details.port`). It was brought back on the old binary (if that also failed, its desired-state
  record was kept for `aproxy restore`), and the remaining instances were not touched. Usually the new
  version rejects the existing config: fix the reported cause and run `aproxy install` again, or
  stay on the old version with `aproxy install <old version> --allow-downgrade`. The new binary is
  already in `<home>/bin/` at this point, so the CLI and some instances run different versions
  until you finish.
- `现场已保留：重新执行同一条命令即可接着装完…` ("state kept; rerun the same command to finish"):
  run the same command again. When the error reads `目标版本没有接手安装（…）`
  ("the target version did not take over"), the downloaded binary exited or hung before starting;
  no instance and no file in `<home>/bin/` was changed.

An install interrupted by a crash, kill or power loss leaves `<home>/install/state.json`. The
watchdog resumes it in the background, and so does any command that changes instances or settings
(`start`, `stop`, `restart`, `restore`, `alias add|remove`, `config set|unset`, `settings set|unset`);
commands that only look (`status`, `logs`, `find`, `doctor`, `alias list`, `config show|get`,
`settings show|get`) never do, and nothing resumes a failure marked halted. Do not delete that file
by hand: it is what lets the install finish. `--abort` works only before the swap
(`安装已中止，现场已清理（二进制与实例未受影响）。`, "aborted; binary and instances untouched");
after it, the install can only be completed. When none of this gets the install through, use
[Force install](force-install.md#force-install).

**Exit codes and `--json`.** 0: installed, or nothing to do; 1: failed; 2: usage error; 3: the
precheck failed; 4: the requested version does not exist (`install_version_not_found`). Every failure
has an `install_*` code (`docs/error-codes.md`), for example `install_download_failed`,
`install_checksum_mismatch`, `install_probe_failed`, `install_swap_failed`, `install_rolled_back` (the
previous binary was put back), `install_restart_failed`, `install_in_progress`, `install_failed`.

With `--json` an install prints one JSON object per line on stdout, each with `"schema":1` and an
`event`:

```json
{"schema":1,"event":"plan","ports":[12346,12347]}
{"schema":1,"event":"phase","phase":"downloading","at":1763540000000}
{"schema":1,"event":"progress","phase":"downloading","bytes":4194304,"total":9437184}
{"schema":1,"event":"instance","port":12346,"outcome":"restarted"}
{"schema":1,"event":"result","outcome":"installed","version":"0.2.0-alpha.4"}
```

- `plan` comes first, once the target version is known and the checks above passed: the running
  instances the install will restart.
- `phase` marks each phase as it begins, in the order `marking`, `downloading`, `downloaded`,
  `broadcasting`, `acked`, `swapping`, `swapped`, `restarting`, `verifying`, `cleaning`; `at` is Unix
  milliseconds.
- `progress` reports the download, at most about ten times a second; `total` is null when unknown.
- `instance` gives each instance's outcome: `restarted`, `rolled_back` (back on the old binary),
  `failed`, or `skipped` (stopped during the install).
- `result` is the last line: `outcome` is `installed`, `up_to_date` or `failed`; a failure adds
  `"error":{"code","message","details"}`, and `version` is null when it failed before the target
  version was known. A failure that early, or a refusal by the checks above, prints only this line.

**If `aproxy` itself will not run.** On Windows the swap renames the old binary to a unique
`<home>/bin/aproxy.old-<nanoseconds>-<pid>.exe` before moving the new one in, and
`<home>/bin/aproxy.bat` runs the newest of those whenever `aproxy.exe` is missing, so the command
keeps working from cmd and PowerShell through the swap. On Linux and macOS the swap is a single
atomic rename, so there is no such gap. If no copy runs, force-install from a downloaded release
binary, as the end of [force-install.md](force-install.md) describes.

Do not replace the binary by hand with `stop all` followed by `restore`: a graceful stop deletes the
desired-state records, so `restore` finds nothing to start. Force install keeps those records and
runs `restore` itself.

### What --adopt sets up

`aproxy install --adopt` is the way to bring a copy installed elsewhere (npm, cargo, a package
manager, a manual download) under the home's management. After the binary is in place it makes
`aproxy` reachable and, for the desktop build, adds an entry that opens the window. Both steps are
per user, need no admin rights, and are idempotent: running adopt again changes nothing that is
already right. The install scripts run the same step at the end of a first install (skip `PATH` with
`--no-path` or `APROXY_NO_PATH=1` for install.sh, `-NoPath` or `APROXY_NO_PATH=1` for install.ps1).

| | `PATH` (skip with `--no-path`) | App menu entry (skip with `--no-shortcut`; desktop build only) |
|---|---|---|
| Windows | appends `<home>\bin` to the user's `Path` in `HKCU\Environment` (keeps `%VAR%` entries and the value type) and tells Explorer, so new terminals see it | Start menu shortcut `aProxy.lnk` running `<home>\bin\aproxy.exe desktop --installed-home`. On Windows 11 24H2 and later it opens no console window; older Windows shows one briefly |
| macOS | writes `<home>/env` and adds `. <home>/env # aProxy` to `~/.profile`, to `~/.bashrc` / `~/.bash_profile` when they exist, to `~/.zshenv`, and a `~/.config/fish/conf.d/aproxy.fish` when fish is installed | `~/Applications/aProxy.app`, whose executable links to `<home>/bin/aproxy`, so it keeps working after `aproxy install` replaces the binary |
| Linux | the same profile lines as macOS (`~/.zshenv` when zsh is the login shell or `~/.zshrc` exists) | `~/.local/share/applications/aproxy.desktop` (under `XDG_DATA_HOME` when set) and icons under `icons/hicolor/` |

When `<home>/bin` is already on `PATH` nothing is written. Terminals that are already open keep their
old `PATH`; open a new one. An entry always opens the home it was made for: for a home other than the
default, its name carries a short id (`aProxy-<id>`), so entries for several homes can coexist.

The binary install is the result that counts: a `PATH` or entry step that fails prints a warning with
the cause and the manual way, and does not change adopt's exit code. With `--json` the final event has
`integration.path` and `integration.shortcut`, each with `status` (`changed`, `unchanged`, `skipped`
or `failed`), `reason` and `touched` (the files or registry value written).

If a step keeps failing (a locked-down profile, an unusual shell, a sandbox), set it up by hand
instead of retrying: both are ordinary per-user settings, and the result only has to match the table
above. Read the `reason` first; it usually names the file or key that could not be written.

- `PATH`: add `<home>/bin` to the user's `PATH` the way that shell or system expects. On Windows that
  is the user-level `Path` environment variable (for example
  `[Environment]::SetEnvironmentVariable('Path', <old value> + ';<home>\bin', 'User')`, keeping
  existing entries); elsewhere a line such as `export PATH="<home>/bin:$PATH"` in the profile the
  user's login shell reads. Then check in a new terminal that `aproxy` resolves to `<home>/bin/`.
- App menu entry: anything that runs `<home>/bin/aproxy desktop --installed-home` works, because
  `--installed-home` makes the window use the home of the binary it runs from. On Windows a shortcut
  with that target and argument; on Linux a `.desktop` file with `Exec=` set to it; on macOS, if the
  `.app` cannot be created, `aproxy desktop` from a terminal opens the same window.

Tell the user what you changed, since these settings live outside aProxy's home.

There is no uninstall command yet. To undo by hand: remove `<home>\bin` from the user's `Path`
(Windows: Settings, "Edit environment variables for your account") or the lines ending in `# aProxy`
from the profile files and `<home>/env`; delete the shortcut, the `.app` or the `.desktop` file and its
icons.
