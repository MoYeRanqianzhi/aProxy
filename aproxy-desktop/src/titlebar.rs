//! Windows custom title bar for a Tauri 2 `WebviewWindow`, keeping the real DWM caption buttons.
//!
//! What it does:
//! - Keeps the native frame (`decorations: true`), then removes the caption from the non-client area
//!   (`WM_NCCALCSIZE`), so the webview fills the whole window, including the title bar strip.
//! - Extends the DWM frame over the caption-button rectangle, so DWM keeps drawing the real
//!   minimize / maximize / close buttons, including hover, press, tooltips and Snap Layouts.
//! - Cuts a window-region hole in wry's `WRY_WEBVIEW` container over those buttons. Without the hole,
//!   the webview covers the buttons and swallows their mouse input.
//! - Adds a transparent child window along the top edge for native top / top-left / top-right resize,
//!   because the client area now starts at the very top of the window.
//!
//! Frontend contract: call `titlebar_insets` on load and on every `resize` event, pad the title bar
//! with `top` / `right`, and size it with `height`; call `set_caption_color` whenever the title bar
//! color changes. See references/frontend.md.
//!
//! Verified on Windows 11 25H2 (build 26200), 150% DPI, WebView2 153, with tauri 2.12.0,
//! wry 0.57.0, tao 0.37.1 and windows 0.62.2. Windows 10 was not tested.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Arbitrary id for our `SetWindowSubclass` entry; must not collide with wry's or Tauri's ids.
const SUBCLASS_ID: usize = 0x5449_544C;
/// Class of the transparent top-edge resize window.
const RESIZE_BAND_CLASS: PCWSTR = w!("TAURI_TITLEBAR_TOP_RESIZE");
/// wry's container HWND that parents the WebView2 controller (wry 0.57: `create_container_hwnd`).
const WRY_CONTAINER_CLASS: PCWSTR = w!("WRY_WEBVIEW");

/// Installs the title bar on `window`. Call once from `setup`, after the window exists.
pub fn install(window: &tauri::WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
    // `hwnd()` returns Tauri's `windows` HWND; rewrap the raw handle as this crate's HWND.
    let hwnd = HWND(window.hwnd()?.0);
    unsafe {
        // Fail loudly instead of silently shipping covered caption buttons if wry changes its container.
        FindWindowExW(Some(hwnd), None, WRY_CONTAINER_CLASS, None)
            .map_err(|_| "WRY_WEBVIEW container not found; check the wry version this module targets")?;

        let hinstance = GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).ok();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(resize_band_proc),
            hInstance: hinstance.unwrap_or_default(),
            lpszClassName: RESIZE_BAND_CLASS,
            ..Default::default()
        };
        // A second window registers the same class again; the failure is harmless.
        RegisterClassExW(&class);
        let band = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            RESIZE_BAND_CLASS,
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(hwnd),
            None,
            hinstance,
            None,
        )?;

        // The band HWND rides along as the subclass ref-data so `refresh` can reposition it.
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, band.0 as usize);
        // Re-run WM_NCCALCSIZE now so the caption disappears before the first paint.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        refresh(hwnd, band);
    }
    Ok(())
}

/// Title bar geometry in CSS (logical) pixels.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Insets {
    /// Rows hidden above the screen while maximized; pad the title bar top by this much. 0 when restored.
    top: f64,
    /// Visible caption-button height; the title bar row should be this tall (below `top`).
    height: f64,
    /// Width reserved on the right for the caption buttons; keep content out of it.
    right: f64,
}

#[tauri::command]
pub fn titlebar_insets(window: tauri::WebviewWindow) -> Result<Insets, String> {
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    unsafe {
        let scale = GetDpiForWindow(hwnd) as f64 / 96.0;
        let buttons = caption_buttons_client(hwnd);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let top = hidden_top(hwnd);
        Ok(Insets {
            top: top as f64 / scale,
            height: (buttons.bottom - top) as f64 / scale,
            right: (client.right - buttons.left) as f64 / scale,
        })
    }
}

/// Colors the native caption behind the buttons to match the web title bar: `color` is "#rrggbb",
/// `None` restores the system color. Windows 11 only (no effect on Windows 10).
///
/// DWM then picks contrasting glyph and hover colors by itself (white glyphs on a dark caption,
/// black on a light one) regardless of the window's dark-mode flag, so there is no need to call
/// `getCurrentWindow().setTheme()` for the buttons. Avoid doing so for that purpose: on Windows, Tauri
/// forwards a window theme change to WebView2's PreferredColorScheme, which also flips the page's
/// `prefers-color-scheme`.
#[tauri::command]
pub fn set_caption_color(window: tauri::WebviewWindow, color: Option<String>) -> Result<(), String> {
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    let value: u32 = match color {
        Some(c) => {
            let rgb = u32::from_str_radix(c.trim_start_matches('#'), 16).map_err(|e| e.to_string())?;
            // COLORREF is 0x00BBGGRR.
            ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
        }
        None => 0xFFFF_FFFF, // DWMWA_COLOR_DEFAULT
    };
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &value as *const _ as *const _, 4);
    }
    Ok(())
}

/// Height of the native top resize band, which equals the invisible side-border width
/// (window rect vs visible frame). Measured on a stock Win32 window: 9 px at 150%.
unsafe fn resize_band_height(hwnd: HWND) -> i32 {
    let mut window = RECT::default();
    let _ = GetWindowRect(hwnd, &mut window);
    let mut visible = RECT::default();
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut visible as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
    );
    (visible.left - window.left).max(1)
}

/// Client rows pushed above the monitor work area while maximized (Windows places a maximized
/// window's frame off-screen). Web content in those rows is invisible.
unsafe fn hidden_top(hwnd: HWND) -> i32 {
    if !IsZoomed(hwnd).as_bool() {
        return 0;
    }
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
    let mut origin = POINT::default();
    let _ = ClientToScreen(hwnd, &mut origin);
    (info.rcWork.top - origin.y).max(0)
}

/// DWM caption-button bounds, converted from window-relative to client coordinates.
unsafe fn caption_buttons_client(hwnd: HWND) -> RECT {
    let mut b = RECT::default();
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CAPTION_BUTTON_BOUNDS,
        &mut b as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
    );
    let mut window = RECT::default();
    let _ = GetWindowRect(hwnd, &mut window);
    let mut origin = POINT::default();
    let _ = ClientToScreen(hwnd, &mut origin);
    let (dx, dy) = (origin.x - window.left, origin.y - window.top);
    RECT { left: b.left - dx, top: b.top - dy, right: b.right - dx, bottom: b.bottom - dy }
}

/// Re-applies everything that depends on window size, DPI or maximized state.
unsafe fn refresh(hwnd: HWND, band: HWND) {
    let buttons = caption_buttons_client(hwnd);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    // DWM only renders its buttons inside the extended frame.
    let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: buttons.bottom.max(1), cyBottomHeight: 0 };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

    // Region = whole client minus the buttons. The region clips the WebView2 child windows too.
    if let Ok(container) = FindWindowExW(Some(hwnd), None, WRY_CONTAINER_CLASS, None) {
        let region = CreateRectRgn(0, 0, client.right, client.bottom);
        let hole = CreateRectRgn(buttons.left, buttons.top.min(0), buttons.right, buttons.bottom);
        CombineRgn(Some(region), Some(region), Some(hole), RGN_DIFF);
        let _ = DeleteObject(hole.into());
        // The system owns `region` after this call; do not delete it.
        SetWindowRgn(container, Some(region), true);
    }

    // A maximized window has no top resize edge.
    let flags = if IsZoomed(hwnd).as_bool() { SWP_HIDEWINDOW } else { SWP_SHOWWINDOW };
    let _ = SetWindowPos(band, Some(HWND_TOP), 0, 0, client.right, resize_band_height(hwnd), flags | SWP_NOACTIVATE);
}

/// Transparent top-edge window. It never paints, so the webview and the DWM buttons stay visible
/// under it, and forwards presses to the top-level window so the system runs its native size loop.
unsafe extern "system" fn resize_band_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            // Spans the full width, including above the caption buttons, like a native frame.
            let mut pt = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ScreenToClient(hwnd, &mut pt);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let corner = rc.bottom; // native corners are about as wide as the band is tall
            let hit = if pt.x < corner {
                HTTOPLEFT
            } else if pt.x >= rc.right - corner {
                HTTOPRIGHT
            } else {
                HTTOP
            };
            LRESULT(hit as isize)
        }
        // Resizing this child itself would be wrong; hand the gesture to the top-level window.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), msg, wparam, lparam);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    band: usize,
) -> LRESULT {
    // Every message goes to DWM first. DWM tracks the buttons' hover/press state from these messages;
    // calling it only for WM_NCHITTEST leaves a button highlighted after the mouse moves into the page.
    let mut result = LRESULT(0);
    if DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut result).as_bool() {
        return result;
    }
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 => {
            // Keep the system's left/right/bottom borders, but give the caption rows to the client.
            // Do this when maximized too: DWM stops hit-testing its buttons if the client top is moved
            // down to the screen edge. The off-screen rows are reported to the page as `Insets::top`.
            let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            params.rgrc[0].top = top;
            r
        }
        WM_NCHITTEST => {
            // Only the button hole reaches this window; the webview and the resize band cover the rest.
            // DwmDefWindowProc above already claimed the buttons, and the default proc handles the side
            // and bottom borders, so anything else here is caption.
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            if r.0 == HTCLIENT as isize {
                LRESULT(HTCAPTION as isize)
            } else {
                r
            }
        }
        WM_ERASEBKGND => {
            // The extended frame shows DWM's buttons only where this window's pixels are black (alpha 0).
            // Without this fill the hole shows an opaque white block and the buttons are invisible.
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            rc.bottom = caption_buttons_client(hwnd).bottom;
            FillRect(HDC(wparam.0 as *mut _), &rc, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            LRESULT(1)
        }
        WM_SIZE | WM_DPICHANGED => {
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            refresh(hwnd, HWND(band as *mut _));
            r
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}
