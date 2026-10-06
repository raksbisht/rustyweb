//! A request id passed to handlers, and an auth check that short-circuits.
//!
//! ```text
//! curl localhost:3000/
//! curl localhost:3000/admin                          # 401
//! curl -H 'authorization: Bearer secret' localhost:3000/admin
//! ```

use std::sync::atomic::{AtomicU64, Ordering};

use rustyweb::prelude::*;

#[derive(Clone, Copy)]
struct RequestId(u64);

/// Gives each request an id that handlers can read (like `res.locals` in
/// Express) and adds it to the response headers.
async fn request_id(mut req: Request, next: Next) -> Response {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    req.extensions_mut().insert(RequestId(id));
    let mut res = next.run(req).await;
    res.headers_mut().insert("x-request-id", id.into());
    res
}

/// Rejects requests without the right bearer token. Returning without calling
/// `next` stops the chain, like not calling `next()` in Express.
async fn require_auth(req: Request, next: Next) -> Result<Response, Error> {
    match req.header("authorization") {
        Some("Bearer secret") => Ok(next.run(req).await),
        _ => Err(Error::unauthorized("missing or wrong token")),
    }
}

fn main() {
    let mut app = App::new();
    app.middleware(logger).middleware(request_id);

    app.get("/", |req| async move {
        let RequestId(id) = *req.extensions().get().unwrap();
        format!("hello, you are request #{id}")
    });
    app.get("/admin", |_req| async { "welcome, admin" })
        .with(require_auth);

    app.listen(3000);
}
