# RustyWeb for Express developers

If you know Express, you already know most of RustyWeb. This guide gets you from zero to a
running app, then maps Express code to RustyWeb code.

## 1. Set up (one time)

Install Rust (this gives you `cargo`, Rust's `npm`):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

On Windows, use the installer from [rustup.rs](https://rustup.rs) instead.

Optional: auto-restart on save, like `nodemon`:

```sh
cargo install cargo-watch
```

## 2. Create an app

```sh
cargo new my-app            # like: mkdir my-app && npm init
cd my-app
cargo add rustyweb         # like: npm install express
```

Replace `src/main.rs` with:

```rust
use rustyweb::prelude::*;

fn main() {
    let mut app = App::new();
    app.middleware(logger);

    app.get("/", |_req| async { "Hello World!" });

    app.listen(3000);
}
```

## 3. Run it

```sh
cargo run            # like: node index.js
cargo watch -x run   # like: nodemon index.js
```

Open <http://localhost:3000>. The first build takes a minute while dependencies
compile; later builds are fast.

## Cheat sheet

| Express | RustyWeb |
|---|---|
| `const app = express()` | `let mut app = App::new();` |
| `app.listen(3000)` | `app.listen(3000);` |
| `app.get("/", (req, res) => res.send("hi"))` | `app.get("/", \|_req\| async { "hi" });` |
| `app.post` / `put` / `patch` / `delete` / `all` | same names |
| `req.params.id` | `req.param("id")` |
| `req.query.q` | `req.query("q")` |
| typed `req.query` | `req.query_as::<T>()?` |
| `req.is("json")` | `req.is("json")` |
| `req.accepts(["html", "json"])` | `req.accepts(&["html", "json"])` |
| `req.get("authorization")` | `req.header("authorization")` |
| `req.ip` | `req.ip()` |
| `req.hostname` / `req.protocol` / `req.secure` | `req.hostname()` / `req.protocol()` / `req.secure()` |
| `app.set("trust proxy", true)` | `app.trust_proxy(true);` |
| `app.set("etag", false)` | `app.etag(false);` |
| `server.on("upgrade", ...)` | `req.upgrade()` in a handler |
| `req.cookies.sid` (cookie-parser) | `req.cookie("sid")` |
| `app.use(express.json())` + `req.body` | `let body: Value = req.json()?;` then `body["name"]` (JSON is built in) |
| `req.body` as text | `req.text()?` |
| `app.use(express.urlencoded())` + `req.body` | `req.form()?` or `req.form_as::<T>()?` (built in) |
| `res.send("hi")` | return `"hi"` |
| `res.json({ ok: true })` | return `Json(json!({ "ok": true }))` |
| `res.json(obj)` with a typed object | return `Json(obj)` |
| `res.status(201).json(obj)` | return `(StatusCode::CREATED, Json(obj))` |
| `res.sendStatus(204)` | return `StatusCode::NO_CONTENT` |
| `res.redirect("/login")` | return `Redirect::to("/login")` |
| `res.cookie("sid", id, { httpOnly: true })` | `res.with_cookie(Cookie::new("sid", id).http_only(true))` |
| `res.clearCookie("sid")` | `res.clear_cookie("sid")` |
| `res.sendFile(path)` | return `File::send(path).await` |
| `res.download(path)` | return `File::download(path).await` |
| `app.use(express.static("public"))` | `app.middleware(static_files("public"));` |
| `app.use("/assets", express.static("public"))` | `app.middleware_at("/assets", static_files("public"));` |
| `res.write(...)` streaming | return `Body::from_stream(stream)` |
| `res.set("x-a", "1")` | in middleware: `res.headers_mut().insert("x-a", ...)` |
| `app.use(morgan("dev"))` | `app.middleware(logger);` |
| `app.use("/api", (req, res, next) => ...)` | `app.middleware_at("/api", mw);` |
| `app.use((req, res, next) => ...)` | `app.middleware(\|req: Request, next: Next\| async move { ... });` |
| `next()` | `next.run(req).await` |
| `app.get("/admin", auth, handler)` | `app.get("/admin", handler).with(auth);` |
| `res.locals.user = user` | `req.extensions_mut().insert(user)` |
| `app.locals.db = db` / `app.set("db", db)` | `app.state(db);` then `req.state::<Db>()` |
| `app.use((err, req, res, next) => ...)` | `app.on_error(\|err: Error\| async move { ... });` |
| `app.use(cors({ origin }))` | `app.middleware(cors(options));` (see the [package guide](packages.md)) |
| `const r = express.Router()` | `let mut r = Router::new();` |
| `app.use("/api", r)` | `app.mount("/api", r);` |
| `app.route("/users").get(h).post(h)` | `app.route("/users").get(h).post(h);` |
| `app.use((req, res) => res.status(404)...)` | `app.fallback(\|req\| async { ... });` |
| `app.get("/files/*", ...)` + `req.params[0]` | `app.get("/files/*", ...)` + `req.param("*")` |

## A typical JSON API

```rust
use rustyweb::prelude::*;
use serde::{Deserialize, Serialize}; // cargo add serde --features derive

#[derive(Deserialize)]
struct NewUser {
    name: String,
}

#[derive(Serialize)]
struct User {
    id: u32,
    name: String,
}

async fn create_user(req: Request) -> Result<(StatusCode, Json<User>), Error> {
    let input: NewUser = req.json()?; // bad JSON -> 400 automatically
    Ok((StatusCode::CREATED, Json(User { id: 1, name: input.name })))
}

fn main() {
    let mut app = App::new();
    app.middleware(logger);
    app.post("/users", create_user);
    app.listen(3000);
}
```

## What's different from Express

**Handlers return the response.** There is no `res` object. Whatever the handler
returns becomes the response: a string, `Json(..)`, a status code, or a tuple of
status and body.

**Middleware wraps the request.** Instead of calling `next()` and changing `res`,
middleware calls `next.run(req).await`, gets the response back, and returns it.
That makes "do something after the handler" easy:

```rust
async fn timing(req: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let mut res = next.run(req).await;          // the handler runs here
    let ms = start.elapsed().as_millis().to_string();
    res.headers_mut().insert("x-time-ms", ms.parse().unwrap());
    res
}
```

To stop the request (like not calling `next()`), return a response without calling
`next.run`.

**Optional values are `Option`s.** `req.param("id")` just gives you the value,
since `:id` is always there for `/users/:id`. Query strings are optional, so
`req.query("q")` returns an `Option`, Rust's version of "maybe `undefined`": use
`req.query("q").unwrap_or_default()` for "the value, or empty if missing".

**`async move`.** If your handler uses `req`, write `|req| async move { ... }`.
The `move` hands `req` to the async block. Without it, the compiler will tell you.

**Errors use `?`.** Return `Result<T, Error>` from a handler and use `?` on anything
that can fail with an `Error`, like `req.json()?`. Make your own with
`Error::bad_request("...")`, `Error::not_found("...")` and so on.

**Errors you didn't plan for become 500s.** If a handler panics (Rust's version of
throwing), that request gets a `500` and the server keeps running.

## Packages

Like Express, the core is small and features come from packages. Middleware
packages plug in with `app.middleware(...)`, and bigger packages (sessions, auth,
admin panels) export a router or a `setup(&mut app, ...)` function. To write your
own, see the [package guide](packages.md).

## More examples

The [`examples/`](../examples) folder has runnable apps:

```sh
cargo run --example hello        # routes, params, query strings
cargo run --example middleware   # auth, passing data to handlers
cargo run --example router       # express.Router()-style grouping
cargo run --example json         # a JSON API
cargo run --example packages     # writing and using packages
```
