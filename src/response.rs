use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::Body;

/// An outgoing HTTP response. Middleware can inspect and modify it freely
/// (`res.status()`, `res.headers_mut()`, ...).
pub type Response = http::Response<Body>;

/// Anything a handler can return.
///
/// | Type | Status | `Content-Type` |
/// |---|---|---|
/// | `()` | 200 | none |
/// | `&'static str`, `String` | 200 | `text/plain; charset=utf-8` |
/// | `&'static [u8]`, `Vec<u8>`, `Bytes` | 200 | `application/octet-stream` |
/// | [`Html<T>`] | 200 | `text/html; charset=utf-8` |
/// | `Json<T>` (feature `json`) | 200 | `application/json` |
/// | [`StatusCode`] | that status | none |
/// | [`Redirect`] | 302 / 301 | none |
/// | `(StatusCode, T)` | overrides `T`'s status | from `T` |
/// | `(StatusCode, HeaderMap, T)` | as above, plus extra headers | from `T` or the map |
/// | `Result<T, E>` | from `T` or `E` | from `T` or `E` |
/// | [`Response`] | as built | as built |
pub trait IntoResponse {
    fn into_response(self) -> Response;
}

fn with_type(body: impl Into<Bytes>, content_type: &'static str) -> Response {
    let mut res = Response::new(Body::from(body.into()));
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

const TEXT: &str = "text/plain; charset=utf-8";
const BINARY: &str = "application/octet-stream";

impl IntoResponse for Body {
    fn into_response(self) -> Response {
        Response::new(self)
    }
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::default()
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        with_type(self, TEXT)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        with_type(self, TEXT)
    }
}

impl IntoResponse for &'static [u8] {
    fn into_response(self) -> Response {
        with_type(self, BINARY)
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> Response {
        with_type(self, BINARY)
    }
}

impl IntoResponse for Bytes {
    fn into_response(self) -> Response {
        with_type(self, BINARY)
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        let mut res = Response::default();
        *res.status_mut() = self;
        res
    }
}

impl<T: IntoResponse> IntoResponse for (StatusCode, T) {
    fn into_response(self) -> Response {
        let mut res = self.1.into_response();
        *res.status_mut() = self.0;
        res
    }
}

impl<T: IntoResponse> IntoResponse for (StatusCode, HeaderMap, T) {
    fn into_response(self) -> Response {
        let mut res = (self.0, self.2).into_response();
        res.headers_mut().extend(self.1);
        res
    }
}

impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(v) => v.into_response(),
            Err(e) => e.into_response(),
        }
    }
}

/// An HTML response.
#[derive(Clone, Debug)]
pub struct Html<T>(pub T);

impl<T: Into<Bytes>> IntoResponse for Html<T> {
    fn into_response(self) -> Response {
        with_type(self.0, "text/html; charset=utf-8")
    }
}

/// A redirect response.
#[derive(Clone, Debug)]
pub struct Redirect {
    status: StatusCode,
    location: HeaderValue,
}

impl Redirect {
    /// `302 Found`, like Express's `res.redirect(url)`.
    ///
    /// # Panics
    ///
    /// Panics if `location` is not a valid header value.
    pub fn to(location: &str) -> Self {
        Self::with_status(StatusCode::FOUND, location)
    }

    /// `301 Moved Permanently`.
    ///
    /// # Panics
    ///
    /// Panics if `location` is not a valid header value.
    pub fn permanent(location: &str) -> Self {
        Self::with_status(StatusCode::MOVED_PERMANENTLY, location)
    }

    fn with_status(status: StatusCode, location: &str) -> Self {
        let location = HeaderValue::try_from(location).expect("invalid redirect location");
        Redirect { status, location }
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        let mut res = self.status.into_response();
        res.headers_mut().insert(header::LOCATION, self.location);
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn body(res: Response) -> Bytes {
        res.into_body().collect().await.unwrap().to_bytes()
    }

    #[tokio::test]
    async fn conversions() {
        let res = "hi".into_response();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], TEXT);
        assert_eq!(body(res).await, "hi");

        let res = (StatusCode::CREATED, String::from("made")).into_response();
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(body(res).await, "made");

        let mut headers = HeaderMap::new();
        headers.insert("x-a", HeaderValue::from_static("1"));
        let res = (StatusCode::ACCEPTED, headers, ()).into_response();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        assert_eq!(res.headers()["x-a"], "1");

        let res = Html("<p>").into_response();
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );

        let res = Redirect::to("/home").into_response();
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(res.headers()[header::LOCATION], "/home");

        let res = Err::<&str, _>(StatusCode::NOT_FOUND).into_response();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
