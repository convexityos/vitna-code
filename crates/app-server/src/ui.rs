//! The interface's own files, served as built (ADR-0006, 9).
//!
//! The page carries its Content-Security-Policy in a `<meta>` of its own,
//! written by its build: its own origin and openrouter.ai, nothing else. A
//! `<meta>` cannot forbid framing, so every reply here adds a header that
//! does, which keeps another site from putting the page in a frame and
//! steering a person's clicks on it.

use bytes::Bytes;
use http_body_util::Full;
use hyper::{header, Method, Response, StatusCode};
use std::fs;
use std::path::Path;

/// The largest file the interface is expected to hold.
const MAX_UI_FILE_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) type Reply = Response<Full<Bytes>>;

fn content_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn reply(status: StatusCode, kind: &str, body: Vec<u8>, head_only: bool) -> Reply {
    let length = body.len();
    let body = if head_only { Vec::new() } else { body };
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(kind)
            .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(header::CONTENT_LENGTH, header::HeaderValue::from(length));
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static("frame-ancestors 'none'"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        header::HeaderValue::from_static("same-origin"),
    );
    response
}

/// Anything but `GET` or `HEAD` outside the API.
pub(crate) fn not_allowed() -> Reply {
    let mut response = reply(
        StatusCode::METHOD_NOT_ALLOWED,
        "text/plain; charset=utf-8",
        b"The interface is read with GET or HEAD only.\n".to_vec(),
        false,
    );
    response
        .headers_mut()
        .insert(header::ALLOW, header::HeaderValue::from_static("GET, HEAD"));
    response
}

fn not_found(head_only: bool) -> Reply {
    reply(
        StatusCode::NOT_FOUND,
        "text/plain; charset=utf-8",
        b"Not found.\n".to_vec(),
        head_only,
    )
}

/// Shown when no interface was given: what this is, and how to get one.
fn missing_interface(folder: &str) -> Vec<u8> {
    let folder = folder
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Vitna</title>\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'\"></head>\
         <body><h1>vitna app is serving {folder}</h1>\
         <p>The Code interface is not installed beside this copy of vitna.</p>\
         <p>Build it in convexityos/vitna with <code>npm run chat:build -- --mode runner</code>, \
         then start <code>vitna app --ui &lt;that build's folder&gt;</code>.</p></body></html>\n"
    )
    .into_bytes()
}

/// Answers a `GET` or `HEAD` for the interface.
pub(crate) fn serve(ui: Option<&Path>, folder: &str, method: &Method, path: &str) -> Reply {
    let head_only = method == Method::HEAD;
    let Some(ui) = ui else {
        return if path == "/" {
            reply(
                StatusCode::OK,
                "text/html; charset=utf-8",
                missing_interface(folder),
                head_only,
            )
        } else {
            not_found(head_only)
        };
    };
    let relative = path.trim_start_matches('/');
    // Built names are plain: anything that could climb, escape or be decoded
    // into something else is not one of them.
    if relative.contains("..")
        || relative.contains('\\')
        || relative.contains('%')
        || relative.contains('\0')
        || relative.contains(':')
    {
        return not_found(head_only);
    }
    let wanted = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    match read(ui, wanted) {
        Some(bytes) => reply(StatusCode::OK, content_type(wanted), bytes, head_only),
        // A route of the page itself (no extension) gets the page, which
        // routes on its own; a missing asset stays missing.
        None if !wanted.rsplit('/').next().unwrap_or("").contains('.') => {
            match read(ui, "index.html") {
                Some(bytes) => reply(StatusCode::OK, "text/html; charset=utf-8", bytes, head_only),
                None => not_found(head_only),
            }
        }
        None => not_found(head_only),
    }
}

/// A regular file under `ui`, resolved on disk and still under it.
fn read(ui: &Path, wanted: &str) -> Option<Vec<u8>> {
    let real = fs::canonicalize(ui.join(wanted)).ok()?;
    if !real.starts_with(ui) {
        return None;
    }
    let meta = fs::metadata(&real).ok()?;
    if !meta.is_file() || meta.len() > MAX_UI_FILE_BYTES {
        return None;
    }
    fs::read(&real).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("assets/app-1a2b.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json"
        );
        assert_eq!(content_type("icon-192.PNG"), "image/png");
        assert_eq!(content_type("LICENSE"), "application/octet-stream");
    }

    #[test]
    fn the_missing_interface_page_escapes_the_folder_name() {
        let page = String::from_utf8(missing_interface("<b>&")).unwrap();
        assert!(page.contains("&lt;b&gt;&amp;"), "{page}");
        assert!(!page.contains("<b>&"), "{page}");
    }
}
