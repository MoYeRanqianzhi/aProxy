# aproxy command reference: force install

Part of [commands.md](../commands.md), which lists the other parts. A normal install keeps failing or the installed binary no longer runs: `install --force`.

## Force install

`aproxy install --force` (0.2.0 and later) is the fallback for when a normal install keeps failing
or hangs, or when the installed version is too broken to run. It does not hand the install over to
the new version or restart instances one at a time. It:

1. gets the target and checks it on this machine before changing anything: online through the
   `aproxy-manifest.json` attached to the release (asset picked by platform, variant and CPU, then
   its sha256 checked), or the file given with `--from`; then it runs the target's `--version`.
   It only repairs an existing installation: when the home directory does not exist, usually a
   mistyped `APROXY_HOME`, it ends with 10 (use the install script for a first install);
2. asks for confirmation;
3. stops every process recorded in `<home>/ledger/`: installers first, then the watchdog, the
   instances and anything else. It checks each process's start time before stopping it, so a pid
   that now belongs to another program is never hit, and it never stops processes by name. It
   stops only processes of the user running it: one owned by another user is skipped and
   reported (`跳过 …（只结束当前用户的进程）`, "skipped (only the current user's processes are
   stopped)"), and keeps running the old binary;
4. empties `<home>/install/`, discarding any unfinished normal install (its progress record
   first);
5. replaces the binary in `<home>/bin/`;
6. runs the new binary's `aproxy restore`, which starts the stopped instances from their
   desired-state records, and checks the ledger for an instance of the new version on each of their
   ports;
7. lists processes that run from `<home>/bin/` but are not in the ledger, without stopping them
   (not on macOS, where the paths of other processes cannot be read).

**Before you run it.** It cuts every request in flight through aProxy at once, including your own
when your client points at one of these instances, and it stops instances the user may be using in
other sessions. Tell the user that and get their agreement first. The command lists the processes
it will stop and asks `确认强制安装？输入 y 继续 [y/N]:` ("confirm the force install? type y to
continue"). When stdin is not a terminal, as in an agent's tool call, it does not wait: it exits 10
and asks for `--yes`. Add `--yes` only once the user has agreed.

**Running it from a tool call.** Give the command a timeout of at least 10 minutes. A slow download
plus waiting for every instance to come back can take several minutes, and a tool timeout that
kills it halfway can leave the instances stopped and not restored; when your own client goes
through aProxy, your session drops with them, and you can no longer tell the user what happened.
Do not pipe it into `head` or anything else that stops reading early: the shell then reports the
exit code of the last command in the pipe instead of force install's, and you lose the lines that
say what happened. Let it run to the end and read its exit code; the table below says what each
one means.

**Choosing the target.** `latest` (the default) and VERSION follow the normal channel rules, `--pre`
included, but only releases that carry `aproxy-manifest.json` count, which means 0.2.0 and later.
A named VERSION is installed even when it is older than the running one, so downgrades need no
flag; nothing below 0.2.0 is installed. `latest`, however, never downgrades: when it resolves to a
version below the running one (a newer release that is missing its manifest), force install exits
10 and says so; name the version to install it anyway. Downloads use HTTPS only and give up after
60 s without data. `--variant` defaults to the variant of the binary you run, the first word in the
parentheses of `aproxy --version`. Like a normal install ("Choosing the variant" in [install.md](install.md)), it judges a
build only by running the downloaded binary's `--version`: without `--variant`, a desktop build that
does not run, or that the release does not publish for this platform at all, is replaced by the
headless build of the same target, then on Linux by the musl one, and the output says so before the
confirmation; with `--variant desktop` it exits 10 instead, before changing anything, and when the
system loader refused the binary it adds the same `带桌面界面的版本在本机起不来…` hint. Without
`--cpu`, the x86-64-v3 build is taken on CPUs with AVX2 when the release has one, and the baseline
build otherwise; an explicit `--cpu` must exist as named, for the headless build it falls back to
as well (the musl build is always baseline). When you named the variant or the CPU level and the
manifest has no such build for this platform, force install exits 10 and the error lists what the
platform does have, for example `这个目标只有 headless（baseline）` ("this target only has headless
(baseline)"): musl builds, for one, come only without the desktop interface. Rerun with a
combination from that list. Force install reads no config file and no settings.json, so pass
`--download-proxy URL` if downloads need a proxy. It does not update the skill documents; run
`aproxy install --skills-only` afterwards.

**Reading the result.**

| Exit | Meaning | Next step |
|---|---|---|
| 0 | done: binary replaced, every stopped instance back on its port (`强制安装完成`, "force install complete") | none; `aproxy install --skills-only` for matching skill documents |
| 10 | nothing was stopped: the home does not exist, the download, checksum or `--version` check failed, the target is below 0.2.0 or of the wrong variant, there was no confirmation (`什么都没有改动`, "nothing was changed"), or a later step failed before any process had been stopped (`一个进程都没有结束`, "no process was stopped") | fix what the `[ERROR]` line names and run it again |
| 11 | at least one process was stopped, but the binary was not replaced: another process holds the install lock or the binary file, a process could not be stopped (for example one running as administrator), or a file could not be written | fix the reported cause and run the same command again; every step can be repeated. To give up for now, `aproxy restore` starts the stopped instances on the old binary |
| 12 | the binary was replaced, but the instances listed after `二进制已换好，但下面这些被结束的实例没有回来` ("binary replaced, but these stopped instances did not come back") are down | each line gives the port and the command to start it, `aproxy start --config "<config path>"`; `<home>/logs/startup.log` has the reason, often a config file that was moved or deleted |

Two force installs on the same home stop each other: the later one ends the earlier, which then
exits with 1 (Windows) or 137 (killed by a signal on Linux and macOS) instead of one of the codes
above; read the later one's result.

Two more notes can appear. `看护者已被结束…` ("the watchdog was stopped") is expected: a restored
instance relaunches it within 5 minutes, and so does the next `aproxy start`.
`注意：下面这些进程从 <dir> 运行，但不在台账里…` ("these processes run from `<dir>` but are not in the
ledger") lists processes force install did not stop. They may be 0.1.x instances or watchdogs, which
keep no ledger, or other programs placed in `bin/`; find out which before acting. Stop 0.1.x ones
with the 0.1.x CLI (compatibility.md, "Moving from 0.1.x to 0.2.0").

**When the installed `aproxy` does not run at all.** Download the release asset for this platform
from https://github.com/MoYeRanqianzhi/aProxy/releases, check it against its `.sha256`, and, once the
user has agreed, run the downloaded file itself:
`<downloaded binary> install --force --from <downloaded binary> --yes`.
