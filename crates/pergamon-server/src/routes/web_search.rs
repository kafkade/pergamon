// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered search view: full-text search with faceted filters,
//! FTS5 snippet highlighting, and save-as-smart-collection.
//!
//! Follows the same conventions as [`super::web`]: Askama templates enhanced
//! with HTMX, handlers query `pergamon-storage` directly, and every action
//! degrades to a plain form submission without JavaScript. Recent searches are
//! a purely client-side enhancement (see `static/app.js`).

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::model::Collection;
use pergamon_core::smart_filter::SmartFilter;
use pergamon_storage::SearchFilter;

use super::web::{
    ItemView, build_item_view, internal_error, is_htmx, non_empty, parse_content_type,
    parse_status, render,
};
use crate::state::AppState;
use crate::util::parse_date_param;

// ======================================================================
// Constants
// ======================================================================

/// Maximum number of search results returned to the page.
const SEARCH_LIMIT: u32 = 100;

/// Content type values offered in the search facet dropdown.
const CONTENT_TYPE_VALUES: [&str; 6] = [
    "feed_item",
    "article",
    "bookmark",
    "highlight",
    "pdf",
    "podcast_episode",
];

/// Status values offered in the search facet dropdown.
const STATUS_VALUES: [&str; 6] = [
    "inbox",
    "later",
    "reference",
    "reading",
    "archived",
    "discarded",
];

// ======================================================================
// View models
// ======================================================================

/// A feed option in the source facet dropdown.
struct FeedOption {
    id: String,
    title: String,
}

/// Current facet selections plus the option lists.
struct SearchFacetsView {
    content_type: String,
    status: String,
    tag: String,
    feed: String,
    since: String,
    before: String,
    content_types: Vec<String>,
    statuses: Vec<String>,
    tags: Vec<String>,
    feeds: Vec<FeedOption>,
}

/// A single ranked search hit with its highlighted snippet.
struct SearchHitView {
    item: ItemView,
    snippet_html: String,
    has_snippet: bool,
}

/// The results region (rendered standalone for HTMX requests).
struct SearchResultsView {
    q: String,
    has_query: bool,
    hits: Vec<SearchHitView>,
    total: usize,
    /// The smart-filter DSL equivalent of the current search, for saving.
    save_dsl: String,
    can_save: bool,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchPageTemplate {
    facets: SearchFacetsView,
    results: SearchResultsView,
}

#[derive(Template)]
#[template(path = "_search_results.html")]
struct SearchResultsTemplate {
    results: SearchResultsView,
}

// ======================================================================
// Query / form types
// ======================================================================

/// Query parameters for the search page.
#[derive(Debug, Default, Deserialize)]
pub struct SearchPageQuery {
    q: Option<String>,
    #[serde(rename = "type")]
    content_type: Option<String>,
    status: Option<String>,
    tag: Option<String>,
    feed: Option<Uuid>,
    since: Option<String>,
    before: Option<String>,
}

/// Form body for saving a search as a smart collection.
#[derive(Debug, Deserialize)]
pub struct SaveSearchForm {
    name: String,
    #[serde(default)]
    dsl: String,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /search` — search page, or the results fragment for HTMX.
pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchPageQuery>,
) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let q = non_empty(query.q.as_deref()).unwrap_or("").to_owned();
    let content_type = parse_content_type(query.content_type.as_deref());
    let status = parse_status(query.status.as_deref());
    let tag = non_empty(query.tag.as_deref()).map(str::to_owned);

    let since: Option<OffsetDateTime> =
        match query.since.as_deref().map(parse_date_param).transpose() {
            Ok(v) => v,
            Err(_) => return internal_error(),
        };
    let before: Option<OffsetDateTime> =
        match query.before.as_deref().map(parse_date_param).transpose() {
            Ok(v) => v,
            Err(_) => return internal_error(),
        };

    // Resolve the feed title once for the save-as-smart-collection DSL.
    let feed_title = query
        .feed
        .and_then(|id| db.get_feed(id).ok())
        .map(|f| f.title);

    let mut hits = Vec::new();
    if !q.is_empty() {
        let filter = SearchFilter {
            content_type,
            status,
            tag_name: tag.clone(),
            feed_id: query.feed,
            since,
            before,
        };
        let Ok(found) = db.search_filtered(&q, &filter, Some(SEARCH_LIMIT)) else {
            return internal_error();
        };
        for hit in found {
            let snippet_html = hit
                .snippet
                .as_deref()
                .map(render_snippet)
                .unwrap_or_default();
            let has_snippet = !snippet_html.is_empty();
            hits.push(SearchHitView {
                item: build_item_view(&db, &hit.item),
                snippet_html,
                has_snippet,
            });
        }
    }

    let save_dsl = build_dsl(
        &q,
        query.content_type.as_deref(),
        query.status.as_deref(),
        tag.as_deref(),
        feed_title.as_deref(),
        non_empty(query.since.as_deref()),
        non_empty(query.before.as_deref()),
    );
    let can_save = !save_dsl.is_empty() && SmartFilter::parse(&save_dsl).is_ok();

    let total = hits.len();
    let results = SearchResultsView {
        has_query: !q.is_empty(),
        q,
        hits,
        total,
        save_dsl,
        can_save,
    };

    if is_htmx(&headers) {
        return render(&SearchResultsTemplate { results });
    }

    let Ok(facets) = build_facets(&db, &query) else {
        return internal_error();
    };
    render(&SearchPageTemplate { facets, results })
}

/// `POST /search/save` — save the current search as a smart collection.
pub async fn save_search(
    State(state): State<AppState>,
    Form(form): Form<SaveSearchForm>,
) -> Response {
    let name = form.name.trim();
    let dsl = form.dsl.trim();
    if name.is_empty() || dsl.is_empty() {
        return Redirect::to("/search").into_response();
    }
    if SmartFilter::parse(dsl).is_err() {
        return Redirect::to("/search").into_response();
    }

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    // Skip silently if a collection with this name already exists.
    if matches!(db.get_collection_by_name(name), Ok(Some(_))) {
        return Redirect::to("/collections").into_response();
    }

    let now = OffsetDateTime::now_utc();
    let collection = Collection {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id: None,
        sort_order: 0,
        is_smart: true,
        filter_query: Some(dsl.to_owned()),
        created_at: now,
        updated_at: now,
    };
    if db.insert_collection(&collection).is_err() {
        return internal_error();
    }

    Redirect::to(&format!("/collections/{}", collection.id)).into_response()
}

// ======================================================================
// Helpers
// ======================================================================

/// Convert an FTS5 snippet into safe HTML.
///
/// The snippet uses `»`/`«` as match delimiters (see the storage layer). Text
/// is HTML-escaped first, then the delimiters are replaced with `<mark>` tags
/// so user content can never inject markup.
fn render_snippet(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 16);
    for ch in raw.chars() {
        match ch {
            '»' => out.push_str("<mark>"),
            '«' => out.push_str("</mark>"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// Build a smart-filter DSL string from the active search query and facets.
fn build_dsl(
    q: &str,
    content_type: Option<&str>,
    status: Option<&str>,
    tag: Option<&str>,
    source: Option<&str>,
    since: Option<&str>,
    before: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !q.trim().is_empty() {
        parts.push(format!("text:{}", quote_value(q.trim())));
    }
    if let Some(ct) = non_empty(content_type) {
        parts.push(format!("type:{ct}"));
    }
    if let Some(s) = non_empty(status) {
        parts.push(format!("status:{s}"));
    }
    if let Some(t) = non_empty(tag) {
        parts.push(format!("tag:{}", quote_value(t)));
    }
    if let Some(src) = non_empty(source) {
        parts.push(format!("source:{}", quote_value(src)));
    }
    if let Some(d) = non_empty(since) {
        parts.push(format!("since:{d}"));
    }
    if let Some(d) = non_empty(before) {
        parts.push(format!("before:{d}"));
    }
    parts.join(" ")
}

/// Quote a DSL value when it contains whitespace or commas.
fn quote_value(value: &str) -> String {
    if value.contains(|c: char| c.is_whitespace() || c == ',') {
        format!("\"{}\"", value.replace('"', ""))
    } else {
        value.to_owned()
    }
}

/// Build the facet dropdown view.
fn build_facets(
    db: &pergamon_storage::Database,
    query: &SearchPageQuery,
) -> Result<SearchFacetsView, pergamon_storage::StorageError> {
    let tags = db
        .list_tags_with_counts()?
        .into_iter()
        .map(|tc| tc.tag_name)
        .collect();
    let feeds = db
        .list_feeds()?
        .into_iter()
        .map(|f| FeedOption {
            id: f.id.to_string(),
            title: f.title,
        })
        .collect();

    Ok(SearchFacetsView {
        content_type: query.content_type.clone().unwrap_or_default(),
        status: query.status.clone().unwrap_or_default(),
        tag: query.tag.clone().unwrap_or_default(),
        feed: query.feed.map(|f| f.to_string()).unwrap_or_default(),
        since: query.since.clone().unwrap_or_default(),
        before: query.before.clone().unwrap_or_default(),
        content_types: CONTENT_TYPE_VALUES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        statuses: STATUS_VALUES.iter().map(|s| (*s).to_owned()).collect(),
        tags,
        feeds,
    })
}
