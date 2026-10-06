//! A small website: static files, a login form, cookies and route chaining.
//!
//! `cargo run --example website`, then open http://localhost:3000

use rustyweb::prelude::*;

#[derive(serde::Deserialize)]
struct Login {
    user: String,
}

async fn login(req: Request) -> Result<Response, Error> {
    let form: Login = req.form_as()?; // 400 if the form is malformed
    let cookie = Cookie::new("user", form.user)
        .http_only(true)
        .same_site(SameSite::Lax);
    Ok(Redirect::to("/me").into_response().with_cookie(cookie))
}

async fn me(req: Request) -> Response {
    match req.cookie("user") {
        Some(user) => Html(format!(
            "<p>Hello, {}!</p><form method=post action=/logout><button>Log out</button></form>",
            escape(&user)
        ))
        .into_response(),
        None => Redirect::to("/").into_response(),
    }
}

async fn logout(_req: Request) -> Response {
    let mut res = Redirect::to("/").into_response();
    res.clear_cookie("user");
    res
}

/// Minimal HTML escaping for user-provided text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn main() {
    let mut app = App::new();
    app.middleware(logger).middleware(static_files(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/public"
    )));

    app.route("/login").post(login);
    app.route("/me").get(me);
    app.route("/logout").post(logout);

    app.listen(3000);
}
