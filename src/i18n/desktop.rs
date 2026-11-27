//! 桌面启动器的终端与对话框共用这些文字，界面内部由前端自行选择语言。

use aproxy_ipc::message;

#[cfg(any(test, not(feature = "desktop")))]
message!(musl(), en: "This headless build of aproxy has no desktop interface. Only headless builds are available for musl. On a glibc Linux distribution, install the GNU desktop build.", zh: "这是无界面的 headless 版本的 aproxy，没有桌面界面。musl 构建只发布 headless 版本；\
         在 glibc 的 Linux 发行版上，可以改装 gnu 构建的带界面版本。");

#[cfg(any(test, not(feature = "desktop")))]
message!(linux(), en: "This headless build of aproxy has no desktop interface. To install the desktop build, run:\n  aproxy install --variant desktop\nAs with an upgrade, instances restart one at a time. The desktop build requires WebKitGTK 2.40 or later (libwebkit2gtk-4.1-0 on Debian and Ubuntu) and a graphical session to open a window. The installer reports why if this machine cannot run the desktop build.", zh: "这是无界面的 headless 版本的 aproxy，没有桌面界面。要用桌面界面，换装带界面的版本：\n  \
         aproxy install --variant desktop\n\
         它和升级一样逐个重启实例。带界面的版本需要 WebKitGTK 2.40 或更新\
         （Debian / Ubuntu：libwebkit2gtk-4.1-0），打开窗口还要图形会话；本机跑不了带界面的版本\
         时，安装会报错并说明原因。");

#[cfg(any(test, not(feature = "desktop")))]
message!(headless(), en: "This headless build of aproxy has no desktop interface. To install the desktop build, run:\n  aproxy install --variant desktop\nAs with an upgrade, instances restart one at a time.", zh: "这是无界面的 headless 版本的 aproxy，没有桌面界面。要用桌面界面，换装带界面的版本：\n  \
         aproxy install --variant desktop\n\
         它和升级一样逐个重启实例。");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(panic(info: &std::panic::PanicHookInfo<'_>), en: "An internal error closed the desktop interface. Proxy instances are unaffected.\n\n{info}", zh: "桌面界面遇到内部错误，即将关闭（代理实例不受影响）。\n\n{info}");

#[cfg(all(feature = "desktop", not(test)))]
message!(running(pid: u32), en: "The aProxy desktop interface is already running (PID {pid}). Select its window on the taskbar; this version cannot bring it to the foreground.", zh: "aProxy 桌面界面已经在运行（pid {pid}）。这个版本还不会把它切到前台，请在任务栏里找到它。");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(opened(pid: u32), en: "Opened the aProxy desktop interface (PID {pid}).", zh: "已打开 aProxy 桌面界面（pid {pid}）。");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(detach(e: &std::io::Error), en: "Cannot open the desktop interface in the background: {e}", zh: "无法在后台打开桌面界面：{e}");

#[cfg(all(feature = "desktop", not(test)))]
message!(runtime(e: &std::io::Error), en: "Cannot create the Tokio runtime: {e}", zh: "无法建立 tokio 运行时：{e}");

#[cfg(all(feature = "desktop", not(test)))]
message!(executable(), en: "Cannot locate the desktop executable", zh: "无法定位桌面程序");

#[cfg(all(feature = "desktop", not(test)))]
message!(home(), en: "Cannot initialize the desktop home directory", zh: "无法初始化桌面主目录");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(webview(detail: &str), en: "Microsoft Edge WebView2 is required for the desktop interface but could not be found ({detail}).\nWindows 11 includes it. On Windows 10, install the Evergreen Bootstrapper from https://developer.microsoft.com/microsoft-edge/webview2/ and run aproxy desktop again.", zh: "没有找到 Microsoft Edge WebView2 运行时，桌面界面要靠它显示（{detail}）。\n\
                     Windows 11 自带它；Windows 10 可以到 https://developer.microsoft.com/microsoft-edge/webview2/ \
                     下载「常青版引导程序」安装，装好后再运行 aproxy desktop。");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(webview_dir(dir: std::path::Display<'_>), en: "\nWEBVIEW2_BROWSER_EXECUTABLE_FOLDER is set to {dir}. WebView2 checks only that directory. Unset the variable if it is not needed.", zh: "\n环境变量 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER 设成了 {dir}，WebView2 只在那里找运行时；不需要的话删掉它。");

#[cfg(all(feature = "desktop", not(test), windows))]
message!(cli_available(), en: "\nThe proxy and command line remain available.", zh: "\n代理与命令行不需要它，照常可用。");

#[cfg(all(feature = "desktop", not(test), not(windows)))]
message!(webkit(detail: &str), en: "Cannot read the WebKitGTK version or open the desktop interface ({detail}). Install webkit2gtk-4.1 version 2.40 or later.\nThe proxy and command line remain available. Machines without a graphical session can use the headless build.", zh: "读不到 WebKitGTK 的版本，打不开桌面界面（{detail}）。桌面界面需要 webkit2gtk-4.1（2.40 或更新）。\n\
                 代理与命令行不需要它，照常可用；没有图形界面的机器可以改装无界面的 headless 版本。");

#[cfg(all(feature = "desktop", not(test)))]
message!(no_display(), en: "No graphical session is available: DISPLAY and WAYLAND_DISPLAY are unset.\nRun aproxy desktop from a desktop session. The proxy and command line do not require a graphical session.", zh: "没有可用的图形界面：DISPLAY 与 WAYLAND_DISPLAY 都没有设置（例如在 ssh 会话或服务器上）。\n\
                 在桌面会话里运行 aproxy desktop；代理与命令行不需要图形界面，照常可用。");

#[cfg(all(feature = "desktop", not(test)))]
message!(startup(detail: &str), en: "Desktop startup failed: {detail}", zh: "桌面界面启动失败：{detail}");
