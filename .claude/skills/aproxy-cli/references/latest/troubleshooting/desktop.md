# Troubleshooting: desktop window and variants

Part of [troubleshooting.md](../troubleshooting.md), which lists the other parts. The desktop window does not open or looks wrong, or the wrong variant is installed.

## Desktop window and variants

`aproxy --version` says which build runs: `desktop` or `headless`, the first word in the
parentheses. A failing window never affects the proxy or the other commands (commands.md,
desktop).

| Symptom | Cause | Fix |
|---|---|---|
| `aproxy desktop` exits 1 with `This headless build of aproxy has no desktop interface. …` | The running binary is the headless build | Only if the user wants the window: `aproxy install --variant desktop`, a normal install that restarts instances one at a time. musl systems have no desktop build |
| `aproxy desktop` exits 1 with `[ERROR] Microsoft Edge WebView2 is required for the desktop interface but could not be found (…)` | Windows without the WebView2 runtime, or `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` names a directory that has none | Install the runtime (the message links the download), or unset the variable |
| `aproxy desktop` exits 1 with `[ERROR] No graphical session is available: DISPLAY and WAYLAND_DISPLAY are unset.` | Linux session without a display, such as ssh or a server | Run it in a desktop session, or do not run it: nothing else needs the window |
| `aproxy desktop` exits 1 with `[ERROR] Cannot read the WebKitGTK version or open the desktop interface (…)` | The web view library did not report a version: older than 2.40, or not working | Install WebKitGTK 4.1 (2.40 or later), or use the headless build |
| `aproxy desktop` prints `The aProxy desktop interface is already running (PID N). …` and exits 0 | One window per home; this version cannot bring it to the front | Nothing to fix; ask the user to find the window in the taskbar |
| On Linux every `aproxy` command, `--version` included, fails with `error while loading shared libraries: libwebkit2gtk-4.1.so.0` or ``version `GLIBC_2.34' not found`` (printed by the system loader, in English) | A desktop build on a machine without WebKitGTK 4.1, or with a glibc older than 2.34 | Install the libraries the loader names, or switch to the headless build: with the binary unusable, download the `aproxy-headless-<target>` asset from the Releases page and, once the user agrees, run `<it> install --force --from <it> --yes` (commands.md, Force install) |
| `aproxy install` prints `Installing the headless variant. …` | Without `--variant`, the downloaded desktop build did not run here, so the installer took the headless build of the same target | Expected on servers. To get the window later, install the libraries the earlier lines name and run `aproxy install --variant desktop` |
| `aproxy install` exits 1 with `The default variant includes the desktop interface. If this release has no desktop build for this system, use --variant headless. …` | No download channel delivered the desktop build: this release may not publish one for this machine, or the network failed briefly | Add `--variant headless` if the window is not needed. `aproxy install --force` reads the release manifest instead and installs headless only when the desktop build is really missing; it stops every aProxy process, so get the user's agreement first |
