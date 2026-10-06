#![allow(dead_code)]

use http_body_util::BodyExt;
use rustyweb::{App, Method, Request, Response};

pub async fn call(app: &App, method: Method, uri: &str) -> Response {
    send(app, method, uri, "").await
}

pub async fn send(app: &App, method: Method, uri: &str, body: &'static str) -> Response {
    let req = http::Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .unwrap();
    app.handle(Request::from(req)).await
}

pub async fn text(res: Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}
