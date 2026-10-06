mod common;

use std::path::PathBuf;

use common::{call, text};
use rustyweb::{App, File, Method, Next, Request, Response, StatusCode, header, static_files};

/// A fresh folder of test files, removed when dropped.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustyweb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("public/docs")).unwrap();
        std::fs::write(dir.join("public/index.html"), "<h1>home</h1>").unwrap();
        std::fs::write(dir.join("public/app.js"), "console.log(1)").unwrap();
        std::fs::write(dir.join("public/docs/index.html"), "docs").unwrap();
        std::fs::write(dir.join("public/.env"), "SECRET=1").unwrap();
        std::fs::write(dir.join("secret.txt"), "outside").unwrap();
        Fixture(dir)
    }

    fn public(&self) -> PathBuf {
        self.0.join("public")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn get(app: &App, uri: &str, headers: &[(&str, &str)]) -> Response {
    let mut req = http::Request::get(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    app.handle(Request::from(req.body("").unwrap())).await
}

#[tokio::test]
async fn static_files_serve_a_folder() {
    let fx = Fixture::new("static");
    let mut app = App::new();
    app.middleware(static_files(fx.public()));
    app.get("/api", |_req| async { "api route" });

    let res = call(&app, Method::GET, "/app.js").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "14");
    assert!(res.headers().contains_key(header::ETAG));
    assert!(res.headers().contains_key(header::LAST_MODIFIED));
    assert_eq!(text(res).await, "console.log(1)");

    assert_eq!(
        text(call(&app, Method::GET, "/").await).await,
        "<h1>home</h1>"
    );
    assert_eq!(text(call(&app, Method::GET, "/docs/").await).await, "docs");

    // A folder without its trailing slash redirects, so relative links work.
    let res = call(&app, Method::GET, "/docs?v=1").await;
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(res.headers()[header::LOCATION], "/docs/?v=1");
    // ... and never to another site.
    let res = call(&app, Method::GET, "//docs").await;
    assert_eq!(res.headers()[header::LOCATION], "/docs/");

    // Not a file: falls through to routes and the 404.
    assert_eq!(
        text(call(&app, Method::GET, "/api").await).await,
        "api route"
    );
    assert_eq!(
        call(&app, Method::GET, "/nope.js").await.status(),
        StatusCode::NOT_FOUND
    );
    // Only GET and HEAD.
    assert_eq!(
        call(&app, Method::POST, "/app.js").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn static_files_refuse_hidden_files_and_traversal() {
    let fx = Fixture::new("traversal");
    let mut app = App::new();
    app.middleware(static_files(fx.public()));

    for uri in [
        "/.env",
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/docs/..%2F..%2Fsecret.txt",
    ] {
        let res = call(&app, Method::GET, uri).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn static_files_under_a_prefix() {
    let fx = Fixture::new("prefix");
    let mut app = App::new();
    app.middleware_at("/assets", static_files(fx.public()));

    assert_eq!(
        text(call(&app, Method::GET, "/assets/app.js").await).await,
        "console.log(1)"
    );
    assert_eq!(
        text(call(&app, Method::GET, "/assets/").await).await,
        "<h1>home</h1>"
    );
    let res = call(&app, Method::GET, "/assets").await;
    assert_eq!(res.headers()[header::LOCATION], "/assets/");
    assert_eq!(
        call(&app, Method::GET, "/app.js").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::GET, "/assetsx/app.js").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn head_requests_get_headers_only() {
    let fx = Fixture::new("head");
    let mut app = App::new();
    app.middleware(static_files(fx.public()));

    let res = call(&app, Method::HEAD, "/app.js").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "14");
    assert_eq!(text(res).await, "");
}

#[tokio::test]
async fn repeat_requests_get_304() {
    let fx = Fixture::new("cache");
    let mut app = App::new();
    app.middleware(static_files(fx.public()));

    let first = call(&app, Method::GET, "/app.js").await;
    let etag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
    let modified = first.headers()[header::LAST_MODIFIED]
        .to_str()
        .unwrap()
        .to_owned();

    let res = get(&app, "/app.js", &[("if-none-match", &etag)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()[header::ETAG], etag.as_str());
    assert!(!res.headers().contains_key(header::CONTENT_LENGTH));
    assert_eq!(text(res).await, "");

    let res = get(&app, "/app.js", &[("if-modified-since", &modified)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let res = get(&app, "/app.js", &[("if-none-match", "\"other\"")]).await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = get(
        &app,
        "/app.js",
        &[("if-modified-since", "Thu, 01 Jan 1970 00:00:00 GMT")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn send_and_download_files() {
    let fx = Fixture::new("send");
    let public = fx.public();
    let mut app = App::new();
    let p = public.clone();
    app.get("/send", move |_req| {
        let p = p.clone();
        async move { File::send(p.join("app.js")).await }
    });
    let p = public.clone();
    app.get("/download", move |_req| {
        let p = p.clone();
        async move { File::download(p.join("app.js")).await }
    });
    let p = public.clone();
    app.get("/renamed", move |_req| {
        let p = p.clone();
        async move {
            File::send(p.join("app.js"))
                .await
                .map(|f| f.filename("script.js"))
        }
    });
    let p = public.clone();
    app.get("/docs/*", move |req| {
        let p = p.clone();
        async move { File::send_from(p, req.param("*")).await }
    });
    app.get("/missing", |_req| async {
        File::send("/definitely/not/here.txt").await
    });

    let res = call(&app, Method::GET, "/send").await;
    assert!(!res.headers().contains_key(header::CONTENT_DISPOSITION));
    assert_eq!(text(res).await, "console.log(1)");

    let res = call(&app, Method::GET, "/download").await;
    assert_eq!(
        res.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"app.js\""
    );

    let res = call(&app, Method::GET, "/renamed").await;
    assert_eq!(
        res.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"script.js\""
    );

    assert_eq!(
        text(call(&app, Method::GET, "/docs/docs/index.html").await).await,
        "docs"
    );
    assert_eq!(
        call(&app, Method::GET, "/docs/../../secret.txt")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::GET, "/docs/docs").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::GET, "/missing").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn base_path_is_set_only_inside_scoped_middleware() {
    let mut app = App::new();
    app.middleware_at("/api", |req: Request, next: Next| async move {
        let seen = req.base_path().to_owned();
        let mut res = next.run(req).await;
        res.headers_mut().insert("x-base", seen.parse().unwrap());
        res
    });
    app.get("/api/users", |req| async move {
        format!("handler sees {:?}", req.base_path())
    });
    app.get("/other", |_req| async { "other" });

    let res = call(&app, Method::GET, "/api/users").await;
    assert_eq!(res.headers()["x-base"], "/api");
    assert_eq!(text(res).await, "handler sees \"\"");
    let res = call(&app, Method::GET, "/other").await;
    assert!(!res.headers().contains_key("x-base"));
}

#[tokio::test]
async fn range_requests() {
    let fx = Fixture::new("range");
    std::fs::write(fx.public().join("digits.txt"), "0123456789").unwrap();
    let mut app = App::new();
    app.middleware(static_files(fx.public()));

    let full = call(&app, Method::GET, "/digits.txt").await;
    assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
    let etag = full.headers()[header::ETAG].to_str().unwrap().to_owned();

    let res = get(&app, "/digits.txt", &[("range", "bytes=2-5")]).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "4");
    assert_eq!(text(res).await, "2345");

    assert_eq!(
        text(get(&app, "/digits.txt", &[("range", "bytes=7-")]).await).await,
        "789"
    );
    assert_eq!(
        text(get(&app, "/digits.txt", &[("range", "bytes=-3")]).await).await,
        "789"
    );

    let res = get(&app, "/digits.txt", &[("range", "bytes=20-")]).await;
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes */10");

    // Multiple or malformed ranges: the whole file.
    let res = get(&app, "/digits.txt", &[("range", "bytes=0-1,4-5")]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(text(res).await, "0123456789");

    // If-Range: a range only if the file is unchanged.
    let res = get(
        &app,
        "/digits.txt",
        &[("range", "bytes=0-0"), ("if-range", &etag)],
    )
    .await;
    assert_eq!(text(res).await, "0");
    let res = get(
        &app,
        "/digits.txt",
        &[("range", "bytes=0-0"), ("if-range", "\"old\"")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(text(res).await, "0123456789");

    // Not when middleware changed the body (here: pretending to compress it).
    let mut app = App::new();
    app.middleware(|req: Request, next: Next| async move {
        let mut res = next.run(req).await;
        res.headers_mut()
            .insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        res
    });
    app.middleware(static_files(fx.public()));
    let res = get(&app, "/digits.txt", &[("range", "bytes=0-1")]).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Ranges only apply to files, not ordinary responses.
    let mut app = App::new();
    app.get("/", |_req| async { "0123456789" });
    let res = get(&app, "/", &[("range", "bytes=0-1")]).await;
    assert_eq!(res.status(), StatusCode::OK);
}
