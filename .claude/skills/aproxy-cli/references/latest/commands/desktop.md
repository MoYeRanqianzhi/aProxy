# aproxy command reference: desktop

Part of [commands.md](../commands.md), which lists the other parts. Opening the desktop window: `desktop`.

## desktop

```
aproxy desktop
```

Opens the desktop window. It is a separate process that runs until its window is closed; opening
or closing it never affects running instances, and like every other `aproxy` command it resumes an
install that was interrupted (see install / upgrade). It opens on the user's screen, so run it only
when the user asks for the window.

The window shows this home's real instances, request history and configs, and lets the user start,
stop and restart instances, restart single format programs, edit configs, switch the watchdog and
start-at-login, and install updates. Everything it does goes through the same records and control
channel as the CLI, so `aproxy status` and the window agree; for your own work, use the CLI, which
you can read. Its preferences, the enabled extensions and their settings are kept in
`<home>/desktop/desktop.toml`.

**Themes and extensions** are installed from the window's settings page, from a local .zip or an
unpacked folder, into `<home>/desktop/extensions/`. An extension is JavaScript that runs inside the
window without a sandbox and can do anything the window can; before installing, the window explains
each risk that applies and asks the user to confirm each one. When the user asks whether a package
is safe, review it with the aproxy-audit-extension or aproxy-audit-theme skill rather than judging
from its description.

- **Windows, from a terminal:** the command returns at once with `已打开 aProxy 桌面界面（pid <pid>）。`
  ("desktop window opened") and the window runs on its own. Exit code 0 here only means the window
  process was started: anything that goes wrong after that is shown in a dialog box on the user's
  screen, never in your terminal, so ask the user whether the window appeared before you rely on
  it. Started from a shortcut, Explorer or a process without a console, its messages appear in a
  dialog box instead.
- **Linux and macOS:** the command stays in the foreground until the window is closed (on macOS
  the window has not been tried on real hardware). When you open the window for the user, start it
  in the background (for example `aproxy desktop &`, or your tool's background option); waiting for
  it would block you until the user closes the window.
- **One window per home.** A second `aproxy desktop` under the same `<home>` prints
  `aProxy 桌面界面已经在运行（pid <pid>）。这个版本还不会把它切到前台，请在任务栏里找到它。`
  ("already running; this version cannot bring it to the front, find it in the taskbar") and exits
  0. Under another `APROXY_HOME` it opens a separate window, with its own web view data in that
  home's `desktop/`.
- **When it cannot open**, it prints `[ERROR] …` on stderr and exits 1 without a window: no
  Microsoft Edge WebView2 runtime on Windows (Windows 11 ships it; Windows 10 may need it
  installed, and `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` pointing at the wrong directory has the
  same effect), or on Linux no graphical session (`DISPLAY` and `WAYLAND_DISPLAY` both unset, as
  in an ssh session). The proxy and every other command work without these, so a failing window
  is never a reason to touch the instances.
- **On Linux a desktop build needs the WebKitGTK 4.1 libraries to run at all.** Without them the
  system loader refuses to start the binary, so even `aproxy --version` fails, naming a missing
  library such as `libwebkit2gtk-4.1.so.0`. Such machines need the headless build; the install
  script and `aproxy install` pick it there on their own (install / upgrade, "Choosing the
  variant").
- **Headless builds have no window.** When `aproxy --version` says `headless`, `aproxy desktop`
  explains that and exits 1; the command is also missing from `aproxy --help` there. The way to
  the desktop build is `aproxy install --variant desktop`, a normal install that restarts the
  instances one at a time. musl builds come only as headless; on a glibc Linux the desktop build
  needs glibc 2.34 or later and WebKitGTK 2.40 or later (Debian and Ubuntu package it as
  `libwebkit2gtk-4.1-0`), and the install stops with an error when either is missing.
