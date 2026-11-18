//! Distribution (`api-v1.md` §4.12): updates, App Links and the installation page, served
//! read-only from `<data_dir>/public/`. No directory is listed, nothing leaves `public/`.

use std::io::SeekFrom;
use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, IF_NONE_MATCH, LAST_MODIFIED, RANGE, REFERRER_POLICY,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::Response;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Auth, Gate, Rl};
use crate::routes::blobs::{parse_range, Window};
use crate::state::AppState;

const CSP_PAGE: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'";

/// Content type by extension (§4.12).
pub fn content_type(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => "application/json",
        Some("apk") => "application/vnd.android.package-archive",
        Some("html") => "text/html; charset=utf-8",
        Some("sig") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// The relative path is acceptable: no `..`, no NUL, no backslash, no empty segment (a
/// directory), no segment starting with a dot.
pub fn safe_relative(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains("..")
        && !rel.contains('\0')
        && !rel.contains('\\')
        && rel
            .split('/')
            .all(|seg| !seg.is_empty() && !seg.starts_with('.'))
}

/// `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date(ms: i64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[usize::from(t.weekday().number_days_from_monday())],
        t.day(),
        MONTHS[usize::from(u8::from(t.month())) - 1],
        t.year(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// What to serve and how.
struct Target {
    file: PathBuf,
    cache: &'static str,
    page: bool,
}

/// Resolves `rel` under `root` and refuses anything that leaves it (symbolic links included).
fn resolve(root: &FsPath, rel: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let file = root.join(rel).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

async fn serve_file(st: &AppState, req: Request, target: Option<Target>) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpStatic],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, _body) = gate(st, req, g).await?;
    let target = target.ok_or_else(ApiError::not_found)?;
    let file = target.file;
    let meta = tokio::fs::metadata(&file)
        .await
        .map_err(|_| ApiError::not_found())?;
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    let etag = format!("\"{size:x}-{mtime_ms:x}\"");
    let mut resp = Response::new(Body::empty());
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(&http_date(mtime_ms)) {
        h.insert(LAST_MODIFIED, v);
    }
    h.insert(CACHE_CONTROL, HeaderValue::from_static(target.cache));
    h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if target.page {
        h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP_PAGE));
        h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    }

    // Conditional request.
    let header = |name: &str| {
        ctx.parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    if header(IF_NONE_MATCH.as_str())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"))
    {
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        return Ok(resp);
    }

    let window = match header(RANGE.as_str()) {
        Some(r) => parse_range(&r, size),
        None => Window::Full,
    };
    let (status, start, end) = match window {
        Window::Full => (StatusCode::OK, 0, size.saturating_sub(1)),
        Window::Partial(a, b) => (StatusCode::PARTIAL_CONTENT, a, b),
        Window::Unsatisfiable => {
            return Err(ApiError::new(Code::RangeNotSatisfiable)
                .detail("The Range header cannot be satisfied.")
                .header(CONTENT_RANGE, &format!("bytes */{size}")))
        }
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static(content_type(&file)));
    h.insert(CONTENT_LENGTH, HeaderValue::from(len));
    if status == StatusCode::PARTIAL_CONTENT {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
            h.insert(CONTENT_RANGE, v);
        }
    }
    if ctx.parts.method != Method::HEAD {
        let mut f = tokio::fs::File::open(&file)
            .await
            .map_err(|_| ApiError::not_found())?;
        f.seek(SeekFrom::Start(start)).await?;
        *resp.body_mut() = Body::from_stream(ReaderStream::with_capacity(f.take(len), 64 * 1024));
    }
    Ok(resp)
}

fn public_dir(st: &AppState) -> PathBuf {
    st.cfg.data_dir.join("public")
}

/// `GET /updates/{path}`.
pub async fn updates(
    State(st): State<AppState>,
    Path(rel): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let target = if safe_relative(&rel) {
        resolve(&public_dir(&st).join("updates"), &rel).map(|file| {
            let latest = rel.rsplit('/').next() == Some("latest.json");
            Target {
                file,
                cache: if latest {
                    "no-cache"
                } else {
                    "public, max-age=31536000, immutable"
                },
                page: false,
            }
        })
    } else {
        None
    };
    serve_file(&st, req, target).await
}

/// `GET /.well-known/assetlinks.json`.
pub async fn assetlinks(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let dir = public_dir(&st).join(".well-known");
    let target = resolve(&dir, "assetlinks.json").map(|file| Target {
        file,
        cache: "no-cache",
        page: false,
    });
    serve_file(&st, req, target).await
}

/// `GET /l/{anything}`: always the installation page. The fragment of a pairing link never
/// reaches the server, and the page runs no script.
pub async fn page(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let dir = public_dir(&st).join("l");
    let target = resolve(&dir, "index.html").map(|file| Target {
        file,
        cache: "no-cache",
        page: true,
    });
    serve_file(&st, req, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_paths_are_refused() {
        for bad in [
            "",
            "../config.toml",
            "a/../b",
            "a//b",
            "a/",
            ".hidden",
            "a/.b",
            "a\\b",
            "a\0b",
            "..",
            "android/",
        ] {
            assert!(!safe_relative(bad), "{bad:?}");
        }
        for ok in [
            "android/latest.json",
            "windows/x.sig",
            "cli/latest.json",
            "a-b_c.1/d",
        ] {
            assert!(safe_relative(ok), "{ok:?}");
        }
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(FsPath::new("a/latest.json")),
            "application/json"
        );
        assert_eq!(
            content_type(FsPath::new("x.apk")),
            "application/vnd.android.package-archive"
        );
        assert_eq!(
            content_type(FsPath::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(FsPath::new("x.sig")), "text/plain");
        assert_eq!(
            content_type(FsPath::new("x.exe")),
            "application/octet-stream"
        );
    }

    #[test]
    fn http_date_format() {
        assert_eq!(http_date(784_111_777_000), "Sun, 06 Nov 1994 08:49:37 GMT");
    }
}
