// SPDX-License-Identifier: AGPL-3.0-only

//! Embedded static assets (CSS, JS) served at `/static/{file}`.
//!
//! Vendored Pico CSS and HTMX plus the app's own stylesheet and script are
//! compiled into the binary, so the server ships as a single self-contained
//! executable with no external asset directory required. The `--static-dir`
//! flag can still override these with on-disk files when set.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// Vendored Pico CSS framework.
const PICO_CSS: &str = include_str!("../../static/pico.min.css");
/// Vendored HTMX library.
const HTMX_JS: &str = include_str!("../../static/htmx.min.js");
/// Application stylesheet.
const APP_CSS: &str = include_str!("../../static/app.css");
/// Application script (progressive keyboard shortcuts).
const APP_JS: &str = include_str!("../../static/app.js");

/// `GET /static/{file}` — serve an embedded asset by file name.
pub async fn serve(Path(file): Path<String>) -> Response {
    let (body, content_type): (&'static str, &'static str) = match file.as_str() {
        "pico.min.css" => (PICO_CSS, "text/css; charset=utf-8"),
        "app.css" => (APP_CSS, "text/css; charset=utf-8"),
        "htmx.min.js" => (HTMX_JS, "text/javascript; charset=utf-8"),
        "app.js" => (APP_JS, "text/javascript; charset=utf-8"),
        _ => return (StatusCode::NOT_FOUND, "asset not found").into_response(),
    };

    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}
