// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered bookmarks view: a filtered grid/list of bookmark-type
//! items with favicon/thumbnail display, link-health indicators, and a
//! quick-add form.
//!
//! Like the rest of the web UI (see [`super::web`]), handlers query
//! `pergamon-storage` directly and every action works without JavaScript.

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{BookmarkMeta, ContentItem};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::{ContentItemFilter, Database};

use super::web::{ItemView, build_item_view, internal_error, parse_status, render};
use crate::state::AppState;

// ======================================================================
// Constants
// ======================================================================

const DEFAULT_PER_PAGE: u32 = 48;
const MAX_PER_PAGE: u32 = 100;

// ======================================================================
// View models
// ======================================================================

/// A bookmark rendered as a card (grid) or row (list).
struct BookmarkCardView {
    item: ItemView,
    thumbnail_url: String,
    has_thumbnail: bool,
    description: String,
    health_label: String,
    health_class: String,
    has_health: bool,
}

#[derive(Template)]
#[template(path = "bookmarks.html")]
struct BookmarksTemplate {
    bookmarks: Vec<BookmarkCardView>,
    layout: String,
    is_grid: bool,
    status: String,
    statuses: Vec<String>,
    total: u64,
    page: u32,
    total_pages: u32,
    prev_url: String,
    next_url: String,
}

// ======================================================================
// Query / form types
// ======================================================================

/// Query parameters for the bookmarks page.
#[derive(Debug, Default, Deserialize)]
pub struct BookmarksQuery {
    layout: Option<String>,
    status: Option<String>,
    page: Option<u32>,
}

/// Form body for the quick-add bookmark form.
#[derive(Debug, Deserialize)]
pub struct AddBookmarkForm {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: String,
}

/// Status values offered in the bookmarks status filter.
const STATUS_VALUES: [&str; 6] = [
    "inbox",
    "later",
    "reference",
    "reading",
    "archived",
    "discarded",
];

// ======================================================================
// Handlers
// ======================================================================

/// `GET /bookmarks` — bookmark-type items in a grid or list layout.
pub async fn bookmarks(
    State(state): State<AppState>,
    Query(query): Query<BookmarksQuery>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let layout = match query.layout.as_deref() {
        Some("list") => "list",
        _ => "grid",
    };
    let status = parse_status(query.status.as_deref());
    let page = query.page.unwrap_or(1).max(1);
    let per_page = DEFAULT_PER_PAGE.min(MAX_PER_PAGE);

    let filter = ContentItemFilter {
        content_type: Some(ContentType::Bookmark),
        status,
        ..ContentItemFilter::default()
    };

    let Ok(total) = db.count_content_items_filtered(&filter) else {
        return internal_error();
    };
    let offset = (page - 1) * per_page;
    let Ok(items) = db.list_content_items_filtered(&filter, Some(per_page), Some(offset)) else {
        return internal_error();
    };

    let cards: Vec<BookmarkCardView> = items.iter().map(|i| build_card(&db, i)).collect();

    let total_pages = total_pages(total, per_page);
    let status_str = status.map(|s| s.as_str().to_owned()).unwrap_or_default();

    render(&BookmarksTemplate {
        bookmarks: cards,
        is_grid: layout == "grid",
        layout: layout.to_owned(),
        status: status_str.clone(),
        statuses: STATUS_VALUES.iter().map(|s| (*s).to_owned()).collect(),
        total,
        page,
        total_pages,
        prev_url: page_url(layout, &status_str, page.saturating_sub(1), page > 1),
        next_url: page_url(layout, &status_str, page + 1, page < total_pages),
    })
}

/// `POST /bookmarks` — quick-add a bookmark.
///
/// Creates the bookmark directly from the submitted URL without fetching the
/// page (keeping the request fast and JS-free). Deduplicates on the
/// canonicalized URL. Redirects back to the bookmarks list.
pub async fn add_bookmark(
    State(state): State<AppState>,
    Form(form): Form<AddBookmarkForm>,
) -> Response {
    let raw = form.url.trim();
    if raw.is_empty() {
        return Redirect::to("/bookmarks").into_response();
    }

    // Validate scheme.
    let Ok(parsed) = raw.parse::<url::Url>() else {
        return Redirect::to("/bookmarks").into_response();
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Redirect::to("/bookmarks").into_response();
    }

    let canonical = pergamon_extract::canonicalize_url(raw).unwrap_or_else(|_| raw.to_owned());

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    // Deduplicate: tag the existing item and return.
    let Ok(existing) = db.get_content_item_by_url(&canonical) else {
        return internal_error();
    };
    let item_id = if let Some(item) = existing {
        item.id
    } else {
        let now = OffsetDateTime::now_utc();
        let title = if form.title.trim().is_empty() {
            host_label(&parsed)
        } else {
            form.title.trim().to_owned()
        };
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(canonical),
            title,
            author: None,
            content_type: ContentType::Bookmark,
            status: DocumentStatus::Inbox,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };
        if db.insert_content_item(&item).is_err() {
            return internal_error();
        }
        let meta = BookmarkMeta {
            content_item_id: item.id,
            original_url: Some(raw.to_owned()),
            saved_from: Some("web".to_owned()),
            thumbnail_url: None,
            description: None,
            site_name: parsed.host_str().map(str::to_owned),
            favicon_url: None,
        };
        let _ = db.upsert_bookmark_meta(&meta);
        item.id
    };

    // Apply any comma/space-separated tags.
    for name in form
        .tags
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(tag) = db.get_or_create_tag(name) {
            let _ = db.tag_content_item(item_id, tag.id);
        }
    }

    Redirect::to("/bookmarks").into_response()
}

// ======================================================================
// Helpers
// ======================================================================

/// Build a card view for a bookmark item, resolving thumbnail and health.
fn build_card(db: &Database, item: &ContentItem) -> BookmarkCardView {
    let base = build_item_view(db, item);
    let meta = db.get_bookmark_meta(item.id).ok();
    let thumbnail_url = meta
        .as_ref()
        .and_then(|m| m.thumbnail_url.clone())
        .unwrap_or_default();
    let description = meta
        .as_ref()
        .and_then(|m| m.description.clone())
        .unwrap_or_default();

    let (health_label, health_class, has_health) = match db.get_link_health(item.id) {
        Ok(Some(h)) => {
            let (label, class) = classify_health(&h);
            (label.to_owned(), class.to_owned(), true)
        }
        _ => (String::new(), String::new(), false),
    };

    BookmarkCardView {
        has_thumbnail: !thumbnail_url.is_empty(),
        thumbnail_url,
        description,
        health_label,
        health_class,
        has_health,
        item: base,
    }
}

/// Map a link-health record to a display label and CSS class.
const fn classify_health(h: &pergamon_core::model::LinkHealth) -> (&'static str, &'static str) {
    if h.error_message.is_some() {
        return ("unreachable", "health-error");
    }
    match h.http_status {
        Some(code) if code >= 400 => ("broken", "health-error"),
        Some(code) if code >= 300 || h.redirect_count > 0 => ("redirect", "health-warn"),
        Some(_) => ("ok", "health-ok"),
        None => ("unreachable", "health-error"),
    }
}

/// A readable label for a URL host (strips a leading `www.`).
fn host_label(parsed: &url::Url) -> String {
    parsed.host_str().map_or_else(
        || "Bookmark".to_owned(),
        |h| h.trim_start_matches("www.").to_owned(),
    )
}

/// Build a pagination URL preserving the layout and status filter.
fn page_url(layout: &str, status: &str, page: u32, enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    ser.append_pair("layout", layout);
    if !status.is_empty() {
        ser.append_pair("status", status);
    }
    ser.append_pair("page", &page.to_string());
    format!("/bookmarks?{}", ser.finish())
}

/// Compute total number of pages (at least 1).
fn total_pages(total: u64, per_page: u32) -> u32 {
    if total == 0 {
        return 1;
    }
    u32::try_from(total.div_ceil(u64::from(per_page))).unwrap_or(u32::MAX)
}
