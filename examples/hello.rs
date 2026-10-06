//! `cargo run --example hello`, then open http://localhost:3000/users/42

use rustyweb::prelude::*;

fn main() {
    let mut app = App::new();
    app.middleware(logger);

    app.get("/", |_req| async { "Hello World!" });
    app.get("/users/:id", |req| async move {
        format!("user {}", req.param("id"))
    });
    app.get("/search", |req| async move {
        format!("searching for {:?}", req.query("q").unwrap_or_default())
    });
    app.post("/echo", |req| async move { req.body().clone() });
    app.get("/teapot", |_req| async {
        (StatusCode::IM_A_TEAPOT, "short and stout")
    });

    app.listen(3000);
}
