//! The extension points packages build on: Middleware, state, setup functions and
//! on_error.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::{call, send, text};
use rustyweb::{
    App, Error, IntoResponse, Method, Middleware, Next, Request, Response, StatusCode, header,
};

/// Middleware with options, as a package would ship it.
struct Tag {
    value: &'static str,
}

impl Middleware for Tag {
    async fn call(&self, req: Request, next: Next) -> Response {
        let mut res = next.run(req).await;
        res.headers_mut()
            .insert("x-tag", self.value.parse().unwrap());
        res
    }
}

/// A factory function returning middleware, like `cors(options)` in Express.
fn tag(value: &'static str) -> impl Middleware {
    move |req: Request, next: Next| async move {
        let mut res = next.run(req).await;
        res.headers_mut()
            .insert("x-factory", value.parse().unwrap());
        res
    }
}

#[tokio::test]
async fn struct_and_factory_middleware() {
    let mut app = App::new();
    app.middleware(Tag { value: "app" }).middleware(tag("f"));
    app.get("/", |_req| async { "ok" })
        .with(Tag { value: "route" });

    let res = call(&app, Method::GET, "/").await;
    // The route-level Tag runs innermost, so the app-level one overwrites it.
    assert_eq!(res.headers()["x-tag"], "app");
    assert_eq!(res.headers()["x-factory"], "f");
}

struct Counter(AtomicUsize);

#[tokio::test]
async fn state_is_shared_across_requests_and_middleware() {
    let mut app = App::new();
    app.state(Counter(AtomicUsize::new(0)));
    app.state(String::from("hello"));
    app.middleware(|req: Request, next: Next| async move {
        req.state::<Counter>().0.fetch_add(1, Ordering::SeqCst);
        next.run(req).await
    });
    app.get("/", |req| async move {
        let n = req.state::<Counter>().0.load(Ordering::SeqCst);
        format!("{} #{n}", req.state::<String>())
    });

    assert_eq!(text(call(&app, Method::GET, "/").await).await, "hello #1");
    assert_eq!(text(call(&app, Method::GET, "/").await).await, "hello #2");
}

#[tokio::test]
async fn missing_state() {
    let mut app = App::new();
    app.get("/try", |req| async move {
        format!("{:?}", req.try_state::<u32>())
    });
    app.get(
        "/panic",
        |req| async move { req.state::<u32>().to_string() },
    );

    assert_eq!(text(call(&app, Method::GET, "/try").await).await, "None");
    let res = call(&app, Method::GET, "/panic").await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// A package's setup function: adds state, middleware and routes in one call.
mod greeter {
    use super::*;

    pub struct Greeting(pub &'static str);

    pub fn setup(app: &mut App, greeting: &'static str) {
        app.state(Greeting(greeting));
        app.middleware(Tag { value: "greeter" });
        app.get("/greet/:name", |req| async move {
            let greeting = req.state::<Greeting>();
            format!("{}, {}", greeting.0, req.param("name"))
        });
    }
}

#[tokio::test]
async fn packages_with_setup_functions() {
    let mut app = App::new();
    greeter::setup(&mut app, "hi");

    let res = call(&app, Method::GET, "/greet/ada").await;
    assert_eq!(res.headers()["x-tag"], "greeter");
    assert_eq!(text(res).await, "hi, ada");
}

#[tokio::test]
async fn on_error_handles_every_kind_of_error() {
    let mut app = App::new();
    app.on_error(|err: Error| async move {
        (
            err.status(),
            format!("custom {}: {}", err.status().as_u16(), err.message()),
        )
    });
    app.get("/fail", |_req| async {
        Err::<&str, _>(Error::forbidden("no entry"))
    });
    app.get("/boom", |_req| async {
        if true {
            panic!("boom");
        }
        "unreachable"
    });
    app.get("/manual", |_req| async { StatusCode::NOT_FOUND });
    app.post("/body", |req| async move { req.text().map(str::to_owned) });

    let res = call(&app, Method::GET, "/fail").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(text(res).await, "custom 403: no entry");

    assert_eq!(
        text(call(&app, Method::GET, "/nope").await).await,
        "custom 404: Not Found"
    );

    let res = call(&app, Method::DELETE, "/fail").await;
    assert_eq!(res.headers()[header::ALLOW], "GET, HEAD, OPTIONS");
    assert_eq!(text(res).await, "custom 405: Method Not Allowed");

    assert_eq!(
        text(call(&app, Method::GET, "/boom").await).await,
        "custom 500: Internal Server Error"
    );

    let req = http::Request::post("/body").body(vec![0xff]).unwrap();
    let res = app.handle(Request::from(req)).await;
    assert_eq!(
        text(res).await,
        "custom 400: request body is not valid UTF-8"
    );

    // A hand-built status response is not an Error and passes through.
    let res = call(&app, Method::GET, "/manual").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(text(res).await, "");

    assert_eq!(
        text(send(&app, Method::POST, "/body", "fine").await).await,
        "fine"
    );
}

#[tokio::test]
async fn error_responses_are_recognisable_in_middleware() {
    let mut app = App::new();
    app.middleware(|req: Request, next: Next| async move {
        let res = next.run(req).await;
        let is_error = res.extensions().get::<Error>().is_some();
        let mut res = res.into_response();
        res.headers_mut()
            .insert("x-error", is_error.to_string().parse().unwrap());
        res
    });
    app.get("/ok", |_req| async { "fine" });

    assert_eq!(
        call(&app, Method::GET, "/ok").await.headers()["x-error"],
        "false"
    );
    assert_eq!(
        call(&app, Method::GET, "/missing").await.headers()["x-error"],
        "true"
    );
}

#[tokio::test]
async fn error_pages_keep_headers_added_by_middleware() {
    let mut app = App::new();
    app.middleware(Tag { value: "kept" });
    app.on_error(|err: Error| async move { (err.status(), rustyweb::Html("<h1>oops</h1>")) });
    app.get("/only-get", |_req| async { "ok" });

    let res = call(&app, Method::GET, "/missing").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(res.headers()["x-tag"], "kept");
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(text(res).await, "<h1>oops</h1>");

    let res = call(&app, Method::POST, "/only-get").await;
    assert_eq!(res.headers()[header::ALLOW], "GET, HEAD, OPTIONS");
    assert_eq!(res.headers()["x-tag"], "kept");
}

#[cfg(feature = "json")]
#[tokio::test]
async fn json_serialization_failures_are_errors() {
    use std::collections::HashMap;

    let mut app = App::new();
    app.on_error(|err: Error| async move { (err.status(), "handled") });
    app.get("/", |_req| async {
        // JSON object keys must be strings, so this map can't be serialized.
        rustyweb::Json(HashMap::from([((1, 2), 3)]))
    });

    let res = call(&app, Method::GET, "/").await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(text(res).await, "handled");
}

#[tokio::test]
async fn remote_addr_is_none_without_a_socket() {
    let mut app = App::new();
    app.get("/", |req| async move { format!("{:?}", req.remote_addr()) });
    assert_eq!(text(call(&app, Method::GET, "/").await).await, "None");
}
