use std::{collections::HashMap, fmt, time::Duration, time::SystemTime};

use http::{HeaderMap, HeaderValue, header};

use crate::{Response, path::decode};

/// A cookie to send to the browser, like the options to Express's
/// `res.cookie()`. Set it on a response with
/// [`ResponseExt::with_cookie`].
///
/// ```
/// use std::time::Duration;
/// use rustyweb::{Cookie, IntoResponse, ResponseExt, SameSite};
///
/// let res = "Logged in".into_response().with_cookie(
///     Cookie::new("session", "abc123")
///         .http_only(true)
///         .secure(true)
///         .same_site(SameSite::Lax)
///         .max_age(Duration::from_secs(60 * 60 * 24)),
/// );
/// ```
///
/// Defaults: `Path=/`, no expiry (a session cookie), not `HttpOnly`, not
/// `Secure`. For login and session cookies, set `http_only(true)` and
/// `secure(true)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    name: String,
    value: String,
    path: Option<String>,
    domain: Option<String>,
    max_age: Option<Duration>,
    expires: Option<SystemTime>,
    http_only: bool,
    secure: bool,
    same_site: Option<SameSite>,
}

/// The `SameSite` cookie attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    /// Sent only with requests from your own site.
    Strict,
    /// Also sent when following a link to your site. The usual choice.
    Lax,
    /// Sent with all requests, including from other sites. Browsers require
    /// `secure(true)` with this.
    None,
}

impl Cookie {
    /// A cookie named `name` holding `value`. The value is percent-encoded
    /// when sent and decoded again by [`Request::cookie`](crate::Request::cookie),
    /// so any text is safe.
    ///
    /// # Panics
    ///
    /// Panics if `name` is empty or contains characters not allowed in
    /// cookie names (spaces, `=`, `;`, `,`, quotes and other separators).
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        assert!(
            !name.is_empty() && name.bytes().all(is_token_byte),
            "invalid cookie name {name:?}"
        );
        Cookie {
            name,
            value: value.into(),
            path: Some("/".to_owned()),
            domain: None,
            max_age: None,
            expires: None,
            http_only: false,
            secure: false,
            same_site: None,
        }
    }

    /// A cookie that tells the browser to delete `name`, like Express's
    /// `res.clearCookie()`. If the cookie was set with a custom path or
    /// domain, set the same ones here.
    pub fn removal(name: impl Into<String>) -> Self {
        let mut cookie = Cookie::new(name, "");
        cookie.max_age = Some(Duration::ZERO);
        cookie.expires = Some(SystemTime::UNIX_EPOCH);
        cookie
    }

    /// The URL path the cookie is sent for (default `/`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The domain the cookie is sent to, for example `example.com` to include
    /// subdomains. By default only the exact host that set it.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// How long the browser keeps the cookie.
    pub fn max_age(mut self, max_age: Duration) -> Self {
        self.max_age = Some(max_age);
        self
    }

    /// When the browser should delete the cookie. Prefer
    /// [`max_age`](Cookie::max_age).
    pub fn expires(mut self, at: SystemTime) -> Self {
        self.expires = Some(at);
        self
    }

    /// Hides the cookie from JavaScript in the browser. Use it for sessions.
    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    /// Sends the cookie over HTTPS only.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = Some(same_site);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// The `Set-Cookie` header value.
impl fmt::Display for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, encode(&self.value))?;
        if let Some(path) = &self.path {
            write!(f, "; Path={}", strip_separators(path))?;
        }
        if let Some(domain) = &self.domain {
            write!(f, "; Domain={}", strip_separators(domain))?;
        }
        if let Some(max_age) = self.max_age {
            write!(f, "; Max-Age={}", max_age.as_secs())?;
        }
        if let Some(expires) = self.expires {
            write!(f, "; Expires={}", httpdate::fmt_http_date(expires))?;
        }
        if self.http_only {
            f.write_str("; HttpOnly")?;
        }
        if self.secure {
            f.write_str("; Secure")?;
        }
        match self.same_site {
            Some(SameSite::Strict) => f.write_str("; SameSite=Strict")?,
            Some(SameSite::Lax) => f.write_str("; SameSite=Lax")?,
            Some(SameSite::None) => f.write_str("; SameSite=None")?,
            None => {}
        }
        Ok(())
    }
}

/// Cookie helpers for [`Response`], like Express's `res.cookie()` and
/// `res.clearCookie()`. Included in the prelude.
pub trait ResponseExt: Sized {
    /// Adds a `Set-Cookie` header.
    fn set_cookie(&mut self, cookie: Cookie);

    /// Adds a `Set-Cookie` header and returns the response, for chaining.
    fn with_cookie(mut self, cookie: Cookie) -> Self {
        self.set_cookie(cookie);
        self
    }

    /// Tells the browser to delete the cookie `name` (with `Path=/`). Use
    /// [`Cookie::removal`] for a cookie with a custom path or domain.
    fn clear_cookie(&mut self, name: &str) {
        self.set_cookie(Cookie::removal(name));
    }
}

impl ResponseExt for Response {
    fn set_cookie(&mut self, cookie: Cookie) {
        let value = HeaderValue::try_from(cookie.to_string())
            .expect("cookie attributes are valid header text");
        self.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// Parses the request's `Cookie` headers. Values are percent-decoded and
/// surrounding quotes removed; for repeated names the first one wins.
pub(crate) fn parse(headers: &HeaderMap) -> HashMap<String, String> {
    let mut cookies = HashMap::new();
    for header in headers.get_all(header::COOKIE) {
        let Ok(header) = header.to_str() else {
            continue;
        };
        for pair in header.split(';') {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            cookies
                .entry(name.to_owned())
                .or_insert_with(|| decode(value, false));
        }
    }
    cookies
}

/// Characters allowed in cookie names (RFC 6265 `token`).
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b)
}

/// Percent-encodes everything but unreserved URL characters, like
/// JavaScript's `encodeURIComponent`, which is what Express does.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Keeps user-supplied attributes from injecting extra attributes or
/// breaking the header.
fn strip_separators(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c != ';' && !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_header() {
        let cookie = Cookie::new("sid", "a b;c=1")
            .domain("example.com")
            .max_age(Duration::from_secs(60))
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Lax);
        assert_eq!(
            cookie.to_string(),
            "sid=a%20b%3Bc%3D1; Path=/; Domain=example.com; Max-Age=60; HttpOnly; Secure; SameSite=Lax"
        );
        assert_eq!(
            Cookie::removal("sid").to_string(),
            "sid=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
        );
        assert_eq!(
            Cookie::new("a", "b").path("/x; Secure").to_string(),
            "a=b; Path=/x Secure"
        );
    }

    #[test]
    #[should_panic(expected = "invalid cookie name")]
    fn bad_names_panic() {
        Cookie::new("a b", "c");
    }

    #[test]
    fn parsing() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            "a=1; b=\"two\"; c=x%20y; a=ignored".parse().unwrap(),
        );
        headers.append(header::COOKIE, "d=4;;=bad; e".parse().unwrap());
        let cookies = parse(&headers);
        assert_eq!(cookies["a"], "1");
        assert_eq!(cookies["b"], "two");
        assert_eq!(cookies["c"], "x y");
        assert_eq!(cookies["d"], "4");
        assert_eq!(cookies.len(), 4);
    }

    #[test]
    fn round_trip() {
        let cookie = Cookie::new("n", "héllo wörld; =&");
        let mut headers = HeaderMap::new();
        let sent = cookie.to_string();
        let value = sent.split(';').next().unwrap();
        headers.insert(header::COOKIE, value.parse().unwrap());
        assert_eq!(parse(&headers)["n"], "héllo wörld; =&");
    }
}
