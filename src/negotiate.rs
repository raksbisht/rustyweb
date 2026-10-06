//! Media type matching for `Request::is` and `Request::accepts`.

/// Expands Express-style shorthands (`json`, `html`, ...) to a media type.
fn expand(ty: &str) -> &str {
    match ty {
        "json" => "application/json",
        "html" => "text/html",
        "text" => "text/plain",
        "xml" => "application/xml",
        "urlencoded" => "application/x-www-form-urlencoded",
        "multipart" => "multipart/*",
        "csv" => "text/csv",
        "js" => "text/javascript",
        "css" => "text/css",
        other => other,
    }
}

/// The `type/subtype` part of a media type, lowercased, without parameters.
fn essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Whether `actual` (a concrete media type) matches `pattern`, which may use
/// `*` for the type or subtype.
fn matches(pattern: &str, actual: &str) -> bool {
    let (Some((pt, ps)), Some((at, as_))) = (pattern.split_once('/'), actual.split_once('/'))
    else {
        return false;
    };
    (pt == "*" || pt == at) && (ps == "*" || ps == as_)
}

/// `Request::is`: does the content type match `ty`?
pub(crate) fn is(content_type: &str, ty: &str) -> bool {
    let actual = essence(content_type);
    if actual.is_empty() {
        return false;
    }
    let wanted = essence(expand(ty));
    // `json` also matches structured types like `application/vnd.api+json`.
    if ty == "json" && actual.ends_with("+json") {
        return true;
    }
    matches(&wanted, &actual)
}

/// `Request::accepts`: the offered type the `Accept` header prefers.
pub(crate) fn accepts<'a>(accept: Option<&str>, offered: &[&'a str]) -> Option<&'a str> {
    let Some(accept) = accept.filter(|a| !a.trim().is_empty()) else {
        return offered.first().copied();
    };
    // (range, q, specificity) for each entry in the header.
    let ranges: Vec<(String, f32, u8)> = accept
        .split(',')
        .filter_map(|entry| {
            let mut parts = entry.split(';');
            let range = essence(parts.next()?);
            let q = parts
                .filter_map(|p| p.trim().strip_prefix("q="))
                .find_map(|q| q.parse::<f32>().ok())
                .unwrap_or(1.0);
            let specificity = match range.split_once('/')? {
                ("*", "*") => 0,
                (_, "*") => 1,
                _ => 2,
            };
            Some((range, q, specificity))
        })
        .collect();

    let mut best: Option<(&'a str, f32)> = None;
    for &ty in offered {
        let wanted = essence(expand(ty));
        // The most specific matching range decides this type's quality.
        let q = ranges
            .iter()
            .filter(|(range, _, _)| matches(range, &wanted))
            .max_by_key(|(_, _, specificity)| *specificity)
            .map(|(_, q, _)| *q);
        if let Some(q) = q.filter(|q| *q > 0.0)
            && best.is_none_or(|(_, best_q)| q > best_q)
        {
            best = Some((ty, q));
        }
    }
    best.map(|(ty, _)| ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_checks() {
        assert!(is("application/json; charset=utf-8", "json"));
        assert!(is("application/vnd.api+json", "json"));
        assert!(is("text/html", "html"));
        assert!(is("text/html", "text/*"));
        assert!(is("multipart/form-data; boundary=x", "multipart"));
        assert!(is("application/x-www-form-urlencoded", "urlencoded"));
        assert!(!is("text/plain", "json"));
        assert!(!is("", "json"));
    }

    #[test]
    fn accept_negotiation() {
        let browser = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
        assert_eq!(accepts(Some(browser), &["json", "html"]), Some("html"));
        assert_eq!(
            accepts(Some("application/json"), &["html", "json"]),
            Some("json")
        );
        assert_eq!(accepts(Some("*/*"), &["html", "json"]), Some("html"));
        assert_eq!(accepts(None, &["json", "html"]), Some("json"));
        assert_eq!(accepts(Some("image/png"), &["json", "html"]), None);
        assert_eq!(
            accepts(Some("text/*;q=0.5, application/json"), &["text", "json"]),
            Some("json")
        );
        assert_eq!(
            accepts(Some("application/json;q=0, */*"), &["json", "html"]),
            Some("html")
        );
    }
}
