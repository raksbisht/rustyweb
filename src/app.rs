use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use http::{HeaderValue, Method, StatusCode, header};
use hyper::body::Body as _;

use crate::{
    Body, Error, IntoResponse, Middleware, Next, Request, Response, Router,
    file::{RangeSource, parse_range},
    middleware::{BoxMiddleware, Endpoint, Scoped, box_handler, box_middleware, catch_panic},
    path::Pattern,
    router::{AddRoute, Route, Table, mounted, route_methods},
    state::State,
};

type ErrorHandler =
    Arc<dyn Fn(Error) -> Pin<Box<dyn Future<Output = Response> + Send>> + Send + Sync>;

/// Default maximum request body size: 1 MiB.
pub const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;

/// Default time allowed for receiving a request body: 60 seconds.
pub const DEFAULT_BODY_TIMEOUT: Duration = Duration::from_secs(60);

/// An application: routes, middleware and a server.
///
/// ```no_run
/// use rustyweb::App;
///
/// fn main() {
///     let mut app = App::new();
///     app.get("/", |_req| async { "Hello World!" });
///     app.listen(3000);
/// }
/// ```
///
/// App-level middleware runs for every request, in registration order, and
/// wraps routing — so it also sees `404`s and `405`s.
pub struct App {
    middleware: Arc<Vec<BoxMiddleware>>,
    table: Arc<Table>,
    state: Arc<State>,
    on_error: Option<ErrorHandler>,
    trust_proxy: bool,
    etag: bool,
    pub(crate) body_limit: usize,
    pub(crate) body_timeout: Duration,
}

impl Default for App {
    fn default() -> Self {
        App {
            middleware: Arc::default(),
            table: Arc::default(),
            state: Arc::default(),
            on_error: None,
            trust_proxy: false,
            etag: true,
            body_limit: DEFAULT_BODY_LIMIT,
            body_timeout: DEFAULT_BODY_TIMEOUT,
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds middleware that runs for every request.
    ///
    /// ```
    /// use rustyweb::{App, Next, Request, Response};
    ///
    /// async fn logger(req: Request, next: Next) -> Response {
    ///     let path = req.path().to_owned();
    ///     let res = next.run(req).await;
    ///     println!("{path} -> {}", res.status());
    ///     res
    /// }
    ///
    /// let mut app = App::new();
    /// app.middleware(logger);
    /// ```
    ///
    /// Closures need their argument types written out:
    /// `app.middleware(|req: Request, next: Next| async move { next.run(req).await })`.
    /// See [`Middleware`] for middleware with options.
    pub fn middleware(&mut self, middleware: impl Middleware) -> &mut Self {
        Arc::make_mut(&mut self.middleware).push(box_middleware(middleware));
        self
    }

    /// Adds middleware that runs only for paths under `prefix`, like
    /// Express's `app.use("/api", fn)`. `/api` covers `/api` and `/api/users`
    /// but not `/apis`. Inside it, [`req.base_path()`](Request::base_path)
    /// is the matched prefix.
    ///
    /// ```
    /// use rustyweb::{App, Next, Request, Response};
    ///
    /// async fn api_only(req: Request, next: Next) -> Response {
    ///     let mut res = next.run(req).await;
    ///     res.headers_mut().insert("x-api", "1".parse().unwrap());
    ///     res
    /// }
    ///
    /// let mut app = App::new();
    /// app.middleware_at("/api", api_only);
    /// ```
    pub fn middleware_at(&mut self, prefix: &str, middleware: impl Middleware) -> &mut Self {
        self.middleware(Scoped {
            prefix: Pattern::parse(prefix),
            inner: middleware,
        })
    }

    /// Stores a value that every handler and middleware can read with
    /// [`req.state::<T>()`](Request::state), like Express's `app.locals`.
    /// There is one value per type; adding a second value of the same type
    /// replaces the first. Use it for database pools, config, template
    /// engines and so on.
    ///
    /// ```
    /// use rustyweb::{App, Request};
    ///
    /// struct Config {
    ///     greeting: String,
    /// }
    ///
    /// let mut app = App::new();
    /// app.state(Config { greeting: "hello".into() });
    /// app.get("/", |req: Request| async move {
    ///     req.state::<Config>().greeting.clone()
    /// });
    /// ```
    ///
    /// For values a handler needs to change, wrap them in a `Mutex` or use
    /// atomics; state is shared by all requests at once.
    pub fn state<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        Arc::make_mut(&mut self.state).insert(value);
        self
    }

    /// Sets one handler for every error response: [`Error`]s returned by
    /// handlers or middleware, `404`s, `405`s, oversized bodies and panics.
    /// Use it for custom error pages or JSON error bodies.
    ///
    /// ```
    /// use rustyweb::{App, Error, Html};
    ///
    /// let mut app = App::new();
    /// app.on_error(|err: Error| async move {
    ///     let page = format!("<h1>{}</h1><p>{}</p>", err.status(), err.message());
    ///     (err.status(), Html(page))
    /// });
    /// ```
    ///
    /// Responses built by hand (for example returning `StatusCode::NOT_FOUND`
    /// directly) are not errors in this sense and pass through unchanged.
    pub fn on_error<F, Fut>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(Error) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: IntoResponse,
    {
        self.on_error = Some(Arc::new(move |err| {
            let fut = handler(err);
            Box::pin(async move { fut.await.into_response() })
        }));
        self
    }

    /// Mounts `router`'s routes under `prefix`.
    pub fn mount(&mut self, prefix: &str, router: Router) -> &mut Self {
        Arc::make_mut(&mut self.table)
            .routes
            .extend(mounted(prefix, router));
        self
    }

    /// Replaces the default `404 Not Found` response for requests that match
    /// no route.
    pub fn fallback<F, Fut>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: IntoResponse,
    {
        Arc::make_mut(&mut self.table).fallback = Some(box_handler(handler));
        self
    }

    /// Trusts the `X-Forwarded-For`, `X-Forwarded-Host` and
    /// `X-Forwarded-Proto` headers set by a reverse proxy (nginx, a load
    /// balancer, a PaaS router), like Express's `app.set("trust proxy", true)`.
    /// This changes what [`req.ip()`](Request::ip),
    /// [`req.hostname()`](Request::hostname) and
    /// [`req.protocol()`](Request::protocol) return.
    ///
    /// Only turn it on when the app is reachable **only** through such a
    /// proxy: otherwise clients can send these headers themselves and fake
    /// their IP address or protocol.
    ///
    /// The values are read from the **last** entry of each header, the one
    /// added by the proxy directly in front of the app. That is right for the
    /// usual single proxy or load balancer. Behind several proxies,
    /// `req.ip()` is the address the last proxy saw.
    pub fn trust_proxy(&mut self, trust: bool) -> &mut Self {
        self.trust_proxy = trust;
        self
    }

    /// Turns automatic `ETag`s on or off (on by default), like Express's
    /// `app.set("etag", false)`. When on, successful `GET` and `HEAD`
    /// responses with an in-memory body get a weak `ETag` computed from the
    /// body, and repeat requests with a matching `If-None-Match` get
    /// `304 Not Modified` instead of the body again. Streamed bodies are left
    /// alone; files get their own `ETag` from their size and modified time.
    pub fn etag(&mut self, enabled: bool) -> &mut Self {
        self.etag = enabled;
        self
    }

    /// Sets the maximum request body size in bytes. Larger requests get
    /// `413 Payload Too Large`. Defaults to [`DEFAULT_BODY_LIMIT`].
    pub fn body_limit(&mut self, bytes: usize) -> &mut Self {
        self.body_limit = bytes;
        self
    }

    /// Sets how long a client may take to send a request body. Slower
    /// requests get `408 Request Timeout`, so clients can't hold connections
    /// open by sending a body very slowly. Defaults to
    /// [`DEFAULT_BODY_TIMEOUT`]; raise it if you accept large uploads over
    /// slow connections.
    pub fn body_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.body_timeout = timeout;
        self
    }

    route_methods! {
        get => GET,
        post => POST,
        put => PUT,
        patch => PATCH,
        delete => DELETE,
    }

    /// Runs a request through the app without a network socket. This is what
    /// the server calls for each request, and the easiest way to test an app.
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")] async fn main() {
    /// use rustyweb::{App, Request};
    ///
    /// let mut app = App::new();
    /// app.get("/", |_req| async { "hi" });
    ///
    /// let req = Request::from(http::Request::get("/").body("").unwrap());
    /// assert_eq!(app.handle(req).await.status(), 200);
    /// # }
    /// ```
    pub async fn handle(&self, mut req: Request) -> Response {
        let is_head = req.method() == Method::HEAD;
        let req_is_get_or_head = req.method() == Method::GET || is_head;
        let conditional = if req_is_get_or_head {
            let headers = req.headers();
            (
                headers.get(header::IF_NONE_MATCH).cloned(),
                headers.get(header::IF_MODIFIED_SINCE).cloned(),
            )
        } else {
            (None, None)
        };
        let range = if req_is_get_or_head {
            let headers = req.headers();
            headers
                .get(header::RANGE)
                .cloned()
                .map(|range| (range, headers.get(header::IF_RANGE).cloned()))
        } else {
            None
        };
        req.set_state(self.state.clone());
        req.set_trust_proxy(self.trust_proxy);
        let next = Next::new(
            self.middleware.clone(),
            Endpoint::Router(self.table.clone()),
        );
        let res = catch_panic(Box::pin(next.run(req))).await;
        let mut res = self.finish(res).await;
        if self.etag && res.status() == StatusCode::OK && req_is_get_or_head {
            add_etag(&mut res);
        }
        if res.status() == StatusCode::OK && is_fresh(&conditional.0, &conditional.1, &res) {
            not_modified(&mut res);
        } else if let Some((range, if_range)) = range {
            serve_range(&mut res, &range, if_range.as_ref()).await;
        }
        if is_head { strip_body(res) } else { res }
    }

    /// Runs the `on_error` handler if `res` came from an [`Error`].
    pub(crate) async fn finish(&self, res: Response) -> Response {
        let (Some(handler), Some(err)) = (&self.on_error, res.extensions().get::<Error>()) else {
            return res;
        };
        let mut custom = catch_panic(handler(err.clone())).await;
        // Keep headers that middleware added (CORS, request ids, a 405's
        // Allow, ...); only the body and its type belong to the error page.
        for (name, value) in res.headers() {
            if name != header::CONTENT_TYPE
                && name != header::CONTENT_LENGTH
                && !custom.headers().contains_key(name)
            {
                custom.headers_mut().append(name, value.clone());
            }
        }
        custom
    }
}

impl AddRoute for App {
    fn add_route(&mut self, route: Route) -> &mut Route {
        let routes = &mut Arc::make_mut(&mut self.table).routes;
        routes.push(route);
        routes.last_mut().unwrap()
    }
}

/// Drops the body of a `HEAD` response, keeping the `Content-Length` the
/// matching `GET` would have sent.
fn strip_body(res: Response) -> Response {
    let (mut parts, body) = res.into_parts();
    if let Some(len) = body.size_hint().exact() {
        parts
            .headers
            .entry(header::CONTENT_LENGTH)
            .or_insert(HeaderValue::from(len));
    }
    Response::from_parts(parts, Body::empty())
}

/// Whether the client's cached copy (from `If-None-Match` or
/// `If-Modified-Since`) is still current for `res`.
fn is_fresh(
    if_none_match: &Option<HeaderValue>,
    if_modified_since: &Option<HeaderValue>,
    res: &Response,
) -> bool {
    if let Some(if_none_match) = if_none_match {
        let Some(etag) = res
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
        else {
            return false;
        };
        let etag = etag.trim_start_matches("W/");
        return if_none_match.to_str().is_ok_and(|list| {
            list.split(',')
                .map(|tag| tag.trim())
                .any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag)
        });
    }
    let parse = |v: &HeaderValue| httpdate::parse_http_date(v.to_str().ok()?).ok();
    match (
        if_modified_since.as_ref().and_then(parse),
        res.headers().get(header::LAST_MODIFIED).and_then(parse),
    ) {
        (Some(since), Some(modified)) => modified <= since,
        _ => false,
    }
}

/// Turns a full file response into `206 Partial Content` (or `416`) for a
/// `Range` request, when the response supports it.
async fn serve_range(res: &mut Response, range: &HeaderValue, if_range: Option<&HeaderValue>) {
    if res.status() != StatusCode::OK {
        return;
    }
    let Some(source) = res.extensions().get::<RangeSource>().cloned() else {
        return;
    };
    // Only if the body is still the file itself: middleware that compressed
    // or rewrote it would leave a different length or an encoding.
    let unchanged = res.headers().get(header::CONTENT_LENGTH)
        == Some(&HeaderValue::from(source.len))
        && !res.headers().contains_key(header::CONTENT_ENCODING);
    if !unchanged {
        return;
    }
    // If-Range: only send part of the file if it hasn't changed since the
    // client got the rest; otherwise send all of it.
    if let Some(if_range) = if_range.and_then(|v| v.to_str().ok()) {
        let validator = if if_range.starts_with('"') {
            header::ETAG
        } else {
            header::LAST_MODIFIED
        };
        if res.headers().get(validator).and_then(|v| v.to_str().ok()) != Some(if_range) {
            return;
        }
    }
    let Some(parsed) = range.to_str().ok().and_then(|r| parse_range(r, source.len)) else {
        return;
    };
    match parsed {
        Ok((first, last)) => {
            let len = last - first + 1;
            let Some(body) = source.body(first, len).await else {
                return;
            };
            *res.status_mut() = StatusCode::PARTIAL_CONTENT;
            *res.body_mut() = body;
            let content_range = format!("bytes {first}-{last}/{}", source.len);
            let headers = res.headers_mut();
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::try_from(content_range).unwrap(),
            );
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
        }
        Err(()) => {
            *res.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            *res.body_mut() = Body::empty();
            let content_range = format!("bytes */{}", source.len);
            let headers = res.headers_mut();
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::try_from(content_range).unwrap(),
            );
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(0));
            headers.remove(header::CONTENT_TYPE);
        }
    }
}

/// Adds a weak `ETag` derived from the body, if the body is in memory and
/// the response doesn't have one yet.
fn add_etag(res: &mut Response) {
    use std::hash::{Hash, Hasher};

    if res.headers().contains_key(header::ETAG) {
        return;
    }
    let Some(bytes) = res.body().as_bytes() else {
        return;
    };
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let etag = format!("W/\"{:x}-{:x}\"", bytes.len(), hasher.finish());
    res.headers_mut().insert(
        header::ETAG,
        HeaderValue::try_from(etag).expect("hex is valid"),
    );
}

fn not_modified(res: &mut Response) {
    *res.status_mut() = StatusCode::NOT_MODIFIED;
    *res.body_mut() = Body::empty();
    let headers = res.headers_mut();
    headers.remove(header::CONTENT_TYPE);
    headers.remove(header::CONTENT_LENGTH);
}
