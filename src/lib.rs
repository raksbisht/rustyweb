//! A minimal, Express-style web framework: simple routing, simple middleware.
//!
//! ```no_run
//! use rustyweb::prelude::*;
//!
//! fn main() {
//!     let mut app = App::new();
//!     app.middleware(logger);
//!
//!     app.get("/", |_req| async { "Hello World!" });
//!     app.get("/users/:id", |req| async move {
//!         format!("user {}", req.param("id"))
//!     });
//!     app.get("/teapot", |_req| async { (StatusCode::IM_A_TEAPOT, "short and stout") });
//!
//!     app.listen(3000); // http://localhost:3000
//! }
//! ```
//!
//! - **Handlers** are `async` functions or closures taking a [`Request`] and
//!   returning anything that implements [`IntoResponse`].
//! - **Middleware** is an `async` function taking a [`Request`] and [`Next`].
//!   Call `next.run(req).await` to continue, or return a response directly to
//!   short-circuit. Code after `next.run` runs once the handler has responded.
//! - **Routes** match in registration order. `:name` captures a segment,
//!   a trailing `*` captures the rest of the path.

mod app;
mod body;
mod cookie;
mod error;
mod file;
#[cfg(feature = "json")]
mod json;
mod logger;
mod middleware;
mod negotiate;
mod path;
mod request;
mod response;
mod router;
mod server;
mod state;

pub use app::{App, DEFAULT_BODY_LIMIT, DEFAULT_BODY_TIMEOUT};
pub use body::Body;
pub use cookie::{Cookie, ResponseExt, SameSite};
pub use error::Error;
pub use file::{File, StaticFiles, static_files};
#[cfg(feature = "json")]
pub use json::Json;
pub use logger::logger;
pub use middleware::{Middleware, Next};
pub use request::Request;
pub use response::{Html, IntoResponse, Redirect, Response};
pub use router::{RouteChain, RouteHandle, Router};
/// Any JSON value. Read a body without declaring a struct:
/// `let body: Value = req.json()?;` then `body["name"]`.
#[cfg(feature = "json")]
pub use serde_json::Value;
/// Build JSON inline, like a JavaScript object literal:
/// `Json(json!({ "ok": true, "items": [1, 2, 3] }))`.
#[cfg(feature = "json")]
pub use serde_json::json;
pub use server::ListenAddr;

pub use http::{self, HeaderMap, Method, StatusCode, header};

/// `Result` with [`Error`] as the default error type, so handlers that can
/// fail read simply:
///
/// ```
/// use rustyweb::prelude::*;
///
/// async fn hello(req: Request) -> Result<String> {
///     let name = req.text()?; // not UTF-8? the client gets a 400
///     Ok(format!("hello {name}"))
/// }
/// ```
///
/// Other error types still work as usual: `Result<T, std::io::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Connection upgrades, the building block for WebSocket and other protocol
/// packages.
///
/// ```no_run
/// use rustyweb::{App, IntoResponse, Request, StatusCode, header};
/// use rustyweb::upgrade::TokioIo;
/// use tokio::io::{AsyncReadExt, AsyncWriteExt};
///
/// let mut app = App::new();
/// // An "echo" protocol: whatever the client sends comes back.
/// app.get("/echo", |mut req: Request| async move {
///     let Some(on_upgrade) = req.upgrade() else {
///         return StatusCode::BAD_REQUEST.into_response();
///     };
///     tokio::spawn(async move {
///         let Ok(upgraded) = on_upgrade.await else { return };
///         let mut io = TokioIo::new(upgraded);
///         let mut buf = [0; 1024];
///         while let Ok(n @ 1..) = io.read(&mut buf).await {
///             if io.write_all(&buf[..n]).await.is_err() {
///                 break;
///             }
///         }
///     });
///     let mut res = StatusCode::SWITCHING_PROTOCOLS.into_response();
///     res.headers_mut().insert(header::UPGRADE, "echo".parse().unwrap());
///     res.headers_mut().insert(header::CONNECTION, "upgrade".parse().unwrap());
///     res
/// });
/// ```
pub mod upgrade {
    pub use hyper::upgrade::{OnUpgrade, Upgraded};
    /// Wraps an [`Upgraded`] connection so it works with tokio's
    /// `AsyncRead`/`AsyncWrite` (and libraries like `tokio-tungstenite`).
    pub use hyper_util::rt::TokioIo;
}

/// Everything most apps need, in one import: `use rustyweb::prelude::*;`
pub mod prelude {
    pub use crate::{
        App, Cookie, Error, File, Html, IntoResponse, Middleware, Next, Redirect, Request,
        Response, ResponseExt, Result, Router, SameSite, StatusCode, logger, static_files,
    };
    #[cfg(feature = "json")]
    pub use crate::{Json, Value, json};
}
