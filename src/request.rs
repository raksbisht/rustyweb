use bytes::Bytes;
use http::{Extensions, HeaderMap, Method, Uri, Version, request::Parts};

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use crate::{Error, path::decode, state::State};

/// An incoming HTTP request with its body fully buffered.
///
/// Middleware can attach typed data for later handlers through
/// [`extensions_mut`](Request::extensions_mut), the equivalent of setting
/// `req.user` in Express.
#[derive(Debug)]
pub struct Request {
    parts: Parts,
    body: Bytes,
    params: Vec<(String, String)>,
    state: Arc<State>,
    remote_addr: Option<SocketAddr>,
    base_path: String,
    trust_proxy: bool,
}

impl Request {
    pub(crate) fn from_parts(parts: Parts, body: Bytes) -> Self {
        Request {
            parts,
            body,
            params: Vec::new(),
            state: Arc::default(),
            remote_addr: None,
            base_path: String::new(),
            trust_proxy: false,
        }
    }

    pub(crate) fn set_state(&mut self, state: Arc<State>) {
        self.state = state;
    }

    pub(crate) fn set_remote_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = Some(addr);
    }

    pub(crate) fn set_trust_proxy(&mut self, trust: bool) {
        self.trust_proxy = trust;
    }

    /// The value a trusted proxy put in header `name`. Proxies append to
    /// these headers, so the **last** entry is the one the proxy in front of
    /// the app added; earlier entries came from the client and can be faked.
    fn forwarded(&self, name: &str) -> Option<&str> {
        if !self.trust_proxy {
            return None;
        }
        let value = self.header(name)?.rsplit(',').next()?.trim();
        (!value.is_empty()).then_some(value)
    }

    /// Replaces the base path, returning the old one.
    pub(crate) fn replace_base_path(&mut self, base: String) -> String {
        std::mem::replace(&mut self.base_path, base)
    }

    pub(crate) fn set_params(&mut self, params: Vec<(String, String)>) {
        self.params = params;
    }

    pub fn method(&self) -> &Method {
        &self.parts.method
    }

    pub fn uri(&self) -> &Uri {
        &self.parts.uri
    }

    pub fn version(&self) -> Version {
        self.parts.version
    }

    /// The request path, without the query string. Not percent-decoded.
    pub fn path(&self) -> &str {
        self.parts.uri.path()
    }

    /// Inside middleware added with
    /// [`App::middleware_at`](crate::App::middleware_at), the part of the
    /// path that matched its prefix, like Express's `req.baseUrl`. Empty
    /// everywhere else.
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    /// The address of the connected client, like Express's `req.ip`. `None`
    /// for requests that didn't come over a socket (for example in tests using
    /// [`App::handle`](crate::App::handle)).
    ///
    /// Behind a reverse proxy this is the proxy's address; use
    /// [`ip`](Request::ip) with [`App::trust_proxy`](crate::App::trust_proxy)
    /// to get the original client.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }

    /// The client's IP address, like Express's `req.ip`. With
    /// [`App::trust_proxy`](crate::App::trust_proxy) on, this is the address
    /// your proxy saw — the last entry in `X-Forwarded-For`, which clients
    /// can't fake; otherwise the connected socket's address.
    pub fn ip(&self) -> Option<IpAddr> {
        if let Some(ip) = self
            .forwarded("x-forwarded-for")
            .and_then(|ip| ip.parse().ok())
        {
            return Some(ip);
        }
        self.remote_addr.map(|addr| addr.ip())
    }

    /// The host name the client asked for, without the port, like Express's
    /// `req.hostname`. Taken from `X-Forwarded-Host` when
    /// [`trust_proxy`](crate::App::trust_proxy) is on, otherwise from the
    /// `Host` header.
    pub fn hostname(&self) -> Option<&str> {
        let host = self
            .forwarded("x-forwarded-host")
            .or_else(|| self.header("host"))
            .or_else(|| self.parts.uri.host())?;
        // Strip the port, taking care with IPv6 addresses like `[::1]:3000`.
        let host = match host.strip_prefix('[') {
            Some(rest) => &host[..rest.find(']').map_or(host.len(), |i| i + 2)],
            None => host.split(':').next().unwrap_or(host),
        };
        (!host.is_empty()).then_some(host)
    }

    /// `"https"` or `"http"`, like Express's `req.protocol`. Taken from
    /// `X-Forwarded-Proto` when [`trust_proxy`](crate::App::trust_proxy) is
    /// on; otherwise `"http"`, since RustyWeb itself serves plain HTTP.
    pub fn protocol(&self) -> &str {
        match self.forwarded("x-forwarded-proto") {
            Some(proto) if proto.eq_ignore_ascii_case("https") => "https",
            Some(_) => "http",
            // RustyWeb itself only speaks plain HTTP. (A client-sent absolute
            // URL like `https://...` must not make this "https".)
            None => "http",
        }
    }

    /// Whether the request came over HTTPS (`protocol() == "https"`), like
    /// Express's `req.secure`.
    pub fn secure(&self) -> bool {
        self.protocol() == "https"
    }

    /// A route parameter captured by `:name` (or `*` for a wildcard),
    /// percent-decoded, like Express's `req.params.name`.
    ///
    /// ```
    /// # let mut app = rustyweb::App::new();
    /// app.get("/users/:id", |req| async move { format!("user {}", req.param("id")) });
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the route has no parameter called `name` — almost always a
    /// typo, since a route's parameters are always present. The request then
    /// gets a `500` and the message names the parameters the route does
    /// have. Use [`try_param`](Request::try_param) to check instead.
    ///
    /// Params are decoded user input. A `*` param can contain `..` segments
    /// (`/files/../secret` gives `../secret`), so never join it onto a
    /// filesystem path yourself; use [`File::send_from`](crate::File::send_from).
    pub fn param(&self, name: &str) -> &str {
        self.try_param(name).unwrap_or_else(|| {
            let known: Vec<&str> = self.params.iter().map(|(k, _)| k.as_str()).collect();
            panic!("no route parameter `{name}`; this route has: {known:?}")
        })
    }

    /// Like [`param`](Request::param), but returns `None` instead of
    /// panicking when there is no parameter called `name`.
    pub fn try_param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// All captured route parameters, in pattern order.
    pub fn params(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The first value of query parameter `name`, percent-decoded.
    pub fn query(&self, name: &str) -> Option<String> {
        self.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// All query parameters in order, percent-decoded. Keys without `=` get
    /// an empty value.
    pub fn query_pairs(&self) -> impl Iterator<Item = (String, String)> + '_ {
        parse_pairs(self.parts.uri.query().unwrap_or(""))
    }

    /// Deserializes the query string into `T`. Missing optional fields are
    /// fine; anything that doesn't fit `T` (a word where a number belongs)
    /// fails with `400 Bad Request`.
    ///
    /// ```
    /// #[derive(serde::Deserialize)]
    /// struct Page {
    ///     page: Option<u32>,
    ///     sort: Option<String>,
    /// }
    ///
    /// async fn list(req: rustyweb::Request) -> Result<String, rustyweb::Error> {
    ///     let q: Page = req.query_as()?; // /users?page=2&sort=name
    ///     Ok(format!("page {}", q.page.unwrap_or(1)))
    /// }
    /// ```
    #[cfg(feature = "form")]
    pub fn query_as<T: serde::de::DeserializeOwned>(&self) -> Result<T, Error> {
        serde_urlencoded::from_str(self.parts.uri.query().unwrap_or(""))
            .map_err(|e| Error::bad_request(format!("invalid query string: {e}")))
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.parts.headers
    }

    /// The value of header `name`, if present and valid visible ASCII.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.parts.headers.get(name)?.to_str().ok()
    }

    /// The value of cookie `name`, percent-decoded, like Express's
    /// `req.cookies.name` (with `cookie-parser`). Set cookies with
    /// [`ResponseExt::with_cookie`](crate::ResponseExt::with_cookie).
    pub fn cookie(&self, name: &str) -> Option<String> {
        crate::cookie::parse(&self.parts.headers).remove(name)
    }

    /// All cookies sent with the request.
    pub fn cookies(&self) -> std::collections::HashMap<String, String> {
        crate::cookie::parse(&self.parts.headers)
    }

    /// Whether the request body's `Content-Type` matches `ty`, like Express's
    /// `req.is()`. `ty` can be a shorthand (`"json"`, `"html"`, `"text"`,
    /// `"urlencoded"`, `"multipart"`, ...), a full type (`"text/html"`) or a
    /// wildcard (`"image/*"`).
    pub fn is(&self, ty: &str) -> bool {
        crate::negotiate::is(self.header("content-type").unwrap_or(""), ty)
    }

    /// Picks which of `offered` the client prefers according to its `Accept`
    /// header, like Express's `req.accepts()`. Returns `None` if it accepts
    /// none of them, and the first one if it sent no `Accept` header.
    ///
    /// ```
    /// # use rustyweb::{Html, IntoResponse, Request, Response};
    /// async fn user(req: Request) -> Response {
    ///     match req.accepts(&["html", "text"]) {
    ///         Some("html") => Html("<h1>Ada</h1>").into_response(),
    ///         _ => "Ada".into_response(),
    ///     }
    /// }
    /// ```
    pub fn accepts<'a>(&self, offered: &[&'a str]) -> Option<&'a str> {
        crate::negotiate::accepts(self.header("accept"), offered)
    }

    /// Takes over the connection after this response, for protocols like
    /// WebSockets, like listening for Node's `upgrade` event. Returns `None`
    /// if the request can't be upgraded (it isn't HTTP/1.1 over a socket, or
    /// it was already taken).
    ///
    /// Respond with `101 Switching Protocols` and the protocol's headers; the
    /// returned future resolves to the raw connection once that response is
    /// sent. See the [`upgrade`](crate::upgrade) module.
    pub fn upgrade(&mut self) -> Option<crate::upgrade::OnUpgrade> {
        self.parts.extensions.remove::<crate::upgrade::OnUpgrade>()
    }

    /// The raw request body.
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// The request body as UTF-8 text. Fails with `400 Bad Request` otherwise.
    pub fn text(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.body)
            .map_err(|_| Error::bad_request("request body is not valid UTF-8"))
    }

    /// Deserializes the request body as JSON. Fails with `400 Bad Request`
    /// if the body is not valid JSON for `T`.
    #[cfg(feature = "json")]
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::bad_request(format!("invalid JSON body: {e}")))
    }

    /// The fields of an HTML form body (`application/x-www-form-urlencoded`),
    /// like `req.body` after Express's `express.urlencoded()`. When a field
    /// appears more than once, the last value wins; use
    /// [`form_pairs`](Request::form_pairs) to see every value.
    ///
    /// Fails with `400 Bad Request` if the body isn't UTF-8.
    pub fn form(&self) -> Result<std::collections::HashMap<String, String>, Error> {
        Ok(self.form_pairs()?.into_iter().collect())
    }

    /// All fields of a form body in order, including repeated ones.
    pub fn form_pairs(&self) -> Result<Vec<(String, String)>, Error> {
        Ok(parse_pairs(self.text()?).collect())
    }

    /// Deserializes a form body into `T`, like [`json`](Request::json) does
    /// for JSON. Fails with `400 Bad Request` if the body doesn't fit `T`.
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Login { user: String, password: String }
    /// async fn login(req: rustyweb::Request) -> Result<String, rustyweb::Error> {
    ///     let form: Login = req.form_as()?;
    ///     Ok(format!("hello {}", form.user))
    /// }
    /// ```
    #[cfg(feature = "form")]
    pub fn form_as<T: serde::de::DeserializeOwned>(&self) -> Result<T, Error> {
        serde_urlencoded::from_bytes(&self.body)
            .map_err(|e| Error::bad_request(format!("invalid form body: {e}")))
    }

    /// A value shared across the whole app, added with
    /// [`App::state`](crate::App::state) — like Express's `app.locals`.
    ///
    /// # Panics
    ///
    /// Panics if no value of type `T` was added. The request then gets a
    /// `500`. Use [`try_state`](Request::try_state) if the value is optional.
    pub fn state<T: Send + Sync + 'static>(&self) -> &T {
        self.try_state().unwrap_or_else(|| {
            panic!(
                "no app state of type `{}`; add it with `app.state(...)`",
                std::any::type_name::<T>()
            )
        })
    }

    /// Like [`state`](Request::state), but returns `None` if no value of type
    /// `T` was added.
    pub fn try_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    pub fn extensions(&self) -> &Extensions {
        &self.parts.extensions
    }

    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.parts.extensions
    }
}

/// Parses `a=1&b=2` (query strings and form bodies), percent-decoding both
/// sides and treating `+` as a space.
fn parse_pairs(input: &str) -> impl Iterator<Item = (String, String)> + '_ {
    input.split('&').filter(|s| !s.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(k, true), decode(v, true))
    })
}

/// Builds a `Request` from an [`http::Request`], mainly for tests driven
/// through [`App::handle`](crate::App::handle).
impl<B: Into<Bytes>> From<http::Request<B>> for Request {
    fn from(req: http::Request<B>) -> Self {
        let (parts, body) = req.into_parts();
        Request::from_parts(parts, body.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing() {
        let req = Request::from(
            http::Request::get("/s?q=a+b%21&flag&q=second&empty=")
                .body("")
                .unwrap(),
        );
        assert_eq!(req.query("q").as_deref(), Some("a b!"));
        assert_eq!(req.query("flag").as_deref(), Some(""));
        assert_eq!(req.query("empty").as_deref(), Some(""));
        assert_eq!(req.query("missing"), None);
        assert_eq!(req.query_pairs().count(), 4);
    }

    #[test]
    fn form_body() {
        let req = Request::from(
            http::Request::post("/")
                .body("name=Ada+Lovelace&lang=rust&lang=c%2B%2B&empty=")
                .unwrap(),
        );
        let form = req.form().unwrap();
        assert_eq!(form["name"], "Ada Lovelace");
        assert_eq!(form["lang"], "c++");
        assert_eq!(form["empty"], "");
        assert_eq!(req.form_pairs().unwrap().len(), 4);

        let req = Request::from(http::Request::post("/").body(vec![0xff]).unwrap());
        assert!(req.form().is_err());
    }

    #[test]
    fn text_body() {
        let req = Request::from(http::Request::post("/").body("hi").unwrap());
        assert_eq!(req.text().unwrap(), "hi");
        let req = Request::from(http::Request::post("/").body(vec![0xff]).unwrap());
        assert!(req.text().is_err());
    }
}
