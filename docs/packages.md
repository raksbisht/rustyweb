# Writing packages for RustyWeb

rustyweb follows Express's approach: a small core, and everything else in packages.
A package can add any of these, and many combine several:

| You want to ship | Build it as | Users write |
|---|---|---|
| Something that runs on every request (CORS, security headers, compression, auth checks) | [Middleware](#middleware) | `app.middleware(cors(options))` |
| Shared resources (database pool, template engine, config) | [App state](#app-state) | `req.state::<Db>()` |
| New ways to read requests (multipart uploads, signed cookies) | [Extension trait](#request-helpers) | `req.csv_rows()?` |
| New kinds of responses (templates, files, CSV) | [`IntoResponse` type](#response-types) | `return Template::new("home")` |
| A set of routes (admin panel, auth endpoints, health checks) | [Router](#routes) | `app.mount("/admin", admin())` |
| All of the above in one setup call | [Setup function](#setup-functions) | `rustyweb_session::setup(&mut app, options)` |
| Custom error pages | [Error handler](#error-pages) | `app.on_error(pretty_errors)` |
| A new protocol (WebSockets, SSE helpers) | [Connection upgrade](#protocols) | `app.get("/ws", ws_handler)` |

## Setting up a package

Name it `rustyweb-<thing>`, like `rustyweb-cors` or `rustyweb-session`, so people can
find it (crates.io has no `@scope/` names). Depend on rustyweb:

```toml
[dependencies]
rustyweb = "0.1"
```

## Middleware

The simplest middleware is an `async fn`, exactly like the ones in an app:

```rust
use rustyweb::{Next, Request, Response};

pub async fn no_cache(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert("cache-control", "no-store".parse().unwrap());
    res
}
```

Middleware with options is usually a struct implementing `Middleware`:

```rust
use rustyweb::{Middleware, Next, Request, Response};

pub struct Cors {
    pub origin: String,
}

impl Middleware for Cors {
    async fn call(&self, req: Request, next: Next) -> Response {
        let mut res = next.run(req).await;
        res.headers_mut().insert(
            "access-control-allow-origin",
            self.origin.parse().unwrap(),
        );
        res
    }
}

/// Express-style factory: `app.middleware(cors("https://example.com"))`.
pub fn cors(origin: &str) -> Cors {
    Cors { origin: origin.to_owned() }
}
```

To stop a request (for example an auth check failing), return a response without
calling `next.run`. Returning an `Error` lets the app's `on_error` page handle it.

To hand data to later handlers, as Express middleware does with `req.user`, use
request extensions:

```rust
req.extensions_mut().insert(CurrentUser { id: 7 });
// later, in a handler:
let user = req.extensions().get::<CurrentUser>();
```

## App state

`app.state(value)` stores one value per type for the whole app; `req.state::<T>()`
reads it. Use a type your package owns, so it can't clash with other packages:

```rust
pub struct Templates { /* compiled templates */ }

// in the app:
app.state(Templates::load("views/"));

// in a handler or your middleware:
let templates = req.state::<Templates>();
```

State is shared by all requests at once, so anything that changes must be
thread-safe (`Mutex`, `RwLock`, atomics), and connection pools should be cheap to
use from many tasks.

## Request helpers

Express packages add properties to `req`. In Rust, a package adds methods with an
*extension trait*:

```rust
use rustyweb::{Error, Request};

pub trait CsvExt {
    fn csv_rows(&self) -> Result<Vec<Vec<String>>, Error>;
}

impl CsvExt for Request {
    fn csv_rows(&self) -> Result<Vec<Vec<String>>, Error> {
        let body = self.text()?; // 400 if not UTF-8
        Ok(body
            .lines()
            .map(|line| line.split(',').map(str::to_owned).collect())
            .collect())
    }
}
```

Users import the trait (`use rustyweb_csv::CsvExt;`) and call `req.csv_rows()?`.

## Response types

Express packages add `res.render()` or `res.sendFile()`. In rustyweb, ship a type
that implements `IntoResponse`, and handlers return it:

```rust
use rustyweb::{Html, IntoResponse, Response};

pub struct Template {
    pub name: &'static str,
}

impl IntoResponse for Template {
    fn into_response(self) -> Response {
        Html(format!("<h1>{}</h1>", self.name)).into_response()
    }
}
```

## Routes

Ship a function that builds a `Router`. Users choose where to mount it:

```rust
use rustyweb::Router;

pub fn health() -> Router {
    let mut r = Router::new();
    r.get("/", |_req| async { "ok" });
    r
}

// app.mount("/health", health());
```

## Setup functions

When a package needs to add several things at once (state, middleware, routes),
export a plain function that takes the app. There is nothing special to implement:

```rust
use rustyweb::App;

pub struct Options {
    pub cookie_name: &'static str,
}

pub fn setup(app: &mut App, options: Options) {
    app.state(SessionStore::new(options.cookie_name)); // shared store
    app.middleware(load_session);                       // reads the cookie each request
    app.post("/logout", logout);                        // a route it provides
}

// in the app:
// rustyweb_session::setup(&mut app, Options { cookie_name: "sid" });
```

Order still matters: middleware the setup function adds runs after middleware added
before the call.

## Error pages

`app.on_error(handler)` receives every `Error`: ones returned by handlers and
middleware, 404s, 405s, oversized bodies and panics. A package can export the
handler:

```rust
use rustyweb::{Error, Html, IntoResponse};

pub async fn pretty_errors(err: Error) -> impl IntoResponse {
    let page = format!("<h1>{}</h1><p>{}</p>", err.status(), err.message());
    (err.status(), Html(page))
}

// app.on_error(pretty_errors);
```

Middleware can also recognise error responses with
`res.extensions().get::<rustyweb::Error>()`.

## Protocols

For WebSockets and similar protocols, a handler takes over the connection with
`req.upgrade()`, answers `101 Switching Protocols`, and then talks to the raw
connection through `rustyweb::upgrade::TokioIo`, which works with libraries such as
`tokio-tungstenite`. See the `rustyweb::upgrade` docs for a complete example. For
server-sent events no upgrade is needed: return `Body::from_stream(...)` with
`Content-Type: text/event-stream`.

## Testing a package

Drive an app in memory with `App::handle`, no server needed:

```rust
#[tokio::test]
async fn adds_cors_header() {
    let mut app = rustyweb::App::new();
    app.middleware(cors("https://example.com"));
    app.get("/", |_req| async { "ok" });

    let req = http::Request::get("/").body("").unwrap();
    let res = app.handle(req.into()).await;
    assert_eq!(res.headers()["access-control-allow-origin"], "https://example.com");
}
```

The runnable example `cargo run --example packages` shows these pieces working
together.
