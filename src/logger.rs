use std::{io::Write, time::Instant};

use crate::{Next, Request, Response};

/// Request-logging middleware, like Express's `morgan("dev")`.
///
/// ```no_run
/// let mut app = rustyweb::App::new();
/// app.middleware(rustyweb::logger);
/// ```
///
/// Prints one line per request to stdout:
///
/// ```text
/// GET /users/7 200 0.214 ms
/// ```
pub async fn logger(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let line = format!("{} {}", req.method(), req.uri());
    let res = next.run(req).await;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    // Not `println!`: that panics when stdout is closed (for example when
    // piped into a program that exited), and logging must never break a
    // request.
    let _ = writeln!(
        std::io::stdout(),
        "{line} {} {ms:.3} ms",
        res.status().as_u16()
    );
    res
}
