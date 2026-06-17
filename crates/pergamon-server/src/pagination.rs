// SPDX-License-Identifier: AGPL-3.0-only

//! Pagination helpers for list endpoints.
//!
//! Uses `?page=N&per_page=M` query parameters with `Link` and
//! `X-Total-Count` response headers.

use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

/// Default number of items per page.
const DEFAULT_PER_PAGE: u32 = 50;
/// Maximum allowed items per page.
const MAX_PER_PAGE: u32 = 100;

/// Pagination query parameters.
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    /// Page number (1-indexed).
    pub page: Option<u32>,
    /// Items per page.
    pub per_page: Option<u32>,
}

impl PaginationParams {
    /// Resolved page number (minimum 1).
    pub fn page(&self) -> u32 {
        self.page.unwrap_or(1).max(1)
    }

    /// Resolved per-page count, clamped to `MAX_PER_PAGE`.
    pub fn per_page(&self) -> u32 {
        self.per_page.unwrap_or(DEFAULT_PER_PAGE).min(MAX_PER_PAGE)
    }

    /// Compute the SQL `OFFSET` for the current page.
    pub fn offset(&self) -> u32 {
        (self.page() - 1) * self.per_page()
    }

    /// Compute the SQL `LIMIT`.
    pub fn limit(&self) -> u32 {
        self.per_page()
    }
}

/// Wraps a JSON response body with pagination headers.
pub struct Paginated<T: serde::Serialize> {
    body: T,
    total: u64,
    page: u32,
    per_page: u32,
    base_path: String,
}

impl<T: serde::Serialize> Paginated<T> {
    /// Create a new paginated response.
    pub fn new(body: T, total: u64, params: &PaginationParams, base_path: &str) -> Self {
        Self {
            body,
            total,
            page: params.page(),
            per_page: params.per_page(),
            base_path: base_path.to_owned(),
        }
    }

    /// Total number of pages.
    fn total_pages(&self) -> u32 {
        if self.total == 0 {
            1
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let total_u32 = self.total.min(u64::from(u32::MAX)) as u32;
            total_u32.div_ceil(self.per_page)
        }
    }

    /// Build the `Link` header value with rel=first, prev, next, last.
    fn link_header(&self) -> String {
        let total_pages = self.total_pages();
        let mut links = Vec::new();

        // first
        links.push(format!(
            "<{}?page=1&per_page={}>; rel=\"first\"",
            self.base_path, self.per_page
        ));

        // prev
        if self.page > 1 {
            links.push(format!(
                "<{}?page={}&per_page={}>; rel=\"prev\"",
                self.base_path,
                self.page - 1,
                self.per_page
            ));
        }

        // next
        if self.page < total_pages {
            links.push(format!(
                "<{}?page={}&per_page={}>; rel=\"next\"",
                self.base_path,
                self.page + 1,
                self.per_page
            ));
        }

        // last
        links.push(format!(
            "<{}?page={}&per_page={}>; rel=\"last\"",
            self.base_path, total_pages, self.per_page
        ));

        links.join(", ")
    }
}

impl<T: serde::Serialize> IntoResponse for Paginated<T> {
    fn into_response(self) -> Response {
        let link = self.link_header();
        let total = self.total.to_string();

        let mut response = axum::Json(&self.body).into_response();
        let headers = response.headers_mut();

        if let Ok(v) = HeaderValue::from_str(&link) {
            headers.insert("Link", v);
        }
        if let Ok(v) = HeaderValue::from_str(&total) {
            headers.insert("X-Total-Count", v);
        }

        response
    }
}
