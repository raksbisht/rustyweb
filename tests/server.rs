//! End-to-end tests over a real socket, speaking raw HTTP/1.1.

use rustyweb::{App, Request};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

async fn start(
    app: App,
) -> (
    std::net::SocketAddr,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        app.serve(listener, async {
            rx.await.ok();
        })
        .await
        .unwrap();
    });
    (addr, tx, server)
}

async fn raw(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn serves_requests_and_enforces_body_limit() {
    let mut app = App::new();
    app.body_limit(8);
    app.get("/users/:id", |req: Request| async move {
        format!("user {}", req.param("id"))
    });
    app.post("/echo", |req: Request| async move { req.body().clone() });
    app.get("/ip", |req: Request| async move {
        req.remote_addr().unwrap().ip().to_string()
    });
    let (addr, stop, server) = start(app).await;

    let res = raw(
        addr,
        "GET /ip HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(res.ends_with("\r\n\r\n127.0.0.1"), "{res}");

    let res = raw(
        addr,
        "GET /users/7 HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200 OK\r\n"), "{res}");
    assert!(res.ends_with("\r\n\r\nuser 7"), "{res}");

    let res = raw(
        addr,
        "POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\nconnection: close\r\n\r\nping",
    )
    .await;
    assert!(res.ends_with("\r\n\r\nping"), "{res}");

    let res = raw(
        addr,
        "POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 9\r\nconnection: close\r\n\r\n123456789",
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 413"), "{res}");

    let res = raw(
        addr,
        "HEAD /users/7 HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200 OK\r\n"), "{res}");
    assert!(res.contains("content-length: 6\r\n"), "{res}");
    assert!(res.ends_with("\r\n\r\n"), "{res}");

    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn keep_alive_serves_multiple_requests_on_one_connection() {
    let mut app = App::new();
    app.get("/", |_req| async { "hi" });
    let (addr, stop, server) = start(app).await;

    let res = raw(
        addr,
        "GET / HTTP/1.1\r\nhost: x\r\n\r\nGET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(res.matches("HTTP/1.1 200 OK").count(), 2, "{res}");

    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
#[should_panic(expected = "use `app.listen_async(addr).await` instead")]
async fn blocking_listen_inside_async_code_explains_the_fix() {
    App::new().listen(0);
}

#[tokio::test]
async fn on_error_covers_oversized_bodies() {
    let mut app = App::new();
    app.body_limit(4);
    app.on_error(|err: rustyweb::Error| async move { (err.status(), "too big, sorry") });
    app.post("/", |_req| async { "ok" });
    let (addr, stop, server) = start(app).await;

    let res = raw(
        addr,
        "POST / HTTP/1.1\r\nhost: x\r\ncontent-length: 9\r\nconnection: close\r\n\r\n123456789",
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 413"), "{res}");
    assert!(res.ends_with("too big, sorry"), "{res}");

    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn streams_responses_chunk_by_chunk() {
    use rustyweb::Body;
    use std::time::Duration;

    let mut app = App::new();
    app.get("/stream", |_req| async {
        let chunks = futures_util::stream::unfold(0, |n| async move {
            if n == 3 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok::<_, std::io::Error>(format!("chunk{n};")), n + 1))
        });
        Body::from_stream(chunks)
    });
    let (addr, stop, server) = start(app).await;

    let res = raw(
        addr,
        "GET /stream HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(res.contains("transfer-encoding: chunked"), "{res}");
    for n in 0..3 {
        assert!(res.contains(&format!("chunk{n};")), "{res}");
    }

    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn connections_can_be_upgraded() {
    use rustyweb::{IntoResponse, StatusCode, header, upgrade::TokioIo};

    let mut app = App::new();
    app.get("/echo", |mut req: Request| async move {
        let Some(on_upgrade) = req.upgrade() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        tokio::spawn(async move {
            let mut io = TokioIo::new(on_upgrade.await.unwrap());
            let mut buf = [0; 64];
            while let Ok(n @ 1..) = io.read(&mut buf).await {
                io.write_all(&buf[..n]).await.unwrap();
            }
        });
        let mut res = StatusCode::SWITCHING_PROTOCOLS.into_response();
        res.headers_mut()
            .insert(header::UPGRADE, "echo".parse().unwrap());
        res.headers_mut()
            .insert(header::CONNECTION, "upgrade".parse().unwrap());
        res
    });
    let (addr, stop, server) = start(app).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /echo HTTP/1.1\r\nhost: x\r\nconnection: upgrade\r\nupgrade: echo\r\n\r\n")
        .await
        .unwrap();
    let mut head = vec![0; 256];
    let n = stream.read(&mut head).await.unwrap();
    let head = String::from_utf8_lossy(&head[..n]);
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols"),
        "{head}"
    );

    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    drop(stream);

    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn slow_bodies_time_out() {
    use std::time::Duration;

    let mut app = App::new();
    app.body_timeout(Duration::from_millis(200));
    app.post("/", |_req| async { "ok" });
    let (addr, stop, server) = start(app).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    // Promise 10 bytes, send 1, then stall.
    stream
        .write_all(b"POST / HTTP/1.1\r\nhost: x\r\ncontent-length: 10\r\n\r\nx")
        .await
        .unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out).await;
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");

    stop.send(()).unwrap();
    server.await.unwrap();
}
