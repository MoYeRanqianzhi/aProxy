//! 包 URL 使用真正的层级路径，CSS 与模块的相对引用由浏览器解析；逐段解码，不能让转义额外造出一级目录。
use super::*;
use http::{Method, Request, Response, StatusCode, header};
fn mime(name: &str) -> &'static str {
    match name
        .rsplit_once('.')
        .map(|(_, s)| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
fn load(home: &Path, raw: &str, head: bool) -> Option<(&'static str, u64, Vec<u8>)> {
    let raw = raw.split(['?', '#']).next()?.strip_prefix('/')?;
    if raw.len() > 904 {
        return None;
    }
    let mut parts = Vec::new();
    for s in raw.split('/') {
        if s.as_bytes().iter().enumerate().any(|(i, &b)| {
            b == b'%'
                && !s
                    .as_bytes()
                    .get(i + 1..i + 3)
                    .is_some_and(|v| v.iter().all(u8::is_ascii_hexdigit))
        }) {
            return None;
        }
        let p = percent_encoding::percent_decode_str(s).decode_utf8().ok()?;
        if p.contains('/') {
            return None;
        }
        parts.push(p.into_owned());
    }
    let (id, files) = parts.split_first()?;
    if !preferences::package_id(id) || !valid_path(&files.join("/")) {
        return None;
    }
    let (name, parents) = files.split_last()?;
    let mut d = Dir::open(home).ok()?;
    for n in ["desktop", "extensions", id]
        .into_iter()
        .chain(parents.iter().map(String::as_str))
    {
        let Node::Dir(child) = d.node(n).ok()?? else {
            return None;
        };
        d = child;
    }
    let Node::File(f) = d.node(name).ok()?? else {
        return None;
    };
    let len = f.metadata().ok()?.len();
    if len > FILE {
        return None;
    }
    let b = if head {
        vec![]
    } else {
        read_bound(f, FILE, "file-bytes", None).ok()?
    };
    Some((mime(name), if head { len } else { b.len() as u64 }, b))
}
/// 主窗口与模块跨源读取包资源；错误不回显路径、系统诊断或上一次加载的正文。
pub fn respond(home: &Path, r: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let (status, content, len, b) = if r.method() != Method::GET && r.method() != Method::HEAD {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "application/octet-stream",
            0,
            vec![],
        )
    } else if let Some((ty, n, b)) = load(home, r.uri().path(), r.method() == Method::HEAD) {
        (StatusCode::OK, ty, n, b)
    } else {
        (StatusCode::NOT_FOUND, "application/octet-stream", 0, vec![])
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(b)
        .expect("fixed package headers are valid")
}
