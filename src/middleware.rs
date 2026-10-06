use std::{fmt, future::Future, panic::AssertUnwindSafe, pin::Pin, sync::Arc};

use futures_util::FutureExt;

use crate::{Error, IntoResponse, Request, Response, router::Table};

pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = Response> + Send>>;
pub(crate) type BoxHandler = Arc<dyn Fn(Request) -> BoxFuture + Send + Sync>;
pub(crate) type BoxMiddleware = Arc<dyn Fn(Request, Next) -> BoxFuture + Send + Sync>;

pub(crate) fn box_handler<F, Fut>(handler: F) -> BoxHandler
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: IntoResponse,
{
    Arc::new(move |req| {
        let fut = handler(req);
        Box::pin(async move { fut.await.into_response() })
    })
}

/// Middleware: code that runs around a request, like an Express `app.use`
/// function.
///
/// Any `async fn(Request, Next) -> impl IntoResponse` is middleware, so most
/// of the time you never need this trait directly. Implement it when your
/// middleware has options, which is the usual shape for a reusable package:
///
/// ```
/// use rustyweb::{App, Middleware, Next, Request, Response};
///
/// /// Adds a fixed header to every response.
/// pub struct SetHeader {
///     pub name: &'static str,
///     pub value: &'static str,
/// }
///
/// impl Middleware for SetHeader {
///     async fn call(&self, req: Request, next: Next) -> Response {
///         let mut res = next.run(req).await;
///         res.headers_mut().insert(self.name, self.value.parse().unwrap());
///         res
///     }
/// }
///
/// let mut app = App::new();
/// app.middleware(SetHeader { name: "x-powered-by", value: "rustyweb" });
/// ```
///
/// A factory function that returns `impl Middleware` works too, like
/// `cors(options)` in Express:
///
/// ```
/// use rustyweb::{Middleware, Next, Request};
///
/// pub fn powered_by(name: &'static str) -> impl Middleware {
///     move |req: Request, next: Next| async move {
///         let mut res = next.run(req).await;
///         res.headers_mut().insert("x-powered-by", name.parse().unwrap());
///         res
///     }
/// }
/// ```
pub trait Middleware: Send + Sync + 'static {
    /// Handles `req`. Call `next.run(req).await` to continue to the rest of
    /// the chain, or return a response without calling it to stop here.
    fn call(&self, req: Request, next: Next) -> impl Future<Output = Response> + Send;
}

impl<F, Fut> Middleware for F
where
    F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: IntoResponse,
{
    fn call(&self, req: Request, next: Next) -> impl Future<Output = Response> + Send {
        let fut = self(req, next);
        async move { fut.await.into_response() }
    }
}

pub(crate) fn box_middleware<M: Middleware>(middleware: M) -> BoxMiddleware {
    let middleware = Arc::new(middleware);
    Arc::new(move |req, next| {
        let middleware = middleware.clone();
        Box::pin(async move { middleware.call(req, next).await })
    })
}

/// Runs `fut`, turning a panic into `500 Internal Server Error`.
pub(crate) async fn catch_panic(fut: BoxFuture) -> Response {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(res) => res,
        Err(_) => Error::internal("Internal Server Error").into_response(),
    }
}

#[derive(Clone)]
pub(crate) enum Endpoint {
    Handler(BoxHandler),
    Router(Arc<Table>),
}

/// The rest of the middleware chain, ending in the route handler.
///
/// Call [`run`](Next::run) to continue; return a response without calling it
/// to short-circuit.
#[derive(Clone)]
pub struct Next {
    stack: Arc<Vec<BoxMiddleware>>,
    index: usize,
    endpoint: Endpoint,
    /// Base path to restore when continuing past path-scoped middleware.
    restore_base: Option<String>,
}

impl Next {
    pub(crate) fn new(stack: Arc<Vec<BoxMiddleware>>, endpoint: Endpoint) -> Self {
        Next {
            stack,
            index: 0,
            endpoint,
            restore_base: None,
        }
    }

    /// Passes `req` to the next middleware, or to the handler if this was the
    /// last one, and returns its response.
    pub async fn run(mut self, mut req: Request) -> Response {
        if let Some(base) = self.restore_base.take() {
            req.replace_base_path(base);
        }
        if let Some(middleware) = self.stack.get(self.index).cloned() {
            self.index += 1;
            return middleware(req, self).await;
        }
        match self.endpoint {
            // Caught here so that middleware sees the 500 a panicking handler
            // produces (a logger, for example, can still record it).
            Endpoint::Handler(handler) => catch_panic(handler(req)).await,
            // Boxed to break the type cycle run -> dispatch -> run.
            Endpoint::Router(table) => Box::pin(crate::router::dispatch(table, req)).await,
        }
    }
}

/// Runs `inner` only for requests under `prefix` (see `App::middleware_at`).
pub(crate) struct Scoped<M> {
    pub(crate) prefix: crate::path::Pattern,
    pub(crate) inner: M,
}

impl<M: Middleware> Middleware for Scoped<M> {
    async fn call(&self, mut req: Request, mut next: Next) -> Response {
        let Some(len) = self.prefix.match_prefix(req.path()) else {
            return next.run(req).await;
        };
        let base = req.path()[..len].to_owned();
        let previous = req.replace_base_path(base);
        next.restore_base = Some(previous);
        self.inner.call(req, next).await
    }
}

impl fmt::Debug for Next {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Next")
            .field("remaining", &(self.stack.len() - self.index))
            .finish_non_exhaustive()
    }
}
