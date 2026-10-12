//! aProxy 的桌面界面：Tauri 2 外壳 + 嵌入的前端。
//!
//! 由 aproxy 的 `desktop` 特性引入，`aproxy desktop` 在主线程上调用 [`run`]。本包不依赖
//! aproxy（aproxy 依赖本包，反过来会成环），home、tokio 运行时这些由调用方经 [`Options`]
//! 交进来；以后要改配置、拉起实例这类需要核心逻辑的操作，由本包定义接口、aproxy 实现后
//! 注入，读数据则经将来的 IPC 客户端。
//!
//! 前端在 `ui/`（Vite + Svelte 5 + TS），构建产物 `dist/` 由 `generate_context!` 在编译期
//! 嵌进二进制（dist 缺失时 build.rs 报错，见那里）。这一版界面里全是演示数据（前端的 mock
//! 后端），本包只管窗口：建窗口、自定义标题栏、Mica 支持与否、不联网的 CSP 与导航限制。以后
//! 真实数据接进来时，前端换一个 Backend 实现，这里加对应的命令。
//!
//! tauri.conf.json 写不了注释，其中的取舍记在这里：
//! - `build.windows.staticVCRuntime: false`：默认的 true 让 tauri-build 往自己的 OUT_DIR 写一个
//!   空的 msvcrt.lib，经 `rustc-link-search` 传给依赖者，配套的 `/NODEFAULTLIB` 却只作用于本包，
//!   依赖本包的 aproxy.exe 在 MSVC 上链接失败（tauri#16238）。aproxy 一直用动态 CRT，关掉与之一致。
//! - `app.windows` 为空：主窗口由 [`run`] 用 `WebviewWindowBuilder` 建，才能指定 webview 的数据
//!   目录、限制导航、单独放宽吉祥物沙箱页的 CSP。
//! - `app.security.csp` 不联网：脚本、样式、图片、字体只来自嵌入的资源（图片另许 `data:`），
//!   `connect-src` 只有 IPC（`ipc:`；WebView2 上是 `http://ipc.localhost`），`frame-src` 只许
//!   嵌入的沙箱页。`style-src` 不放行 `'unsafe-inline'`：页面不写 style 属性，Svelte 经 CSSOM
//!   设的样式不受它管。Tauri 给每个 HTML 响应附上这份 CSP，再加上该页内联脚本与样式的哈希。
//! - `withGlobalTauri: false`：页面用打包进来的 @tauri-apps/api，不需要 Tauri 往页面里另注入
//!   一份全局 API（`window.__TAURI__`）。
//! - 权限（capabilities/main.json）只授权页面实际用到的窗口接口与本包的三个命令，不含
//!   `core:default`。
//!
//! 进程与线程：事件循环必须占住主线程（macOS 硬性要求，Windows 与 Linux 默认也如此）。
//! [`run`] 在事件循环结束（最后一个窗口关掉）后返回退出码，不调用 `std::process::exit`，
//! 调用方据此收尾（删台账条目）。Tauri 的回调都跑在事件循环所在的主线程上，回调里不要
//! `block_on` 等异步任务，那会卡住整个界面。
//!
//! 失败要变成 [`Error`]，不能 panic：Tauri 在很多运行期失败上直接 panic（Linux 没有显示时
//! tao 在初始化 GTK 时 expect；setup 返回错误时 `panic!("Failed to setup app")`）。所以建窗口
//! 之前先 [`preflight`]，setup 里的失败记下来、请事件循环以非零退出码结束，图形后端初始化
//! 时的 panic 在 [`run`] 里接住。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::http::HeaderValue;
use tauri::utils::config::{Csp, CspDirectiveSources};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

// Windows 自定义标题栏：tauri-custom-titlebar skill 的 assets/windows_titlebar.rs 原样复制，
// 一行都不改（不改格式，也不按本仓库的习惯改成中文注释）。它依赖 wry 的私有窗口类
// WRY_WEBVIEW，每升级一次 tauri / wry 都要重跑那个 skill 的验证脚本，原样保留才能直接
// 与 skill 的新版本比对。rustfmt::skip 让 `cargo fmt` 不去重排它；edition 2024 默认警告
// unsafe fn 里不加 unsafe 块的调用，那是它按 edition 2021 写的写法，不是缺陷。
#[cfg(windows)]
#[rustfmt::skip]
#[allow(unsafe_op_in_unsafe_fn)]
mod titlebar;

/// 窗口 label：capabilities/main.json 按它授权，前端的 `getCurrentWindow()` 就是它。
const MAIN_WINDOW: &str = "main";

/// 窗口的初始与最小逻辑尺寸。初始尺寸放不进屏幕工作区时按工作区缩小（见 [`fit_to_monitor`]）。
const INITIAL_SIZE: (f64, f64) = (1200.0, 780.0);
const MIN_SIZE: (f64, f64) = (900.0, 600.0);

/// 窗口以 `visible: false` 创建，由页面在脚本跑起来之后调用 `show()`——这样启动时不会先
/// 闪一下白底（WebView2 挂上之前窗口是白的）。页面出了错（脚本异常、CSP 拦截、权限不对）
/// 就永远不会调用它，所以超过这个时限还没显示时由这里兜底显示。
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// 自定义吉祥物的沙箱页。它要把吉祥物源码转成 blob: URL 再 `import()`，所以只给这一页的
/// CSP 加上 `script-src blob:`；主界面的 CSP 不放行 blob 脚本。
const MASCOT_SANDBOX: &str = "/mascot-sandbox.html";

/// 调用方交给 [`run`] 的东西。
pub struct Options {
    /// webview 的数据目录（WebView2 的用户数据目录、WebKitGTK 的网站数据）。aproxy 给
    /// `<home>/desktop/webview`：不显式指定的话，Tauri 在 Windows 与 Linux 上会强制用
    /// app_local_data_dir（按 tauri.conf.json 的 identifier 建在用户的应用数据目录下），
    /// 跑到 APROXY_HOME 之外，测试与多份部署就隔离不了。
    pub data_dir: PathBuf,
    /// aproxy 建好的 tokio 运行时。Tauri 的异步任务跑在它上面，不再惰性地另建一个。
    pub runtime: tokio::runtime::Handle,
}

/// 桌面界面打不开的原因。`Display` 是给用户看的说明（暂为中文，界面文字的 i18n 在 W3）。
#[derive(Debug)]
pub enum Error {
    /// 找不到 webview 运行时：Windows 上是 WebView2，Linux 上是 WebKitGTK。附上底层的原因。
    NoWebview(String),
    /// Linux：没有图形会话（DISPLAY 与 WAYLAND_DISPLAY 都没设）。
    NoDisplay,
    /// 建应用或窗口失败（含图形后端初始化时的 panic）。附上底层的原因。
    Startup(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(windows)]
            Error::NoWebview(detail) => {
                write!(
                    f,
                    "没有找到 Microsoft Edge WebView2 运行时，桌面界面要靠它显示（{detail}）。\n\
                     Windows 11 自带它；Windows 10 可以到 https://developer.microsoft.com/microsoft-edge/webview2/ \
                     下载「常青版引导程序」安装，装好后再运行 aproxy desktop。"
                )?;
                // 这个环境变量会让 WebView2 只在它指向的目录里找运行时，指错了就像没装一样
                if let Some(dir) = std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER") {
                    write!(
                        f,
                        "\n环境变量 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER 设成了 {}，WebView2 只在那里找运行时；不需要的话删掉它。",
                        std::path::Path::new(&dir).display()
                    )?;
                }
                write!(f, "\n代理与命令行不需要它，照常可用。")
            }
            #[cfg(not(windows))]
            Error::NoWebview(detail) => write!(
                f,
                "读不到 WebKitGTK 的版本，打不开桌面界面（{detail}）。桌面界面需要 webkit2gtk-4.1（2.40 或更新）。\n\
                 代理与命令行不需要它，照常可用；没有图形界面的机器可以改装无界面的 headless 版本。"
            ),
            Error::NoDisplay => write!(
                f,
                "没有可用的图形界面：DISPLAY 与 WAYLAND_DISPLAY 都没有设置（例如在 ssh 会话或服务器上）。\n\
                 在桌面会话里运行 aproxy desktop；代理与命令行不需要图形界面，照常可用。"
            ),
            Error::Startup(detail) => write!(f, "桌面界面启动失败：{detail}"),
        }
    }
}

impl std::error::Error for Error {}

/// 建窗口之前的检查：缺了这些 Tauri 不会返回错误，而是 panic，所以先查一遍、给出能照着做
/// 的说明。
///
/// - Linux 先看有没有图形会话：没有的话 tao 初始化 GTK 时直接 panic。
/// - 再问 webview 的版本：Windows 上没装 WebView2（或 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER 指向
///   不存在的目录）时这里就报错；Linux 上 WebKitGTK 是动态链接的，缺库时进程在 main 之前就
///   起不来，走到这里多半能读到版本。macOS 的 WebKit 是系统框架。
pub fn preflight() -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        if !set("DISPLAY") && !set("WAYLAND_DISPLAY") {
            return Err(Error::NoDisplay);
        }
    }
    tauri::webview_version()
        .map(drop)
        .map_err(|e| Error::NoWebview(e.to_string()))
}

/// 打开桌面界面，阻塞到窗口关掉（事件循环结束），返回进程该用的退出码。必须在主线程上
/// 调用，整个进程只调用一次（Tauri 的异步运行时只能设一次）。调用之前先过 [`preflight`]。
pub fn run(options: Options) -> Result<i32, Error> {
    tauri::async_runtime::set(options.runtime);

    // setup 里出的错不能交还给 Tauri（它会 panic），记在这里，请事件循环退出后再报告
    let failure: Arc<Mutex<Option<Error>>> = Arc::default();
    let setup_failure = failure.clone();
    let data_dir = options.data_dir;

    let builder = tauri::Builder::default()
        .manage(Mutex::new(DesktopInfo::default()))
        .invoke_handler(tauri::generate_handler![
            desktop_info,
            #[cfg(windows)]
            titlebar::titlebar_insets,
            #[cfg(windows)]
            titlebar::set_caption_color,
        ])
        .setup(move |app| {
            if let Err(e) = open_main_window(app, data_dir) {
                *setup_failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                app.handle().exit(1);
            }
            Ok(())
        });

    // Linux 上 X 服务器连不上（DISPLAY 设了却无效）时，tao 初始化 GTK 会直接 panic。预检挡
    // 不住这种情形，在这里接住，换成一条说明。默认的 panic 钩子会先把「thread 'main'
    // panicked」打到终端，看起来像崩溃，所以建应用的这一小段换一个不出声的钩子。
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        builder.build(tauri::generate_context!())
    }));
    std::panic::set_hook(previous);
    let app = match built {
        Ok(Ok(app)) => app,
        Ok(Err(e)) => return Err(Error::Startup(e.to_string())),
        Err(panic) => return Err(Error::Startup(panic_message(&*panic))),
    };

    let code = app.run_return(|_, _| {});
    match failure.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(e) => Err(e),
        None => Ok(code),
    }
}

/// 弹出系统对话框（Windows）。从快捷方式启动时控制台已经释放，打不开界面的原因只能这样
/// 告诉用户。阻塞到用户关掉它。
#[cfg(windows)]
pub fn message_box(text: &str, error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("aProxy"));
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            None,
            windows::core::PCWSTR(text.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            MB_OK | icon,
        );
    }
}

/// 页面启动时经 `desktop_info` 命令取的窗口能力。前端据此决定用不用自定义标题栏的尺寸、
/// Mica 开关可不可用（见 ui/src/lib/shell.ts）。字段名按前端习惯用 camelCase。
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopInfo {
    /// Windows 自定义标题栏装上了（原生按钮留在右上角，页面画其余部分）。没装上（非
    /// Windows，或 install 失败降级）时页面的标题栏只是原生标题栏下面的一行工具栏。
    custom_titlebar: bool,
    mica: MicaSupport,
    /// Windows 的系统版本号（build），设置页说明 Mica 为什么不可用时要用；非 Windows 为 None
    windows_build: Option<u32>,
}

/// 这台电脑上 Mica 的情形，取值与前端 `MicaSupport` 一致。
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MicaSupport {
    Available,
    /// 不是 Windows，或者 Windows 版本没有文档化的背景材质接口（见 [`mica`]）
    #[default]
    Unsupported,
    /// 支持，但系统的「透明效果」关着：Mica 会显示为纯色
    TransparencyOff,
}

#[tauri::command]
fn desktop_info(state: tauri::State<'_, Mutex<DesktopInfo>>) -> DesktopInfo {
    state.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 建主窗口并装上标题栏、判定 Mica。窗口始终 `visible: false`，由页面调用 `show()`。
fn open_main_window(app: &tauri::App, data_dir: PathBuf) -> Result<(), Error> {
    let builder = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .title("aProxy")
        .inner_size(INITIAL_SIZE.0, INITIAL_SIZE.1)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
        .center()
        .visible(false)
        .data_directory(data_dir)
        // 界面只显示嵌入的页面：外部网址（扩展或页面里的链接试图把窗口带走）一律不导航
        .on_navigation(is_app_url)
        .on_web_resource_request(|request, response| {
            if request.uri().path() == MASCOT_SANDBOX {
                allow_blob_scripts(response);
            }
        });
    // Mica 要窗口透明、页面在需要透出的地方不画底色（tauri-windows-mica skill）。Mica 只有
    // Windows 有，别的平台不开透明，免得某些 Linux 合成器把窗口画成透明或黑底。
    #[cfg(windows)]
    let builder = builder.transparent(true);
    let window = builder.build().map_err(|e| Error::Startup(e.to_string()))?;

    fit_to_monitor(&window);

    #[cfg(windows)]
    let info = {
        // 装不上（wry 改了容器窗口类）就退回原生标题栏，不让界面因此打不开
        let custom_titlebar = match titlebar::install(&window) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "custom title bar unavailable; using the native one");
                false
            }
        };
        let (mica, windows_build) = mica::support(&window);
        DesktopInfo {
            custom_titlebar,
            mica,
            windows_build,
        }
    };
    #[cfg(not(windows))]
    let info = DesktopInfo::default();
    *app.state::<Mutex<DesktopInfo>>()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = info;

    let fallback = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_FALLBACK);
        // 窗口已经关掉（这时调用返回错误）或页面已经显示过，都什么也不做
        if fallback.is_visible().is_ok_and(|visible| !visible) {
            tracing::warn!("the page did not show the window in time; showing it anyway");
            let _ = fallback.show();
        }
    });
    Ok(())
}

/// 初始尺寸放不进屏幕工作区时（1366×768 的笔记本、150% 缩放的 1080p 屏）缩到工作区的九成
/// 并重新居中，不让窗口的下半截落到任务栏后面、屏幕外面。
fn fit_to_monitor(window: &WebviewWindow) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area().size.to_logical::<f64>(scale);
    let (width, height) = (
        INITIAL_SIZE.0.min(area.width * 0.9).max(MIN_SIZE.0),
        INITIAL_SIZE.1.min(area.height * 0.9).max(MIN_SIZE.1),
    );
    if (width, height) != INITIAL_SIZE {
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        let _ = window.center();
    }
}

/// 窗口只许停在嵌入的页面上。嵌入页面的地址：Windows 是 `http://tauri.localhost`（WebView2
/// 用 http 冒充自定义协议），其余平台是 `tauri://localhost`。
fn is_app_url(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

/// 给一个 HTML 响应的 CSP 加上 `script-src blob:`（只用于吉祥物沙箱页）。Tauri 已经按
/// tauri.conf.json 的 CSP 加上了这一页自己的哈希与 nonce，这里在它的基础上追加。
fn allow_blob_scripts(response: &mut tauri::http::Response<std::borrow::Cow<'static, [u8]>>) {
    let Some(header) = response.headers_mut().get_mut("Content-Security-Policy") else {
        return;
    };
    let Ok(policy) = header.to_str() else {
        return;
    };
    let mut csp: std::collections::HashMap<String, CspDirectiveSources> =
        Csp::Policy(policy.to_string()).into();
    csp.entry("script-src".to_string())
        .or_default()
        .push("blob:");
    if let Ok(value) = HeaderValue::from_str(&Csp::DirectiveMap(csp).to_string()) {
        *header = value;
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "图形界面初始化失败".to_string())
}

/// Windows 11 的 Mica 背景材质：判定支持与否（切换由页面经 Tauri 的窗口接口做）。
#[cfg(windows)]
mod mica {
    use super::MicaSupport;
    use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
    use windows::Win32::Graphics::Dwm::{
        DWMSBT_AUTO, DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute,
    };
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;

    /// 文档化的背景材质接口（DWMWA_SYSTEMBACKDROP_TYPE）从这个 build 起才有。22000–22522 的
    /// Windows 11 只有未文档化的属性，window-vibrancy 会改用它，但 tauri-windows-mica skill
    /// 没在这些版本上验证过，按不支持处理。
    const DOCUMENTED_BACKDROP_BUILD: u32 = 22523;

    /// 判定这台电脑上 Mica 的情形，同时返回系统 build（设置页的说明要用）。
    ///
    /// Tauri 应用窗口效果时会丢掉 window-vibrancy 返回的错误，页面经 `setEffects` 打开 Mica
    /// 时成没成功无从知道，所以这里直接调用一次 window-vibrancy（与 Tauri 用的是同一个版本，
    /// 见 Cargo.toml）拿到结果。窗口此时还没显示，试完立刻把背景材质恢复成新窗口的默认值
    /// DWMSBT_AUTO：Mica 默认关着，由用户在设置里打开。
    pub(super) fn support(window: &tauri::WebviewWindow) -> (MicaSupport, Option<u32>) {
        let build = windows_version::OsVersion::current().build;
        if build < DOCUMENTED_BACKDROP_BUILD {
            return (MicaSupport::Unsupported, Some(build));
        }
        let applied = window_vibrancy::apply_mica(window, None);
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                let _ = DwmSetWindowAttribute(
                    HWND(hwnd.0),
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &DWMSBT_AUTO as *const _ as *const _,
                    std::mem::size_of_val(&DWMSBT_AUTO) as u32,
                );
            }
        }
        if let Err(e) = applied {
            tracing::warn!(error = %e, build, "Mica is not available on this window");
            return (MicaSupport::Unsupported, Some(build));
        }
        let support = if transparency_off() {
            MicaSupport::TransparencyOff
        } else {
            MicaSupport::Available
        };
        (support, Some(build))
    }

    /// 系统设置 › 个性化 › 颜色 › 透明效果关着：这时 Windows 把 Mica 画成纯色（浅色
    /// #f3f3f3、深色 #202020）。读不到这个值（从没改过的系统没有它）按开着算。
    fn transparency_off() -> bool {
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("EnableTransparency"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        status == ERROR_SUCCESS && value == 0
    }
}
