//! Smart filter DSL for auto-populated collections and saved searches.
//!
//! The filter language uses `key:value` pairs separated by whitespace.
//! Multiple filters are combined with AND. Comma-separated values within
//! a single key are combined with OR:
//!
//! ```text
//! type:article tag:rust,python status:inbox
//! ```
//!
//! Means: content type is article AND (tag is rust OR python) AND status is inbox.
//!
//! Negation uses a `-` prefix: `-status:discarded` excludes discarded items.
//!
//! Supported keys: `type`, `tag`, `status`, `source`, `since`, `before`, `text`.
//!
//! This module is pure computation — no I/O, no SQL. The storage layer
//! translates [`SmartFilter`] into SQL queries.

use std::fmt;
use std::str::FromStr;

use crate::content_type::ContentType;
use crate::error::CoreError;
use crate::status::DocumentStatus;

/// A parsed predicate in the smart filter DSL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterPredicate {
    /// Match items with one of these content types (OR).
    ContentType(Vec<ContentType>),
    /// Match items tagged with any of these tag names (OR).
    Tag(Vec<String>),
    /// Match items with one of these statuses (OR).
    Status(Vec<DocumentStatus>),
    /// Exclude items with any of these statuses.
    ExcludeStatus(Vec<DocumentStatus>),
    /// Match items from feeds whose title contains this substring.
    Source(String),
    /// Match items created on or after this date (YYYY-MM-DD).
    CreatedSince(String),
    /// Match items created before this date (YYYY-MM-DD).
    CreatedBefore(String),
    /// Full-text search query (FTS5 MATCH).
    Text(String),
}

/// A parsed smart filter — a conjunction (AND) of predicates.
///
/// When applied, all predicates must match for an item to be included.
/// Within multi-value predicates (e.g., `Tag(["rust", "python"])`),
/// values are combined with OR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartFilter {
    predicates: Vec<FilterPredicate>,
}

/// Errors from parsing a smart filter query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterParseError {
    /// A filter key is not recognized.
    UnknownKey(String),
    /// A filter is missing the `:` separator.
    MissingSeparator(String),
    /// A value could not be parsed for the given key.
    InvalidValue {
        /// The filter key that was being parsed.
        key: String,
        /// The value that failed to parse.
        value: String,
        /// Why the value is invalid.
        reason: String,
    },
    /// The filter string is empty.
    Empty,
}

impl fmt::Display for FilterParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "unknown filter key: {key}"),
            Self::MissingSeparator(token) => {
                write!(f, "filter must be key:value, got: {token}")
            }
            Self::InvalidValue { key, value, reason } => {
                write!(f, "invalid value for {key}: {value} ({reason})")
            }
            Self::Empty => f.write_str("empty filter"),
        }
    }
}

impl std::error::Error for FilterParseError {}

impl SmartFilter {
    /// Parse a filter query string into a [`SmartFilter`].
    ///
    /// # Errors
    ///
    /// Returns [`FilterParseError`] if the query contains unknown keys,
    /// missing separators, or invalid values.
    pub fn parse(query: &str) -> Result<Self, FilterParseError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(FilterParseError::Empty);
        }

        let mut predicates = Vec::new();

        for token in tokenize(query) {
            let pred = parse_token(token)?;
            predicates.push(pred);
        }

        if predicates.is_empty() {
            return Err(FilterParseError::Empty);
        }

        Ok(Self { predicates })
    }

    /// Access the predicates.
    #[must_use]
    pub fn predicates(&self) -> &[FilterPredicate] {
        &self.predicates
    }

    /// Whether this filter has no predicates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }

    /// Whether this filter includes a full-text search predicate.
    #[must_use]
    pub fn has_text_query(&self) -> bool {
        self.predicates
            .iter()
            .any(|p| matches!(p, FilterPredicate::Text(_)))
    }

    /// Extract the text query predicate value, if present.
    #[must_use]
    pub fn text_query(&self) -> Option<&str> {
        self.predicates.iter().find_map(|p| match p {
            FilterPredicate::Text(q) => Some(q.as_str()),
            _ => None,
        })
    }
}

impl fmt::Display for SmartFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for pred in &self.predicates {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            match pred {
                FilterPredicate::ContentType(types) => {
                    let vals: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
                    write!(f, "type:{}", vals.join(","))?;
                }
                FilterPredicate::Tag(tags) => {
                    write!(f, "tag:{}", tags.join(","))?;
                }
                FilterPredicate::Status(statuses) => {
                    let vals: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
                    write!(f, "status:{}", vals.join(","))?;
                }
                FilterPredicate::ExcludeStatus(statuses) => {
                    let vals: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
                    write!(f, "-status:{}", vals.join(","))?;
                }
                FilterPredicate::Source(src) => write!(f, "source:{src}")?,
                FilterPredicate::CreatedSince(date) => write!(f, "since:{date}")?,
                FilterPredicate::CreatedBefore(date) => write!(f, "before:{date}")?,
                FilterPredicate::Text(q) => write!(f, "text:{q}")?,
            }
        }
        Ok(())
    }
}

impl FromStr for SmartFilter {
    type Err = FilterParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ======================================================================
// Tokenizer
// ======================================================================

/// Split the query into tokens, respecting quoted values.
///
/// Tokens are whitespace-separated. A value containing spaces can be
/// quoted: `text:"hello world"` or `text:'hello world'`.
fn tokenize(query: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut chars = query.char_indices().peekable();
    let mut token_start: Option<usize> = None;

    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            if let Some(start) = token_start.take() {
                tokens.push(&query[start..i]);
            }
            chars.next();
        } else {
            if token_start.is_none() {
                token_start = Some(i);
            }
            chars.next();
            // If we hit a quote after ':', consume until closing quote.
            if c == ':' {
                if let Some(&(_, q)) = chars.peek() {
                    if q == '"' || q == '\'' {
                        chars.next(); // skip opening quote
                        while let Some(&(_, ch)) = chars.peek() {
                            chars.next();
                            if ch == q {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(start) = token_start {
        tokens.push(&query[start..]);
    }

    tokens
}

// ======================================================================
// Parser
// ======================================================================

/// Parse a single `key:value` token into a [`FilterPredicate`].
fn parse_token(token: &str) -> Result<FilterPredicate, FilterParseError> {
    let (negated, token) = token
        .strip_prefix('-')
        .map_or((false, token), |rest| (true, rest));

    let colon_pos = token
        .find(':')
        .ok_or_else(|| FilterParseError::MissingSeparator(token.to_owned()))?;

    let key = &token[..colon_pos];
    let raw_value = &token[colon_pos + 1..];

    // Strip surrounding quotes from the value.
    let value = raw_value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| {
            raw_value
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
        })
        .unwrap_or(raw_value);

    if value.is_empty() {
        return Err(FilterParseError::InvalidValue {
            key: key.to_owned(),
            value: value.to_owned(),
            reason: "value cannot be empty".to_owned(),
        });
    }

    match key {
        "type" => parse_type_predicate(value, negated),
        "tag" => parse_tag_predicate(value, negated),
        "status" => parse_status_predicate(value, negated),
        "source" => parse_simple_predicate("source", value, negated, FilterPredicate::Source),
        "since" => parse_date_predicate("since", value, negated, FilterPredicate::CreatedSince),
        "before" => parse_date_predicate("before", value, negated, FilterPredicate::CreatedBefore),
        "text" => parse_simple_predicate("text", value, negated, FilterPredicate::Text),
        _ => Err(FilterParseError::UnknownKey(key.to_owned())),
    }
}

/// Parse a `type:` predicate.
fn parse_type_predicate(value: &str, negated: bool) -> Result<FilterPredicate, FilterParseError> {
    let types = parse_comma_list(value, |v| {
        v.parse::<ContentType>()
            .map_err(|e: CoreError| e.to_string())
    })
    .map_err(|reason| FilterParseError::InvalidValue {
        key: "type".to_owned(),
        value: value.to_owned(),
        reason,
    })?;
    reject_negation("type", value, negated)?;
    Ok(FilterPredicate::ContentType(types))
}

/// Parse a `tag:` predicate.
fn parse_tag_predicate(value: &str, negated: bool) -> Result<FilterPredicate, FilterParseError> {
    reject_negation("tag", value, negated)?;
    let tags: Vec<String> = value.split(',').map(|s| s.trim().to_owned()).collect();
    Ok(FilterPredicate::Tag(tags))
}

/// Parse a `status:` / `-status:` predicate.
fn parse_status_predicate(value: &str, negated: bool) -> Result<FilterPredicate, FilterParseError> {
    let statuses = parse_comma_list(value, |v| {
        v.parse::<DocumentStatus>()
            .map_err(|e: CoreError| e.to_string())
    })
    .map_err(|reason| FilterParseError::InvalidValue {
        key: "status".to_owned(),
        value: value.to_owned(),
        reason,
    })?;
    if negated {
        Ok(FilterPredicate::ExcludeStatus(statuses))
    } else {
        Ok(FilterPredicate::Status(statuses))
    }
}

/// Parse a predicate that takes a single string value and doesn't support negation.
fn parse_simple_predicate(
    key: &str,
    value: &str,
    negated: bool,
    make: fn(String) -> FilterPredicate,
) -> Result<FilterPredicate, FilterParseError> {
    reject_negation(key, value, negated)?;
    Ok(make(value.to_owned()))
}

/// Parse a date predicate (`since:` / `before:`).
fn parse_date_predicate(
    key: &str,
    value: &str,
    negated: bool,
    make: fn(String) -> FilterPredicate,
) -> Result<FilterPredicate, FilterParseError> {
    reject_negation(key, value, negated)?;
    validate_date(value)?;
    Ok(make(value.to_owned()))
}

/// Reject negation for a key that doesn't support it.
fn reject_negation(key: &str, value: &str, negated: bool) -> Result<(), FilterParseError> {
    if negated {
        Err(FilterParseError::InvalidValue {
            key: key.to_owned(),
            value: value.to_owned(),
            reason: format!("negation is not supported for {key} filters"),
        })
    } else {
        Ok(())
    }
}

/// Parse a comma-separated list of values using the given parser.
fn parse_comma_list<T, F>(value: &str, parser: F) -> Result<Vec<T>, String>
where
    F: Fn(&str) -> Result<T, String>,
{
    value
        .split(',')
        .map(|v| parser(v.trim()))
        .collect::<Result<Vec<T>, String>>()
}

/// Validate that a date string looks like YYYY-MM-DD.
fn validate_date(date: &str) -> Result<(), FilterParseError> {
    if date.len() != 10 {
        return Err(FilterParseError::InvalidValue {
            key: "date".to_owned(),
            value: date.to_owned(),
            reason: "expected YYYY-MM-DD format".to_owned(),
        });
    }
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(FilterParseError::InvalidValue {
            key: "date".to_owned(),
            value: date.to_owned(),
            reason: "expected YYYY-MM-DD format".to_owned(),
        });
    }
    for part in &parts {
        if part.parse::<u32>().is_err() {
            return Err(FilterParseError::InvalidValue {
                key: "date".to_owned(),
                value: date.to_owned(),
                reason: "expected numeric components in YYYY-MM-DD".to_owned(),
            });
        }
    }
    Ok(())
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn parse_single_type() {
        let f = SmartFilter::parse("type:article").unwrap();
        assert_eq!(f.predicates.len(), 1);
        assert_eq!(
            f.predicates[0],
            FilterPredicate::ContentType(vec![ContentType::Article])
        );
    }

    #[test]
    fn parse_multiple_types() {
        let f = SmartFilter::parse("type:article,bookmark").unwrap();
        assert_eq!(f.predicates.len(), 1);
        assert_eq!(
            f.predicates[0],
            FilterPredicate::ContentType(vec![ContentType::Article, ContentType::Bookmark])
        );
    }

    #[test]
    fn parse_combined_filters() {
        let f = SmartFilter::parse("type:article tag:rust status:inbox").unwrap();
        assert_eq!(f.predicates.len(), 3);
        assert_eq!(
            f.predicates[0],
            FilterPredicate::ContentType(vec![ContentType::Article])
        );
        assert_eq!(
            f.predicates[1],
            FilterPredicate::Tag(vec!["rust".to_owned()])
        );
        assert_eq!(
            f.predicates[2],
            FilterPredicate::Status(vec![DocumentStatus::Inbox])
        );
    }

    #[test]
    fn parse_multi_tag() {
        let f = SmartFilter::parse("tag:rust,python,go").unwrap();
        assert_eq!(
            f.predicates[0],
            FilterPredicate::Tag(vec![
                "rust".to_owned(),
                "python".to_owned(),
                "go".to_owned()
            ])
        );
    }

    #[test]
    fn parse_negated_status() {
        let f = SmartFilter::parse("-status:discarded,archived").unwrap();
        assert_eq!(
            f.predicates[0],
            FilterPredicate::ExcludeStatus(vec![
                DocumentStatus::Discarded,
                DocumentStatus::Archived,
            ])
        );
    }

    #[test]
    fn parse_date_range() {
        let f = SmartFilter::parse("since:2025-01-01 before:2025-12-31").unwrap();
        assert_eq!(f.predicates.len(), 2);
        assert_eq!(
            f.predicates[0],
            FilterPredicate::CreatedSince("2025-01-01".to_owned())
        );
        assert_eq!(
            f.predicates[1],
            FilterPredicate::CreatedBefore("2025-12-31".to_owned())
        );
    }

    #[test]
    fn parse_source_filter() {
        let f = SmartFilter::parse("source:hackernews").unwrap();
        assert_eq!(
            f.predicates[0],
            FilterPredicate::Source("hackernews".to_owned())
        );
    }

    #[test]
    fn parse_text_filter_quoted() {
        let f = SmartFilter::parse("text:\"hello world\"").unwrap();
        assert_eq!(
            f.predicates[0],
            FilterPredicate::Text("hello world".to_owned())
        );
    }

    #[test]
    fn parse_full_example() {
        let f = SmartFilter::parse(
            "type:article tag:rust,python -status:discarded source:blog since:2025-01-01",
        )
        .unwrap();
        assert_eq!(f.predicates.len(), 5);
    }

    #[test]
    fn display_round_trip() {
        let original =
            "type:article tag:rust,python -status:discarded source:blog since:2025-01-01";
        let f = SmartFilter::parse(original).unwrap();
        let displayed = f.to_string();
        let reparsed = SmartFilter::parse(&displayed).unwrap();
        assert_eq!(f, reparsed);
    }

    #[test]
    fn error_unknown_key() {
        let err = SmartFilter::parse("foo:bar").unwrap_err();
        assert_eq!(err, FilterParseError::UnknownKey("foo".to_owned()));
    }

    #[test]
    fn error_missing_separator() {
        let err = SmartFilter::parse("nocolon").unwrap_err();
        assert!(matches!(err, FilterParseError::MissingSeparator(_)));
    }

    #[test]
    fn error_empty_value() {
        let err = SmartFilter::parse("type:").unwrap_err();
        assert!(matches!(err, FilterParseError::InvalidValue { .. }));
    }

    #[test]
    fn error_invalid_type() {
        let err = SmartFilter::parse("type:unknown_type").unwrap_err();
        assert!(matches!(err, FilterParseError::InvalidValue { .. }));
    }

    #[test]
    fn error_invalid_date() {
        let err = SmartFilter::parse("since:not-a-date").unwrap_err();
        assert!(matches!(err, FilterParseError::InvalidValue { .. }));
    }

    #[test]
    fn error_empty_query() {
        let err = SmartFilter::parse("").unwrap_err();
        assert_eq!(err, FilterParseError::Empty);
    }

    #[test]
    fn has_text_query_true() {
        let f = SmartFilter::parse("text:hello tag:rust").unwrap();
        assert!(f.has_text_query());
        assert_eq!(f.text_query(), Some("hello"));
    }

    #[test]
    fn has_text_query_false() {
        let f = SmartFilter::parse("tag:rust").unwrap();
        assert!(!f.has_text_query());
        assert_eq!(f.text_query(), None);
    }

    #[test]
    fn negated_type_rejected() {
        let err = SmartFilter::parse("-type:article").unwrap_err();
        assert!(matches!(err, FilterParseError::InvalidValue { .. }));
    }
}
