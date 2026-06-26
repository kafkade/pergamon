// SPDX-License-Identifier: AGPL-3.0-only

//! Small shared helpers for request handlers.

use time::OffsetDateTime;

use crate::error::ApiError;

/// Parse a `YYYY-MM-DD` date string into an [`OffsetDateTime`] at midnight UTC.
///
/// Returns a 400 [`ApiError`] when the value is not a valid date.
pub fn parse_date_param(s: &str) -> Result<OffsetDateTime, ApiError> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    let date = time::Date::parse(s, format)
        .map_err(|_| ApiError::bad_request(format!("invalid date '{s}', expected YYYY-MM-DD")))?;
    Ok(date.midnight().assume_utc())
}
