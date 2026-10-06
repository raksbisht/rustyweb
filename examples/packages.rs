//! Writing and using packages: middleware with options, shared state, a setup
//! function and a custom error page. In a real project each of these would
//! live in its own `rustyweb-*` package.
//!
//! ```text
//! curl -i localhost:3000/                 # CORS header added
//! curl localhost:3000/visits              # counter kept in app state
//! curl localhost:3000/health              # route added by a package
//! curl localhost:3000/nope                # custom error page
//! ```

use std::sync::atomic::{AtomicU64, Ordering};

use rustyweb::prelude::*;

// ---- would be `rustyweb-cors` -------------------------------------------

/// Middleware with options.
struct Cors {
    origin: &'static str,
}

impl Middleware for Cors {
    async fn call(&self, req: Request, next: Next) -> Response {
        let mut res = next.run(req).await;
        res.headers_mut()
            .insert("access-control-allow-origin", self.origin.parse().unwrap());
        res
    }
}

/// Express-style factory: `cors("*")`.
fn cors(origin: &'static str) -> Cors {
    Cors { origin }
}

// ---- would be `rustyweb-visits` ----------------------------------------

/// Shared state the package owns.
struct Visits(AtomicU64);

/// The package's setup function: adds state, middleware and a route in one
/// call. Users write `visits::setup(&mut app, "/visits")`.
fn setup_visits(app: &mut App, path: &str) {
    app.state(Visits(AtomicU64::new(0)));
    app.middleware(|req: Request, next: Next| async move {
        req.state::<Visits>().0.fetch_add(1, Ordering::Relaxed);
        next.run(req).await
    });
    app.get(path, |req| async move {
        format!(
            "{} visits so far",
            req.state::<Visits>().0.load(Ordering::Relaxed)
        )
    });
}

// ---- would be `rustyweb-health` ----------------------------------------

/// A package that only adds routes can export a router.
fn health() -> Router {
    let mut router = Router::new();
    router.get("/", |_req| async { "ok" });
    router
}

// ---- would be `rustyweb-pretty-errors` ---------------------------------

async fn pretty_errors(err: Error) -> impl IntoResponse {
    let page = format!(
        "<h1>{} {}</h1><p>{}</p>",
        err.status().as_u16(),
        err.status().canonical_reason().unwrap_or(""),
        err.message()
    );
    (err.status(), Html(page))
}

// ---- the app -------------------------------------------------------------

fn main() {
    let mut app = App::new();
    app.middleware(logger)
        .middleware(cors("*"))
        .on_error(pretty_errors);
    setup_visits(&mut app, "/visits");
    app.mount("/health", health());

    app.get("/", |_req| async { "Hello World!" });

    app.listen(3000);
}
