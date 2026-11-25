# aproxy command reference

Every `aproxy` subcommand: syntax, options, how targets are resolved, what the output means and the
exit codes. Read it when you need the exact form of a command or have to interpret what one printed.
What happens inside a running instance is in behaviors.md; symptom-driven fixes are in
troubleshooting.md.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `commands.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [commands/before-you-run.md](commands/before-you-run.md) | What to know before running any command: the home directory, isolated experiments, how targets are resolved, output and `--json`, exit codes, which processes exist. | [Before you run commands](commands/before-you-run.md#before-you-run-commands) · [Home directory](commands/before-you-run.md#home-directory) · [Experiment in an isolated home](commands/before-you-run.md#experiment-in-an-isolated-home) · [Targets](commands/before-you-run.md#targets) · [Output and exit codes](commands/before-you-run.md#output-and-exit-codes) · [Processes you may see](commands/before-you-run.md#processes-you-may-see) |
| [commands/start.md](commands/start.md) | Starting an instance: `start` and every option it takes. | [Options for start](commands/start.md#options-for-start) · [start](commands/start.md#start) |
| [commands/status-and-logs.md](commands/status-and-logs.md) | Reading what runs and what it logged: `status`, `logs`. | [status](commands/status-and-logs.md#status) · [logs](commands/status-and-logs.md#logs) |
| [commands/stop-restart-restore.md](commands/stop-restart-restore.md) | Stopping, restarting or bringing back instances: `stop`, `restart`, `restore`. | [stop](commands/stop-restart-restore.md#stop) · [restart](commands/stop-restart-restore.md#restart) · [Restarting only the format programs](commands/stop-restart-restore.md#restarting-only-the-format-programs) · [restore](commands/stop-restart-restore.md#restore) |
| [commands/alias-find-doctor.md](commands/alias-find-doctor.md) | Naming configs, locating them, and checking a setup: `alias`, `find`, `doctor`. | [alias](commands/alias-find-doctor.md#alias) · [find](commands/alias-find-doctor.md#find) · [doctor](commands/alias-find-doctor.md#doctor) |
| [commands/config-settings-migrate.md](commands/config-settings-migrate.md) | Reading or editing config.toml and settings.json from the command line, and converting 0.1.x files: `config`, `settings`, `migrate`. | [migrate](commands/config-settings-migrate.md#migrate) · [config](commands/config-settings-migrate.md#config) · [settings](commands/config-settings-migrate.md#settings) |
| [commands/desktop.md](commands/desktop.md) | Opening the desktop window: `desktop`. | [desktop](commands/desktop.md#desktop) |
| [commands/install.md](commands/install.md) | Installing, upgrading, checking for an update, choosing the variant or CPU build, adopting a copy installed elsewhere (`--adopt`, PATH and the app menu entry). | [install / upgrade](commands/install.md#install--upgrade) · [What --adopt sets up](commands/install.md#what---adopt-sets-up) |
| [commands/force-install.md](commands/force-install.md) | A normal install keeps failing or the installed binary no longer runs: `install --force`. | [Force install](commands/force-install.md#force-install) |
