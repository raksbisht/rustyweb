use std::sync::Arc;

use http::{HeaderValue, Method, StatusCode, header};

use crate::{
    Error, IntoResponse, Middleware, Next, Request, Response,
    middleware::{BoxHandler, BoxMiddleware, Endpoint, box_handler, box_middleware, catch_panic},
    path::Pattern,
};

#[derive(Clone)]
pub(crate) struct Route {
    /// `None` matches every method (registered with `all`).
    method: Option<Method>,
    pattern: Pattern,
    handler: BoxHandler,
    middleware: Arc<Vec<BoxMiddleware>>,
}

impl Route {
    pub(crate) fn new(method: Option<Method>, path: &str, handler: BoxHandler) -> Self {
        Route {
            method,
            pattern: Pattern::parse(path),
            handler,
            middleware: Arc::default(),
        }
    }
}

/// Moves `router`'s routes under `prefix`, wrapping each in the router's
/// middleware.
pub(crate) fn mounted(prefix: &str, router: Router) -> impl Iterator<Item = Route> {
    let prefix = Pattern::parse(prefix);
    let Router { routes, middleware } = router;
    routes.into_iter().map(move |mut route| {
        route.pattern = route.pattern.prefixed(&prefix);
        if !middleware.is_empty() {
            let mut stack = middleware.clone();
            stack.extend(route.middleware.iter().cloned());
            route.middleware = Arc::new(stack);
        }
        route
    })
}

/// A route that was just registered. Use [`with`](RouteHandle::with) to add
/// middleware that runs for this route only.
pub struct RouteHandle<'a> {
    route: &'a mut Route,
}

impl<'a> RouteHandle<'a> {
    pub(crate) fn new(route: &'a mut Route) -> Self {
        RouteHandle { route }
    }

    /// Adds middleware that runs only for this route, after any app- or
    /// router-level middleware.
    pub fn with(self, middleware: impl Middleware) -> Self {
        Arc::make_mut(&mut self.route.middleware).push(box_middleware(middleware));
        self
    }
}

/// Something routes can be added to: `App` or `Router`.
pub(crate) trait AddRoute {
    fn add_route(&mut self, route: Route) -> &mut Route;
}

/// Several handlers for one path, from [`App::route`](crate::App::route) or
/// [`Router::route`], like Express's `app.route("/users").get(..).post(..)`.
///
/// ```
/// use rustyweb::App;
///
/// let mut app = App::new();
/// app.route("/users")
///     .get(|_req| async { "list users" })
///     .post(|_req| async { "create a user" });
/// ```
pub struct RouteChain<'a> {
    target: &'a mut dyn AddRoute,
    path: String,
}

impl<'a> RouteChain<'a> {
    pub(crate) fn new(target: &'a mut dyn AddRoute, path: &str) -> Self {
        RouteChain {
            target,
            path: path.to_owned(),
        }
    }

    /// Adds a handler for `method` requests to this path.
    pub fn on<F, Fut>(self, method: Method, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future + Send + 'static,
        Fut::Output: IntoResponse,
    {
        let route = Route::new(Some(method), &self.path, box_handler(handler));
        self.target.add_route(route);
        self
    }

    /// Adds a handler for requests to this path with any method.
    pub fn all<F, Fut>(self, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future + Send + 'static,
        Fut::Output: IntoResponse,
    {
        let route = Route::new(None, &self.path, box_handler(handler));
        self.target.add_route(route);
        self
    }
}

macro_rules! chain_methods {
    ($($name:ident => $method:ident),* $(,)?) => {
        impl RouteChain<'_> {$(
            #[doc = concat!("Adds a handler for `", stringify!($method), "` requests to this path.")]
            pub fn $name<F, Fut>(self, handler: F) -> Self
            where
                F: Fn(Request) -> Fut + Send + Sync + 'static,
                Fut: std::future::Future + Send + 'static,
                Fut::Output: IntoResponse,
            {
                self.on(Method::$method, handler)
            }
        )*}
    };
}
chain_methods! {
    get => GET,
    post => POST,
    put => PUT,
    patch => PATCH,
    delete => DELETE,
}

/// Generates the route registration methods shared by `App` and `Router`.
/// The implementing type must implement `AddRoute`.
macro_rules! route_methods {
    ($($name:ident => $method:ident),* $(,)?) => {
        $(
            #[doc = concat!("Registers a handler for `", stringify!($method), "` requests to `path`.")]
            pub fn $name<F, Fut>(&mut self, path: &str, handler: F) -> $crate::RouteHandle<'_>
            where
                F: Fn($crate::Request) -> Fut + Send + Sync + 'static,
                Fut: ::std::future::Future + Send + 'static,
                Fut::Output: $crate::IntoResponse,
            {
                self.on(::http::Method::$method, path, handler)
            }
        )*

        /// Starts a chain of handlers for one path, like Express's
        /// `app.route(path)`. See [`RouteChain`]($crate::RouteChain).
        pub fn route(&mut self, path: &str) -> $crate::RouteChain<'_> {
            $crate::RouteChain::new(self, path)
        }

        /// Registers a handler for requests to `path` with any method.
        pub fn all<F, Fut>(&mut self, path: &str, handler: F) -> $crate::RouteHandle<'_>
        where
            F: Fn($crate::Request) -> Fut + Send + Sync + 'static,
            Fut: ::std::future::Future + Send + 'static,
            Fut::Output: $crate::IntoResponse,
        {
            let route = $crate::router::Route::new(None, path, $crate::middleware::box_handler(handler));
            $crate::RouteHandle::new(self.add_route(route))
        }

        /// Registers a handler for `method` requests to `path`. Use this for
        /// methods without a shorthand, such as `OPTIONS`.
        pub fn on<F, Fut>(&mut self, method: ::http::Method, path: &str, handler: F) -> $crate::RouteHandle<'_>
        where
            F: Fn($crate::Request) -> Fut + Send + Sync + 'static,
            Fut: ::std::future::Future + Send + 'static,
            Fut::Output: $crate::IntoResponse,
        {
            let route = $crate::router::Route::new(Some(method), path, $crate::middleware::box_handler(handler));
            $crate::RouteHandle::new(self.add_route(route))
        }
    };
}
pub(crate) use route_methods;

/// A group of routes with its own middleware, mounted into an
/// [`App`](crate::App) (or another `Router`) under a path prefix — Express's
/// `express.Router()`.
///
/// ```
/// use rustyweb::{App, Router};
///
/// let mut api = Router::new();
/// api.get("/users/:id", |req| async move { format!("user {}", req.param("id")) });
///
/// let mut app = App::new();
/// app.mount("/api", api); // serves GET /api/users/:id
/// ```
///
/// A router's middleware runs only for requests that match one of its routes,
/// in registration order, after the app-level middleware.
#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
    middleware: Vec<BoxMiddleware>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds middleware that wraps every route in this router.
    pub fn middleware(&mut self, middleware: impl Middleware) -> &mut Self {
        self.middleware.push(box_middleware(middleware));
        self
    }

    /// Mounts `router`'s routes under `prefix`.
    pub fn mount(&mut self, prefix: &str, router: Router) -> &mut Self {
        self.routes.extend(mounted(prefix, router));
        self
    }

    route_methods! {
        get => GET,
        post => POST,
        put => PUT,
        patch => PATCH,
        delete => DELETE,
    }
}

impl AddRoute for Router {
    fn add_route(&mut self, route: Route) -> &mut Route {
        self.routes.push(route);
        self.routes.last_mut().unwrap()
    }
}

/// The App's frozen route table, shared by in-flight requests.
#[derive(Clone, Default)]
pub(crate) struct Table {
    pub(crate) routes: Vec<Route>,
    pub(crate) fallback: Option<BoxHandler>,
}

/// Finds the first route matching `req` and runs it through its middleware.
pub(crate) async fn dispatch(table: Arc<Table>, mut req: Request) -> Response {
    let method = req.method().clone();
    let mut found = None;
    let mut head_as_get = None;
    let mut allowed: Vec<Method> = Vec::new();

    for route in &table.routes {
        let Some(params) = route.pattern.matches(req.path()) else {
            continue;
        };
        match &route.method {
            None => {
                found = Some((route, params));
                break;
            }
            Some(m) if *m == method => {
                found = Some((route, params));
                break;
            }
            Some(m) => {
                if method == Method::HEAD && *m == Method::GET && head_as_get.is_none() {
                    head_as_get = Some((route, params));
                }
                allowed.push(m.clone());
            }
        }
    }

    let Some((route, params)) = found.or(head_as_get) else {
        if !allowed.is_empty() {
            // Like Express, answer OPTIONS for known paths automatically.
            return if method == Method::OPTIONS {
                let mut res = StatusCode::NO_CONTENT.into_response();
                res.headers_mut()
                    .insert(header::ALLOW, allow_header(allowed));
                res
            } else {
                method_not_allowed(allowed)
            };
        }
        return match &table.fallback {
            Some(fallback) => catch_panic(fallback(req)).await,
            None => Error::not_found("Not Found").into_response(),
        };
    };

    req.set_params(params);
    let next = Next::new(
        route.middleware.clone(),
        Endpoint::Handler(route.handler.clone()),
    );
    next.run(req).await
}

fn method_not_allowed(allowed: Vec<Method>) -> Response {
    let mut res = Error::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response();
    res.headers_mut()
        .insert(header::ALLOW, allow_header(allowed));
    res
}

/// The `Allow` header for a path: its routes' methods, plus HEAD when GET is
/// allowed and OPTIONS, which is always answered.
fn allow_header(mut allowed: Vec<Method>) -> HeaderValue {
    if allowed.contains(&Method::GET) {
        allowed.push(Method::HEAD);
    }
    allowed.push(Method::OPTIONS);
    let mut names: Vec<&str> = allowed.iter().map(Method::as_str).collect();
    names.sort_unstable();
    names.dedup();
    HeaderValue::try_from(names.join(", ")).expect("method names are valid header values")
}
