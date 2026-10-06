//! A mounted router with its own middleware.
//!
//! ```text
//! curl localhost:3000/api/users
//! curl localhost:3000/api/users/7
//! curl localhost:3000/nope        # custom 404
//! ```

use rustyweb::prelude::*;

async fn api_version(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut()
        .insert("x-api-version", "1".parse().unwrap());
    res
}

fn users() -> Router {
    let mut users = Router::new();
    users.get("/", |_req| async { "all users" });
    users.get(
        "/:id",
        |req| async move { format!("user {}", req.param("id")) },
    );
    users
}

fn main() {
    let mut api = Router::new();
    api.middleware(api_version);
    api.mount("/users", users());

    let mut app = App::new();
    app.middleware(logger);
    app.get("/", |_req| async { "home" });
    app.mount("/api", api);
    app.fallback(|req| async move {
        (
            StatusCode::NOT_FOUND,
            format!("no route for {}", req.path()),
        )
    });

    app.listen(3000);
}
