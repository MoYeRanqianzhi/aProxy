//! 沙箱只得到构建时已经核验的固定文档。包字节不是 URL 资源；宿主页继续使用原有响应策略。
use aproxy_ipc::extension_shell::{PATH, fingerprints};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::borrow::Cow;
use tauri::http::{HeaderValue, Request, Response, StatusCode, header};
const DOCUMENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/extension-frame.html"));
const HASHES: Option<([u8; 32], [u8; 32])> = include!(concat!(env!("OUT_DIR"), "/frame-hashes.rs"));
fn verified() -> Option<[u8; 32]> {
    let hashes = HASHES?;
    (fingerprints(DOCUMENT) == Some(hashes)).then_some(hashes.1)
}
/// 不复用 Tauri 对 HTML 重写后的字节或 SPA 回退；只有完全相同的固定文档配这份策略。
pub(super) fn respond(req: &Request<Vec<u8>>, out: &mut Response<Cow<'static, [u8]>>) {
    let hash = verified().filter(|_| {
        req.uri().path() == PATH
            && req.uri().query().is_none()
            && req.method() == tauri::http::Method::GET
    });
    let policy = match hash {
        Some(s) => format!(
            "sandbox allow-scripts; default-src 'none'; script-src 'sha256-{}' blob:; script-src-attr 'none'; style-src 'none'; img-src data: blob:; connect-src 'none'; font-src 'none'; media-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; webrtc 'block'",
            STANDARD.encode(s)
        ),
        None => "sandbox; default-src 'none'".into(),
    };
    *out.status_mut() = if hash.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    *out.body_mut() = Cow::Borrowed(if hash.is_some() { DOCUMENT } else { &[] });
    let h = out.headers_mut();
    h.clear();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&policy).expect("fixed ASCII policy"),
    );
    for (k, v) in [
        (
            "permissions-policy",
            "clipboard-read=(), clipboard-write=(), camera=(), microphone=(), geolocation=(), display-capture=(), fullscreen=(), payment=(), usb=(), serial=(), hid=()",
        ),
        ("referrer-policy", "no-referrer"),
        ("x-dns-prefetch-control", "off"),
        ("x-content-type-options", "nosniff"),
        ("cache-control", "no-store"),
    ] {
        h.insert(k, HeaderValue::from_static(v));
    }
}
#[cfg(windows)]
pub(super) use native::{install, trusted_top};
#[cfg(windows)]
mod native {
    use super::*;
    use crate::{MASCOT_SANDBOX, bridge::Bridge, is_app_url};
    use aproxy_ipc::desktop::ScriptSupport;
    use std::{cell::Cell, rc::Rc, sync::Arc};
    use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
    use windows::core::{Error, Interface, PWSTR, Result as ComResult};
    // 只有真实 WebView2 窗口通过主文档 IPC、子框架导航/能力拒绝及正常通道回归检查后才可打开。
    const WEBVIEW2_SCRIPT_QUALIFIED: bool = false;
    pub(crate) fn trusted_top(u: &tauri::Url) -> bool {
        is_app_url(u)
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && matches!(u.path(), "/" | "/index.html")
    }
    fn child_url(a: &ICoreWebView2NavigationStartingEventArgs) -> ComResult<String> {
        unsafe {
            let mut s = PWSTR::null();
            a.Uri(&mut s)?;
            Ok(take_pwstr(s))
        }
    }
    fn local_frame(s: &str) -> bool {
        tauri::Url::parse(s).is_ok_and(|u| {
            is_app_url(&u)
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && matches!(u.path(), PATH | MASCOT_SANDBOX)
        })
    }
    fn needed<T>(v: Option<T>) -> ComResult<T> {
        v.ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_POINTER))
    }
    /// 回调注册失败时窗口仍能显示；既不能宣告支持，也不将先前租约留到下一次轮询。
    pub(crate) fn install(w: &tauri::WebviewWindow, b: Arc<Bridge>) {
        let _ = w.with_webview(move |wv| {
            // 控件持有事件处理器；处理器只弱引用业务桥，不延长窗口或订阅的寿命。
            let weak = Arc::downgrade(&b);
            let ok = Rc::new(Cell::new(true));
            let live = ok.clone();
            let closed = Rc::new(move || {
                if live.replace(false)
                    && let Some(b) = weak.upgrade()
                {
                    b.qualify(ScriptSupport::Unverified)
                }
            });
            let result = unsafe {
                wv.controller()
                    .CoreWebView2()
                    .and_then(|web| register(&web, &wv.environment(), ok.clone(), closed.clone()))
            };
            if result.is_err() {
                closed();
            } else if ok.get() && verified().is_some() && WEBVIEW2_SCRIPT_QUALIFIED {
                b.qualify(ScriptSupport::Available);
            }
        });
    }
    // 全局先收窄所有子框架的地址；每个框架另记第一次固定文档的导航，连同一地址的再次导航也取消。
    fn register(
        web: &ICoreWebView2,
        env: &ICoreWebView2Environment,
        ok: Rc<Cell<bool>>,
        closed: Rc<dyn Fn()>,
    ) -> ComResult<()> {
        unsafe {
            let mut id = 0;
            // 导航通知不是网络拦截器。框架文档的资源请求还必须在交给网络之前用本地空响应结束；
            // 旧的 AddWebResourceRequestedFilter 不涵盖所有 iframe，不能用它降级。
            let web22: ICoreWebView2_22 = web.cast()?;
            web22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                windows::core::w!("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?;
            let blocked=env.CreateWebResourceResponse(None,403,windows::core::w!("Forbidden"),windows::core::w!("Cache-Control: no-store\r\nContent-Security-Policy: sandbox; default-src 'none'"))?;
            let fail = closed.clone();
            web.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    let res = (|| {
                        let a = needed(args)?;
                        let mut kind = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                        // 控件里的过滤器是合并的，Wry 自己注册了应用与 IPC 的资源；不能按文档规则截断其他上下文。
                        let known = a.ResourceContext(&mut kind).is_ok();
                        if known && kind != COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                            return Ok(());
                        }
                        let uri = a.Request().and_then(|r| {
                            let mut s = PWSTR::null();
                            r.Uri(&mut s)?;
                            Ok(take_pwstr(s))
                        });
                        if !known
                            || !uri.is_ok_and(|s| {
                                s == "about:blank"
                                    || local_frame(&s)
                                    || tauri::Url::parse(&s).is_ok_and(|u| trusted_top(&u))
                            })
                        {
                            a.SetResponse(&blocked)?
                        }
                        Ok(())
                    })();
                    if res.is_err() {
                        fail();
                    }
                    res
                })),
                &mut id,
            )?;
            let fail = closed.clone();
            let live = ok.clone();
            web.add_FrameNavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let res = (|| {
                        let a = needed(args)?;
                        let url = child_url(&a).unwrap_or_default();
                        if url != "about:blank"
                            && (!local_frame(&url)
                                || (!live.get()
                                    && tauri::Url::parse(&url).is_ok_and(|u| u.path() == PATH)))
                        {
                            a.SetCancel(true)?;
                        }
                        Ok(())
                    })();
                    if res.is_err() {
                        fail();
                    }
                    res
                })),
                &mut id,
            )?;
            let web4: ICoreWebView2_4 = web.cast()?;
            let fail = closed.clone();
            web4.add_FrameCreated(
                &FrameCreatedEventHandler::create(Box::new(move |_, args| {
                    let res = (|| {
                        let frame = needed(args)?.Frame()?;
                        let f2: ICoreWebView2Frame2 = frame.cast()?;
                        let f3: ICoreWebView2Frame3 = frame.cast()?;
                        let navigated = Cell::new(false);
                        let fail_nav = fail.clone();
                        let mut cookie = 0;
                        f2.add_NavigationStarting(
                            &FrameNavigationStartingEventHandler::create(Box::new(
                                move |_, args| {
                                    let res = (|| {
                                        let a = needed(args)?;
                                        let url = child_url(&a).unwrap_or_default();
                                        if navigated.get()
                                            || (!local_frame(&url) && url != "about:blank")
                                        {
                                            a.SetCancel(true)?;
                                        } else if url != "about:blank" {
                                            navigated.set(true)
                                        }
                                        Ok(())
                                    })();
                                    if res.is_err() {
                                        fail_nav();
                                    }
                                    res
                                },
                            )),
                            &mut cookie,
                        )?;
                        let fail_perm = fail.clone();
                        f3.add_PermissionRequested(
                            &FramePermissionRequestedEventHandler::create(Box::new(
                                move |_, args| {
                                    let res = (|| {
                                        let a = needed(args)?;
                                        a.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                                        a.SetHandled(true)
                                    })();
                                    if res.is_err() {
                                        fail_perm();
                                    }
                                    res
                                },
                            )),
                            &mut cookie,
                        )
                    })();
                    if res.is_err() {
                        fail();
                    }
                    res
                })),
                &mut id,
            )?;
            let fail = closed.clone();
            web.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, a| {
                    let r = needed(a).and_then(|v| v.SetHandled(true));
                    if r.is_err() {
                        fail();
                    }
                    r
                })),
                &mut id,
            )?;
            let fail = closed.clone();
            web4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, a| {
                    let r = needed(a).and_then(|v| v.SetCancel(true));
                    if r.is_err() {
                        fail();
                    }
                    r
                })),
                &mut id,
            )?;
            let fail = closed.clone();
            web.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(move |_, a| {
                    let r = needed(a).and_then(|v| v.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY));
                    if r.is_err() {
                        fail();
                    }
                    r
                })),
                &mut id,
            )?;
            let web18: ICoreWebView2_18 = web.cast()?;
            web18.add_LaunchingExternalUriScheme(
                &LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, a| {
                    let r = needed(a).and_then(|v| v.SetCancel(true));
                    if r.is_err() {
                        closed();
                    }
                    r
                })),
                &mut id,
            )?;
            Ok(())
        }
    }
}
