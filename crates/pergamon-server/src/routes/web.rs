// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered HTML views: inbox/library and article reader.
//!
//! These handlers render HTML with Askama templates and enhance interactions
//! with HTMX. They query `pergamon-storage` directly (the same pattern the
//! JSON API handlers use) and degrade gracefully without JavaScript: every
//! action is reachable via a plain link or form submission, and handlers
//! return a redirect when the request is not an HTMX request.

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::ContentItem;
use pergamon_core::status::DocumentStatus;
use pergamon_storage::{ContentItemFilter, ContentItemSort, Database};

use crate::state::AppState;

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
