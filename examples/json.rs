//! `cargo run --example json`, then:
//!
//! ```text
//! curl -d '{"name":"Ada"}' localhost:3000/greet
//! ```

use rustyweb::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct GreetRequest {
    name: String,
}

#[derive(Serialize)]
struct GreetResponse {
    message: String,
}

async fn greet(req: Request) -> Result<Json<GreetResponse>, Error> {
    let body: GreetRequest = req.json()?; // 400 if the body isn't valid JSON
    Ok(Json(GreetResponse {
        message: format!("hello, {}", body.name),
    }))
}

fn main() {
    let mut app = App::new();
    app.middleware(logger);
    app.post("/greet", greet);
    app.listen(3000);
}
