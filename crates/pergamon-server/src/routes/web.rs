// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered HTML views: inbox/library and article reader.
//!
//! These handlers render HTML with Askama templates and enhance interactions
//! with HTMX. They query `pergamon-storage` directly (the same pattern the
//! JSON API handlers use) and degrade gracefully without JavaScript: every
//! action is reachable via a plain link or form submission, and handlers
//! return a redirect when the request is not an HTMX request.

#![allow(clippy::significant_drop_tightening)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::fsrs::Rating;
use pergamon_core::model::{ContentItem, HighlightMeta, Note, ReviewCard, ReviewStatsReport};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::{ContentItemFilter, ContentItemSort, Database, StorageError};

use crate::routes::review::apply_review_rating;
use crate::state::AppState;
use crate::util::parse_date_param;

// ======================================================================
// View models
// ======================================================================

/// A single content item rendered as a list row or reader header.
struct ItemView {
    id: String,
    title: String,
    reader_url: String,
    has_url: bool,
    url: String,
    favicon_url: String,
    source: String,
    type_label: String,
    date: String,
    is_read: bool,
    status: String,
    excerpt: String,
}

/// A sidebar navigation link with an item count.
struct NavLink {
    label: String,
    href: String,
    count: i64,
    active: bool,
}

/// A feed entry in the sidebar.
struct FeedNav {
    title: String,
    href: String,
    count: i64,
    active: bool,
}

/// A folder grouping feeds in the sidebar.
struct FolderNav {
    name: String,
    href: String,
    active: bool,
    feeds: Vec<FeedNav>,
}

/// The full sidebar.
struct SidebarView {
    status_links: Vec<NavLink>,
    folders: Vec<FolderNav>,
    feeds_root: Vec<FeedNav>,
    tags: Vec<NavLink>,
}

/// A hidden form field preserving the active filter across submissions.
struct HiddenField {
    name: String,
    value: String,
}

/// Current filter selections plus the option lists for the filter bar.
struct FilterView {
    status: String,
    content_type: String,
    tag: String,
    sort: String,
    feed: String,
    folder: String,
    statuses: Vec<String>,
    content_types: Vec<String>,
    tags: Vec<String>,
}

/// The paginated item list and its surrounding controls.
struct ListView {
    items: Vec<ItemView>,
    total: u64,
    page: u32,
    total_pages: u32,
    prev_url: String,
    next_url: String,
    hidden_filters: Vec<HiddenField>,
}

// ======================================================================
// Templates
// ======================================================================

#[derive(Template)]
#[template(path = "inbox.html")]
struct InboxTemplate {
    sidebar: SidebarView,
    filter: FilterView,
    list: ListView,
}

#[derive(Template)]
#[template(path = "_item_list.html")]
struct ItemListTemplate {
    list: ListView,
}

#[derive(Template)]
#[template(path = "_item_row.html")]
struct ItemRowTemplate {
    item: ItemView,
}

#[derive(Template)]
#[template(path = "reader.html")]
struct ReaderTemplate {
    item_id: String,
    title: String,
    author: String,
    source: String,
    date: String,
    has_url: bool,
    url: String,
    type_label: String,
    status: String,
    back_url: String,
    tags: Vec<String>,
    paragraphs: Vec<String>,
    has_content: bool,
}

#[derive(Template)]
#[template(path = "_tags.html")]
struct TagSectionTemplate {
    item_id: String,
    tags: Vec<String>,
}

// ======================================================================
// Query / form types
// ======================================================================

/// Query parameters accepted by the inbox view and bulk action.
#[derive(Debug, Default, Deserialize)]
pub struct InboxQuery {
    status: Option<String>,
    #[serde(rename = "type")]
    content_type: Option<String>,
    tag: Option<String>,
    feed: Option<Uuid>,
    folder: Option<Uuid>,
    sort: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

/// Form body for a single-item status change.
#[derive(Debug, Deserialize)]
pub struct StatusForm {
    action: String,
    #[serde(default)]
    view: Option<String>,
}

/// Form body for adding a tag.
#[derive(Debug, Deserialize)]
pub struct AddTagForm {
    name: String,
}

// ======================================================================
// Constants
// ======================================================================

/// Default items per page in the web UI.
const DEFAULT_PER_PAGE: u32 = 50;
/// Maximum items per page.
const MAX_PER_PAGE: u32 = 100;
/// Maximum number of tags shown in the sidebar.
const SIDEBAR_TAG_LIMIT: usize = 20;

/// Statuses surfaced as quick filters, in display order.
const SIDEBAR_STATUSES: [DocumentStatus; 5] = [
    DocumentStatus::Inbox,
    DocumentStatus::Later,
    DocumentStatus::Reading,
    DocumentStatus::Reference,
    DocumentStatus::Archived,
];

// ======================================================================
// Handlers
// ======================================================================

/// `GET /` — redirect to the inbox.
pub async fn index() -> Redirect {
    Redirect::to("/inbox")
}

/// `GET /inbox` — full library page, or the item-list fragment for HTMX.
pub async fn inbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<InboxQuery>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let status = parse_status(query.status.as_deref());
    let content_type = parse_content_type(query.content_type.as_deref());
    let tag_name = non_empty(query.tag.as_deref());
    let sort = parse_sort(query.sort.as_deref());
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);

    let tag_id = match tag_name {
        Some(name) => match db.get_tag_by_name(name) {
            Ok(Some(t)) => Some(t.id),
            Ok(None) => None,
            Err(_) => return internal_error(),
        },
        None => None,
    };

    let filter = ContentItemFilter {
        content_type,
        status,
        feed_id: query.feed,
        folder_id: query.folder,
        tag_id,
        sort,
        ..ContentItemFilter::default()
    };

    let Ok(total) = db.count_content_items_filtered(&filter) else {
        return internal_error();
    };
    let offset = (page - 1) * per_page;
    let Ok(items) = db.list_content_items_filtered(&filter, Some(per_page), Some(offset)) else {
        return internal_error();
    };

    let item_views: Vec<ItemView> = items.iter().map(|i| build_item_view(&db, i)).collect();

    // Query pairs that describe the active filter (everything except page).
    let filter_pairs = active_filter_pairs(&query, sort);
    let total_pages = total_pages(total, per_page);
    let list = ListView {
        items: item_views,
        total,
        page,
        total_pages,
        prev_url: page_url(&filter_pairs, page.saturating_sub(1), per_page, page > 1),
        next_url: page_url(&filter_pairs, page + 1, per_page, page < total_pages),
        hidden_filters: filter_pairs
            .iter()
            .map(|(k, v)| HiddenField {
                name: (*k).to_owned(),
                value: v.clone(),
            })
            .collect(),
    };

    // HTMX requests get just the list fragment.
    if is_htmx(&headers) {
        return render(&ItemListTemplate { list });
    }

    let Ok(sidebar) = build_sidebar(&db, status, &query, tag_name) else {
        return internal_error();
    };
    let Ok(filter_view) = build_filter_view(&db, &query, status, content_type, sort, tag_name)
    else {
        return internal_error();
    };

    render(&InboxTemplate {
        sidebar,
        filter: filter_view,
        list,
    })
}

/// `GET /items/{id}` — the article reader.
pub async fn reader(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(item) = db.get_content_item(id) else {
        return not_found();
    };

    let tags = db
        .tags_for_item(id)
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect();

    let source = item_source(&db, &item);
    let paragraphs = split_paragraphs(item.content_text.as_deref());

    render(&ReaderTemplate {
        item_id: item.id.to_string(),
        title: item.title.clone(),
        author: item.author.clone().unwrap_or_default(),
        source,
        date: fmt_date(item.published_at.or(Some(item.created_at))),
        has_url: item.url.is_some(),
        url: item.url.clone().unwrap_or_default(),
        type_label: type_label(item.content_type).to_owned(),
        status: item.status.as_str().to_owned(),
        back_url: "/inbox".to_owned(),
        tags,
        has_content: !paragraphs.is_empty(),
        paragraphs,
    })
}

/// `POST /items/{id}/status` — change an item's triage status.
pub async fn item_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<StatusForm>,
) -> Response {
    let Some(new_status) = action_to_status(&form.action) else {
        return (StatusCode::BAD_REQUEST, "unknown action").into_response();
    };
    let from_reader = form.view.as_deref() == Some("status");

    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        if db.update_content_item_status(id, new_status).is_err() {
            return not_found();
        }
    }

    if !is_htmx(&headers) {
        return if from_reader {
            Redirect::to(&format!("/items/{id}")).into_response()
        } else {
            Redirect::to("/inbox").into_response()
        };
    }

    // Reader status badge: return just the new status text.
    if from_reader {
        return Html(new_status.as_str().to_owned()).into_response();
    }

    // Inbox row: return the refreshed row.
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    db.get_content_item(id).map_or_else(
        |_| not_found(),
        |item| {
            let view = build_item_view(&db, &item);
            render(&ItemRowTemplate { item: view })
        },
    )
}
/// `POST /items/{id}/tags` — add a tag to an item.
pub async fn add_tag(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<AddTagForm>,
) -> Response {
    let name = form.name.trim();
    if !name.is_empty() {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        match db.get_or_create_tag(name) {
            Ok(tag) => {
                let _ = db.tag_content_item(id, tag.id);
            }
            Err(_) => return internal_error(),
        }
    }
    tag_section_response(&state, id, &headers)
}

/// `POST /items/{id}/tags/{tag}/delete` — remove a tag from an item.
///
/// Uses POST rather than DELETE so plain HTML forms (no JS) can submit it.
pub async fn remove_tag(
    State(state): State<AppState>,
    Path((id, tag)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        if let Ok(Some(t)) = db.get_tag_by_name(&tag) {
            let _ = db.untag_content_item(id, t.id);
        }
    }
    tag_section_response(&state, id, &headers)
}

/// `POST /items/bulk` — apply an action to a set of selected items.
///
/// The body is parsed manually because `serde_urlencoded` (used by
/// `axum::Form`) cannot deserialize the repeated `ids` field into a `Vec`.
pub async fn bulk(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut action = String::new();
    let mut query = InboxQuery::default();

    for (key, value) in url::form_urlencoded::parse(&body) {
        match key.as_ref() {
            "ids" => {
                if let Ok(id) = value.parse::<Uuid>() {
                    ids.push(id);
                }
            }
            "action" => action = value.into_owned(),
            "status" => query.status = non_empty(Some(&value)).map(str::to_owned),
            "type" => query.content_type = non_empty(Some(&value)).map(str::to_owned),
            "tag" => query.tag = non_empty(Some(&value)).map(str::to_owned),
            "sort" => query.sort = non_empty(Some(&value)).map(str::to_owned),
            "feed" => query.feed = value.parse().ok(),
            "folder" => query.folder = value.parse().ok(),
            "page" => query.page = value.parse().ok(),
            "per_page" => query.per_page = value.parse().ok(),
            _ => {}
        }
    }

    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        for id in &ids {
            if action == "delete" {
                let _ = db.delete_content_item(*id);
            } else if let Some(status) = action_to_status(&action) {
                let _ = db.update_content_item_status(*id, status);
            }
        }
    }

    if !is_htmx(&headers) {
        let pairs = active_filter_pairs(&query, parse_sort(query.sort.as_deref()));
        let target = if pairs.is_empty() {
            "/inbox".to_owned()
        } else {
            format!("/inbox?{}", encode_pairs(&pairs))
        };
        return Redirect::to(&target).into_response();
    }

    // Re-render the list fragment with the same filters applied.
    inbox(State(state), headers, Query(query)).await
}

// ======================================================================
// Response helpers
// ======================================================================

/// Render a template into an HTML response, mapping render errors to 500.
fn render<T: Template>(tmpl: &T) -> Response {
    match tmpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            internal_error()
        }
    }
}

/// Build the tag-section fragment (HTMX) or redirect (no-JS).
fn tag_section_response(state: &AppState, id: Uuid, headers: &HeaderMap) -> Response {
    if !is_htmx(headers) {
        return Redirect::to(&format!("/items/{id}")).into_response();
    }
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    let tags = db
        .tags_for_item(id)
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect();
    render(&TagSectionTemplate {
        item_id: id.to_string(),
        tags,
    })
}

/// A simple 404 HTML response.
fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html("<h1>404</h1><p>Not found. <a href=\"/inbox\">Back to inbox</a></p>".to_owned()),
    )
        .into_response()
}

/// A simple 500 HTML response.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("<h1>500</h1><p>Something went wrong.</p>".to_owned()),
    )
        .into_response()
}

/// Whether the request was issued by HTMX.
fn is_htmx(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "true")
}

// ======================================================================
// Building view models
// ======================================================================

/// Construct an [`ItemView`] for a content item, resolving its source.
fn build_item_view(db: &Database, item: &ContentItem) -> ItemView {
    let favicon = if let Ok(meta) = db.get_bookmark_meta(item.id)
        && let Some(f) = meta.favicon_url
    {
        f
    } else {
        String::new()
    };

    ItemView {
        id: item.id.to_string(),
        title: item.title.clone(),
        reader_url: format!("/items/{}", item.id),
        has_url: item.url.is_some(),
        url: item.url.clone().unwrap_or_default(),
        favicon_url: favicon,
        source: item_source(db, item),
        type_label: type_label(item.content_type).to_owned(),
        date: fmt_date(item.published_at.or(Some(item.created_at))),
        is_read: item.read_at.is_some() || item.status == DocumentStatus::Archived,
        status: item.status.as_str().to_owned(),
        excerpt: item.excerpt.clone().unwrap_or_default(),
    }
}

/// Resolve a human-readable source label for an item.
fn item_source(db: &Database, item: &ContentItem) -> String {
    if item.content_type == ContentType::FeedItem {
        if let Ok(meta) = db.get_feed_item_meta(item.id)
            && let Ok(feed) = db.get_feed(meta.feed_id)
        {
            return feed.title;
        }
        return "Feed".to_owned();
    }
    if let Ok(meta) = db.get_bookmark_meta(item.id)
        && let Some(site) = meta.site_name
    {
        return site;
    }
    item.url
        .as_deref()
        .and_then(host_of)
        .unwrap_or_else(|| type_label(item.content_type).to_owned())
}

/// Build the sidebar navigation.
fn build_sidebar(
    db: &Database,
    active_status: Option<DocumentStatus>,
    query: &InboxQuery,
    active_tag: Option<&str>,
) -> Result<SidebarView, pergamon_storage::StorageError> {
    let no_feed_filter = query.feed.is_none() && query.folder.is_none() && active_tag.is_none();

    let mut status_links = Vec::new();
    for status in SIDEBAR_STATUSES {
        let count = db.count_content_items_filtered(&ContentItemFilter {
            status: Some(status),
            ..ContentItemFilter::default()
        })?;
        status_links.push(NavLink {
            label: capitalize(status.as_str()),
            href: format!("/inbox?status={}", status.as_str()),
            count: i64::try_from(count).unwrap_or(i64::MAX),
            active: no_feed_filter && active_status == Some(status),
        });
    }

    let folders = db.list_feed_folders()?;
    let feeds = db.list_feeds()?;

    let feed_nav =
        |feed: &pergamon_core::model::Feed| -> Result<FeedNav, pergamon_storage::StorageError> {
            let count = db.count_content_items_filtered(&ContentItemFilter {
                feed_id: Some(feed.id),
                ..ContentItemFilter::default()
            })?;
            Ok(FeedNav {
                title: feed.title.clone(),
                href: format!("/inbox?feed={}", feed.id),
                count: i64::try_from(count).unwrap_or(i64::MAX),
                active: query.feed == Some(feed.id),
            })
        };

    let mut folder_navs = Vec::new();
    for folder in &folders {
        let mut feed_navs = Vec::new();
        for feed in feeds.iter().filter(|f| f.folder_id == Some(folder.id)) {
            feed_navs.push(feed_nav(feed)?);
        }
        folder_navs.push(FolderNav {
            name: folder.name.clone(),
            href: format!("/inbox?folder={}", folder.id),
            active: query.folder == Some(folder.id),
            feeds: feed_navs,
        });
    }

    let mut feeds_root = Vec::new();
    for feed in feeds.iter().filter(|f| f.folder_id.is_none()) {
        feeds_root.push(feed_nav(feed)?);
    }

    let tags = db
        .list_tags_with_counts()?
        .into_iter()
        .take(SIDEBAR_TAG_LIMIT)
        .map(|tc| NavLink {
            active: active_tag == Some(tc.tag_name.as_str()),
            label: tc.tag_name.clone(),
            href: format!("/inbox?tag={}", urlencode(&tc.tag_name)),
            count: tc.count,
        })
        .collect();

    Ok(SidebarView {
        status_links,
        folders: folder_navs,
        feeds_root,
        tags,
    })
}

/// Build the filter-bar view (current selections plus option lists).
fn build_filter_view(
    db: &Database,
    query: &InboxQuery,
    status: Option<DocumentStatus>,
    content_type: Option<ContentType>,
    sort: ContentItemSort,
    tag_name: Option<&str>,
) -> Result<FilterView, pergamon_storage::StorageError> {
    let tags = db
        .list_tags_with_counts()?
        .into_iter()
        .map(|tc| tc.tag_name)
        .collect();

    Ok(FilterView {
        status: status.map(|s| s.as_str().to_owned()).unwrap_or_default(),
        content_type: content_type
            .map(|c| c.as_str().to_owned())
            .unwrap_or_default(),
        tag: tag_name.unwrap_or_default().to_owned(),
        sort: sort_value(sort).to_owned(),
        feed: query.feed.map(|f| f.to_string()).unwrap_or_default(),
        folder: query.folder.map(|f| f.to_string()).unwrap_or_default(),
        statuses: SIDEBAR_STATUSES_ALL
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        content_types: CONTENT_TYPE_VALUES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        tags,
    })
}

// ======================================================================
// Parsing and formatting helpers
// ======================================================================

/// All status string values offered in the filter dropdown.
const SIDEBAR_STATUSES_ALL: [&str; 6] = [
    "inbox",
    "later",
    "reference",
    "reading",
    "archived",
    "discarded",
];

/// All content type values offered in the filter dropdown.
const CONTENT_TYPE_VALUES: [&str; 6] = [
    "feed_item",
    "article",
    "bookmark",
    "highlight",
    "pdf",
    "podcast_episode",
];

/// Treat empty strings as absent.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// Parse a status query value.
fn parse_status(value: Option<&str>) -> Option<DocumentStatus> {
    non_empty(value).and_then(|v| v.parse().ok())
}

/// Parse a content type query value.
fn parse_content_type(value: Option<&str>) -> Option<ContentType> {
    non_empty(value).and_then(|v| v.parse().ok())
}

/// Parse a sort query value, defaulting to newest-first.
fn parse_sort(value: Option<&str>) -> ContentItemSort {
    match non_empty(value) {
        Some("title") => ContentItemSort::TitleAsc,
        Some("source") => ContentItemSort::SourceAsc,
        _ => ContentItemSort::CreatedDesc,
    }
}

/// The query string value for a sort order.
const fn sort_value(sort: ContentItemSort) -> &'static str {
    match sort {
        ContentItemSort::CreatedDesc => "date",
        ContentItemSort::TitleAsc => "title",
        ContentItemSort::SourceAsc => "source",
    }
}

/// Map a triage action keyword to a document status.
fn action_to_status(action: &str) -> Option<DocumentStatus> {
    match action {
        "read" | "archive" => Some(DocumentStatus::Archived),
        "later" => Some(DocumentStatus::Later),
        "reading" => Some(DocumentStatus::Reading),
        "reference" => Some(DocumentStatus::Reference),
        "discard" => Some(DocumentStatus::Discarded),
        "inbox" => Some(DocumentStatus::Inbox),
        _ => None,
    }
}

/// Human-readable label for a content type.
const fn type_label(ct: ContentType) -> &'static str {
    match ct {
        ContentType::FeedItem => "Feed",
        ContentType::Article => "Article",
        ContentType::Bookmark => "Bookmark",
        ContentType::Highlight => "Highlight",
        ContentType::Pdf => "PDF",
        ContentType::PodcastEpisode => "Podcast",
    }
}

/// Format an optional timestamp as `YYYY-MM-DD`.
fn fmt_date(dt: Option<OffsetDateTime>) -> String {
    dt.map_or_else(String::new, |d| {
        let date = d.date();
        format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            u8::from(date.month()),
            date.day()
        )
    })
}

/// Split extracted plain text into non-empty paragraphs.
fn split_paragraphs(text: Option<&str>) -> Vec<String> {
    let Some(text) = text else {
        return Vec::new();
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let paras: Vec<String> = trimmed
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if paras.is_empty() {
        vec![trimmed.to_owned()]
    } else {
        paras
    }
}

/// Extract the host portion of a URL for display.
fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|u| {
        u.host_str()
            .map(|h| h.trim_start_matches("www.").to_owned())
    })
}

/// Capitalize the first character of a lowercase keyword.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// Percent-encode a value for use in a query string.
fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

// ======================================================================
// Pagination / query-string helpers
// ======================================================================

/// Collect the active filter as ordered (key, value) pairs (excluding page).
fn active_filter_pairs(query: &InboxQuery, sort: ContentItemSort) -> Vec<(&'static str, String)> {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if let Some(s) = non_empty(query.status.as_deref()) {
        pairs.push(("status", s.to_owned()));
    }
    if let Some(t) = non_empty(query.content_type.as_deref()) {
        pairs.push(("type", t.to_owned()));
    }
    if let Some(t) = non_empty(query.tag.as_deref()) {
        pairs.push(("tag", t.to_owned()));
    }
    if let Some(f) = query.feed {
        pairs.push(("feed", f.to_string()));
    }
    if let Some(f) = query.folder {
        pairs.push(("folder", f.to_string()));
    }
    if sort != ContentItemSort::CreatedDesc {
        pairs.push(("sort", sort_value(sort).to_owned()));
    }
    pairs
}

/// Encode (key, value) pairs into a query string.
fn encode_pairs(pairs: &[(&'static str, String)]) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.finish()
}

/// Build a pagination URL, or an empty string when the page is unavailable.
fn page_url(
    filter_pairs: &[(&'static str, String)],
    page: u32,
    per_page: u32,
    enabled: bool,
) -> String {
    if !enabled {
        return String::new();
    }
    let mut pairs = filter_pairs.to_vec();
    pairs.push(("page", page.to_string()));
    if per_page != DEFAULT_PER_PAGE {
        pairs.push(("per_page", per_page.to_string()));
    }
    format!("/inbox?{}", encode_pairs(&pairs))
}

/// Compute the total number of pages (at least 1).
fn total_pages(total: u64, per_page: u32) -> u32 {
    if total == 0 {
        return 1;
    }
    let pages = total.div_ceil(u64::from(per_page));
    u32::try_from(pages).unwrap_or(u32::MAX)
}

// ======================================================================
// Highlights, notes, and review web views
// ======================================================================

struct HighlightSourceOptionView {
    id: String,
    title: String,
}

struct HighlightFilterView {
    tag: String,
    source: String,
    since: String,
    before: String,
    color: String,
    tags: Vec<String>,
    sources: Vec<HighlightSourceOptionView>,
    colors: Vec<String>,
}

struct HighlightRowView {
    id: String,
    quote_text: String,
    note: String,
    color: String,
    created_at: String,
    source_title: String,
    source_href: String,
}

struct HighlightGroupView {
    key: String,
    title: String,
    href: String,
    highlights: Vec<HighlightRowView>,
}

#[derive(Template)]
#[template(path = "highlights.html")]
struct HighlightsTemplate {
    filter: HighlightFilterView,
    groups: Vec<HighlightGroupView>,
    total: usize,
}

#[derive(Template)]
#[template(path = "_highlight_row.html")]
struct HighlightRowTemplate {
    row: HighlightRowView,
}

struct NoteSourceOptionView {
    id: String,
    title: String,
}

struct NoteTargetOptionView {
    id: String,
    label: String,
}

struct NoteRowView {
    id: String,
    body: String,
    created_at: String,
    updated_at: String,
    source_title: String,
    source_href: String,
}

struct NotesPanelView {
    query: String,
    source: String,
    sources: Vec<NoteSourceOptionView>,
    create_targets: Vec<NoteTargetOptionView>,
    notes: Vec<NoteRowView>,
    total: usize,
}

#[derive(Template)]
#[template(path = "notes.html")]
struct NotesTemplate {
    panel: NotesPanelView,
}

#[derive(Template)]
#[template(path = "_notes_panel.html")]
struct NotesPanelTemplate {
    panel: NotesPanelView,
}

struct ReviewCardView {
    card_id: String,
    source_title: String,
    source_href: String,
    state: String,
    due_at: String,
    quote_text: String,
    note: String,
    color: String,
}

struct ReviewPanelView {
    has_card: bool,
    card: ReviewCardView,
    queue_remaining: usize,
    reviewed_today: i64,
    due_count: i64,
    current_streak: i64,
    last_rating: String,
}

#[derive(Template)]
#[template(path = "review.html")]
struct ReviewTemplate {
    panel: ReviewPanelView,
}

#[derive(Template)]
#[template(path = "_review_panel.html")]
struct ReviewPanelTemplate {
    panel: ReviewPanelView,
}

struct StatCardView {
    label: String,
    value: String,
}

struct StatSeriesPointView {
    label: String,
    reviews: i64,
    retention: String,
    review_percent: i64,
}

struct MaturityPointView {
    label: String,
    count: i64,
    percent: String,
    bar_percent: i64,
}

#[derive(Template)]
#[template(path = "review_stats.html")]
struct ReviewStatsTemplate {
    cards: Vec<StatCardView>,
    daily: Vec<StatSeriesPointView>,
    weekly: Vec<StatSeriesPointView>,
    monthly: Vec<StatSeriesPointView>,
    maturity: Vec<MaturityPointView>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct HighlightsQuery {
    tag: Option<String>,
    source: Option<Uuid>,
    since: Option<String>,
    before: Option<String>,
    color: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct HighlightsExportQuery {
    format: Option<String>,
    tag: Option<String>,
    source: Option<Uuid>,
    since: Option<String>,
    before: Option<String>,
    color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HighlightNoteForm {
    #[serde(default)]
    note: String,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct NotesQuery {
    q: Option<String>,
    source: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNoteWebForm {
    content_item_id: Uuid,
    body: String,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    source: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNoteWebForm {
    body: String,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    source: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DeleteNoteWebForm {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    source: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ReviewQuery {
    last: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitReviewWebForm {
    rating: u32,
}

#[derive(Debug, Serialize)]
struct HighlightExportRow {
    id: String,
    source_id: Option<String>,
    source_title: String,
    source_href: String,
    quote_text: String,
    note: Option<String>,
    color: Option<String>,
    created_at: String,
}

/// `GET /highlights` — list highlights grouped by source with filters.
pub async fn highlights(
    State(state): State<AppState>,
    Query(query): Query<HighlightsQuery>,
) -> Response {
    let Ok(since) = parse_optional_date(query.since.as_deref()) else {
        return bad_request();
    };
    let Ok(before) = parse_optional_date(query.before.as_deref()) else {
        return bad_request();
    };

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let tag = non_empty(query.tag.as_deref());
    let color = non_empty(query.color.as_deref());
    let Ok(rows) = list_highlights_for_filters(&db, query.source, tag, since, before, color) else {
        return internal_error();
    };
    let Ok(all_rows) = db.list_highlights(None, None, None, None, None) else {
        return internal_error();
    };
    let template = build_highlights_template(&db, &query, &rows, &all_rows);
    render(&template)
}

/// `GET /highlights/export` — export filtered highlights as JSON or Markdown.
pub async fn highlights_export(
    State(state): State<AppState>,
    Query(query): Query<HighlightsExportQuery>,
) -> Response {
    let Ok(since) = parse_optional_date(query.since.as_deref()) else {
        return bad_request();
    };
    let Ok(before) = parse_optional_date(query.before.as_deref()) else {
        return bad_request();
    };

    let format = query.format.as_deref().unwrap_or("json");
    let tag = non_empty(query.tag.as_deref());
    let color = non_empty(query.color.as_deref());

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    let Ok(rows) = list_highlights_for_filters(&db, query.source, tag, since, before, color) else {
        return internal_error();
    };

    let export_rows: Vec<HighlightExportRow> = rows
        .iter()
        .map(|(item, meta)| {
            let (_source_key, source_title, source_href) = highlight_source_context(&db, meta);
            HighlightExportRow {
                id: item.id.to_string(),
                source_id: meta.source_item_id.map(|id| id.to_string()),
                source_title,
                source_href,
                quote_text: meta.quote_text.clone(),
                note: meta.note.clone(),
                color: meta.color.clone(),
                created_at: fmt_date(Some(item.created_at)),
            }
        })
        .collect();

    if format.eq_ignore_ascii_case("markdown") {
        let mut out = String::new();
        let _ = writeln!(out, "# pergamon highlights export");
        let _ = writeln!(out);
        let mut current_group = String::new();
        for row in &export_rows {
            let group = if row.source_title.is_empty() {
                "Unlinked source"
            } else {
                row.source_title.as_str()
            };
            if current_group != group {
                group.clone_into(&mut current_group);
                let _ = writeln!(out, "## {group}");
                let _ = writeln!(out);
            }
            let _ = writeln!(out, "> {}", row.quote_text);
            if let Some(note) = &row.note
                && !note.trim().is_empty()
            {
                let _ = writeln!(out, "*{note}*");
            }
            if let Some(color_value) = &row.color
                && !color_value.is_empty()
            {
                let _ = writeln!(out, "- color: {color_value}");
            }
            if !row.source_href.is_empty() {
                let _ = writeln!(out, "- source: [{}]({})", row.source_title, row.source_href);
            }
            let _ = writeln!(out, "- captured: {}", row.created_at);
            let _ = writeln!(out);
        }
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"highlights.md\"",
                ),
            ],
            out,
        )
            .into_response();
    }

    let Ok(json_body) = serde_json::to_string_pretty(&export_rows) else {
        return internal_error();
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"highlights.json\"",
            ),
        ],
        json_body,
    )
        .into_response()
}

/// `POST /highlights/{id}/note` — update a highlight note inline.
pub async fn update_highlight_note(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<HighlightNoteForm>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(existing) = db.get_highlight_meta(id) else {
        return not_found();
    };

    let updated_note = if form.note.trim().is_empty() {
        None
    } else {
        Some(form.note.as_str())
    };
    if db
        .update_highlight_meta(id, updated_note, existing.color.as_deref())
        .is_err()
    {
        return not_found();
    }

    if !is_htmx(&headers) {
        return Redirect::to("/highlights").into_response();
    }

    let Ok(item) = db.get_content_item(id) else {
        return not_found();
    };
    let Ok(meta) = db.get_highlight_meta(id) else {
        return not_found();
    };
    let row = build_highlight_row_view(&db, &item, &meta);
    render(&HighlightRowTemplate { row })
}

/// `GET /notes` — list notes with source context and search.
pub async fn notes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NotesQuery>,
) -> Response {
    render_notes_response(&state, &headers, &query)
}

/// `POST /notes/create` — create a note from the notes page.
pub async fn create_note_web(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CreateNoteWebForm>,
) -> Response {
    if form.body.trim().is_empty() {
        return bad_request();
    }
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    if db.get_content_item(form.content_item_id).is_err() {
        return not_found();
    }
    let now = OffsetDateTime::now_utc();
    let note = Note {
        id: Uuid::new_v4(),
        content_item_id: form.content_item_id,
        body: form.body.clone(),
        created_at: now,
        updated_at: now,
    };
    if db.insert_note(&note).is_err() {
        return internal_error();
    }
    drop(db);

    let query = NotesQuery {
        q: form.q.clone(),
        source: form.source,
    };
    if !is_htmx(&headers) {
        return Redirect::to(&notes_url(&query)).into_response();
    }
    render_notes_panel(&state, &query)
}

/// `POST /notes/{id}/update` — update an existing note inline.
pub async fn update_note_web(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<UpdateNoteWebForm>,
) -> Response {
    if form.body.trim().is_empty() {
        return bad_request();
    }
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    match db.update_note(id, &form.body) {
        Ok(()) => {}
        Err(err) => {
            if matches!(err, StorageError::NotFound { .. }) {
                return not_found();
            }
            return internal_error();
        }
    }
    drop(db);

    let query = NotesQuery {
        q: form.q.clone(),
        source: form.source,
    };
    if !is_htmx(&headers) {
        return Redirect::to(&notes_url(&query)).into_response();
    }
    render_notes_panel(&state, &query)
}

/// `POST /notes/{id}/delete` — delete a note inline.
pub async fn delete_note_web(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<DeleteNoteWebForm>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    match db.delete_note(id) {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return internal_error(),
    }
    drop(db);

    let query = NotesQuery {
        q: form.q.clone(),
        source: form.source,
    };
    if !is_htmx(&headers) {
        return Redirect::to(&notes_url(&query)).into_response();
    }
    render_notes_panel(&state, &query)
}

/// `GET /review` — card-based review queue.
pub async fn review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ReviewQuery>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    let Ok(panel) = build_review_panel(&db, query.last.as_deref()) else {
        return internal_error();
    };
    if is_htmx(&headers) {
        return render(&ReviewPanelTemplate { panel });
    }
    render(&ReviewTemplate { panel })
}

/// `POST /review/{card_id}` — submit a rating from the web review queue.
pub async fn submit_review_web(
    State(state): State<AppState>,
    Path(card_id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<SubmitReviewWebForm>,
) -> Response {
    let Some(rating) = Rating::from_value(form.rating) else {
        return bad_request();
    };

    let last_rating = match rating {
        Rating::Again => "again",
        Rating::Hard => "hard",
        Rating::Good => "good",
        Rating::Easy => "easy",
    };

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    match apply_review_rating(&db, card_id, rating, OffsetDateTime::now_utc()) {
        Ok(_) => {}
        Err(err) => {
            if matches!(err, StorageError::NotFound { .. }) {
                return not_found();
            }
            return internal_error();
        }
    }

    if !is_htmx(&headers) {
        return Redirect::to(&format!("/review?last={last_rating}")).into_response();
    }

    let Ok(panel) = build_review_panel(&db, Some(last_rating)) else {
        return internal_error();
    };
    render(&ReviewPanelTemplate { panel })
}

/// `GET /review/stats` — review statistics dashboard page.
pub async fn review_stats_page(State(state): State<AppState>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    let Ok(report) = db.review_stats_report(OffsetDateTime::now_utc()) else {
        return internal_error();
    };

    let cards = vec![
        StatCardView {
            label: "Total cards".to_owned(),
            value: report.stats.total_cards.to_string(),
        },
        StatCardView {
            label: "Due now".to_owned(),
            value: report.stats.due_count.to_string(),
        },
        StatCardView {
            label: "Reviews today".to_owned(),
            value: report.stats.reviews_today.to_string(),
        },
        StatCardView {
            label: "Observed retention".to_owned(),
            value: format!("{:.1}%", report.stats.observed_retention * 100.0),
        },
        StatCardView {
            label: "Current streak".to_owned(),
            value: report.stats.current_streak.to_string(),
        },
        StatCardView {
            label: "Longest streak".to_owned(),
            value: report.stats.longest_streak.to_string(),
        },
    ];

    let daily = build_series_points_daily(&report);
    let weekly = build_series_points_weekly(&report);
    let monthly = build_series_points_monthly(&report);
    let maturity = build_maturity_points(&report);

    render(&ReviewStatsTemplate {
        cards,
        daily,
        weekly,
        monthly,
        maturity,
    })
}

fn parse_optional_date(value: Option<&str>) -> Result<Option<OffsetDateTime>, ()> {
    non_empty(value).map_or_else(
        || Ok(None),
        |v| parse_date_param(v).map(Some).map_err(|_| ()),
    )
}

fn list_highlights_for_filters(
    db: &Database,
    source: Option<Uuid>,
    tag: Option<&str>,
    since: Option<OffsetDateTime>,
    before: Option<OffsetDateTime>,
    color: Option<&str>,
) -> Result<Vec<(ContentItem, HighlightMeta)>, StorageError> {
    let mut rows = db.list_highlights(source, tag, since, before, None)?;
    if let Some(color_filter) = color {
        rows.retain(|(_, meta)| {
            meta.color
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(color_filter))
        });
    }
    Ok(rows)
}

fn build_highlights_template(
    db: &Database,
    query: &HighlightsQuery,
    rows: &[(ContentItem, HighlightMeta)],
    all_rows: &[(ContentItem, HighlightMeta)],
) -> HighlightsTemplate {
    let tags = db
        .list_tags_with_counts()
        .map(|rows| rows.into_iter().map(|r| r.tag_name).collect())
        .unwrap_or_default();

    let mut source_opts = BTreeSet::<(String, String)>::new();
    let mut color_opts = BTreeSet::<String>::new();
    for (_item, meta) in all_rows {
        let (_, title, _) = highlight_source_context(db, meta);
        if let Some(source_id) = meta.source_item_id {
            source_opts.insert((source_id.to_string(), title));
        }
        if let Some(color) = &meta.color
            && !color.is_empty()
        {
            color_opts.insert(color.clone());
        }
    }

    let mut groups: Vec<HighlightGroupView> = Vec::new();
    for (item, meta) in rows {
        let (source_key, source_title, source_href) = highlight_source_context(db, meta);
        let row = build_highlight_row_view(db, item, meta);
        if let Some(group) = groups.iter_mut().find(|group| group.key == source_key) {
            group.highlights.push(row);
        } else {
            groups.push(HighlightGroupView {
                key: source_key,
                title: source_title,
                href: source_href,
                highlights: vec![row],
            });
        }
    }

    HighlightsTemplate {
        filter: HighlightFilterView {
            tag: query.tag.clone().unwrap_or_default(),
            source: query.source.map(|id| id.to_string()).unwrap_or_default(),
            since: query.since.clone().unwrap_or_default(),
            before: query.before.clone().unwrap_or_default(),
            color: query.color.clone().unwrap_or_default(),
            tags,
            sources: source_opts
                .into_iter()
                .map(|(id, title)| HighlightSourceOptionView { id, title })
                .collect(),
            colors: color_opts.into_iter().collect(),
        },
        total: rows.len(),
        groups,
    }
}

fn build_highlight_row_view(
    db: &Database,
    item: &ContentItem,
    meta: &HighlightMeta,
) -> HighlightRowView {
    let (_, source_title, source_href) = highlight_source_context(db, meta);
    HighlightRowView {
        id: item.id.to_string(),
        quote_text: meta.quote_text.clone(),
        note: meta.note.clone().unwrap_or_default(),
        color: meta.color.clone().unwrap_or_default(),
        created_at: fmt_date(Some(item.created_at)),
        source_title,
        source_href,
    }
}

fn highlight_source_context(db: &Database, meta: &HighlightMeta) -> (String, String, String) {
    if let Some(source_id) = meta.source_item_id
        && let Ok(source_item) = db.get_content_item(source_id)
    {
        return (
            source_id.to_string(),
            source_item.title,
            format!("/items/{source_id}"),
        );
    }
    (
        "unlinked".to_owned(),
        "Unlinked source".to_owned(),
        String::new(),
    )
}

fn render_notes_response(state: &AppState, headers: &HeaderMap, query: &NotesQuery) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    let Ok(panel) = build_notes_panel(&db, query) else {
        return internal_error();
    };
    if is_htmx(headers) {
        return render(&NotesPanelTemplate { panel });
    }
    render(&NotesTemplate { panel })
}

fn render_notes_panel(state: &AppState, query: &NotesQuery) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    build_notes_panel(&db, query).map_or_else(
        |_| internal_error(),
        |panel| render(&NotesPanelTemplate { panel }),
    )
}

fn build_notes_panel(db: &Database, query: &NotesQuery) -> Result<NotesPanelView, StorageError> {
    let query_text = query.q.clone().unwrap_or_default();
    let query_lower = query_text.to_lowercase();
    let mut notes = db.list_all_notes()?;
    notes.retain(|note| {
        if let Some(source_id) = query.source
            && note.content_item_id != source_id
        {
            return false;
        }
        if query_lower.is_empty() {
            return true;
        }
        note.body.to_lowercase().contains(&query_lower)
    });
    notes.sort_by_key(|note| std::cmp::Reverse(note.updated_at));

    let all_notes = db.list_all_notes()?;
    let mut source_opts = BTreeSet::<(String, String)>::new();
    for note in &all_notes {
        if let Ok(item) = db.get_content_item(note.content_item_id) {
            source_opts.insert((item.id.to_string(), item.title));
        }
    }

    let recent_items =
        db.list_content_items_filtered(&ContentItemFilter::default(), Some(200), Some(0))?;
    let create_targets = recent_items
        .iter()
        .map(|item| NoteTargetOptionView {
            id: item.id.to_string(),
            label: format!("{} · {}", item.title, item_source(db, item)),
        })
        .collect();

    let mut note_rows = Vec::new();
    for note in notes {
        if let Ok(item) = db.get_content_item(note.content_item_id) {
            note_rows.push(NoteRowView {
                id: note.id.to_string(),
                body: note.body,
                created_at: fmt_date(Some(note.created_at)),
                updated_at: fmt_date(Some(note.updated_at)),
                source_title: item.title,
                source_href: format!("/items/{}", item.id),
            });
        }
    }

    let total = note_rows.len();
    Ok(NotesPanelView {
        query: query_text,
        source: query.source.map(|id| id.to_string()).unwrap_or_default(),
        sources: source_opts
            .into_iter()
            .map(|(id, title)| NoteSourceOptionView { id, title })
            .collect(),
        create_targets,
        notes: note_rows,
        total,
    })
}

fn notes_url(query: &NotesQuery) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(q) = non_empty(query.q.as_deref()) {
        pairs.push(("q", q.to_owned()));
    }
    if let Some(source) = query.source {
        pairs.push(("source", source.to_string()));
    }
    if pairs.is_empty() {
        "/notes".to_owned()
    } else {
        format!("/notes?{}", encode_pairs(&pairs))
    }
}

fn build_review_panel(
    db: &Database,
    last_rating: Option<&str>,
) -> Result<ReviewPanelView, StorageError> {
    let now = OffsetDateTime::now_utc();
    let queue = db.list_due_review_cards(now)?;
    let stats = db.review_stats(now)?;
    let card = queue
        .first()
        .and_then(|card| build_review_card_view(db, card));

    let fallback_card = ReviewCardView {
        card_id: String::new(),
        source_title: String::new(),
        source_href: String::new(),
        state: String::new(),
        due_at: String::new(),
        quote_text: String::new(),
        note: String::new(),
        color: String::new(),
    };

    Ok(ReviewPanelView {
        has_card: card.is_some(),
        card: card.unwrap_or(fallback_card),
        queue_remaining: queue.len(),
        reviewed_today: stats.reviews_today,
        due_count: stats.due_count,
        current_streak: stats.current_streak,
        last_rating: last_rating.unwrap_or_default().to_owned(),
    })
}

fn build_review_card_view(db: &Database, card: &ReviewCard) -> Option<ReviewCardView> {
    let meta = db.get_highlight_meta(card.content_item_id).ok()?;
    let (_source_key, source_title, source_href) = highlight_source_context(db, &meta);
    Some(ReviewCardView {
        card_id: card.id.to_string(),
        source_title,
        source_href,
        state: card.state.as_str().to_owned(),
        due_at: fmt_date(Some(card.due_at)),
        quote_text: meta.quote_text,
        note: meta.note.unwrap_or_default(),
        color: meta.color.unwrap_or_default(),
    })
}

fn build_series_points_daily(report: &ReviewStatsReport) -> Vec<StatSeriesPointView> {
    let max_reviews = report
        .daily_history
        .iter()
        .map(|point| point.reviews)
        .max()
        .unwrap_or(1)
        .max(1);
    report
        .daily_history
        .iter()
        .map(|point| StatSeriesPointView {
            label: point.date.clone(),
            reviews: point.reviews,
            retention: format_ratio_percent(point.successes, point.reviews),
            review_percent: (point.reviews * 100 / max_reviews),
        })
        .collect()
}

fn build_series_points_weekly(report: &ReviewStatsReport) -> Vec<StatSeriesPointView> {
    let max_reviews = report
        .weekly_history
        .iter()
        .map(|point| point.reviews)
        .max()
        .unwrap_or(1)
        .max(1);
    report
        .weekly_history
        .iter()
        .map(|point| StatSeriesPointView {
            label: point.week.clone(),
            reviews: point.reviews,
            retention: format_ratio_percent(point.successes, point.reviews),
            review_percent: (point.reviews * 100 / max_reviews),
        })
        .collect()
}

fn build_series_points_monthly(report: &ReviewStatsReport) -> Vec<StatSeriesPointView> {
    let max_reviews = report
        .monthly_history
        .iter()
        .map(|point| point.reviews)
        .max()
        .unwrap_or(1)
        .max(1);
    report
        .monthly_history
        .iter()
        .map(|point| StatSeriesPointView {
            label: point.month.clone(),
            reviews: point.reviews,
            retention: format_ratio_percent(point.successes, point.reviews),
            review_percent: (point.reviews * 100 / max_reviews),
        })
        .collect()
}

fn build_maturity_points(report: &ReviewStatsReport) -> Vec<MaturityPointView> {
    let total = report.stats.total_cards.max(1);
    let points = [
        ("New", report.stats.new_count),
        ("Learning", report.stats.learning_count),
        ("Review", report.stats.review_count),
        ("Relearning", report.stats.relearning_count),
    ];
    points
        .into_iter()
        .map(|(label, count)| MaturityPointView {
            label: label.to_owned(),
            count,
            percent: format_ratio_percent(count, total),
            bar_percent: (count * 100 / total),
        })
        .collect()
}

fn format_ratio_percent(numerator: i64, denominator: i64) -> String {
    if denominator <= 0 {
        return "0.0%".to_owned();
    }
    let scaled_tenths = numerator.saturating_mul(1000) / denominator;
    let whole = scaled_tenths / 10;
    let tenths = scaled_tenths.rem_euclid(10);
    format!("{whole}.{tenths}%")
}

fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html("<h1>400</h1><p>Invalid request.</p>".to_owned()),
    )
        .into_response()
}
