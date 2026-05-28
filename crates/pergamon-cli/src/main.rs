//! # pergamon CLI
//!
//! Command-line interface for pergamon — unified personal information
//! system. Combines RSS reader, read-later, bookmark manager, and
//! knowledge retention engine into a single CLI + ratatui TUI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use pergamon_core::content_type::ContentType;
use pergamon_core::model::{ContentItem, Feed, FeedItemMeta};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::Database;
use time::OffsetDateTime;
use uuid::Uuid;

/// Default database location.
fn default_db_path() -> PathBuf {
    // Use $PERGAMON_DATA_DIR if set, otherwise current directory.
    std::env::var_os("PERGAMON_DATA_DIR")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("pergamon.db")
}

/// pergamon — unified personal information system.
#[derive(Debug, Parser)]
#[command(name = "pergamon", version, about)]
struct Cli {
    /// Print version information.
    #[arg(long)]
    info: bool,

    /// Path to the database file.
    #[arg(long, env = "PERGAMON_DB", global = true)]
    db: Option<PathBuf>,

    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Manage feed subscriptions.
    Feed {
        #[command(subcommand)]
        action: FeedAction,
    },
    /// Refresh all feeds (alias for `feed refresh`).
    Sync,
}

/// Feed management subcommands.
#[derive(Debug, Subcommand)]
enum FeedAction {
    /// Subscribe to a new feed.
    Add {
        /// Feed URL (RSS/Atom/JSON Feed).
        url: String,
    },
    /// List all subscribed feeds.
    List,
    /// Refresh feeds to fetch new items.
    Refresh {
        /// Only refresh the feed with this ID.
        #[arg(long)]
        feed: Option<String>,
    },
    /// Remove a feed subscription.
    Remove {
        /// Feed ID to remove.
        id: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.info {
        println!("pergamon-core {}", pergamon_core::VERSION);
        return Ok(());
    }

    let Some(command) = cli.command else {
        println!("pergamon-core {}", pergamon_core::VERSION);
        println!("Run `pergamon --help` for usage.");
        return Ok(());
    };

    let db_path = cli.db.unwrap_or_else(default_db_path);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create data directory: {}", parent.display()))?;
    }
    let db = Database::open(&db_path)
        .with_context(|| format!("failed to open database: {}", db_path.display()))?;

    match command {
        Command::Feed { action } => handle_feed(&db, action),
        Command::Sync => refresh_feeds(&db, None),
    }
}

/// HTTP client with sensible defaults.
fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("pergamon/{}", pergamon_core::VERSION))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")
}

/// Dispatch feed subcommand.
fn handle_feed(db: &Database, action: FeedAction) -> Result<()> {
    match action {
        FeedAction::Add { url } => feed_add(db, &url),
        FeedAction::List => feed_list(db),
        FeedAction::Refresh { feed } => refresh_feeds(db, feed.as_deref()),
        FeedAction::Remove { id } => feed_remove(db, &id),
    }
}

/// Subscribe to a new feed.
fn feed_add(db: &Database, url: &str) -> Result<()> {
    // Check for duplicate subscription.
    if let Some(existing) = db
        .get_feed_by_url(url)
        .context("failed to check for existing feed")?
    {
        println!("Already subscribed: {} ({})", existing.title, existing.id);
        return Ok(());
    }

    // Fetch the feed.
    let client = http_client()?;
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to fetch {url}"))?;

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let final_url = response.url().to_string();

    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response from {url}"))?;

    // Parse the feed.
    let parsed = pergamon_feed::parse_feed(&bytes, &final_url)
        .with_context(|| format!("failed to parse feed from {url}"))?;

    let now = OffsetDateTime::now_utc();
    let feed = Feed {
        id: Uuid::new_v4(),
        title: parsed.title.clone(),
        url: final_url,
        site_url: parsed.site_url.clone(),
        description: parsed.description.clone(),
        etag,
        last_modified_header: last_modified,
        error_count: 0,
        last_error: None,
        last_fetched_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    db.insert_feed(&feed).context("failed to insert feed")?;

    // Ingest initial items.
    let count = ingest_entries(db, &feed, &parsed.entries)?;

    db.update_feed_fetch_success(
        feed.id,
        feed.etag.as_deref(),
        feed.last_modified_header.as_deref(),
    )
    .context("failed to update feed status")?;

    println!(
        "Subscribed: {} ({} items) [{}]",
        parsed.title, count, feed.id
    );
    Ok(())
}

/// List all subscribed feeds.
fn feed_list(db: &Database) -> Result<()> {
    let feeds = db.list_feeds().context("failed to list feeds")?;

    if feeds.is_empty() {
        println!("No feeds subscribed. Use `pergamon feed add <url>` to add one.");
        return Ok(());
    }

    for feed in &feeds {
        let status = if feed.error_count > 0 {
            format!(
                "ERR({}): {}",
                feed.error_count,
                feed.last_error.as_deref().unwrap_or("unknown")
            )
        } else {
            feed.last_fetched_at.map_or_else(
                || "never fetched".to_owned(),
                |t| format!("ok @ {}", fmt_relative(t)),
            )
        };
        println!("  {} {} [{}]", feed.title, status, feed.id);
    }

    println!("\n{} feed(s)", feeds.len());
    Ok(())
}

/// Refresh one or all feeds.
fn refresh_feeds(db: &Database, feed_id: Option<&str>) -> Result<()> {
    let feeds = if let Some(id_str) = feed_id {
        let id = Uuid::parse_str(id_str).context("invalid feed ID")?;
        vec![db.get_feed(id).context("feed not found")?]
    } else {
        db.list_feeds().context("failed to list feeds")?
    };

    if feeds.is_empty() {
        println!("No feeds to refresh.");
        return Ok(());
    }

    let client = http_client()?;
    let mut total_new = 0u64;
    let mut errors = 0u64;

    for feed in &feeds {
        match refresh_single_feed(db, &client, feed) {
            Ok(count) => {
                total_new += count;
                if count > 0 {
                    println!("  {} +{count} new", feed.title);
                }
            }
            Err(e) => {
                errors += 1;
                let msg = format!("{e:#}");
                eprintln!("  {} ERROR: {msg}", feed.title);
                let _ = db.update_feed_fetch_error(feed.id, &msg);
            }
        }
    }

    println!(
        "Refreshed {} feed(s): {total_new} new item(s), {errors} error(s)",
        feeds.len()
    );
    Ok(())
}

/// Refresh a single feed: conditional GET, parse, dedup, ingest.
fn refresh_single_feed(
    db: &Database,
    client: &reqwest::blocking::Client,
    feed: &Feed,
) -> Result<u64> {
    let mut req = client.get(&feed.url);

    // Conditional GET headers.
    if let Some(etag) = &feed.etag {
        req = req.header("If-None-Match", etag);
    }
    if let Some(lm) = &feed.last_modified_header {
        req = req.header("If-Modified-Since", lm);
    }

    let response = req
        .send()
        .with_context(|| format!("failed to fetch {}", feed.url))?;

    // 304 Not Modified — nothing new.
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        db.update_feed_fetch_success(
            feed.id,
            feed.etag.as_deref(),
            feed.last_modified_header.as_deref(),
        )?;
        return Ok(0);
    }

    if !response.status().is_success() {
        bail!("HTTP {} for {}", response.status(), feed.url);
    }

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let bytes = response.bytes()?;
    let parsed = pergamon_feed::parse_feed(&bytes, &feed.url)?;

    let count = ingest_entries(db, feed, &parsed.entries)?;

    db.update_feed_fetch_success(feed.id, etag.as_deref(), last_modified.as_deref())?;

    Ok(count)
}

/// Ingest parsed entries, skipping duplicates. Returns the number of new items.
fn ingest_entries(
    db: &Database,
    feed: &Feed,
    entries: &[pergamon_feed::ParsedEntry],
) -> Result<u64> {
    let mut count = 0u64;

    for entry in entries {
        // Dedup: prefer GUID, fall back to URL.
        let is_dup = if let Some(guid) = &entry.guid {
            db.feed_item_exists_by_guid(feed.id, guid)?
        } else if let Some(url) = &entry.url {
            db.feed_item_exists_by_url(feed.id, url)?
        } else {
            false
        };

        if is_dup {
            continue;
        }

        let now = OffsetDateTime::now_utc();
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: entry.url.clone(),
            title: entry.title.clone(),
            author: entry.author.clone(),
            content_type: ContentType::FeedItem,
            status: DocumentStatus::Inbox,
            content_text: entry.content.clone(),
            excerpt: entry.summary.clone(),
            published_at: entry.published_at,
            created_at: now,
            updated_at: now,
        };

        db.insert_content_item(&item)
            .context("failed to insert content item")?;

        let meta = FeedItemMeta {
            content_item_id: item.id,
            feed_id: feed.id,
            guid: entry.guid.clone(),
            summary: entry.summary.clone(),
        };

        db.insert_feed_item_meta(&meta)
            .context("failed to insert feed item meta")?;

        count += 1;
    }

    Ok(count)
}

/// Remove a feed subscription.
fn feed_remove(db: &Database, id_str: &str) -> Result<()> {
    let id = Uuid::parse_str(id_str).context("invalid feed ID")?;
    let feed = db.get_feed(id).context("feed not found")?;

    if db.delete_feed(id).context("failed to delete feed")? {
        println!("Removed: {} [{}]", feed.title, feed.id);
    } else {
        println!("Feed not found: {id_str}");
    }
    Ok(())
}

/// Format a timestamp as a human-friendly relative string.
fn fmt_relative(t: OffsetDateTime) -> String {
    let now = OffsetDateTime::now_utc();
    let delta = now - t;

    if delta.whole_seconds() < 60 {
        "just now".to_owned()
    } else if delta.whole_minutes() < 60 {
        format!("{}m ago", delta.whole_minutes())
    } else if delta.whole_hours() < 24 {
        format!("{}h ago", delta.whole_hours())
    } else {
        format!("{}d ago", delta.whole_days())
    }
}
