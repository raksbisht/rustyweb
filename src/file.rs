use std::{
    io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use bytes::BytesMut;
use http::{HeaderValue, Method, header};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{
    Body, Error, IntoResponse, Middleware, Next, Redirect, Request, Response, path::decode,
};

const CHUNK_SIZE: usize = 64 * 1024;

/// A file to send as a response, streamed from disk. Like Express's
/// `res.sendFile()` and `res.download()`.
///
/// ```no_run
/// use rustyweb::{App, File};
///
/// let mut app = App::new();
/// app.get("/report", |_req| async { File::send("reports/latest.pdf").await });
/// app.get("/export", |_req| async { File::download("exports/data.csv").await });
/// ```
///
/// Responses include `Content-Type` (from the file extension),
/// `Content-Length`, `Last-Modified` and `ETag`, so browsers can cache them;
/// the app answers repeat requests with `304 Not Modified` automatically.
#[derive(Debug)]
pub struct File {
    file: tokio::fs::File,
    len: u64,
    modified: Option<SystemTime>,
    content_type: &'static str,
    disposition: Option<String>,
}

impl File {
    /// Opens `path` to send inline, so the browser displays it if it can.
    ///
    /// Fails with a `404` [`Error`] if the file doesn't exist or is a
    /// directory. The path is used as given, so don't build it from user
    /// input; use [`send_from`](File::send_from) for that.
    pub async fn send(path: impl AsRef<Path>) -> Result<File, Error> {
        let path = path.as_ref();
        let file = tokio::fs::File::open(path).await.map_err(open_error)?;
        let meta = file.metadata().await.map_err(open_error)?;
        if !meta.is_file() {
            return Err(Error::not_found("Not Found"));
        }
        Ok(File {
            file,
            len: meta.len(),
            modified: meta.modified().ok(),
            content_type: mime_type(path),
            disposition: None,
        })
    }

    /// Opens `path` to send as a download (`Content-Disposition:
    /// attachment`), named after the file. Use
    /// [`filename`](File::filename) to suggest a different name.
    pub async fn download(path: impl AsRef<Path>) -> Result<File, Error> {
        let name = path
            .as_ref()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let file = Self::send(path).await?;
        Ok(match name {
            Some(name) => file.filename(&name),
            None => file,
        })
    }

    /// Opens a file inside `root` named by an untrusted, URL-style path —
    /// for example a `*` route param. Fails with `404` for anything that
    /// would leave `root` (`..`), hidden files (names starting with `.`) and
    /// directories.
    ///
    /// ```no_run
    /// use rustyweb::{App, File};
    ///
    /// let mut app = App::new();
    /// app.get("/docs/*", |req| async move {
    ///     File::send_from("docs", req.param("*")).await
    /// });
    /// ```
    pub async fn send_from(root: impl AsRef<Path>, path: &str) -> Result<File, Error> {
        match safe_join(root.as_ref(), path) {
            Some(path) => Self::send(path).await,
            None => Err(Error::not_found("Not Found")),
        }
    }

    /// Sends the file as a download with the given file name.
    pub fn filename(mut self, name: &str) -> Self {
        self.disposition = Some(content_disposition(name));
        self
    }

    /// Overrides the `Content-Type` guessed from the file extension.
    pub fn content_type(mut self, content_type: &'static str) -> Self {
        self.content_type = content_type;
        self
    }
}

impl IntoResponse for File {
    fn into_response(self) -> Response {
        let File {
            file,
            len,
            modified,
            content_type,
            disposition,
        } = self;

        let slot = Arc::new(Mutex::new(Some(file)));
        let body = file_body(Source::Slot(slot.clone()), len);

        let mut res = Response::new(body);
        res.extensions_mut().insert(RangeSource { slot, len });
        let headers = res.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Some(modified) = modified {
            let since_epoch = modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            // Strong, so browsers can use it with `If-Range` to resume.
            let etag = format!("\"{len:x}-{:x}\"", since_epoch.as_secs());
            headers.insert(header::ETAG, HeaderValue::try_from(etag).unwrap());
            let date = httpdate::fmt_http_date(modified);
            headers.insert(header::LAST_MODIFIED, HeaderValue::try_from(date).unwrap());
        }
        if let Some(disposition) = disposition {
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::try_from(disposition).unwrap(),
            );
        }
        res
    }
}

/// Where a file body reads from: the shared slot (until the app takes the
/// file to serve a range), or a file it owns.
enum Source {
    Slot(Arc<Mutex<Option<tokio::fs::File>>>),
    Open(tokio::fs::File),
}

/// Streams up to `remaining` bytes from the file's current position.
fn file_body(source: Source, remaining: u64) -> Body {
    let chunks =
        futures_util::stream::try_unfold((source, remaining), |(source, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            let mut file = match source {
                Source::Open(file) => file,
                // Taken by a range request: this body is being replaced.
                Source::Slot(slot) => match slot.lock().unwrap().take() {
                    Some(file) => file,
                    None => return Ok(None),
                },
            };
            let want = remaining.min(CHUNK_SIZE as u64);
            let mut buf = BytesMut::with_capacity(want as usize);
            let read = (&mut file).take(want).read_buf(&mut buf).await?;
            if read == 0 {
                return Ok::<_, io::Error>(None); // the file got shorter
            }
            Ok(Some((
                buf.freeze(),
                (Source::Open(file), remaining - read as u64),
            )))
        });
    Body::from_stream(chunks)
}

/// Attached to file responses so the app can answer `Range` requests by
/// reading from the same open file.
#[derive(Clone)]
pub(crate) struct RangeSource {
    slot: Arc<Mutex<Option<tokio::fs::File>>>,
    pub(crate) len: u64,
}

impl RangeSource {
    /// A body for `len` bytes starting at `start`, or `None` if the file is
    /// no longer available.
    pub(crate) async fn body(&self, start: u64, len: u64) -> Option<Body> {
        let mut file = self.slot.lock().unwrap().take()?;
        file.seek(io::SeekFrom::Start(start)).await.ok()?;
        Some(file_body(Source::Open(file), len))
    }
}

/// Parses a `Range` header against a body of `len` bytes. `None` means
/// "ignore it and send everything" (missing, malformed or multiple ranges);
/// `Some(Err(()))` means the range can't be satisfied (416).
pub(crate) fn parse_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let (first, last) = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 || len == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(suffix), len - 1)
    } else {
        let first: u64 = start.parse().ok()?;
        let last = if end.is_empty() {
            u64::MAX
        } else {
            end.parse().ok()?
        };
        if last < first {
            return None;
        }
        if first >= len {
            return Some(Err(()));
        }
        (first, last.min(len - 1))
    };
    Some(Ok((first, last)))
}

/// Serves files from a folder, like Express's `express.static()`.
///
/// ```no_run
/// use rustyweb::{App, static_files};
///
/// let mut app = App::new();
/// app.middleware(static_files("public"));                    // /logo.png -> public/logo.png
/// app.middleware_at("/assets", static_files("assets"));      // /assets/app.js -> assets/app.js
/// ```
///
/// Only `GET` and `HEAD` requests are served. A request for a folder serves
/// its `index.html` (and `/docs` redirects to `/docs/`, so relative links
/// work). Requests that don't match a file — including hidden
/// files and anything trying to leave the folder with `..` — fall through to
/// the rest of the app, so routes and the 404 page still work.
///
/// Two things to know, both the same as in Express:
///
/// - **Symbolic links are followed**, even ones pointing outside the folder.
///   Only put links in it that you mean to publish.
/// - **Hidden files and folders are never served**, including `.well-known`.
///   If you need it (for example for Let's Encrypt), serve it from its own
///   folder with a route, using [`File::send_from`] so `..` stays blocked:
///   `app.get("/.well-known/*", |req| async move {
///   File::send_from("well-known", req.param("*")).await })`
pub fn static_files(root: impl Into<PathBuf>) -> StaticFiles {
    StaticFiles {
        root: root.into(),
        index: Some("index.html".to_owned()),
    }
}

/// Middleware returned by [`static_files`].
#[derive(Clone, Debug)]
pub struct StaticFiles {
    root: PathBuf,
    index: Option<String>,
}

impl StaticFiles {
    /// The file served for folder requests (default `index.html`). `None`
    /// disables it.
    pub fn index(mut self, index: Option<&str>) -> Self {
        self.index = index.map(str::to_owned);
        self
    }

    async fn find(&self, req: &Request) -> Option<Found> {
        let path = req.path().strip_prefix(req.base_path())?;
        let mut file = safe_join(&self.root, path)?;
        if tokio::fs::metadata(&file).await.ok()?.is_dir() {
            file.push(self.index.as_deref()?);
            // Like Express, send `/docs` to `/docs/` so relative links in the
            // index page resolve inside the folder.
            if !req.path().ends_with('/') {
                File::send(&file).await.ok()?;
                return Some(Found::Redirect(folder_location(req)));
            }
        }
        File::send(file).await.ok().map(Found::File)
    }
}

enum Found {
    File(File),
    Redirect(String),
}

/// `/docs?x=1` -> `/docs/?x=1`. Leading slashes are collapsed so a request
/// for `//evil.com` can't become a redirect to another site.
fn folder_location(req: &Request) -> String {
    let mut location = format!("/{}/", req.path().trim_start_matches('/'));
    if let Some(query) = req.uri().query() {
        location.push('?');
        location.push_str(query);
    }
    location
}

impl Middleware for StaticFiles {
    async fn call(&self, req: Request, next: Next) -> Response {
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return next.run(req).await;
        }
        match self.find(&req).await {
            Some(Found::File(file)) => file.into_response(),
            Some(Found::Redirect(location)) => Redirect::permanent(&location).into_response(),
            None => next.run(req).await,
        }
    }
}

/// Joins an untrusted URL path onto `root`, refusing anything that could
/// escape it or reveal hidden files.
fn safe_join(root: &Path, path: &str) -> Option<PathBuf> {
    let mut joined = root.to_path_buf();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let segment = decode(segment, false);
        // Decoding can produce separators (%2F, %5C) or NULs; reject those
        // and dot-segments so the result stays inside `root`.
        if segment.starts_with('.') || segment.contains(['/', '\\', '\0']) {
            return None;
        }
        let mut components = Path::new(&segment).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(part)), None) => joined.push(part),
            _ => return None,
        }
    }
    Some(joined)
}

fn open_error(err: io::Error) -> Error {
    match err.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => Error::not_found("Not Found"),
        _ => Error::internal("could not read file"),
    }
}

fn content_disposition(name: &str) -> String {
    let plain = name
        .chars()
        .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ');
    if plain {
        format!("attachment; filename=\"{name}\"")
    } else {
        let encoded: String = name
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        format!("attachment; filename*=UTF-8''{encoded}")
    }
}

/// Guesses a `Content-Type` from the file extension.
fn mime_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("wasm") => "application/wasm",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_stays_inside_root() {
        let root = Path::new("/srv/public");
        assert_eq!(safe_join(root, "/a/b.txt"), Some(root.join("a/b.txt")));
        assert_eq!(safe_join(root, "/"), Some(root.to_path_buf()));
        assert_eq!(
            safe_join(root, "/caf%C3%A9.txt"),
            Some(root.join("café.txt"))
        );
        for bad in [
            "/../etc/passwd",
            "/a/../../x",
            "/%2e%2e/x",
            "/a%2F..%2F..%2Fx",
            "/a%5C..%5Cx",
            "/.env",
            "/.git/config",
            "/a%00b",
        ] {
            assert_eq!(safe_join(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=900-", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-100", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-5000", 1000), Some(Ok((0, 999))));
        assert_eq!(parse_range("bytes=1000-", 1000), Some(Err(())));
        assert_eq!(parse_range("bytes=-0", 1000), Some(Err(())));
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=x-1", 1000), None);
        assert_eq!(parse_range("bytes=0-", 0), Some(Err(())));
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_type(Path::new("a.HTML")), "text/html; charset=utf-8");
        assert_eq!(mime_type(Path::new("a.woff2")), "font/woff2");
        assert_eq!(mime_type(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn dispositions() {
        assert_eq!(
            content_disposition("report 1.pdf"),
            "attachment; filename=\"report 1.pdf\""
        );
        assert_eq!(
            content_disposition("résumé.pdf"),
            "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"
        );
        assert_eq!(
            content_disposition("a\"b.txt"),
            "attachment; filename*=UTF-8''a%22b.txt"
        );
    }
}
