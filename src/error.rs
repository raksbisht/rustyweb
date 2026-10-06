use std::{borrow::Cow, fmt};

use http::StatusCode;

use crate::{IntoResponse, Response};

/// An HTTP error: a status code and a plain-text message.
///
/// Return `Result<T, Error>` from a handler to use `?`:
///
/// ```
/// use rustyweb::{Error, Request};
///
/// async fn handler(req: Request) -> Result<String, Error> {
///     let name = req.text()?;
///     if name.is_empty() {
///         return Err(Error::bad_request("name is required"));
///     }
///     Ok(format!("hello {name}"))
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Error {
    status: StatusCode,
    message: Cow<'static, str>,
}

impl Error {
    pub fn new(status: StatusCode, message: impl Into<Cow<'static, str>>) -> Self {
        Error {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}

impl std::error::Error for Error {}

/// The response carries a copy of the `Error` in its extensions, which is how
/// [`App::on_error`](crate::App::on_error) and middleware can recognise error
/// responses: `res.extensions().get::<rustyweb::Error>()`.
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let body = match &self.message {
            Cow::Borrowed(s) => s.into_response(),
            Cow::Owned(s) => s.clone().into_response(),
        };
        let mut res = (self.status, body).into_response();
        res.extensions_mut().insert(self);
        res
    }
}
