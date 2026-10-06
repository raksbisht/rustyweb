mod common;

use std::sync::{Arc, Mutex};

use common::{call, text};
use rustyweb::{App, Method, Next, Request, Response, Router, StatusCode};

type Log = Arc<Mutex<Vec<String>>>;

async fn must_not_run(_req: Request) -> &'static str {
    panic!("handler must not run")
}

async fn panicking_handler(_req: Request) -> &'static str {
    panic!("boom")
}

async fn panicking_middleware(_req: Request, _next: Next) -> Response {
    panic!("boom")
}

/// Middleware that records when it runs before and after the handler.
fn tracer(
    log: &Log,
    name: &'static str,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
+ Send
+ Sync
+ 'static {
    let log = log.clone();
    move |req, next| {
        let log = log.clone();
        Box::pin(async move {
            log.lock().unwrap().push(format!("{name} before"));
            let res = next.run(req).await;
            log.lock().unwrap().push(format!("{name} after"));
            res
        })
    }
}

#[tokio::test]
async fn runs_in_order_around_the_handler() {
    let log = Log::default();
    let mut router = Router::new();
    router.middleware(tracer(&log, "router"));
    let handler_log = log.clone();
    router
        .get("/", move |_req| {
            let log = handler_log.clone();
            async move {
                log.lock().unwrap().push("handler".into());
                "ok"
            }
        })
        .with(tracer(&log, "route"));

    let mut app = App::new();
    app.middleware(tracer(&log, "a"))
        .middleware(tracer(&log, "b"));
    app.mount("/x", router);

    assert_eq!(text(call(&app, Method::GET, "/x").await).await, "ok");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "a before",
            "b before",
            "router before",
            "route before",
            "handler",
            "route after",
            "router after",
            "b after",
            "a after",
        ]
    );
}

#[tokio::test]
async fn app_middleware_sees_not_found() {
    let log = Log::default();
    let mut app = App::new();
    app.middleware(tracer(&log, "a"));

    assert_eq!(
        call(&app, Method::GET, "/nope").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(*log.lock().unwrap(), ["a before", "a after"]);
}

#[tokio::test]
async fn router_middleware_only_wraps_its_routes() {
    let log = Log::default();
    let mut router = Router::new();
    router.middleware(tracer(&log, "router"));
    router.get("/", |_req| async { "inside" });

    let mut app = App::new();
    app.get("/", |_req| async { "outside" });
    app.mount("/r", router);

    call(&app, Method::GET, "/").await;
    assert!(log.lock().unwrap().is_empty());
    call(&app, Method::GET, "/r").await;
    assert_eq!(*log.lock().unwrap(), ["router before", "router after"]);
}

#[tokio::test]
async fn short_circuit() {
    let mut app = App::new();
    app.middleware(|req: Request, next: Next| async move {
        if req.header("x-token") == Some("ok") {
            next.run(req).await
        } else {
            rustyweb::IntoResponse::into_response(StatusCode::UNAUTHORIZED)
        }
    });
    app.get("/", must_not_run);

    assert_eq!(
        call(&app, Method::GET, "/").await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn extensions_pass_data_to_handlers() {
    #[derive(Clone)]
    struct User(&'static str);

    let mut app = App::new();
    app.middleware(|mut req: Request, next: Next| async move {
        req.extensions_mut().insert(User("ada"));
        next.run(req).await
    });
    app.get("/", |req: Request| async move {
        req.extensions().get::<User>().unwrap().0
    });

    assert_eq!(text(call(&app, Method::GET, "/").await).await, "ada");
}

#[tokio::test]
async fn middleware_can_modify_response_and_sees_handler_panics() {
    let mut app = App::new();
    app.middleware(|req: Request, next: Next| async move {
        let mut res = next.run(req).await;
        let status = res.status().as_u16();
        res.headers_mut().insert("x-seen", status.into());
        res
    });
    app.get("/ok", |_req| async { "fine" });
    app.get("/boom", panicking_handler);

    assert_eq!(
        call(&app, Method::GET, "/ok").await.headers()["x-seen"],
        "200"
    );
    assert_eq!(
        call(&app, Method::GET, "/boom").await.headers()["x-seen"],
        "500"
    );
}

#[tokio::test]
async fn middleware_panic_is_500() {
    let mut app = App::new();
    app.middleware(panicking_middleware);
    app.get("/", |_req| async { "ok" });

    assert_eq!(
        call(&app, Method::GET, "/").await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
