//! Route patterns and URL decoding.
//!
//! A pattern is a `/`-separated list of segments:
//!
//! - `users`  — matches the literal segment `users`
//! - `:id`    — matches any single segment and captures it as `id`
//! - `*`      — only allowed last; matches the rest of the path (possibly empty)
//!   and captures it as `*`
//!
//! Leading, trailing and repeated slashes are ignored, so `/users`, `/users/`
//! and `//users` are equivalent. Matching is case-sensitive.

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Param(String),
    Wildcard,
}

#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    segments: Vec<Segment>,
}

impl Pattern {
    /// Parses a route pattern.
    ///
    /// # Panics
    ///
    /// Panics on an invalid pattern (an unnamed `:` param or a `*` that is
    /// not the last segment). Routes are registered at startup, so this is a
    /// programming error rather than a runtime condition.
    pub(crate) fn parse(pattern: &str) -> Self {
        let raw: Vec<&str> = split(pattern).collect();
        let mut segments = Vec::with_capacity(raw.len());
        for (i, seg) in raw.iter().enumerate() {
            let segment = if *seg == "*" {
                assert!(
                    i == raw.len() - 1,
                    "invalid route pattern {pattern:?}: `*` must be the last segment"
                );
                Segment::Wildcard
            } else if let Some(name) = seg.strip_prefix(':') {
                assert!(
                    !name.is_empty(),
                    "invalid route pattern {pattern:?}: `:` parameter needs a name"
                );
                Segment::Param(name.to_owned())
            } else {
                Segment::Literal(decode(seg, false))
            };
            segments.push(segment);
        }
        Pattern { segments }
    }

    /// Returns a new pattern with `prefix` in front of `self`.
    pub(crate) fn prefixed(&self, prefix: &Pattern) -> Pattern {
        assert!(
            prefix.segments.last() != Some(&Segment::Wildcard),
            "a mount prefix cannot end in `*`"
        );
        let mut segments = prefix.segments.clone();
        segments.extend(self.segments.iter().cloned());
        Pattern { segments }
    }

    /// Matches a request path, returning captured params on success.
    pub(crate) fn matches(&self, path: &str) -> Option<Vec<(String, String)>> {
        let parts: Vec<&str> = split(path).collect();
        let mut params = Vec::new();
        for (i, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Wildcard => {
                    params.push((
                        "*".to_owned(),
                        decode(&parts.get(i..).unwrap_or_default().join("/"), false),
                    ));
                    return Some(params);
                }
                Segment::Literal(lit) => {
                    if decode(parts.get(i)?, false) != *lit {
                        return None;
                    }
                }
                Segment::Param(name) => {
                    params.push((name.clone(), decode(parts.get(i)?, false)));
                }
            }
        }
        (parts.len() == self.segments.len()).then_some(params)
    }
}

impl Pattern {
    /// Matches the start of a request path, segment by segment, returning
    /// how many bytes of `path` the prefix covers. `/api` matches `/api` and
    /// `/api/users` but not `/apis`.
    pub(crate) fn match_prefix(&self, path: &str) -> Option<usize> {
        let mut end = 0;
        let mut segments = self.segments.iter();
        let mut next = segments.next();
        let mut start = 0;
        for part in path.split('/') {
            let part_start = start;
            start += part.len() + 1;
            if part.is_empty() {
                continue;
            }
            let Some(segment) = next else { break };
            match segment {
                Segment::Wildcard => return Some(path.len()),
                Segment::Literal(lit) if decode(part, false) != *lit => return None,
                _ => {}
            }
            end = part_start + part.len();
            next = segments.next();
        }
        match next {
            None | Some(Segment::Wildcard) => Some(end),
            Some(_) => None,
        }
    }
}

fn split(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}

/// Percent-decodes `input`. With `plus_as_space`, `+` decodes to a space (as in
/// query strings). Malformed escapes are left as-is; invalid UTF-8 is replaced
/// with U+FFFD.
pub(crate) fn decode(input: &str, plus_as_space: bool) -> String {
    let bytes = input.as_bytes();
    if !bytes
        .iter()
        .any(|&b| b == b'%' || (plus_as_space && b == b'+'))
    {
        return input.to_owned();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => match (
                bytes.get(i + 1).and_then(hex),
                bytes.get(i + 2).and_then(hex),
            ) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' if plus_as_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: &u8) -> Option<u8> {
    (*b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
        Pattern::parse(pattern).matches(path)
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn literals() {
        assert_eq!(params("/", "/"), Some(vec![]));
        assert_eq!(params("/", ""), Some(vec![]));
        assert_eq!(params("/users", "/users"), Some(vec![]));
        assert_eq!(params("/users", "/users/"), Some(vec![]));
        assert_eq!(params("/users/", "/users"), Some(vec![]));
        assert_eq!(params("/users", "/Users"), None);
        assert_eq!(params("/users", "/users/1"), None);
        assert_eq!(params("/users/1", "/users"), None);
        assert_eq!(params("/", "/users"), None);
    }

    #[test]
    fn params_capture_one_segment() {
        assert_eq!(
            params("/users/:id", "/users/42"),
            Some(pairs(&[("id", "42")]))
        );
        assert_eq!(
            params("/a/:x/b/:y", "/a/1/b/2"),
            Some(pairs(&[("x", "1"), ("y", "2")]))
        );
        assert_eq!(params("/users/:id", "/users"), None);
        assert_eq!(params("/users/:id", "/users/1/2"), None);
    }

    #[test]
    fn wildcard_captures_rest() {
        assert_eq!(
            params("/files/*", "/files/a/b.txt"),
            Some(pairs(&[("*", "a/b.txt")]))
        );
        assert_eq!(params("/files/*", "/files"), Some(pairs(&[("*", "")])));
        assert_eq!(
            params("*", "/anything/at/all"),
            Some(pairs(&[("*", "anything/at/all")]))
        );
        assert_eq!(params("/files/*", "/other/a"), None);
    }

    #[test]
    fn decoding() {
        assert_eq!(
            params("/users/:name", "/users/a%20b"),
            Some(pairs(&[("name", "a b")]))
        );
        assert_eq!(params("/caf%C3%A9", "/café"), Some(vec![]));
        assert_eq!(params("/café", "/caf%C3%A9"), Some(vec![]));
        assert_eq!(decode("a+b%2", true), "a b%2");
        assert_eq!(decode("100%", false), "100%");
        assert_eq!(decode("%zz", false), "%zz");
        assert_eq!(decode("a+b", false), "a+b");
    }

    #[test]
    fn prefix_matching() {
        let p = Pattern::parse("/assets");
        assert_eq!(p.match_prefix("/assets"), Some(7));
        assert_eq!(p.match_prefix("/assets/"), Some(7));
        assert_eq!(p.match_prefix("/assets/app.js"), Some(7));
        assert_eq!(p.match_prefix("/assetsx/app.js"), None);
        assert_eq!(p.match_prefix("/other"), None);
        assert_eq!(p.match_prefix("/"), None);
        assert_eq!(Pattern::parse("/").match_prefix("/anything"), Some(0));
        assert_eq!(Pattern::parse("/a/:id").match_prefix("/a/7/b"), Some(4));
        assert_eq!(Pattern::parse("/a/:id").match_prefix("/a"), None);
    }

    #[test]
    fn prefixed() {
        let p = Pattern::parse("/:id").prefixed(&Pattern::parse("/api/users"));
        assert_eq!(p.matches("/api/users/7"), Some(pairs(&[("id", "7")])));
    }

    #[test]
    #[should_panic(expected = "must be the last segment")]
    fn wildcard_must_be_last() {
        Pattern::parse("/*/x");
    }

    #[test]
    #[should_panic(expected = "needs a name")]
    fn unnamed_param() {
        Pattern::parse("/users/:");
    }
}
