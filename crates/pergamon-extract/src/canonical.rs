//! URL canonicalization for deduplication.
//!
//! Normalizes URLs to a canonical form so that trivially different
//! representations (trailing slashes, tracking params, mixed case)
//! resolve to the same identity string.

use url::Url;

use crate::error::ExtractError;

/// Tracking query parameters to strip during canonicalization.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "ref",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
];

/// Canonicalize a URL for dedup comparisons.
///
/// Applies the following normalisations:
/// - Validates scheme is `http` or `https`
/// - Lowercases scheme and host
/// - Removes default ports (80 for HTTP, 443 for HTTPS)
/// - Removes trailing slash (unless path is exactly `/`)
/// - Strips tracking query parameters
/// - Sorts remaining query parameters alphabetically
/// - Removes fragment
///
/// # Errors
///
/// Returns an error if the URL cannot be parsed or uses an
/// unsupported scheme (anything other than `http`/`https`).
pub fn canonicalize_url(raw: &str) -> Result<String, ExtractError> {
    let mut parsed =
        Url::parse(raw).map_err(|e| ExtractError::Extract(format!("invalid URL: {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ExtractError::Extract(format!(
                "unsupported URL scheme: {other}"
            )));
        }
    }

    // Remove fragment.
    parsed.set_fragment(None);

    // Remove default ports.
    let is_default_port = matches!(
        (parsed.scheme(), parsed.port()),
        ("http", Some(80)) | ("https", Some(443))
    );
    if is_default_port {
        let _ = parsed.set_port(None);
    }

    // Filter and sort query parameters.
    let query_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| {
            let k = key.to_lowercase();
            !TRACKING_PARAMS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    if query_pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let mut sorted = query_pairs;
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let qs: String = sorted
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&qs));
    }

    let mut result = parsed.to_string();

    // Remove trailing slash unless path is exactly "/".
    if result.ends_with('/') && parsed.path() != "/" {
        result.pop();
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tracking_params() {
        let url = "https://example.com/page?utm_source=twitter&id=42&fbclid=abc";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/page?id=42");
    }

    #[test]
    fn removes_fragment() {
        let url = "https://example.com/page#section-2";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/page");
    }

    #[test]
    fn removes_trailing_slash() {
        let url = "https://example.com/article/";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/article");
    }

    #[test]
    fn preserves_root_slash() {
        let url = "https://example.com/";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/");
    }

    #[test]
    fn removes_default_ports() {
        let http =
            canonicalize_url("http://example.com:80/page").unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(http, "http://example.com/page");

        let https = canonicalize_url("https://example.com:443/page")
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(https, "https://example.com/page");
    }

    #[test]
    fn preserves_non_default_ports() {
        let url = "https://example.com:8080/page";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com:8080/page");
    }

    #[test]
    fn sorts_query_params() {
        let url = "https://example.com/search?z=1&a=2&m=3";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/search?a=2&m=3&z=1");
    }

    #[test]
    fn lowercases_host() {
        let url = "https://EXAMPLE.COM/Page";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        // Host lowercased, path case preserved.
        assert_eq!(canonical, "https://example.com/Page");
    }

    #[test]
    fn rejects_unsupported_scheme() {
        let result = canonicalize_url("ftp://example.com/file");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_invalid_url() {
        let result = canonicalize_url("not a url at all");
        assert!(result.is_err());
    }

    #[test]
    fn handles_no_query_string() {
        let url = "https://example.com/article";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/article");
    }

    #[test]
    fn strips_all_tracking_leaves_no_query() {
        let url = "https://example.com/page?utm_source=a&utm_medium=b";
        let canonical = canonicalize_url(url).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(canonical, "https://example.com/page");
    }
}
