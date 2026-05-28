//! # pergamon CLI
//!
//! Command-line interface for pergamon — unified personal information
//! system. Combines RSS reader, read-later, bookmark manager, and
//! knowledge retention engine into a single CLI + ratatui TUI.

mod tui;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use pergamon_core::content_type::ContentType;
use pergamon_core::model::{Collection, ContentItem, Feed, FeedFolder, FeedItemMeta, Tag};
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
    /// Save a URL as an article (fetch, extract, store).
    Save {
        /// URL to save (reads from stdin if omitted).
        url: Option<String>,
        /// Add tags to the saved item (repeatable).
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
        /// Save as bookmark without article extraction.
        #[arg(long)]
        bookmark: bool,
    },
    /// Open the TUI inbox / reader.
    Read,
    /// Search across all content (title, author, content, tags).
    Search {
        /// Search query.
        query: String,
        /// Filter by content type (`feed_item`, `article`, `bookmark`, etc.).
        #[arg(long = "type")]
        content_type: Option<String>,
        /// Filter by tag name.
        #[arg(long)]
        tag: Option<String>,
        /// Filter by status (inbox, later, reading, reference, archived, discarded).
        #[arg(long)]
        status: Option<String>,
        /// Filter by feed (title substring or UUID).
        #[arg(long)]
        source: Option<String>,
        /// Only items created on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Only items created before this date (YYYY-MM-DD).
        #[arg(long)]
        before: Option<String>,
        /// Maximum number of results (default: 20).
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Output format: text or json.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Import data from external sources.
    Import {
        #[command(subcommand)]
        action: ImportAction,
    },
    /// Export data to standard formats.
    Export {
        #[command(subcommand)]
        action: ExportAction,
    },
    /// Manage collections (hierarchical folders for organising content).
    Collection {
        #[command(subcommand)]
        action: CollectionAction,
    },
    /// Manage tags across all content types.
    Tag {
        #[command(subcommand)]
        action: TagAction,
    },
    /// Bulk operations on content items.
    Bulk {
        #[command(subcommand)]
        action: BulkAction,
    },
    /// Show current configuration.
    Config,
    /// Generate shell completions.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
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
    List {
        /// Show feeds grouped by folder in a tree view.
        #[arg(long)]
        tree: bool,
    },
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
    /// Move a feed to a folder.
    Move {
        /// Feed ID to move.
        id: String,
        /// Target folder name (created if it doesn't exist).
        #[arg(long)]
        folder: String,
    },
}

/// Import subcommands.
#[derive(Debug, Subcommand)]
enum ImportAction {
    /// Import feed subscriptions from an OPML file.
    Opml {
        /// Path to the OPML file.
        file: PathBuf,
        /// Show what would be imported without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore from a full backup archive.
    Backup {
        /// Path to the backup ZIP file.
        file: PathBuf,
    },
}

/// Export subcommands.
#[derive(Debug, Subcommand)]
enum ExportAction {
    /// Export feed subscriptions as OPML.
    Opml {
        /// Output file path (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Create a full backup archive.
    Backup {
        /// Output file path.
        #[arg(long, short)]
        output: PathBuf,
    },
}

/// Collection management subcommands.
#[derive(Debug, Subcommand)]
enum CollectionAction {
    /// Create a new collection.
    Create {
        /// Name of the collection.
        name: String,
        /// Parent collection (name or UUID).
        #[arg(long)]
        parent: Option<String>,
    },
    /// List all collections.
    List {
        /// Show collections in a tree view.
        #[arg(long)]
        tree: bool,
    },
    /// Rename a collection.
    Rename {
        /// Collection to rename (name or UUID).
        collection: String,
        /// New name.
        #[arg(long)]
        to: String,
    },
    /// Move a collection under a new parent.
    Move {
        /// Collection to move (name or UUID).
        collection: String,
        /// Target parent collection (name or UUID). Use --root to move to top level.
        #[arg(long, conflicts_with = "root")]
        parent: Option<String>,
        /// Move to the top level (no parent).
        #[arg(long, conflicts_with = "parent")]
        root: bool,
    },
    /// Delete a collection.
    Delete {
        /// Collection to delete (name or UUID).
        collection: String,
    },
    /// Add items to a collection.
    Add {
        /// Collection (name or UUID).
        collection: String,
        /// Content item IDs to add.
        items: Vec<String>,
    },
    /// Remove items from a collection.
    Remove {
        /// Collection (name or UUID).
        collection: String,
        /// Content item IDs to remove.
        items: Vec<String>,
    },
    /// Show items in a collection.
    Show {
        /// Collection (name or UUID).
        collection: String,
    },
}

/// Tag management subcommands.
#[derive(Debug, Subcommand)]
enum TagAction {
    /// Add a tag to one or more items (creates the tag if it doesn't exist).
    Add {
        /// Tag name.
        tag: String,
        /// Content item IDs to tag.
        items: Vec<String>,
    },
    /// Remove a tag from one or more items.
    Remove {
        /// Tag name.
        tag: String,
        /// Content item IDs to untag.
        items: Vec<String>,
    },
    /// List all tags.
    List,
    /// Rename a tag.
    Rename {
        /// Current tag name.
        tag: String,
        /// New tag name.
        #[arg(long)]
        to: String,
    },
    /// Delete a tag entirely.
    Delete {
        /// Tag name.
        tag: String,
    },
    /// Show items with a specific tag.
    Show {
        /// Tag name.
        tag: String,
    },
}

/// Bulk operation subcommands.
#[derive(Debug, Subcommand)]
enum BulkAction {
    /// Tag all items matching a filter.
    Tag {
        /// Tag name to apply.
        tag: String,
        /// Filter by status (inbox, later, reading, reference, archived, discarded).
        #[arg(long)]
        status: Option<String>,
        /// Filter by content type.
        #[arg(long = "type")]
        content_type: Option<String>,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Move all matching items to a collection.
    Move {
        /// Target collection (name or UUID).
        collection: String,
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
        /// Filter by content type.
        #[arg(long = "type")]
        content_type: Option<String>,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Archive all matching items.
    Archive {
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
        /// Filter by content type.
        #[arg(long = "type")]
        content_type: Option<String>,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Discard all matching items (soft delete).
    Delete {
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
        /// Filter by content type.
        #[arg(long = "type")]
        content_type: Option<String>,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
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

    // Commands that do not need a database.
    match &command {
        Command::Config => return show_config(),
        Command::Completions { shell } => {
            generate_completions(*shell);
            return Ok(());
        }
        _ => {}
    }

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
        Command::Save {
            url,
            tags,
            bookmark,
        } => save_url(&db, url.as_deref(), &tags, bookmark),
        Command::Read => run_tui(&db),
        Command::Search {
            query,
            content_type,
            tag,
            status,
            source,
            since,
            before,
            limit,
            format,
        } => handle_search(
            &db,
            &query,
            content_type.as_deref(),
            tag.as_deref(),
            status.as_deref(),
            source.as_deref(),
            since.as_deref(),
            before.as_deref(),
            limit,
            &format,
        ),
        Command::Import { action } => handle_import(&db, action),
        Command::Export { action } => handle_export(&db, action),
        Command::Collection { action } => handle_collection(&db, action),
        Command::Tag { action } => handle_tag(&db, action),
        Command::Bulk { action } => handle_bulk(&db, action),
        // Already handled above — unreachable at runtime.
        Command::Config | Command::Completions { .. } => Ok(()),
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
        FeedAction::List { tree } => {
            if tree {
                feed_list_tree(db)
            } else {
                feed_list(db)
            }
        }
        FeedAction::Refresh { feed } => refresh_feeds(db, feed.as_deref()),
        FeedAction::Remove { id } => feed_remove(db, &id),
        FeedAction::Move { id, folder } => feed_move(db, &id, &folder),
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
        folder_id: None,
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

/// Move a feed to a folder (created if it doesn't exist).
fn feed_move(db: &Database, id_str: &str, folder_name: &str) -> Result<()> {
    let feed_id = Uuid::parse_str(id_str).context("invalid feed ID")?;
    let feed = db.get_feed(feed_id).context("feed not found")?;

    let folder = get_or_create_folder(db, folder_name, None)?;

    db.update_feed_folder_id(feed_id, Some(folder.id))
        .context("failed to move feed")?;

    println!("Moved: {} → {}", feed.title, folder.name);
    Ok(())
}

/// List feeds grouped by folder in a tree view.
fn feed_list_tree(db: &Database) -> Result<()> {
    let feeds = db.list_feeds().context("failed to list feeds")?;
    let folders = db
        .list_feed_folders()
        .context("failed to list feed folders")?;

    if feeds.is_empty() {
        println!("No feeds subscribed. Use `pergamon feed add <url>` to add one.");
        return Ok(());
    }

    // Group feeds by folder_id.
    let mut by_folder: std::collections::HashMap<Option<Uuid>, Vec<&Feed>> =
        std::collections::HashMap::new();
    for feed in &feeds {
        by_folder.entry(feed.folder_id).or_default().push(feed);
    }

    // Print folders with their feeds.
    for folder in &folders {
        if let Some(folder_feeds) = by_folder.get(&Some(folder.id)) {
            println!("📁 {}", folder.name);
            for feed in folder_feeds {
                let status = feed_status_label(feed);
                println!("  ├─ {} {status} [{}]", feed.title, feed.id);
            }
        }
    }

    // Print unfoldered feeds.
    if let Some(unfoldered) = by_folder.get(&None) {
        if !folders.is_empty() {
            println!("📄 (no folder)");
        }
        for feed in unfoldered {
            let status = feed_status_label(feed);
            if folders.is_empty() {
                println!("  {} {status} [{}]", feed.title, feed.id);
            } else {
                println!("  ├─ {} {status} [{}]", feed.title, feed.id);
            }
        }
    }

    println!("\n{} feed(s) in {} folder(s)", feeds.len(), folders.len());
    Ok(())
}

/// Format a feed's status for display.
fn feed_status_label(feed: &Feed) -> String {
    if feed.error_count > 0 {
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
    }
}

// ======================================================================
// Import / Export commands
// ======================================================================

/// Dispatch import subcommand.
fn handle_import(db: &Database, action: ImportAction) -> Result<()> {
    match action {
        ImportAction::Opml { file, dry_run } => import_opml(db, &file, dry_run),
        ImportAction::Backup { file } => restore_backup(db, &file),
    }
}

/// Dispatch export subcommand.
fn handle_export(db: &Database, action: ExportAction) -> Result<()> {
    match action {
        ExportAction::Opml { output } => export_opml(db, output.as_deref()),
        ExportAction::Backup { output } => export_backup(db, &output),
    }
}

/// Import feed subscriptions from an OPML file.
fn import_opml(db: &Database, path: &std::path::Path, dry_run: bool) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read OPML file: {}", path.display()))?;

    let doc = pergamon_feed::parse_opml(&bytes)
        .with_context(|| format!("failed to parse OPML file: {}", path.display()))?;

    let (total_feeds, total_folders) = pergamon_feed::count_outlines(&doc.outlines);

    if dry_run {
        println!("Dry run — no changes will be made.\n");
    }

    println!(
        "OPML: \"{}\" ({total_feeds} feeds, {total_folders} folders)",
        doc.title
    );

    let mut stats = ImportStats::default();
    import_outlines(db, &doc.outlines, None, dry_run, &mut stats)?;

    println!(
        "\n{}: {} created, {} existing",
        if dry_run {
            "Folders (would)"
        } else {
            "Folders"
        },
        stats.folders_created,
        stats.folders_existing,
    );
    println!(
        "{}: {} added, {} existing, {} moved",
        if dry_run { "Feeds (would)" } else { "Feeds" },
        stats.feeds_added,
        stats.feeds_existing,
        stats.feeds_moved,
    );

    Ok(())
}

/// Import statistics.
#[derive(Default)]
struct ImportStats {
    folders_created: u64,
    folders_existing: u64,
    feeds_added: u64,
    feeds_existing: u64,
    feeds_moved: u64,
}

/// Recursively import OPML outlines into the database.
fn import_outlines(
    db: &Database,
    outlines: &[pergamon_feed::OpmlOutline],
    parent_folder_id: Option<Uuid>,
    dry_run: bool,
    stats: &mut ImportStats,
) -> Result<()> {
    for outline in outlines {
        if outline.is_feed() {
            let xml_url = outline
                .xml_url
                .as_deref()
                .unwrap_or_else(|| unreachable!("is_feed but no xml_url"));
            let title = outline.display_name();

            // Check for existing subscription (idempotent).
            let existing = db
                .get_feed_by_url(xml_url)
                .context("failed to check existing feed")?;

            if let Some(existing_feed) = existing {
                // Feed already exists — check if folder changed.
                if existing_feed.folder_id == parent_folder_id {
                    println!("  ✓ {title} (already subscribed)");
                    stats.feeds_existing += 1;
                } else {
                    if !dry_run {
                        db.update_feed_folder_id(existing_feed.id, parent_folder_id)
                            .context("failed to move feed")?;
                    }
                    println!("  ↻ {title} (moved to folder)");
                    stats.feeds_moved += 1;
                }
            } else {
                // New subscription — create without fetching.
                if !dry_run {
                    let now = OffsetDateTime::now_utc();
                    let feed = Feed {
                        id: Uuid::new_v4(),
                        title: title.to_owned(),
                        url: xml_url.to_owned(),
                        site_url: outline.html_url.clone(),
                        description: None,
                        etag: None,
                        last_modified_header: None,
                        error_count: 0,
                        last_error: None,
                        last_fetched_at: None,
                        folder_id: parent_folder_id,
                        created_at: now,
                        updated_at: now,
                    };
                    db.insert_feed(&feed)
                        .with_context(|| format!("failed to insert feed: {xml_url}"))?;
                }
                println!("  + {title}");
                stats.feeds_added += 1;
            }
        } else {
            // Folder outline.
            let folder_name = outline.display_name();

            if folder_name.is_empty() {
                // Skip unnamed folders — treat children as belonging to parent.
                import_outlines(db, &outline.children, parent_folder_id, dry_run, stats)?;
                continue;
            }

            let folder_id = if dry_run {
                // In dry-run mode, check existence but don't create.
                let existing = db
                    .get_feed_folder_by_name(folder_name, parent_folder_id)
                    .context("failed to check existing folder")?;
                if let Some(f) = existing {
                    println!("  📁 {folder_name} (exists)");
                    stats.folders_existing += 1;
                    Some(f.id)
                } else {
                    println!("  📁 {folder_name} (would create)");
                    stats.folders_created += 1;
                    // Use a temporary ID so children can reference it in output.
                    Some(Uuid::new_v4())
                }
            } else {
                let folder = get_or_create_folder(db, folder_name, parent_folder_id)?;
                let is_new = folder.created_at == folder.updated_at;
                if is_new {
                    println!("  📁 {folder_name} (created)");
                    stats.folders_created += 1;
                } else {
                    println!("  📁 {folder_name} (exists)");
                    stats.folders_existing += 1;
                }
                Some(folder.id)
            };

            import_outlines(db, &outline.children, folder_id, dry_run, stats)?;
        }
    }
    Ok(())
}

/// Get an existing folder by name or create a new one.
fn get_or_create_folder(db: &Database, name: &str, parent_id: Option<Uuid>) -> Result<FeedFolder> {
    if let Some(existing) = db
        .get_feed_folder_by_name(name, parent_id)
        .context("failed to check existing folder")?
    {
        return Ok(existing);
    }

    let now = OffsetDateTime::now_utc();
    let folder = FeedFolder {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id,
        created_at: now,
        updated_at: now,
    };
    db.insert_feed_folder(&folder)
        .context("failed to create folder")?;
    Ok(folder)
}

/// Export all feed subscriptions as OPML.
fn export_opml(db: &Database, output: Option<&std::path::Path>) -> Result<()> {
    let feeds = db.list_feeds().context("failed to list feeds")?;
    let folders = db
        .list_feed_folders()
        .context("failed to list feed folders")?;

    // Build a tree of outlines from feeds and folders.
    let outlines = build_opml_tree(&feeds, &folders);

    let xml = pergamon_feed::generate_opml("pergamon subscriptions", &outlines)
        .context("failed to generate OPML")?;

    if let Some(path) = output {
        std::fs::write(path, &xml)
            .with_context(|| format!("failed to write OPML to {}", path.display()))?;
        println!("Exported {} feed(s) to {}", feeds.len(), path.display());
    } else {
        println!("{xml}");
    }

    Ok(())
}

/// Build an OPML outline tree from database feeds and folders.
fn build_opml_tree(feeds: &[Feed], folders: &[FeedFolder]) -> Vec<pergamon_feed::OpmlOutline> {
    // Group feeds by folder_id.
    let mut by_folder: std::collections::HashMap<Option<Uuid>, Vec<&Feed>> =
        std::collections::HashMap::new();
    for feed in feeds {
        by_folder.entry(feed.folder_id).or_default().push(feed);
    }

    let mut outlines = Vec::new();

    // Root-level folders (parent_id = None).
    for folder in folders {
        if folder.parent_id.is_none() {
            outlines.push(build_folder_outline(folder, &by_folder, folders));
        }
    }

    // Root-level feeds (no folder).
    if let Some(root_feeds) = by_folder.get(&None) {
        for feed in root_feeds {
            outlines.push(feed_to_outline(feed));
        }
    }

    outlines
}

/// Build a folder outline recursively.
fn build_folder_outline(
    folder: &FeedFolder,
    by_folder: &std::collections::HashMap<Option<Uuid>, Vec<&Feed>>,
    all_folders: &[FeedFolder],
) -> pergamon_feed::OpmlOutline {
    let mut children = Vec::new();

    // Add child folders.
    for child_folder in all_folders {
        if child_folder.parent_id == Some(folder.id) {
            children.push(build_folder_outline(child_folder, by_folder, all_folders));
        }
    }

    // Add feeds in this folder.
    if let Some(folder_feeds) = by_folder.get(&Some(folder.id)) {
        for feed in folder_feeds {
            children.push(feed_to_outline(feed));
        }
    }

    pergamon_feed::OpmlOutline {
        text: folder.name.clone(),
        title: Some(folder.name.clone()),
        xml_url: None,
        html_url: None,
        feed_type: None,
        children,
    }
}

/// Convert a feed to an OPML outline.
fn feed_to_outline(feed: &Feed) -> pergamon_feed::OpmlOutline {
    pergamon_feed::OpmlOutline {
        text: feed.title.clone(),
        title: Some(feed.title.clone()),
        xml_url: Some(feed.url.clone()),
        html_url: feed.site_url.clone(),
        feed_type: Some("rss".to_owned()),
        children: Vec::new(),
    }
}

// ======================================================================
// Save command
// ======================================================================

/// Resolve the URL to save from a CLI argument or stdin pipe.
fn resolve_url_input(raw_url: Option<&str>) -> Result<String> {
    use std::io::IsTerminal;

    if let Some(u) = raw_url {
        return Ok(u.to_owned());
    }

    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        bail!("No URL provided. Usage: pergamon save <url>");
    }
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("failed to read URL from stdin")?;
    let trimmed = line.trim().to_owned();
    if trimmed.is_empty() {
        bail!("No URL provided on stdin");
    }
    Ok(trimmed)
}

/// Extract content from fetched bytes, returning structured fields.
fn extract_content(
    bytes: &[u8],
    final_url: &str,
    canonical_url: &str,
    bookmark: bool,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<OffsetDateTime>,
    ContentType,
) {
    if bookmark {
        let html = String::from_utf8_lossy(bytes);
        let meta = pergamon_extract::extract_metadata(&html);
        (
            meta.title.unwrap_or_else(|| canonical_url.to_owned()),
            meta.author,
            None,
            meta.description,
            None,
            ContentType::Bookmark,
        )
    } else if let Ok(article) = pergamon_extract::extract_article(bytes, final_url) {
        (
            article.title.unwrap_or_else(|| canonical_url.to_owned()),
            article.author,
            Some(article.content_text),
            article.excerpt,
            article.published_at,
            ContentType::Article,
        )
    } else {
        let html = String::from_utf8_lossy(bytes);
        let meta = pergamon_extract::extract_metadata(&html);
        (
            meta.title.unwrap_or_else(|| canonical_url.to_owned()),
            meta.author,
            None,
            meta.description,
            None,
            ContentType::Bookmark,
        )
    }
}

/// Save a URL as an article: fetch → extract → store.
///
/// Deduplicates against the canonical form of the final (post-redirect)
/// URL. When a duplicate is found, still applies any requested tags.
fn save_url(db: &Database, raw_url: Option<&str>, tags: &[String], bookmark: bool) -> Result<()> {
    let url = resolve_url_input(raw_url)?;

    // Fetch the page (follows redirects).
    let client = http_client()?;
    let response = client
        .get(&url)
        .send()
        .with_context(|| format!("failed to fetch {url}"))?;

    let final_url = response.url().to_string();

    if !response.status().is_success() {
        bail!("HTTP {} for {url}", response.status());
    }

    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response from {url}"))?;

    // Canonicalize the final URL for dedup.
    let canonical_url =
        pergamon_extract::canonicalize_url(&final_url).unwrap_or_else(|_| final_url.clone());

    // Check for duplicate.
    if let Some(existing) = db
        .get_content_item_by_url(&canonical_url)
        .context("failed to check for duplicate")?
    {
        let applied = apply_tags(db, existing.id, tags)?;
        if applied.is_empty() {
            println!("Already saved: {} [{}]", existing.title, existing.id);
        } else {
            println!(
                "Already saved: {} [{}] — tags added: {}",
                existing.title,
                existing.id,
                applied.join(", ")
            );
        }
        return Ok(());
    }

    let now = OffsetDateTime::now_utc();
    let (title, author, content_text, excerpt, published_at, content_type) =
        extract_content(&bytes, &final_url, &canonical_url, bookmark);

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some(canonical_url),
        title,
        author,
        content_type,
        status: DocumentStatus::Inbox,
        content_text,
        excerpt,
        published_at,
        created_at: now,
        updated_at: now,
    };

    db.insert_content_item(&item)
        .context("failed to save item")?;

    let applied = apply_tags(db, item.id, tags)?;

    let type_label = item.content_type.as_str();
    if applied.is_empty() {
        println!("Saved {type_label}: {} [{}]", item.title, item.id);
    } else {
        println!(
            "Saved {type_label}: {} [{}] — tags: {}",
            item.title,
            item.id,
            applied.join(", ")
        );
    }
    Ok(())
}

/// Apply tags to a content item, returning the list of tag names applied.
fn apply_tags(db: &Database, item_id: Uuid, tag_names: &[String]) -> Result<Vec<String>> {
    let mut applied = Vec::new();
    for name in tag_names {
        let tag = db
            .get_or_create_tag(name)
            .with_context(|| format!("failed to get or create tag '{name}'"))?;
        db.tag_content_item(item_id, tag.id)
            .with_context(|| format!("failed to apply tag '{name}'"))?;
        applied.push(tag.name);
    }
    Ok(applied)
}

// ======================================================================
// Search command
// ======================================================================

/// Handle the `search` command: full-text search with faceted filters.
#[allow(clippy::too_many_arguments)]
fn handle_search(
    db: &Database,
    query: &str,
    content_type: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    source: Option<&str>,
    since: Option<&str>,
    before: Option<&str>,
    limit: u32,
    format: &str,
) -> Result<()> {
    let filter = build_search_filter(db, content_type, tag, status, source, since, before)?;

    let hits = db
        .search_filtered(query, &filter, Some(limit))
        .context("search failed")?;

    if hits.is_empty() {
        println!("No results for \"{query}\"");
        return Ok(());
    }

    if format == "json" {
        print_search_json(&hits)?;
    } else {
        print_search_text(query, &hits);
    }

    Ok(())
}

/// Build a [`SearchFilter`] from CLI flag values.
fn build_search_filter(
    db: &Database,
    content_type: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    source: Option<&str>,
    since: Option<&str>,
    before: Option<&str>,
) -> Result<pergamon_storage::SearchFilter> {
    use pergamon_storage::SearchFilter;

    let ct = content_type
        .map(|s| {
            s.parse::<ContentType>()
                .map_err(|_| anyhow::anyhow!("invalid content type: {s}"))
        })
        .transpose()?;

    let st = status
        .map(|s| {
            s.parse::<DocumentStatus>()
                .map_err(|_| anyhow::anyhow!("invalid status: {s}"))
        })
        .transpose()?;

    let feed_id = source.map(|s| resolve_source(db, s)).transpose()?;

    let since_dt = since.map(parse_date_arg).transpose()?;
    let before_dt = before.map(parse_date_arg).transpose()?;

    Ok(SearchFilter {
        content_type: ct,
        status: st,
        tag_name: tag.map(String::from),
        feed_id,
        since: since_dt,
        before: before_dt,
    })
}

/// Resolve a `--source` value to a feed UUID.
///
/// Accepts a UUID directly, or a feed title substring (case-insensitive).
/// If multiple feeds match the title, lists them and returns an error.
fn resolve_source(db: &Database, source: &str) -> Result<Uuid> {
    // Try UUID first.
    if let Ok(id) = Uuid::parse_str(source) {
        return Ok(id);
    }

    // Search by title substring.
    let feeds = db.list_feeds().context("failed to list feeds")?;
    let lower = source.to_lowercase();
    let matches: Vec<_> = feeds
        .iter()
        .filter(|f| f.title.to_lowercase().contains(&lower))
        .collect();

    match matches.len() {
        0 => bail!("no feed matching \"{source}\""),
        1 => Ok(matches[0].id),
        _ => {
            eprintln!("Multiple feeds match \"{source}\":");
            for feed in &matches {
                eprintln!("  {} [{}]", feed.title, feed.id);
            }
            bail!("use a more specific name or pass the feed UUID")
        }
    }
}

/// Parse a YYYY-MM-DD date argument into an `OffsetDateTime` at midnight UTC.
fn parse_date_arg(s: &str) -> Result<OffsetDateTime> {
    let format =
        time::format_description::parse("[year]-[month]-[day]").context("invalid date format")?;
    let date = time::Date::parse(s, &format)
        .with_context(|| format!("invalid date: {s} (expected YYYY-MM-DD)"))?;
    Ok(date
        .with_hms(0, 0, 0)
        .unwrap_or_else(|_| unreachable!("midnight is valid"))
        .assume_utc())
}

/// Print search results as JSON.
fn print_search_json(hits: &[pergamon_core::model::SearchHit]) -> Result<()> {
    #[derive(serde::Serialize)]
    struct JsonResult {
        id: String,
        title: String,
        url: Option<String>,
        author: Option<String>,
        content_type: String,
        status: String,
        rank: f64,
        snippet: Option<String>,
    }

    let results: Vec<JsonResult> = hits
        .iter()
        .map(|hit| JsonResult {
            id: hit.item.id.to_string(),
            title: hit.item.title.clone(),
            url: hit.item.url.clone(),
            author: hit.item.author.clone(),
            content_type: hit.item.content_type.as_str().to_owned(),
            status: hit.item.status.as_str().to_owned(),
            rank: hit.rank,
            snippet: hit.snippet.clone(),
        })
        .collect();

    let json =
        serde_json::to_string_pretty(&results).context("failed to serialize search results")?;
    println!("{json}");
    Ok(())
}

/// Print search results as formatted text.
fn print_search_text(query: &str, hits: &[pergamon_core::model::SearchHit]) {
    println!("Search: \"{}\" ({} results)\n", query, hits.len());

    for (i, hit) in hits.iter().enumerate() {
        let item = &hit.item;
        let type_label = item.content_type.as_str();
        let status_label = item.status.as_str();

        println!(
            "  {:<3} {} [{}] ({}/{})",
            i + 1,
            item.title,
            item.id,
            type_label,
            status_label,
        );

        if let Some(ref url) = item.url {
            println!("      {url}");
        }

        if let Some(ref snippet) = hit.snippet {
            let clean = snippet.replace('\n', " ");
            println!("      {clean}");
        }

        println!();
    }
}

// ======================================================================
// Collection commands
// ======================================================================

/// Resolve a collection reference (name or UUID) to a `Collection`.
fn resolve_collection(db: &Database, reference: &str) -> Result<Collection> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return db
            .get_collection(id)
            .with_context(|| format!("collection not found: {reference}"));
    }
    db.get_collection_by_name(reference)
        .context("failed to look up collection")?
        .ok_or_else(|| anyhow::anyhow!("collection not found: {reference}"))
}

/// Dispatch collection subcommand.
fn handle_collection(db: &Database, action: CollectionAction) -> Result<()> {
    match action {
        CollectionAction::Create { name, parent } => {
            collection_create(db, &name, parent.as_deref())
        }
        CollectionAction::List { tree } => {
            if tree {
                collection_list_tree(db)
            } else {
                collection_list(db)
            }
        }
        CollectionAction::Rename { collection, to } => {
            let coll = resolve_collection(db, &collection)?;
            db.rename_collection(coll.id, &to)
                .context("failed to rename collection")?;
            println!("Renamed collection '{}' → '{to}'", coll.name);
            Ok(())
        }
        CollectionAction::Move {
            collection,
            parent,
            root,
        } => {
            let coll = resolve_collection(db, &collection)?;
            let new_parent = if root {
                None
            } else if let Some(ref p) = parent {
                Some(resolve_collection(db, p)?.id)
            } else {
                bail!("specify --parent <collection> or --root");
            };
            db.move_collection(coll.id, new_parent)
                .context("failed to move collection")?;
            if let Some(pid) = new_parent {
                let parent_coll = db.get_collection(pid)?;
                println!("Moved '{}' under '{}'", coll.name, parent_coll.name);
            } else {
                println!("Moved '{}' to top level", coll.name);
            }
            Ok(())
        }
        CollectionAction::Delete { collection } => {
            let coll = resolve_collection(db, &collection)?;
            db.delete_collection(coll.id)
                .context("failed to delete collection")?;
            println!("Deleted collection '{}'", coll.name);
            Ok(())
        }
        CollectionAction::Add { collection, items } => {
            let coll = resolve_collection(db, &collection)?;
            let mut count = 0u64;
            for item_ref in &items {
                let item_id = Uuid::parse_str(item_ref).context("invalid item ID")?;
                db.add_to_collection(item_id, coll.id, 0)
                    .with_context(|| format!("failed to add {item_id} to collection"))?;
                count += 1;
            }
            println!("Added {count} item(s) to '{}'", coll.name);
            Ok(())
        }
        CollectionAction::Remove { collection, items } => {
            let coll = resolve_collection(db, &collection)?;
            let mut count = 0u64;
            for item_ref in &items {
                let item_id = Uuid::parse_str(item_ref).context("invalid item ID")?;
                if db
                    .remove_from_collection(item_id, coll.id)
                    .with_context(|| format!("failed to remove {item_id} from collection"))?
                {
                    count += 1;
                }
            }
            println!("Removed {count} item(s) from '{}'", coll.name);
            Ok(())
        }
        CollectionAction::Show { collection } => collection_show(db, &collection),
    }
}

/// Create a new collection.
fn collection_create(db: &Database, name: &str, parent_ref: Option<&str>) -> Result<()> {
    let parent_id = parent_ref
        .map(|p| resolve_collection(db, p).map(|c| c.id))
        .transpose()?;
    let now = OffsetDateTime::now_utc();
    let coll = Collection {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id,
        sort_order: 0,
        created_at: now,
        updated_at: now,
    };
    db.insert_collection(&coll)
        .context("failed to create collection")?;
    println!("Created collection '{}' ({})", coll.name, coll.id);
    Ok(())
}

/// List all collections (flat).
fn collection_list(db: &Database) -> Result<()> {
    let colls = db
        .list_collections()
        .context("failed to list collections")?;
    if colls.is_empty() {
        println!("No collections.");
        return Ok(());
    }
    // Count items per collection.
    let all_memberships = db
        .list_all_collection_items()
        .context("failed to load collection memberships")?;
    let mut item_counts: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
    for &(_, coll_id, _) in &all_memberships {
        *item_counts.entry(coll_id).or_default() += 1;
    }

    for coll in &colls {
        let count = item_counts.get(&coll.id).copied().unwrap_or(0);
        let parent_label = coll
            .parent_id
            .and_then(|pid| colls.iter().find(|c| c.id == pid))
            .map_or(String::new(), |p| format!(" (in {})", p.name));
        println!(
            "  {} [{}, {} item(s)]{parent_label}",
            coll.name, coll.id, count,
        );
    }
    Ok(())
}

/// List collections in a tree view.
fn collection_list_tree(db: &Database) -> Result<()> {
    let colls = db
        .list_collections()
        .context("failed to list collections")?;
    if colls.is_empty() {
        println!("No collections.");
        return Ok(());
    }

    // Build the tree.
    let roots: Vec<&Collection> = colls.iter().filter(|c| c.parent_id.is_none()).collect();
    for root in &roots {
        print_collection_tree(root, &colls, 0);
    }
    Ok(())
}

/// Recursively print a collection tree.
fn print_collection_tree(coll: &Collection, all: &[Collection], depth: usize) {
    let indent = "  ".repeat(depth);
    println!("{indent}{} [{}]", coll.name, coll.id);
    let children: Vec<&Collection> = all
        .iter()
        .filter(|c| c.parent_id == Some(coll.id))
        .collect();
    for child in &children {
        print_collection_tree(child, all, depth + 1);
    }
}

/// Show items in a collection.
fn collection_show(db: &Database, reference: &str) -> Result<()> {
    let coll = resolve_collection(db, reference)?;
    let items = db
        .list_collection_items(coll.id)
        .context("failed to list collection items")?;

    println!("Collection: {} ({} item(s))", coll.name, items.len());
    println!();
    for item in &items {
        let type_label = item.content_type.as_str();
        let status_label = item.status.as_str();
        println!(
            "  {} [{}, {}/{}]",
            item.title, item.id, type_label, status_label,
        );
        if let Some(ref url) = item.url {
            println!("    {url}");
        }
    }
    Ok(())
}

// ======================================================================
// Tag commands
// ======================================================================

/// Resolve a tag by name, returning an error if not found.
fn resolve_tag(db: &Database, name: &str) -> Result<Tag> {
    db.get_tag_by_name(name)
        .context("failed to look up tag")?
        .ok_or_else(|| anyhow::anyhow!("tag not found: {name}"))
}

/// Dispatch tag subcommand.
fn handle_tag(db: &Database, action: TagAction) -> Result<()> {
    match action {
        TagAction::Add { tag, items } => {
            let t = db
                .get_or_create_tag(&tag)
                .with_context(|| format!("failed to get or create tag '{tag}'"))?;
            let mut count = 0u64;
            for item_ref in &items {
                let item_id = Uuid::parse_str(item_ref).context("invalid item ID")?;
                db.tag_content_item(item_id, t.id)
                    .with_context(|| format!("failed to tag item {item_id}"))?;
                count += 1;
            }
            println!("Tagged {count} item(s) with '{}'", t.name);
            Ok(())
        }
        TagAction::Remove { tag, items } => {
            let t = resolve_tag(db, &tag)?;
            let mut count = 0u64;
            for item_ref in &items {
                let item_id = Uuid::parse_str(item_ref).context("invalid item ID")?;
                if db
                    .untag_content_item(item_id, t.id)
                    .with_context(|| format!("failed to untag item {item_id}"))?
                {
                    count += 1;
                }
            }
            println!("Removed tag '{}' from {count} item(s)", t.name);
            Ok(())
        }
        TagAction::List => {
            let tags = db.list_tags().context("failed to list tags")?;
            if tags.is_empty() {
                println!("No tags.");
                return Ok(());
            }
            for t in &tags {
                let items = db.list_items_by_tag(t.id).unwrap_or_default();
                println!("  {} [{}, {} item(s)]", t.name, t.id, items.len());
            }
            Ok(())
        }
        TagAction::Rename { tag, to } => {
            let t = resolve_tag(db, &tag)?;
            db.rename_tag(t.id, &to).context("failed to rename tag")?;
            println!("Renamed tag '{}' → '{to}'", t.name);
            Ok(())
        }
        TagAction::Delete { tag } => {
            let t = resolve_tag(db, &tag)?;
            db.delete_tag(t.id).context("failed to delete tag")?;
            println!("Deleted tag '{}'", t.name);
            Ok(())
        }
        TagAction::Show { tag } => {
            let t = resolve_tag(db, &tag)?;
            let items = db
                .list_items_by_tag(t.id)
                .context("failed to list items by tag")?;
            println!("Tag: {} ({} item(s))", t.name, items.len());
            println!();
            for item in &items {
                let type_label = item.content_type.as_str();
                let status_label = item.status.as_str();
                println!(
                    "  {} [{}, {}/{}]",
                    item.title, item.id, type_label, status_label,
                );
                if let Some(ref url) = item.url {
                    println!("    {url}");
                }
            }
            Ok(())
        }
    }
}

// ======================================================================
// Bulk commands
// ======================================================================

/// Build a `ContentItemFilter` from CLI arguments.
fn build_bulk_filter(
    status: Option<&str>,
    content_type: Option<&str>,
) -> Result<pergamon_storage::ContentItemFilter> {
    let mut filter = pergamon_storage::ContentItemFilter::default();
    if let Some(s) = status {
        filter.status = Some(
            s.parse::<DocumentStatus>()
                .map_err(|_| anyhow::anyhow!("unknown status: {s}"))?,
        );
    }
    if let Some(ct) = content_type {
        filter.content_type = Some(
            ct.parse::<ContentType>()
                .map_err(|_| anyhow::anyhow!("unknown content type: {ct}"))?,
        );
    }
    Ok(filter)
}

/// Ask for confirmation on a bulk operation.
fn confirm_bulk(action: &str, count: u64, yes: bool) -> Result<bool> {
    if yes || count == 0 {
        return Ok(true);
    }
    print!("{action} {count} item(s)? [y/N] ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

/// Dispatch bulk subcommand.
fn handle_bulk(db: &Database, action: BulkAction) -> Result<()> {
    match action {
        BulkAction::Tag {
            tag,
            status,
            content_type,
            yes,
        } => bulk_tag(db, &tag, status.as_deref(), content_type.as_deref(), yes),
        BulkAction::Move {
            collection,
            status,
            content_type,
            yes,
        } => bulk_move(
            db,
            &collection,
            status.as_deref(),
            content_type.as_deref(),
            yes,
        ),
        BulkAction::Archive {
            status,
            content_type,
            yes,
        } => bulk_archive(db, status.as_deref(), content_type.as_deref(), yes),
        BulkAction::Delete {
            status,
            content_type,
            yes,
        } => bulk_delete(db, status.as_deref(), content_type.as_deref(), yes),
    }
}

/// Bulk tag items matching a filter.
fn bulk_tag(
    db: &Database,
    tag: &str,
    status: Option<&str>,
    content_type: Option<&str>,
    yes: bool,
) -> Result<()> {
    let filter = build_bulk_filter(status, content_type)?;
    let items = db
        .list_content_items_filtered(&filter, None, None)
        .context("failed to list matching items")?;
    if items.is_empty() {
        println!("No items match the filter.");
        return Ok(());
    }
    if !confirm_bulk(&format!("Tag with '{tag}'"), items.len() as u64, yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    let t = db
        .get_or_create_tag(tag)
        .with_context(|| format!("failed to get or create tag '{tag}'"))?;
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let count = db.bulk_tag(&ids, t.id).context("failed to bulk tag")?;
    println!("Tagged {count} item(s) with '{}'", t.name);
    Ok(())
}

/// Bulk move items to a collection.
fn bulk_move(
    db: &Database,
    collection: &str,
    status: Option<&str>,
    content_type: Option<&str>,
    yes: bool,
) -> Result<()> {
    let filter = build_bulk_filter(status, content_type)?;
    let items = db
        .list_content_items_filtered(&filter, None, None)
        .context("failed to list matching items")?;
    if items.is_empty() {
        println!("No items match the filter.");
        return Ok(());
    }
    let coll = resolve_collection(db, collection)?;
    if !confirm_bulk(&format!("Move to '{}'", coll.name), items.len() as u64, yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let count = db
        .bulk_add_to_collection(&ids, coll.id)
        .context("failed to bulk move")?;
    println!("Added {count} item(s) to '{}'", coll.name);
    Ok(())
}

/// Bulk archive items matching a filter.
fn bulk_archive(
    db: &Database,
    status: Option<&str>,
    content_type: Option<&str>,
    yes: bool,
) -> Result<()> {
    let filter = build_bulk_filter(status, content_type)?;
    let items = db
        .list_content_items_filtered(&filter, None, None)
        .context("failed to list matching items")?;
    if items.is_empty() {
        println!("No items match the filter.");
        return Ok(());
    }
    if !confirm_bulk("Archive", items.len() as u64, yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let count = db.bulk_archive(&ids).context("failed to bulk archive")?;
    println!("Archived {count} item(s).");
    Ok(())
}

/// Bulk discard items matching a filter (soft delete).
fn bulk_delete(
    db: &Database,
    status: Option<&str>,
    content_type: Option<&str>,
    yes: bool,
) -> Result<()> {
    let filter = build_bulk_filter(status, content_type)?;
    let items = db
        .list_content_items_filtered(&filter, None, None)
        .context("failed to list matching items")?;
    if items.is_empty() {
        println!("No items match the filter.");
        return Ok(());
    }
    if !confirm_bulk("Discard", items.len() as u64, yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let count = db.bulk_discard(&ids).context("failed to bulk discard")?;
    println!("Discarded {count} item(s).");
    Ok(())
}

// ======================================================================
// TUI command
// ======================================================================

/// Terminal guard that restores the terminal state on drop.
///
/// Ensures raw mode and the alternate screen are cleaned up even
/// if the application panics.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

/// Launch the TUI inbox and article reader.
fn run_tui(db: &Database) -> Result<()> {
    use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
    use pergamon_storage::ContentItemFilter;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    const ITEM_LIMIT: u32 = 1000;

    // Set up terminal.
    enable_raw_mode().context("failed to enable raw mode")?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)
        .context("failed to enter alternate screen")?;

    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;

    // Initial load: inbox items.
    let initial_filter = ContentItemFilter {
        status: Some(DocumentStatus::Inbox),
        ..ContentItemFilter::default()
    };
    let items = db
        .list_content_items_filtered(&initial_filter, Some(ITEM_LIMIT), None)
        .context("failed to load content items")?;

    let mut app = tui::app::App::new(items);

    // Load metadata for the status bar and pickers.
    reload_metadata(&mut app, db)?;

    // Main loop.
    while !app.should_quit {
        terminal
            .draw(|frame| tui::ui::render(&app, frame))
            .context("failed to draw TUI")?;

        match tui::event::handle_events(&mut app, db)? {
            tui::event::Action::Reload => {
                if let tui::app::FilterMode::Search(ref query) = app.filter {
                    let search_filter = pergamon_storage::SearchFilter::default();
                    let hits = db
                        .search_filtered(query, &search_filter, Some(ITEM_LIMIT))
                        .context("failed to run search")?;
                    app.items = hits.into_iter().map(|h| h.item).collect();
                    app.set_status(format!("Search: {} result(s)", app.items.len()));
                } else {
                    let filter = tui::event::build_filter(&app.filter);
                    app.items = db
                        .list_content_items_filtered(&filter, Some(ITEM_LIMIT), None)
                        .context("failed to reload content items")?;
                }
                app.clamp_selection();
                reload_metadata(&mut app, db)?;
            }
            tui::event::Action::None => {}
        }
    }

    Ok(())
}

/// Reload counts and picker data from the database.
fn reload_metadata(app: &mut tui::app::App, db: &Database) -> Result<()> {
    use pergamon_storage::ContentItemFilter;

    // Unread count (always inbox).
    let inbox_filter = ContentItemFilter {
        status: Some(DocumentStatus::Inbox),
        ..ContentItemFilter::default()
    };
    app.unread_count = db
        .count_content_items_filtered(&inbox_filter)
        .context("failed to count unread items")?;

    // Total items matching current filter.
    if let tui::app::FilterMode::Search(ref query) = app.filter {
        let search_filter = pergamon_storage::SearchFilter::default();
        let hits = db
            .search_filtered(query, &search_filter, None)
            .context("failed to count search results")?;
        app.total_count = hits.len() as u64;
    } else {
        let current_filter = tui::event::build_filter(&app.filter);
        app.total_count = db
            .count_content_items_filtered(&current_filter)
            .context("failed to count items")?;
    }

    // Feeds and folders for the picker.
    app.feeds = db.list_feeds().context("failed to load feeds")?;
    app.folders = db.list_feed_folders().context("failed to load folders")?;

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

// ------------------------------------------------------------------
// Backup export
// ------------------------------------------------------------------

/// Manifest embedded in every backup archive.
#[derive(serde::Serialize, serde::Deserialize)]
struct BackupManifest {
    /// Application name (always "pergamon").
    app: String,
    /// Schema version at the time of the backup.
    schema_version: i64,
    /// ISO-8601 timestamp of when the backup was created.
    created_at: String,
}

/// Create a full backup archive (ZIP with JSON files).
fn export_backup(db: &Database, output: &std::path::Path) -> Result<()> {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(output)
        .with_context(|| format!("failed to create backup file: {}", output.display()))?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Gather all data.
    let feed_folders = db.list_feed_folders().context("listing feed folders")?;
    let feeds = db.list_feeds().context("listing feeds")?;
    let content_items = db
        .list_all_content_items()
        .context("listing content items")?;
    let tags = db.list_tags().context("listing tags")?;
    let collections = db.list_collections().context("listing collections")?;
    let feed_item_meta = db
        .list_all_feed_item_meta()
        .context("listing feed item meta")?;
    let bookmark_meta = db
        .list_all_bookmark_meta()
        .context("listing bookmark meta")?;
    let highlight_meta = db
        .list_all_highlight_meta()
        .context("listing highlight meta")?;
    let content_item_tags = db
        .list_all_content_item_tags()
        .context("listing content item tags")?;
    let collection_items = db
        .list_all_collection_items()
        .context("listing collection items")?;

    let manifest = BackupManifest {
        app: "pergamon".to_owned(),
        schema_version: db.schema_version().context("reading schema version")?,
        created_at: OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };

    // Write files in deterministic order.
    write_json_entry(&mut zip, &opts, "manifest.json", &manifest)?;
    write_json_entry(&mut zip, &opts, "feed_folders.json", &feed_folders)?;
    write_json_entry(&mut zip, &opts, "feeds.json", &feeds)?;
    write_json_entry(&mut zip, &opts, "content_items.json", &content_items)?;
    write_json_entry(&mut zip, &opts, "tags.json", &tags)?;
    write_json_entry(&mut zip, &opts, "collections.json", &collections)?;
    write_json_entry(&mut zip, &opts, "feed_item_meta.json", &feed_item_meta)?;
    write_json_entry(&mut zip, &opts, "bookmark_meta.json", &bookmark_meta)?;
    write_json_entry(&mut zip, &opts, "highlight_meta.json", &highlight_meta)?;
    write_json_entry(
        &mut zip,
        &opts,
        "content_item_tags.json",
        &content_item_tags,
    )?;
    write_json_entry(&mut zip, &opts, "collection_items.json", &collection_items)?;

    zip.finish().context("failed to finalize backup archive")?;

    let total = feed_folders.len()
        + feeds.len()
        + content_items.len()
        + tags.len()
        + collections.len()
        + feed_item_meta.len()
        + bookmark_meta.len()
        + highlight_meta.len()
        + content_item_tags.len()
        + collection_items.len();

    println!("Backup written to {}", output.display());
    println!(
        "  {} feeds, {} items, {} tags, {} collections ({total} records total)",
        feeds.len(),
        content_items.len(),
        tags.len(),
        collections.len(),
    );

    Ok(())
}

/// Write a single JSON entry to the ZIP archive.
fn write_json_entry<W: Write + std::io::Seek, T: serde::Serialize>(
    zip: &mut zip::ZipWriter<W>,
    opts: &zip::write::SimpleFileOptions,
    name: &str,
    data: &T,
) -> Result<()> {
    zip.start_file(name, *opts)
        .with_context(|| format!("failed to start ZIP entry: {name}"))?;
    serde_json::to_writer_pretty(&mut *zip, data)
        .with_context(|| format!("failed to write JSON entry: {name}"))?;
    Ok(())
}

// ------------------------------------------------------------------
// Backup restore
// ------------------------------------------------------------------

/// Restore from a full backup archive.
fn restore_backup(db: &Database, path: &std::path::Path) -> Result<()> {
    use pergamon_core::model::{BookmarkMeta, Collection, HighlightMeta, Tag};
    use zip::ZipArchive;

    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open backup file: {}", path.display()))?;
    let mut archive =
        ZipArchive::new(file).with_context(|| "failed to read backup archive as ZIP")?;

    // Read and validate manifest.
    let manifest: BackupManifest = read_json_entry(&mut archive, "manifest.json")?;
    if manifest.app != "pergamon" {
        bail!("not a pergamon backup (manifest.app = {:?})", manifest.app);
    }

    let current_version = db
        .schema_version()
        .context("reading current schema version")?;
    if manifest.schema_version > current_version {
        bail!(
            "backup schema version {} is newer than current {} — upgrade pergamon first",
            manifest.schema_version,
            current_version
        );
    }

    // Deserialize all tables.
    let feed_folders: Vec<FeedFolder> = read_json_entry(&mut archive, "feed_folders.json")?;
    let feeds: Vec<Feed> = read_json_entry(&mut archive, "feeds.json")?;
    let content_items: Vec<ContentItem> = read_json_entry(&mut archive, "content_items.json")?;
    let tags: Vec<Tag> = read_json_entry(&mut archive, "tags.json")?;
    let collections: Vec<Collection> = read_json_entry(&mut archive, "collections.json")?;
    let feed_item_meta: Vec<FeedItemMeta> = read_json_entry(&mut archive, "feed_item_meta.json")?;
    let bookmark_meta: Vec<BookmarkMeta> = read_json_entry(&mut archive, "bookmark_meta.json")?;
    let highlight_meta: Vec<HighlightMeta> = read_json_entry(&mut archive, "highlight_meta.json")?;
    let content_item_tags: Vec<(Uuid, Uuid)> =
        read_json_entry(&mut archive, "content_item_tags.json")?;
    let collection_items: Vec<(Uuid, Uuid, i32)> =
        read_json_entry(&mut archive, "collection_items.json")?;

    db.restore_backup(
        &feed_folders,
        &feeds,
        &content_items,
        &tags,
        &collections,
        &feed_item_meta,
        &bookmark_meta,
        &highlight_meta,
        &content_item_tags,
        &collection_items,
    )
    .context("failed to restore backup into database")?;

    let total = feed_folders.len()
        + feeds.len()
        + content_items.len()
        + tags.len()
        + collections.len()
        + feed_item_meta.len()
        + bookmark_meta.len()
        + highlight_meta.len()
        + content_item_tags.len()
        + collection_items.len();

    println!("Backup restored from {}", path.display());
    println!(
        "  {} feeds, {} items, {} tags, {} collections ({total} records total)",
        feeds.len(),
        content_items.len(),
        tags.len(),
        collections.len(),
    );

    Ok(())
}

/// Read a JSON entry from a ZIP archive.
fn read_json_entry<T: serde::de::DeserializeOwned>(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<T> {
    let entry = archive
        .by_name(name)
        .with_context(|| format!("missing backup entry: {name}"))?;
    serde_json::from_reader(entry).with_context(|| format!("failed to parse backup entry: {name}"))
}

// ------------------------------------------------------------------
// Configuration
// ------------------------------------------------------------------

/// Configuration loaded from `config.toml`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Config {
    /// Default output format.
    #[serde(default = "Config::default_format")]
    default_format: String,
    /// Whether to use color output (respects `NO_COLOR` env var).
    #[serde(default = "Config::default_color")]
    color: bool,
    /// Strftime-style date format.
    #[serde(default = "Config::default_date_format")]
    date_format: String,
    /// Feed refresh interval in minutes.
    #[serde(default = "Config::default_refresh_interval")]
    feed_refresh_interval_minutes: u32,
}

impl Config {
    fn default_format() -> String {
        "table".to_owned()
    }

    const fn default_color() -> bool {
        true
    }

    fn default_date_format() -> String {
        "%Y-%m-%d %H:%M".to_owned()
    }

    const fn default_refresh_interval() -> u32 {
        60
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_format: Self::default_format(),
            color: Self::default_color(),
            date_format: Self::default_date_format(),
            feed_refresh_interval_minutes: Self::default_refresh_interval(),
        }
    }
}

/// Platform-standard config file path.
fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pergamon")
        .join("config.toml")
}

/// Load configuration from disk, falling back to defaults.
fn load_config() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path).map_or_else(
        |_| Config::default(),
        |contents| toml::from_str(&contents).unwrap_or_default(),
    )
}

/// Show current configuration.
fn show_config() -> Result<()> {
    let path = config_path();
    let config = load_config();

    println!("Config file: {}", path.display());
    if path.exists() {
        println!("Status: loaded");
    } else {
        println!("Status: using defaults (file not found)");
    }
    println!();
    let toml_str = toml::to_string_pretty(&config).context("failed to serialize default config")?;
    print!("{toml_str}");
    Ok(())
}

// ------------------------------------------------------------------
// Shell completions
// ------------------------------------------------------------------

/// Generate shell completions and write to stdout.
fn generate_completions(shell: clap_complete::Shell) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "pergamon", &mut std::io::stdout());
}
