<p align="center">
  <img src="assets/logo.png" alt="RustyWeb" width="220">
</p>

<h3 align="center">Build web servers in Rust the way you do in Express.</h3>

```rust
use rustyweb::prelude::*;

fn main() {
    let mut app = App::new();

    app.get("/", |_req| async { "Hello World!" });

    app.listen(3000);
}
```

## Get started

```sh
cargo new my-app
cd my-app
cargo add rustyweb
```

Paste the code above into `src/main.rs`, run `cargo run`, and open <http://localhost:3000>.

No Rust yet? Install it with one command from [rustup.rs](https://rustup.rs).

## Everything you need, in one example

```rust
use rustyweb::prelude::*;

// Read JSON and send JSON
async fn create_user(req: Request) -> Result<Json<Value>> {
    let body: Value = req.json()?; // like req.body
    Ok(Json(json!({ "created": body["name"] }))) // like res.json(...)
}

// Middleware: runs before the route, like app.use(fn)
async fn require_login(req: Request, next: Next) -> Result<Response> {
    if req.cookie("user").is_none() {
        return Err(Error::unauthorized("Please log in"));
    }
    Ok(next.run(req).await) // like next()
}

fn main() {
    let mut app = App::new();

    app.middleware(logger); // log every request
    app.middleware(static_files("public")); // serve your HTML, CSS, JS and images

    app.get("/users/:id", |req| async move {
        format!("User {}", req.param("id")) // like req.params.id
    });
    app.post("/users", create_user);
    app.get("/admin", |_req| async { "Welcome!" }).with(require_login);

    app.listen(3000);
}
```

## Express → RustyWeb

| Express | RustyWeb |
|---|---|
| `req.params.id` | `req.param("id")` |
| `req.query.q` | `req.query("q")` |
| `req.body` | `req.json()?` or `req.form()?` |
| `req.cookies.user` | `req.cookie("user")` |
| `res.send("Hi")` | return `"Hi"` |
| `res.json({ ok: true })` | return `Json(json!({ "ok": true }))` |
| `res.status(404).send("Nope")` | return `(StatusCode::NOT_FOUND, "Nope")` |
| `res.redirect("/login")` | return `Redirect::to("/login")` |
| `res.sendFile(path)` | return `File::send(path).await` |
| `res.cookie("user", "ada")` | `res.with_cookie(Cookie::new("user", "ada"))` |
| `app.use(fn)` | `app.middleware(fn)` |
| `express.static("public")` | `static_files("public")` |
| `express.Router()` | `Router::new()`, then `app.mount("/api", router)` |
| `next()` | `next.run(req).await` |
| error handler | `app.on_error(...)` |

More in the [Express guide](docs/express.md).

## Good to know

- **`async move`**: if the compiler asks you to add `move`, add it.
- **`?`**: means "if this failed, stop and send the error". For example bad JSON becomes a `400`
  automatically. Use it in functions that return `Result<...>`.
- **Auto-restart** when you save, like `nodemon`: `cargo install cargo-watch`, then `cargo watch -x run`.
- **Going live**: use `app.listen("0.0.0.0:3000")`, build with `cargo build --release`, and run
  `./target/release/my-app`.

## Built in

Logging, JSON, forms, cookies, static files, file downloads, custom error pages, browser
caching, protection against slow or oversized requests, and graceful shutdown. A bug in one
request returns a `500` and the server keeps running.

## Learn more

- [Express guide](docs/express.md): every common Express call, side by side
- [Reference](docs/reference.md): all features and settings
- [Writing packages](docs/packages.md): share middleware and routes as crates
- [Examples](examples/): run one with `cargo run --example hello`
- [API documentation](https://docs.rs/rustyweb)

## License

[MIT](LICENSE)
