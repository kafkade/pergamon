// SPDX-License-Identifier: AGPL-3.0-only

//! Admin diagnostics view.
//!
//! A server-rendered dashboard for monitoring feed health, content extraction,
//! import history, system statistics, link health, and content-rule match
//! counts. Access is gated by [`crate::auth`] when admin credentials are
//! configured. Every action degrades gracefully without JavaScript.

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use pergamon_core::diagnostics::{ExtractionEvent, FeedHealthStatus, ImportLogEntry};

use super::feeds::refresh_single_feed;
use super::web::{fmt_date, internal_error, render};
use crate::state::AppState;

/// Number of days without a successful fetch before a feed is considered stale.
const STALE_AFTER_DAYS: i64 = 7;
/// Maximum rows shown in the recent-extraction / import / broken-link tables.
const RECENT_LIMIT: u32 = 25;

// ======================================================================
// View models
// ======================================================================

/// The full admin dashboard.
#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {
    auth_enabled: bool,
    system: SystemStatsView,
    feed_summary: FeedSummaryView,
    feeds: Vec<FeedHealthView>,
    extraction: ExtractionView,
    imports: Vec<ImportLogView>,
    broken_links: Vec<BrokenLinkView>,
    rules: Vec<RuleView>,
}

/// System statistics block.
struct SystemStatsView {
    total_items: i64,
    total_feeds: i64,
    total_tags: i64,
    total_collections: i64,
    total_highlights: i64,
    total_notes: i64,
    total_review_cards: i64,
    db_size: String,
    fts_ok: bool,
    content_types: Vec<DistributionRow>,
    statuses: Vec<DistributionRow>,
}

/// A labelled count with a percentage of the whole.
struct DistributionRow {
    label: String,
    count: i64,
    percent: String,
}

/// Aggregate counts across all feeds.
struct FeedSummaryView {
    total: usize,
    healthy: usize,
    warning: usize,
    error: usize,
    stale: usize,
}

/// A single feed's health row.
struct FeedHealthView {
    feed_id: String,
    title: String,
    url: String,
    status_class: String,
    status_label: String,
    error_count: i32,
    last_error: String,
    last_fetched: String,
    is_stale: bool,
}

/// Extraction statistics and recent events.
struct ExtractionView {
    total: i64,
    succeeded: i64,
    failed: i64,
    success_rate: String,
    failures: Vec<ExtractionEventView>,
    recent: Vec<ExtractionEventView>,
}

/// A single extraction event row.
struct ExtractionEventView {
    source: String,
    extractor: String,
    success: bool,
    url: String,
    error: String,
    created: String,
}

/// A single import-history row.
struct ImportLogView {
    source: String,
    file_name: String,
    added: i64,
    existing: i64,
    skipped: i64,
    errors: i64,
    error_detail: String,
    created: String,
}

/// A single broken-link row.
struct BrokenLinkView {
    title: String,
    url: String,
    http_status: String,
    error: String,
    last_checked: String,
}

/// A single content-rule monitor row.
struct RuleView {
    name: String,
    enabled: bool,
    priority: i64,
    filter_query: String,
    match_count: i64,
    actions: String,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /admin` — render the diagnostics dashboard.
pub async fn dashboard(State(state): State<AppState>) -> Response {
    let auth_enabled = state.admin_auth.is_some();
    let now = time::OffsetDateTime::now_utc();

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(sys_stats) = db.system_stats() else {
        return internal_error();
    };
    let Ok(feed_rows) = db.feed_health(STALE_AFTER_DAYS, now) else {
        return internal_error();
    };
    let Ok(extraction_stats) = db.extraction_stats() else {
        return internal_error();
    };
    let Ok(failures) = db.list_extraction_events(RECENT_LIMIT, true) else {
        return internal_error();
    };
    let Ok(recent) = db.list_extraction_events(RECENT_LIMIT, false) else {
        return internal_error();
    };
    let Ok(imports) = db.list_import_logs(RECENT_LIMIT) else {
        return internal_error();
    };
    let Ok(broken) = db.list_broken_links(RECENT_LIMIT) else {
        return internal_error();
    };
    let Ok(rules) = db.rule_monitor() else {
        return internal_error();
    };

    let feed_summary = summarize_feeds(&feed_rows);
    let feeds = feed_rows.into_iter().map(feed_view).collect();

    let system = SystemStatsView {
        total_items: sys_stats.total_items,
        total_feeds: sys_stats.total_feeds,
        total_tags: sys_stats.total_tags,
        total_collections: sys_stats.total_collections,
        total_highlights: sys_stats.total_highlights,
        total_notes: sys_stats.total_notes,
        total_review_cards: sys_stats.total_review_cards,
        db_size: fmt_bytes(sys_stats.db_size_bytes),
        fts_ok: sys_stats.fts_ok,
        content_types: sys_stats
            .content_types
            .into_iter()
            .map(|c| distribution_row(c.content_type, c.count, sys_stats.total_items))
            .collect(),
        statuses: sys_stats
            .statuses
            .into_iter()
            .map(|s| distribution_row(s.status, s.count, sys_stats.total_items))
            .collect(),
    };

    let extraction = ExtractionView {
        total: extraction_stats.total,
        succeeded: extraction_stats.succeeded,
        failed: extraction_stats.failed,
        success_rate: format!("{:.1}", extraction_stats.success_rate),
        failures: failures.into_iter().map(extraction_view).collect(),
        recent: recent.into_iter().map(extraction_view).collect(),
    };

    let imports = imports.into_iter().map(import_view).collect();

    let broken_links = broken
        .into_iter()
        .map(|b| BrokenLinkView {
            title: b.title,
            url: b.url.unwrap_or_default(),
            http_status: b
                .http_status
                .map_or_else(|| "—".to_owned(), |s| s.to_string()),
            error: b.error_message.unwrap_or_default(),
            last_checked: fmt_date(Some(b.last_checked_at)),
        })
        .collect();

    let rules = rules
        .into_iter()
        .map(|r| RuleView {
            name: r.name,
            enabled: r.enabled,
            priority: r.priority,
            filter_query: r.filter_query,
            match_count: r.match_count,
            actions: r.action_summary,
        })
        .collect();

    render(&AdminTemplate {
        auth_enabled,
        system,
        feed_summary,
        feeds,
        extraction,
        imports,
        broken_links,
        rules,
    })
}

/// `POST /admin/sync` — refresh all feeds, then return to the dashboard.
pub async fn sync_all(State(state): State<AppState>) -> Response {
    let feeds = {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        db.list_feeds().unwrap_or_default()
    };

    for feed in &feeds {
        if let Err(e) = refresh_single_feed(&state, feed).await {
            tracing::warn!(feed_id = %feed.id, error = %e, "admin feed sync error");
            if let Ok(db) = state.db.lock() {
                let _ = db.update_feed_fetch_error(feed.id, &e.to_string());
            }
        }
    }

    Redirect::to("/admin").into_response()
}

/// `POST /admin/sync/{id}` — refresh a single feed, then return to the dashboard.
pub async fn sync_one(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let feed = {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        match db.get_feed(id) {
            Ok(feed) => feed,
            Err(_) => return Redirect::to("/admin").into_response(),
        }
    };

    if let Err(e) = refresh_single_feed(&state, &feed).await {
        tracing::warn!(feed_id = %feed.id, error = %e, "admin feed sync error");
        if let Ok(db) = state.db.lock() {
            let _ = db.update_feed_fetch_error(feed.id, &e.to_string());
        }
    }

    Redirect::to("/admin").into_response()
}

// ======================================================================
// Helpers
// ======================================================================

/// Summarize feed health into aggregate counts.
fn summarize_feeds(rows: &[pergamon_core::diagnostics::FeedHealthRow]) -> FeedSummaryView {
    let mut summary = FeedSummaryView {
        total: rows.len(),
        healthy: 0,
        warning: 0,
        error: 0,
        stale: 0,
    };
    for row in rows {
        match row.status {
            FeedHealthStatus::Healthy => summary.healthy += 1,
            FeedHealthStatus::Warning => summary.warning += 1,
            FeedHealthStatus::Error => summary.error += 1,
        }
        if row.is_stale {
            summary.stale += 1;
        }
    }
    summary
}

/// Convert a feed-health row into its view model.
fn feed_view(row: pergamon_core::diagnostics::FeedHealthRow) -> FeedHealthView {
    let status_label = match row.status {
        FeedHealthStatus::Healthy => "Healthy",
        FeedHealthStatus::Warning => "Warning",
        FeedHealthStatus::Error => "Error",
    };
    FeedHealthView {
        feed_id: row.feed_id.to_string(),
        title: row.title,
        url: row.url,
        status_class: row.status.as_str().to_owned(),
        status_label: status_label.to_owned(),
        error_count: row.error_count,
        last_error: row.last_error.unwrap_or_default(),
        last_fetched: fmt_date(row.last_fetched_at),
        is_stale: row.is_stale,
    }
}

/// Convert an extraction event into its view model.
fn extraction_view(event: ExtractionEvent) -> ExtractionEventView {
    ExtractionEventView {
        source: event.source.label().to_owned(),
        extractor: event.extractor.unwrap_or_default(),
        success: event.success,
        url: event.url.unwrap_or_default(),
        error: event.error_message.unwrap_or_default(),
        created: fmt_date(Some(event.created_at)),
    }
}

/// Convert an import-log entry into its view model.
fn import_view(entry: ImportLogEntry) -> ImportLogView {
    ImportLogView {
        source: entry.source.label().to_owned(),
        file_name: entry.file_name.unwrap_or_default(),
        added: entry.items_added,
        existing: entry.items_existing,
        skipped: entry.items_skipped,
        errors: entry.errors,
        error_detail: entry.error_detail.unwrap_or_default(),
        created: fmt_date(Some(entry.created_at)),
    }
}

/// Build a distribution row with a percentage of the total.
fn distribution_row(label: String, count: i64, total: i64) -> DistributionRow {
    let percent = if total > 0 {
        #[allow(clippy::cast_precision_loss)]
        let pct = (count as f64 / total as f64) * 100.0;
        format!("{pct:.1}")
    } else {
        "0.0".to_owned()
    };
    DistributionRow {
        label,
        count,
        percent,
    }
}

/// Format a byte count as a human-readable size.
fn fmt_bytes(bytes: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mut size = bytes as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_scales() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1_572_864), "1.5 MB");
    }

    #[test]
    fn distribution_percentage() {
        let row = distribution_row("article".into(), 25, 100);
        assert_eq!(row.percent, "25.0");
        let zero = distribution_row("article".into(), 5, 0);
        assert_eq!(zero.percent, "0.0");
    }
}
