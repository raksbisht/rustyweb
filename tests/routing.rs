mod common;

use common::{call, send, text};
use rustyweb::{App, Error, Method, Request, Router, StatusCode, header};

#[tokio::test]
async fn routes_by_method_and_path() {
    let mut app = App::new();
    app.get("/", |_req| async { "root" });
    app.get("/users", |_req| async { "list" });
    app.post("/users", |_req| async { (StatusCode::CREATED, "created") });
    app.delete("/users/:id", |req: Request| async move {
        format!("deleted {}", req.param("id"))
    });

    assert_eq!(text(call(&app, Method::GET, "/").await).await, "root");
    assert_eq!(text(call(&app, Method::GET, "/users/").await).await, "list");
    let res = call(&app, Method::POST, "/users").await;
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(text(res).await, "created");
    assert_eq!(
        text(call(&app, Method::DELETE, "/users/9").await).await,
        "deleted 9"
    );
}

#[tokio::test]
async fn first_match_wins() {
    let mut app = App::new();
    app.get("/users/me", |_req| async { "me" });
    app.get("/users/:id", |_req| async { "by id" });
    app.get("/users/:id", |_req| async { "never" });

    assert_eq!(text(call(&app, Method::GET, "/users/me").await).await, "me");
    assert_eq!(
        text(call(&app, Method::GET, "/users/1").await).await,
        "by id"
    );
}

#[tokio::test]
async fn params_query_and_wildcard() {
    let mut app = App::new();
    app.get("/files/*", |req: Request| async move {
        req.param("*").to_owned()
    });
    app.get("/search", |req: Request| async move {
        req.query("q").unwrap_or_default()
    });

    assert_eq!(
        text(call(&app, Method::GET, "/files/a/b%20c.txt").await).await,
        "a/b c.txt"
    );
    assert_eq!(
        text(call(&app, Method::GET, "/search?q=rust+web").await).await,
        "rust web"
    );
}

#[tokio::test]
async fn not_found_and_method_not_allowed() {
    let mut app = App::new();
    app.get("/thing", |_req| async { "get" });
    app.put("/thing", |_req| async { "put" });

    let res = call(&app, Method::GET, "/missing").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = call(&app, Method::POST, "/thing").await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(res.headers()[header::ALLOW], "GET, HEAD, OPTIONS, PUT");
}

#[tokio::test]
async fn custom_fallback() {
    let mut app = App::new();
    app.fallback(
        |req: Request| async move { (StatusCode::NOT_FOUND, format!("no {}", req.path())) },
    );

    let res = call(&app, Method::GET, "/x").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(text(res).await, "no /x");
}

#[tokio::test]
async fn head_falls_back_to_get_without_body() {
    let mut app = App::new();
    app.get("/", |_req| async { "hello" });

    let res = call(&app, Method::HEAD, "/").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
    assert_eq!(text(res).await, "");
}

#[tokio::test]
async fn all_and_on() {
    let mut app = App::new();
    app.on(Method::OPTIONS, "/x", |_req| async { "options" });
    app.all("/x", |req: Request| async move { req.method().to_string() });

    assert_eq!(
        text(call(&app, Method::OPTIONS, "/x").await).await,
        "options"
    );
    assert_eq!(text(call(&app, Method::PATCH, "/x").await).await, "PATCH");
}

#[tokio::test]
async fn result_handlers_and_body() {
    let mut app = App::new();
    app.post("/name", |req: Request| async move {
        let name = req.text()?;
        if name.is_empty() {
            return Err(Error::bad_request("name required"));
        }
        Ok(format!("hi {name}"))
    });

    assert_eq!(
        text(send(&app, Method::POST, "/name", "ada").await).await,
        "hi ada"
    );
    let res = send(&app, Method::POST, "/name", "").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(text(res).await, "name required");
}

#[tokio::test]
async fn mounted_routers() {
    let mut users = Router::new();
    users.get("/", |_req| async { "users" });
    users.get(
        "/:id",
        |req: Request| async move { req.param("id").to_owned() },
    );

    let mut api = Router::new();
    api.mount("/users", users);

    let mut app = App::new();
    app.mount("/api/:version", api);

    assert_eq!(
        text(call(&app, Method::GET, "/api/v1/users").await).await,
        "users"
    );
    let res = call(&app, Method::GET, "/api/v1/users/5").await;
    assert_eq!(text(res).await, "5");
    assert_eq!(
        call(&app, Method::GET, "/users").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn handler_panic_is_500() {
    let mut app = App::new();
    app.get("/boom", |_req| async {
        if true {
            panic!("boom");
        }
        "unreachable"
    });

    let res = call(&app, Method::GET, "/boom").await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[cfg(feature = "json")]
#[tokio::test]
async fn json_in_and_out() {
    use rustyweb::Json;

    #[derive(serde::Deserialize, serde::Serialize)]
    struct Point {
        x: i32,
        y: i32,
    }

    let mut app = App::new();
    app.post("/flip", |req: Request| async move {
        let p: Point = req.json()?;
        Ok::<_, Error>(Json(Point { x: p.y, y: p.x }))
    });

    let res = send(&app, Method::POST, "/flip", r#"{"x":1,"y":2}"#).await;
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(text(res).await, r#"{"x":2,"y":1}"#);

    let res = send(&app, Method::POST, "/flip", "nope").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[cfg(feature = "form")]
#[tokio::test]
async fn form_bodies() {
    #[derive(serde::Deserialize)]
    struct Login {
        user: String,
        remember: Option<bool>,
    }

    let mut app = App::new();
    app.post("/login", |req: Request| async move {
        let form: Login = req.form_as()?;
        Ok::<_, Error>(format!("{} {:?}", form.user, form.remember))
    });
    app.post("/raw", |req: Request| async move {
        Ok::<_, Error>(req.form()?.get("q").cloned().unwrap_or_default())
    });

    let res = send(&app, Method::POST, "/login", "user=ada&remember=true").await;
    assert_eq!(text(res).await, "ada Some(true)");
    let res = send(&app, Method::POST, "/login", "remember=maybe").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        text(send(&app, Method::POST, "/raw", "q=a%20b").await).await,
        "a b"
    );
}

#[tokio::test]
async fn cookies_round_trip() {
    use rustyweb::{Cookie, IntoResponse, ResponseExt};

    let mut app = App::new();
    app.post("/login", |_req| async {
        "welcome"
            .into_response()
            .with_cookie(Cookie::new("session", "abc 123").http_only(true))
            .with_cookie(Cookie::new("theme", "dark"))
    });
    app.post("/logout", |_req| async {
        let mut res = "bye".into_response();
        res.clear_cookie("session");
        res
    });
    app.get("/me", |req: Request| async move {
        format!("{:?} {:?}", req.cookie("session"), req.cookies().len())
    });

    let res = call(&app, Method::POST, "/login").await;
    let set: Vec<_> = res.headers().get_all(header::SET_COOKIE).iter().collect();
    assert_eq!(set.len(), 2);
    assert_eq!(set[0], "session=abc%20123; Path=/; HttpOnly");
    assert_eq!(set[1], "theme=dark; Path=/");

    let res = call(&app, Method::POST, "/logout").await;
    assert!(
        res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    let req = http::Request::get("/me")
        .header(header::COOKIE, "session=abc%20123; theme=dark")
        .body("")
        .unwrap();
    assert_eq!(
        text(app.handle(Request::from(req)).await).await,
        "Some(\"abc 123\") 2"
    );
}

#[tokio::test]
async fn options_is_answered_automatically() {
    let mut app = App::new();
    app.get("/users", |_req| async { "list" });
    app.post("/users", |_req| async { "created" });
    app.on(Method::OPTIONS, "/custom", |_req| async { "my options" });

    let res = call(&app, Method::OPTIONS, "/users").await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(res.headers()[header::ALLOW], "GET, HEAD, OPTIONS, POST");

    // An explicit OPTIONS route wins; unknown paths are still 404.
    assert_eq!(
        text(call(&app, Method::OPTIONS, "/custom").await).await,
        "my options"
    );
    assert_eq!(
        call(&app, Method::OPTIONS, "/nope").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn hostname_protocol_and_trust_proxy() {
    fn describe(app: &App) -> impl std::future::Future<Output = String> + '_ {
        let req = http::Request::get("/")
            .header("host", "example.com:8080")
            // The client faked the first entries; the proxy appended the
            // real values last.
            .header("x-forwarded-host", "fake.example, public.example")
            .header("x-forwarded-proto", "http, https")
            .header("x-forwarded-for", "1.2.3.4, 203.0.113.7")
            .body("")
            .unwrap();
        async move { text(app.handle(Request::from(req)).await).await }
    }
    let handler = |req: Request| async move {
        format!(
            "{:?} {} {} {:?}",
            req.hostname(),
            req.protocol(),
            req.secure(),
            req.ip()
        )
    };

    let mut app = App::new();
    app.get("/", handler);
    assert_eq!(
        describe(&app).await,
        "Some(\"example.com\") http false None"
    );

    let mut app = App::new();
    app.trust_proxy(true).get("/", handler);
    assert_eq!(
        describe(&app).await,
        "Some(\"public.example\") https true Some(203.0.113.7)"
    );

    // An absolute-form URL must not make plain HTTP look secure.
    let mut app = App::new();
    app.get("/", handler);
    let req = http::Request::get("https://example.com/").body("").unwrap();
    let described = text(app.handle(Request::from(req)).await).await;
    assert!(described.contains(" http false"), "{described}");

    let req = http::Request::get("/")
        .header("host", "[::1]:3000")
        .body("")
        .unwrap();
    let mut app = App::new();
    app.get("/", |req: Request| async move {
        req.hostname().unwrap().to_owned()
    });
    assert_eq!(text(app.handle(Request::from(req)).await).await, "[::1]");
}

#[tokio::test]
async fn route_chaining() {
    let mut users = Router::new();
    users
        .route("/:id")
        .get(|req| async move { format!("show {}", req.param("id")) })
        .delete(|req| async move { format!("delete {}", req.param("id")) });

    let mut app = App::new();
    app.route("/users")
        .get(|_req| async { "list" })
        .post(|_req| async { (StatusCode::CREATED, "created") })
        .all(|req| async move { format!("fallback {}", req.method()) });
    app.mount("/users", users);

    assert_eq!(text(call(&app, Method::GET, "/users").await).await, "list");
    assert_eq!(
        call(&app, Method::POST, "/users").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        text(call(&app, Method::PATCH, "/users").await).await,
        "fallback PATCH"
    );
    assert_eq!(
        text(call(&app, Method::GET, "/users/7").await).await,
        "show 7"
    );
    assert_eq!(
        text(call(&app, Method::DELETE, "/users/7").await).await,
        "delete 7"
    );
}

#[tokio::test]
async fn automatic_etags() {
    let mut app = App::new();
    app.get("/", |_req| async { "same body" });
    app.post("/", |_req| async { "posted" });
    app.get("/stream", |_req| async {
        rustyweb::Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>("x")]))
    });

    let res = call(&app, Method::GET, "/").await;
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert!(etag.starts_with("W/\""), "{etag}");
    // Same body, same tag.
    assert_eq!(
        call(&app, Method::GET, "/").await.headers()[header::ETAG],
        etag.as_str()
    );

    let req = http::Request::get("/")
        .header(header::IF_NONE_MATCH, &etag)
        .body("")
        .unwrap();
    let res = app.handle(Request::from(req)).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(text(res).await, "");

    assert!(
        !call(&app, Method::POST, "/")
            .await
            .headers()
            .contains_key(header::ETAG)
    );
    assert!(
        !call(&app, Method::GET, "/stream")
            .await
            .headers()
            .contains_key(header::ETAG)
    );

    let mut app = App::new();
    app.etag(false).get("/", |_req| async { "same body" });
    assert!(
        !call(&app, Method::GET, "/")
            .await
            .headers()
            .contains_key(header::ETAG)
    );
}

#[cfg(feature = "form")]
#[tokio::test]
async fn typed_query_strings() {
    #[derive(serde::Deserialize)]
    struct Page {
        page: Option<u32>,
        q: String,
    }

    let mut app = App::new();
    app.get("/search", |req: Request| async move {
        let p: Page = req.query_as()?;
        Ok::<_, Error>(format!("{} page {}", p.q, p.page.unwrap_or(1)))
    });

    assert_eq!(
        text(call(&app, Method::GET, "/search?q=rust+web&page=3").await).await,
        "rust web page 3"
    );
    assert_eq!(
        text(call(&app, Method::GET, "/search?q=x").await).await,
        "x page 1"
    );
    assert_eq!(
        call(&app, Method::GET, "/search?q=x&page=two")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, Method::GET, "/search").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn content_negotiation() {
    let mut app = App::new();
    app.post("/", |req: Request| async move {
        format!("json={} form={}", req.is("json"), req.is("urlencoded"))
    });
    app.get("/", |req: Request| async move {
        req.accepts(&["html", "json"]).unwrap_or("none").to_owned()
    });

    let req = http::Request::post("/")
        .header("content-type", "application/json")
        .body("{}")
        .unwrap();
    assert_eq!(
        text(app.handle(Request::from(req)).await).await,
        "json=true form=false"
    );
    let req = http::Request::get("/")
        .header("accept", "application/json")
        .body("")
        .unwrap();
    assert_eq!(text(app.handle(Request::from(req)).await).await, "json");
    let req = http::Request::get("/")
        .header("accept", "image/*")
        .body("")
        .unwrap();
    assert_eq!(text(app.handle(Request::from(req)).await).await, "none");
}

#[tokio::test]
async fn params_without_unwrap() {
    let mut app = App::new();
    app.get("/users/:id", |req| async move {
        format!("user {}", req.param("id"))
    });
    app.get(
        "/typo/:id",
        |req| async move { req.param("idd").to_owned() },
    );
    app.get("/maybe/:id", |req| async move {
        format!("{:?}", req.try_param("other"))
    });

    assert_eq!(
        text(call(&app, Method::GET, "/users/7").await).await,
        "user 7"
    );
    // A mistyped name is a bug in the app: 500, not a crash.
    assert_eq!(
        call(&app, Method::GET, "/typo/7").await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        text(call(&app, Method::GET, "/maybe/7").await).await,
        "None"
    );
}
