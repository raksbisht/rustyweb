# RustyWeb reference

Everything RustyWeb does, in one place. New here? Start with the
[README](../README.md); coming from Express, see the [Express guide](express.md).

## Routing

```rust
app.get("/users", list);            // also post, put, patch, delete
app.all("/ping", ping);             // any method
app.on(Method::OPTIONS, "/x", opts); // any other method
app.get("/users/:id", show);        // req.param("id")
app.get("/files/*", file);          // req.param("*") == "a/b.txt" for /files/a/b.txt
```

- Routes match **in registration order**; the first match wins.
- Trailing slashes are ignored. Params are percent-decoded.
- Unknown path → `404` (override with `app.fallback(handler)`). Known path, wrong
  method → `405` with an `Allow` header. `HEAD` uses the `GET` route with no body, and
  `OPTIONS` is answered automatically.

Several methods on one path:

```rust
app.route("/users").get(list).post(create);
```

Group routes with a `Router` and mount it under a prefix:

```rust
let mut users = Router::new();
users.get("/", list);
users.get("/:id", show);

app.mount("/api/users", users);
```

## Handlers

A handler is an async function or closure that takes a `Request` and returns anything
implementing `IntoResponse`:

| Return | Response |
|---|---|
| `&'static str`, `String` | 200, `text/plain` |
| `Vec<u8>`, `Bytes` | 200, `application/octet-stream` |
| `Html(..)` | 200, `text/html` |
| `Json(..)` | 200, `application/json` |
| `StatusCode` | that status, empty body |
| `(StatusCode, T)` / `(StatusCode, HeaderMap, T)` | `T` with a different status / extra headers |
| `Redirect::to("/x")` | 302 |
| `File::send(path).await` / `File::download(path).await` | the file, streamed, with caching headers |
| `Body::from_stream(stream)` | sent chunk by chunk as it's produced |
| `Result<T, E>` | either side — so `?` works |
| `Response` | as built |

JSON without declaring a struct: `let body: Value = req.json()?;` reads any JSON, and
`Json(json!({ "ok": true }))` sends some.

`rustyweb::Error` is a ready-made error type with a status and message, and
`rustyweb::Result<T>` is short for `Result<T, Error>`:

```rust
async fn create(req: Request) -> Result<String> {
    let name = req.text()?;                 // 400 if not UTF-8
    if name.is_empty() {
        return Err(Error::bad_request("name required"));
    }
    Ok(format!("created {name}"))
}
```

`Request` gives you `method()`, `path()`, `param()`, `query()`, `query_as::<T>()`, `header()`,
`cookie()`, `body()`, `text()`, `json()`, `form()`, `is("json")`, `accepts(&["html", "json"])`,
`ip()`, `hostname()`, `protocol()`, `state()` for app-wide values and `extensions()` for data set
by middleware.

## Cookies

```rust
app.post("/login", |_req| async {
    "Welcome".into_response()
        .with_cookie(Cookie::new("session", "abc123").http_only(true).secure(true))
});
app.get("/me", |req| async move { format!("{:?}", req.cookie("session")) });
```

`res.clear_cookie("session")` removes one.

## Static files

```rust
app.middleware(static_files("public"));                // GET /logo.png -> public/logo.png
app.middleware_at("/assets", static_files("assets"));  // GET /assets/app.js -> assets/app.js
app.get("/report", |_req| async { File::send("reports/latest.pdf").await });
```

Files are streamed, so large ones don't fill memory. Responses carry `ETag` and
`Last-Modified`, repeat requests get `304 Not Modified`, and `Range` requests are supported,
so videos can seek and downloads can resume. Folders serve their `index.html` (`/docs`
redirects to `/docs/` so relative links work). `..` and hidden files like
`.env` are never served. For a `*` route param, use `File::send_from(root, path)`, which
applies the same checks.

## Middleware

`rustyweb::logger` is built in, like `morgan("dev")`:

```rust
app.middleware(logger); // GET /users/7 200 0.214 ms
```

Your own middleware is an async function taking the request and `Next`:

```rust
async fn require_auth(req: Request, next: Next) -> Result<Response, Error> {
    match req.header("authorization") {
        Some("Bearer secret") => Ok(next.run(req).await), // continue
        _ => Err(Error::unauthorized("nope")),            // short-circuit
    }
}
```

Code after `next.run(req).await` runs once the handler has responded, so one function
can act before and after (timing, headers, logging). Pass data to handlers with
`req.extensions_mut().insert(value)`.

Inline closures need their argument types: `app.middleware(|req: Request, next: Next| async move { ... })`.

Three levels, outermost first:

1. `app.middleware(mw)`: every request, including 404s.
   `app.middleware_at("/api", mw)`: only paths under `/api`.
2. `router.middleware(mw)`: requests matching that router's routes.
3. `app.get(..).with(mw)`: that one route.

A panic in a handler or middleware becomes a `500` instead of killing the connection.

## Extending

Like Express, the core stays small and features come from packages. A package is an
ordinary crate that exports middleware, routers, response types or a setup function:

```rust
app.middleware(cors("*"));                 // middleware, with options via the Middleware trait
app.state(db_pool);                        // shared values: req.state::<DbPool>()
app.mount("/admin", admin::router());      // a package's routes
rustyweb_session::setup(&mut app, opts);   // a package that adds several things at once
app.on_error(pretty_errors);               // one handler for every error response
```

Packages can also add request helpers (like `req.csv_rows()`) through extension traits and new
response types through `IntoResponse`. See the [package guide](packages.md) to write
your own, and `cargo run --example packages` for a working example.

## Server

- `app.listen(3000)` serves on `localhost:3000` and blocks until Ctrl-C, then gives
  in-flight requests up to 10 seconds to finish. No `async fn main` needed.
- `app.listen("0.0.0.0:3000")` makes the app reachable from other machines, which
  you want when deploying.
- Inside async code (e.g. `#[tokio::main]`), use `app.listen_async(3000).await`.
- `app.serve(listener, shutdown_future).await` takes your own listener and shutdown signal.
- Behind nginx or a load balancer, `app.trust_proxy(true)` makes `req.ip()`,
  `req.hostname()` and `req.protocol()` use the `X-Forwarded-*` headers. Leave it off
  otherwise, or clients can fake them.

Other responses with an in-memory body also get an automatic `ETag` (turn it off with
`app.etag(false)`), so browsers can revalidate cheaply.

Request bodies are buffered up to `app.body_limit(bytes)` (default 1 MiB); larger
ones get `413`. Clients that take longer than 30 seconds to send their request
headers are disconnected, and bodies must arrive within `app.body_timeout(...)` (default
60 seconds) or the request gets `408`.

## Testing

Drive the app directly, without a socket:

```rust
let req = Request::from(http::Request::get("/users/7").body("").unwrap());
let res = app.handle(req).await;
assert_eq!(res.status(), 200);
```

## Examples

```sh
cargo run --example hello
cargo run --example middleware
cargo run --example router
cargo run --example json
cargo run --example packages
cargo run --example website
```

## Features

- `json` (on by default): `Json` and `req.json()`.
- `form` (on by default): `req.form_as::<T>()` and `req.query_as::<T>()`. (`req.form()` and
  `req.query()` always work.)

To use your own structs with them, add serde: `cargo add serde --features derive`.
Turn both off with `default-features = false`.

## Not built in

TLS, HTTP/2, WebSockets, templating and sessions. WebSockets, templating and sessions fit
naturally as [packages](packages.md) (the core provides `req.upgrade()` for protocols like
WebSockets); for TLS, run behind a reverse proxy such as nginx or Caddy.
