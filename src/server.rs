use std::{
    convert::Infallible, future::Future, io, io::Write, net::SocketAddr, sync::Arc, time::Duration,
};

use http::StatusCode;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::{
    net::TcpListener,
    sync::{mpsc, watch},
};

use crate::{App, Error, IntoResponse, Request, Response};

/// Clients must send their request headers within this time, so slow or idle
/// clients can't hold connections open forever.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// After shutdown is requested, how long in-flight requests get to finish.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// An address [`App::listen`] can bind to.
///
/// - a port number, like `3000`: listens on `localhost` only
/// - a string, like `"0.0.0.0:3000"`: listens on that address (`0.0.0.0` makes
///   the app reachable from other machines, which you want when deploying)
/// - a [`SocketAddr`]
pub trait ListenAddr {
    fn to_listen_addr(&self) -> String;
}

impl ListenAddr for u16 {
    fn to_listen_addr(&self) -> String {
        format!("127.0.0.1:{self}")
    }
}

impl ListenAddr for &str {
    fn to_listen_addr(&self) -> String {
        (*self).to_owned()
    }
}

impl ListenAddr for String {
    fn to_listen_addr(&self) -> String {
        self.clone()
    }
}

impl ListenAddr for SocketAddr {
    fn to_listen_addr(&self) -> String {
        self.to_string()
    }
}

impl App {
    /// Starts the server and blocks until Ctrl-C, like Express's
    /// `app.listen(3000)`. In-flight requests finish before it returns.
    ///
    /// ```no_run
    /// let mut app = rustyweb::App::new();
    /// app.get("/", |_req| async { "Hello World!" });
    /// app.listen(3000); // http://localhost:3000
    /// ```
    ///
    /// No `async fn main` is needed. If the address can't be bound (for
    /// example, the port is already in use) it prints the error and exits the
    /// process with status 1. Inside async code, use
    /// [`listen_async`](App::listen_async) instead.
    pub fn listen(self, addr: impl ListenAddr) {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "App::listen blocks and can't be called from async code; use `app.listen_async(addr).await` instead"
        );
        let addr = addr.to_listen_addr();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the async runtime");
        if let Err(e) = runtime.block_on(self.listen_async(addr.as_str())) {
            let _ = writeln!(std::io::stderr(), "error: could not listen on {addr}: {e}");
            std::process::exit(1);
        }
    }

    /// Like [`listen`](App::listen), for use inside async code such as
    /// `#[tokio::main] async fn main()`. Returns errors instead of exiting.
    pub async fn listen_async(self, addr: impl ListenAddr) -> io::Result<()> {
        let listener = TcpListener::bind(addr.to_listen_addr()).await?;
        let local = listener.local_addr()?;
        // `writeln!` rather than `println!`, which panics if stdout is closed.
        let mut stdout = std::io::stdout();
        let _ = if local.ip().is_loopback() {
            writeln!(stdout, "Listening on http://localhost:{}", local.port())
        } else {
            writeln!(stdout, "Listening on http://{local}")
        };
        self.serve(listener, ctrl_c()).await
    }

    /// Serves requests on an already-bound `listener` until `shutdown`
    /// completes, then waits for in-flight requests to finish.
    ///
    /// Useful for tests (bind to port 0 and read the port back from the
    /// listener) or for custom shutdown signals.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let app = Arc::new(self);
        let mut shutdown = std::pin::pin!(shutdown);
        // Tells connections to wind down once shutdown starts.
        let (stopping_tx, stopping_rx) = watch::channel(false);
        // Every connection task holds a clone; when all are dropped,
        // `recv` returns `None` and shutdown is complete.
        let (alive_tx, mut alive_rx) = mpsc::channel::<()>(1);

        loop {
            let (stream, remote) = tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok(accepted) => accepted,
                    // Usually out of file descriptors; back off instead of
                    // spinning or taking the whole server down.
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                },
                () = &mut shutdown => break,
            };

            let app = app.clone();
            let service = service_fn(move |req| {
                let app = app.clone();
                async move { Ok::<_, Infallible>(app.handle_hyper(req, remote).await) }
            });
            let conn = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            let mut stopping = stopping_rx.clone();
            let alive = alive_tx.clone();
            tokio::spawn(async move {
                let _alive = alive;
                let mut conn = std::pin::pin!(conn);
                // Errors here are client-side (reset connections, bad
                // requests); there is nobody to report them to.
                tokio::select! {
                    _ = conn.as_mut() => return,
                    _ = stopping.wait_for(|stopping| *stopping) => {}
                }
                // Finish the request in progress, then close.
                conn.as_mut().graceful_shutdown();
                let _ = conn.await;
            });
        }

        drop(listener);
        let _ = stopping_tx.send(true);
        drop(alive_tx);
        // Idle connections close at once; busy ones get a grace period so a
        // stuck request can't keep the process alive forever.
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, alive_rx.recv()).await;
        Ok(())
    }

    async fn handle_hyper(&self, req: hyper::Request<Incoming>, remote: SocketAddr) -> Response {
        let (parts, body) = req.into_parts();
        let collect = Limited::new(body, self.body_limit).collect();
        let Ok(collected) = tokio::time::timeout(self.body_timeout, collect).await else {
            let err = Error::new(StatusCode::REQUEST_TIMEOUT, "Request Timeout");
            return self.finish(err.into_response()).await;
        };
        let body = match collected {
            Ok(collected) => collected.to_bytes(),
            Err(e) if e.is::<LengthLimitError>() => {
                let err = Error::new(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large");
                return self.finish(err.into_response()).await;
            }
            Err(_) => {
                return self
                    .finish(Error::bad_request("Bad Request").into_response())
                    .await;
            }
        };
        let mut req = Request::from_parts(parts, body);
        req.set_remote_addr(remote);
        self.handle(req).await
    }
}

async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        // No signal handler available: run until the process is killed.
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addrs() {
        assert_eq!(3000.to_listen_addr(), "127.0.0.1:3000");
        assert_eq!("0.0.0.0:80".to_listen_addr(), "0.0.0.0:80");
        assert_eq!(String::from("[::1]:1").to_listen_addr(), "[::1]:1");
        let addr: SocketAddr = "10.0.0.1:9".parse().unwrap();
        assert_eq!(addr.to_listen_addr(), "10.0.0.1:9");
    }
}
