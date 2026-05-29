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
use pergamon_core::model::{
    BookmarkMeta, Collection, ContentItem, Feed, FeedFolder, FeedItemMeta, LinkHealth, Tag,
};
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
        /// Save this search as a smart collection with the given name.
        #[arg(long)]
        save: Option<String>,
    },
    /// Run a previously saved search by name.
    SavedSearch {
        /// Name of the saved search (smart collection).
        name: String,
        /// Maximum number of results (default: 20).
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// List all saved searches (smart collections).
    ListSaved,
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
    /// Diagnose and fix data quality issues.
    Doctor {
        #[command(subcommand)]
        action: DoctorAction,
    },
    /// Manage highlights across all content.
    Highlight {
        #[command(subcommand)]
        action: HighlightAction,
    },
    /// Manage notes on content items.
    Note {
        #[command(subcommand)]
        action: NoteAction,
    },
    /// Spaced repetition review (FSRS-5).
    Review {
        #[command(subcommand)]
        action: ReviewAction,
    },
    /// Show current configuration.
    Config,
    /// View statistics dashboards.
    Stats {
        #[command(subcommand)]
        action: StatsAction,
    },
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
    /// Import bookmarks from a Raindrop.io CSV export.
    Raindrop {
        /// Path to the Raindrop CSV file.
        file: PathBuf,
        /// Show what would be imported without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import bookmarks from a Pocket HTML export.
    Pocket {
        /// Path to the Pocket HTML file.
        file: PathBuf,
        /// Show what would be imported without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import highlights from a Kindle My Clippings.txt file.
    Kindle {
        /// Path to the My Clippings.txt file.
        file: PathBuf,
        /// Show what would be imported without making changes.
        #[arg(long)]
        dry_run: bool,
        /// Automatically enable spaced repetition for imported highlights.
        #[arg(long)]
        enable_review: bool,
    },
    /// Import highlights from a Readwise CSV export.
    Readwise {
        /// Path to the Readwise CSV file.
        file: PathBuf,
        /// Show what would be imported without making changes.
        #[arg(long)]
        dry_run: bool,
        /// Automatically enable spaced repetition for imported highlights.
        #[arg(long)]
        enable_review: bool,
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
    /// Export highlights and bookmarks to an Obsidian vault.
    Obsidian {
        /// Path to the Obsidian vault root directory.
        #[arg(long)]
        vault: PathBuf,
        /// Folder name within the vault (default: "Pergamon").
        #[arg(long, default_value = "Pergamon")]
        folder: String,
        /// Preview what would be exported without writing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Export content items as Markdown files with YAML frontmatter.
    Markdown {
        /// Output directory.
        #[arg(long, short)]
        output: PathBuf,
        /// Filename template (default: "{title}--{id}").
        /// Placeholders: {title}, {date}, {id}, {type}.
        #[arg(long, default_value = "{title}--{id}")]
        filename: String,
        /// Generate wikilink backlinks between related items.
        #[arg(long)]
        backlinks: bool,
        /// Tag format: "yaml" (default), "hashtag", or "both".
        #[arg(long, default_value = "yaml")]
        tag_format: String,
        /// Filter by content type (e.g. "article", "bookmark", "highlight").
        #[arg(long, name = "type")]
        type_filter: Option<String>,
        /// Preview what would be exported without writing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Export content items as versioned JSON.
    Json {
        /// Output file path (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Pretty-print the JSON output.
        #[arg(long)]
        pretty: bool,
        /// Include full content text (can be large).
        #[arg(long)]
        include_content: bool,
        /// Filter by content type (e.g. "article", "bookmark", "highlight").
        #[arg(long, name = "type")]
        type_filter: Option<String>,
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
        /// Create a smart (auto-populated) collection with the given filter.
        #[arg(long)]
        smart: Option<String>,
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
    /// Update the filter query of a smart collection.
    EditFilter {
        /// Collection (name or UUID).
        collection: String,
        /// New filter query string (DSL syntax).
        filter: String,
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

/// Doctor subcommands — data quality and deduplication.
#[derive(Debug, Subcommand)]
enum DoctorAction {
    /// Scan for duplicate URLs (exact and canonical matches).
    Dupes,
    /// Merge two duplicate items, keeping one and discarding the other.
    Merge {
        /// ID of the item to keep.
        keep: String,
        /// ID of the item to discard (will be deleted after merge).
        discard: String,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Check link health by probing saved URLs for dead/broken links.
    Links {
        /// Only check links not checked in the last N days.
        #[arg(long)]
        stale: Option<u32>,
    },
}

/// Highlight management subcommands.
#[derive(Debug, Subcommand)]
enum HighlightAction {
    /// Create a new highlight from a source item.
    Add {
        /// Source content item ID.
        source: String,
        /// Quoted text to highlight.
        text: String,
        /// Optional annotation / note on the highlight.
        #[arg(long)]
        note: Option<String>,
        /// Highlight color (e.g. yellow, green, blue, red).
        #[arg(long)]
        color: Option<String>,
        /// Tag to apply to the highlight (repeatable).
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
    },
    /// List all highlights with optional filters.
    List {
        /// Filter by source content item ID.
        #[arg(long)]
        source: Option<String>,
        /// Filter by tag name.
        #[arg(long)]
        tag: Option<String>,
        /// Only highlights created on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Only highlights created before this date (YYYY-MM-DD).
        #[arg(long)]
        before: Option<String>,
        /// Maximum number of results (default: 50).
        #[arg(long, default_value = "50")]
        limit: u32,
        /// Output format: text or json.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Show a specific highlight with full details.
    Show {
        /// Highlight content item ID.
        id: String,
    },
    /// Export highlights to Markdown or JSON.
    Export {
        /// Output format: md or json.
        #[arg(long, default_value = "md")]
        format: String,
        /// Output file path (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Filter by source content item ID.
        #[arg(long)]
        source: Option<String>,
    },
}

/// Note management subcommands.
#[derive(Debug, Subcommand)]
enum NoteAction {
    /// Add a note to a content item.
    Add {
        /// Content item ID to attach the note to.
        item: String,
        /// Note body text.
        text: String,
    },
    /// List notes (all or for a specific item).
    List {
        /// Content item ID (list all notes if omitted).
        item: Option<String>,
        /// Output format: text or json.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Edit a note's body text.
    Edit {
        /// Note ID.
        id: String,
        /// New body text.
        text: String,
    },
    /// Delete a note.
    Delete {
        /// Note ID.
        id: String,
    },
}

/// Spaced-repetition review subcommands.
#[derive(Debug, Subcommand)]
enum ReviewAction {
    /// Enable spaced repetition for a highlight.
    Enable {
        /// Highlight content item ID.
        id: String,
    },
    /// Disable spaced repetition for a highlight.
    Disable {
        /// Highlight content item ID.
        id: String,
    },
    /// List due review cards.
    Due {
        /// Maximum number of cards to show.
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output format: text or json.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Show review statistics.
    Stats {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Show stats in a TUI dashboard instead of text/JSON output.
        #[arg(long)]
        tui: bool,
    },
    /// Start an interactive review session in the TUI.
    Start {
        /// Maximum number of cards to review (default: all due).
        #[arg(long)]
        limit: Option<usize>,
    },
}

/// Statistics subcommands.
#[derive(Debug, Subcommand)]
enum StatsAction {
    /// Show retention and review statistics dashboard.
    Review {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Show stats in a TUI dashboard instead of text/JSON output.
        #[arg(long)]
        tui: bool,
    },
}

/// CLI output format for commands supporting structured output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    /// Human-readable text.
    Text,
    /// Machine-readable JSON.
    Json,
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
            save,
        } => {
            handle_search(
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
            )?;
            if let Some(name) = save {
                save_search(
                    &db,
                    &name,
                    &query,
                    content_type.as_deref(),
                    tag.as_deref(),
                    status.as_deref(),
                    source.as_deref(),
                    since.as_deref(),
                    before.as_deref(),
                )?;
            }
            Ok(())
        }
        Command::SavedSearch { name, limit } => run_saved_search(&db, &name, limit),
        Command::ListSaved => list_saved_searches(&db),
        Command::Import { action } => handle_import(&db, action),
        Command::Export { action } => handle_export(&db, action),
        Command::Collection { action } => handle_collection(&db, action),
        Command::Tag { action } => handle_tag(&db, action),
        Command::Bulk { action } => handle_bulk(&db, action),
        Command::Doctor { action } => handle_doctor(&db, action),
        Command::Highlight { action } => handle_highlight(&db, action),
        Command::Note { action } => handle_note(&db, action),
        Command::Review { action } => handle_review(&db, action),
        Command::Stats { action } => handle_stats(&db, &action),
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
        ImportAction::Raindrop { file, dry_run } => import_raindrop(db, &file, dry_run),
        ImportAction::Pocket { file, dry_run } => import_pocket(db, &file, dry_run),
        ImportAction::Kindle {
            file,
            dry_run,
            enable_review,
        } => import_kindle(db, &file, dry_run, enable_review),
        ImportAction::Readwise {
            file,
            dry_run,
            enable_review,
        } => import_readwise(db, &file, dry_run, enable_review),
        ImportAction::Backup { file } => restore_backup(db, &file),
    }
}

/// Dispatch export subcommand.
fn handle_export(db: &Database, action: ExportAction) -> Result<()> {
    match action {
        ExportAction::Opml { output } => export_opml(db, output.as_deref()),
        ExportAction::Backup { output } => export_backup(db, &output),
        ExportAction::Obsidian {
            vault,
            folder,
            dry_run,
        } => export_obsidian(db, &vault, &folder, dry_run),
        ExportAction::Markdown {
            output,
            filename,
            backlinks,
            tag_format,
            type_filter,
            dry_run,
        } => export_markdown(
            db,
            &output,
            &filename,
            backlinks,
            &tag_format,
            type_filter.as_deref(),
            dry_run,
        ),
        ExportAction::Json {
            output,
            pretty,
            include_content,
            type_filter,
        } => export_json(
            db,
            output.as_deref(),
            pretty,
            include_content,
            type_filter.as_deref(),
        ),
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

// ======================================================================
// Raindrop.io import
// ======================================================================

/// Import bookmarks from a Raindrop.io CSV export.
fn import_raindrop(db: &Database, path: &std::path::Path, dry_run: bool) -> Result<()> {
    use pergamon_core::model::BookmarkMeta;

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read Raindrop CSV: {}", path.display()))?;

    let items = pergamon_import::parse_raindrop_csv(&bytes)
        .with_context(|| format!("failed to parse Raindrop CSV: {}", path.display()))?;

    if dry_run {
        println!("Dry run — no changes will be made.\n");
    }
    println!(
        "Raindrop.io: {} bookmark(s) in {}",
        items.len(),
        path.display()
    );

    let mut stats = BookmarkImportStats::default();
    let now = OffsetDateTime::now_utc();

    for item in &items {
        let canonical_url =
            pergamon_extract::canonicalize_url(&item.url).unwrap_or_else(|_| item.url.clone());

        // Dedup by canonical URL.
        if let Some(existing) = db
            .get_content_item_by_url(&canonical_url)
            .context("failed to check for duplicate")?
        {
            if !dry_run {
                apply_tags(db, existing.id, &item.tags)?;
                if let Some(ref folder) = item.folder {
                    apply_collection(db, existing.id, folder)?;
                }
            }
            println!("  ✓ {} (exists, updated tags/collections)", item.title);
            stats.existing += 1;
            continue;
        }

        if dry_run {
            println!("  + {} (would create)", item.title);
            stats.created += 1;
            continue;
        }

        let created_at = item.created.unwrap_or(now);
        let content_item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(canonical_url),
            title: item.title.clone(),
            author: None,
            content_type: ContentType::Bookmark,
            status: DocumentStatus::Inbox,
            content_text: None,
            excerpt: item.excerpt.clone(),
            published_at: None,
            created_at,
            updated_at: now,
        };

        db.insert_content_item(&content_item)
            .with_context(|| format!("failed to insert: {}", item.url))?;

        // Combine note + highlights into description for BookmarkMeta.
        let description = build_raindrop_description(item.note.as_ref(), &item.highlights);
        let bookmark_meta = BookmarkMeta {
            content_item_id: content_item.id,
            original_url: Some(item.url.clone()),
            saved_from: Some(format!("raindrop:{}", item.id)),
            thumbnail_url: item.cover.clone(),
            description,
            site_name: None,
            favicon_url: None,
        };
        db.insert_bookmark_meta(&bookmark_meta)
            .with_context(|| format!("failed to insert bookmark meta: {}", item.url))?;

        apply_tags(db, content_item.id, &item.tags)?;
        if let Some(ref folder) = item.folder {
            apply_collection(db, content_item.id, folder)?;
        }

        println!("  + {}", item.title);
        stats.created += 1;
    }

    println!(
        "\n{}: {} created, {} existing (updated)",
        if dry_run { "Would import" } else { "Imported" },
        stats.created,
        stats.existing,
    );
    Ok(())
}

/// Combine note and highlights into a description string.
fn build_raindrop_description(note: Option<&String>, highlights: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = note {
        parts.push(n.clone());
    }
    if !highlights.is_empty() {
        parts.push(format!("Highlights:\n{}", highlights.join("\n")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

// ======================================================================
// Pocket import
// ======================================================================

/// Import bookmarks from a Pocket HTML export.
fn import_pocket(db: &Database, path: &std::path::Path, dry_run: bool) -> Result<()> {
    use pergamon_core::model::BookmarkMeta;

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read Pocket HTML: {}", path.display()))?;

    let items = pergamon_import::parse_pocket_html(&bytes)
        .with_context(|| format!("failed to parse Pocket HTML: {}", path.display()))?;

    if dry_run {
        println!("Dry run — no changes will be made.\n");
    }
    println!("Pocket: {} bookmark(s) in {}", items.len(), path.display());

    let mut stats = BookmarkImportStats::default();
    let now = OffsetDateTime::now_utc();

    for item in &items {
        let canonical_url =
            pergamon_extract::canonicalize_url(&item.url).unwrap_or_else(|_| item.url.clone());

        // Dedup by canonical URL.
        if let Some(existing) = db
            .get_content_item_by_url(&canonical_url)
            .context("failed to check for duplicate")?
        {
            if !dry_run {
                apply_tags(db, existing.id, &item.tags)?;
            }
            println!("  ✓ {} (exists, updated tags)", item.title);
            stats.existing += 1;
            continue;
        }

        if dry_run {
            println!("  + {} (would create)", item.title);
            stats.created += 1;
            continue;
        }

        let created_at = item.add_date.unwrap_or(now);
        let content_item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(canonical_url),
            title: item.title.clone(),
            author: None,
            content_type: ContentType::Bookmark,
            status: DocumentStatus::Inbox,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at,
            updated_at: now,
        };

        db.insert_content_item(&content_item)
            .with_context(|| format!("failed to insert: {}", item.url))?;

        let bookmark_meta = BookmarkMeta {
            content_item_id: content_item.id,
            original_url: Some(item.url.clone()),
            saved_from: Some("pocket".to_owned()),
            thumbnail_url: None,
            description: None,
            site_name: None,
            favicon_url: None,
        };
        db.insert_bookmark_meta(&bookmark_meta)
            .with_context(|| format!("failed to insert bookmark meta: {}", item.url))?;

        apply_tags(db, content_item.id, &item.tags)?;

        println!("  + {}", item.title);
        stats.created += 1;
    }

    println!(
        "\n{}: {} created, {} existing (updated)",
        if dry_run { "Would import" } else { "Imported" },
        stats.created,
        stats.existing,
    );
    Ok(())
}

/// Statistics for bookmark imports (Raindrop, Pocket).
#[derive(Default)]
struct BookmarkImportStats {
    created: u64,
    existing: u64,
}

/// Statistics for highlight imports (Kindle, Readwise).
#[derive(Default)]
struct HighlightImportStats {
    sources_created: u64,
    sources_existing: u64,
    highlights_created: u64,
    highlights_existing: u64,
    notes_created: u64,
    review_cards_created: u64,
}

// ======================================================================
// Kindle import
// ======================================================================

/// Import highlights from a Kindle My Clippings.txt file.
#[allow(clippy::too_many_lines)]
fn import_kindle(
    db: &Database,
    path: &std::path::Path,
    dry_run: bool,
    enable_review: bool,
) -> Result<()> {
    use pergamon_core::model::{HighlightMeta, Note};
    use pergamon_import::kindle::{KindleClippingType, kindle_highlight_key, kindle_source_key};

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read Kindle clippings: {}", path.display()))?;

    let clippings = pergamon_import::parse_kindle_clippings(&bytes)
        .with_context(|| format!("failed to parse Kindle clippings: {}", path.display()))?;

    if dry_run {
        println!("Dry run — no changes will be made.\n");
    }

    let total = clippings.len();
    let highlight_count = clippings
        .iter()
        .filter(|c| c.clipping_type == KindleClippingType::Highlight)
        .count();
    let note_count = clippings
        .iter()
        .filter(|c| c.clipping_type == KindleClippingType::Note)
        .count();
    let bookmark_count = total - highlight_count - note_count;

    println!(
        "Kindle: {} entries ({} highlights, {} notes, {} bookmarks) in {}",
        total,
        highlight_count,
        note_count,
        bookmark_count,
        path.display()
    );

    let mut stats = HighlightImportStats::default();
    let now = OffsetDateTime::now_utc();

    // Cache of source book URL → content item ID to avoid repeated lookups.
    let mut source_cache: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();

    if !dry_run {
        db.begin_transaction()
            .context("failed to begin transaction")?;
    }

    let result = (|| -> Result<()> {
        for clipping in &clippings {
            // Skip bookmarks — they have no content.
            if clipping.clipping_type == KindleClippingType::Bookmark {
                continue;
            }

            let source_url = kindle_source_key(&clipping.book_title, clipping.author.as_deref());

            // Find or create the source book content item.
            let source_id = if let Some(&cached_id) = source_cache.get(&source_url) {
                cached_id
            } else if let Some(existing) = db
                .get_content_item_by_url(&source_url)
                .context("failed to look up source book")?
            {
                source_cache.insert(source_url.clone(), existing.id);
                stats.sources_existing += 1;
                println!("  📖 {} (exists)", clipping.book_title);
                existing.id
            } else if dry_run {
                let book_id = Uuid::new_v4();
                source_cache.insert(source_url.clone(), book_id);
                stats.sources_created += 1;
                println!("  📖 {} (would create)", clipping.book_title);
                book_id
            } else {
                let book_id = Uuid::new_v4();
                let created_at = clipping.added_at.unwrap_or(now);
                let book_item = ContentItem {
                    id: book_id,
                    url: Some(source_url.clone()),
                    title: clipping.book_title.clone(),
                    author: clipping.author.clone(),
                    content_type: ContentType::Article,
                    status: DocumentStatus::Reference,
                    content_text: None,
                    excerpt: None,
                    published_at: None,
                    created_at,
                    updated_at: now,
                };
                db.insert_content_item(&book_item).with_context(|| {
                    format!("failed to create source book: {}", clipping.book_title)
                })?;
                let kindle_tag = vec!["kindle".to_owned()];
                apply_tags(db, book_id, &kindle_tag)?;
                source_cache.insert(source_url.clone(), book_id);
                stats.sources_created += 1;
                println!("  📖 {}", clipping.book_title);
                book_id
            };

            match clipping.clipping_type {
                KindleClippingType::Highlight => {
                    let highlight_url = kindle_highlight_key(
                        &clipping.book_title,
                        clipping.author.as_deref(),
                        clipping.location.as_deref(),
                        &clipping.content,
                    );

                    // Dedup by highlight URL.
                    if db
                        .get_content_item_by_url(&highlight_url)
                        .context("failed to check for duplicate highlight")?
                        .is_some()
                    {
                        stats.highlights_existing += 1;
                        continue;
                    }

                    if dry_run {
                        let preview = truncate_str(&clipping.content, 60);
                        println!("    + \"{preview}\" (would create)");
                        stats.highlights_created += 1;
                        continue;
                    }

                    let highlight_id = Uuid::new_v4();
                    let created_at = clipping.added_at.unwrap_or(now);
                    let highlight_item = ContentItem {
                        id: highlight_id,
                        url: Some(highlight_url),
                        title: format!("Highlight from {}", truncate_str(&clipping.book_title, 80)),
                        author: clipping.author.clone(),
                        content_type: ContentType::Highlight,
                        status: DocumentStatus::Reference,
                        content_text: Some(clipping.content.clone()),
                        excerpt: Some(truncate_str(&clipping.content, 200)),
                        published_at: None,
                        created_at,
                        updated_at: now,
                    };
                    db.insert_content_item(&highlight_item)
                        .context("failed to insert highlight")?;

                    let position = clipping.location.as_ref().and_then(|l| {
                        l.split('-')
                            .next()
                            .and_then(|s| s.replace("Page ", "").parse::<i64>().ok())
                    });

                    let meta = HighlightMeta {
                        content_item_id: highlight_id,
                        source_item_id: Some(source_id),
                        quote_text: clipping.content.clone(),
                        note: None,
                        position_start: position,
                        position_end: None,
                        color: None,
                    };
                    db.insert_highlight_meta(&meta)
                        .context("failed to insert highlight meta")?;

                    if enable_review {
                        create_review_card(db, highlight_id, now)?;
                        stats.review_cards_created += 1;
                    }

                    stats.highlights_created += 1;
                }
                KindleClippingType::Note => {
                    if clipping.content.is_empty() {
                        continue;
                    }

                    // Dedup: skip if an identical note already exists on this source.
                    let existing_notes = db
                        .list_notes_for_item(source_id)
                        .context("failed to list existing notes")?;
                    if existing_notes.iter().any(|n| n.body == clipping.content) {
                        continue;
                    }

                    if dry_run {
                        let preview = truncate_str(&clipping.content, 60);
                        println!("    📝 \"{preview}\" (would create note)");
                        stats.notes_created += 1;
                        continue;
                    }

                    let note = Note {
                        id: Uuid::new_v4(),
                        content_item_id: source_id,
                        body: clipping.content.clone(),
                        created_at: clipping.added_at.unwrap_or(now),
                        updated_at: now,
                    };
                    db.insert_note(&note)
                        .context("failed to insert Kindle note")?;

                    stats.notes_created += 1;
                }
                KindleClippingType::Bookmark => {}
            }
        }
        Ok(())
    })();

    if !dry_run {
        if result.is_ok() {
            db.commit_transaction()
                .context("failed to commit transaction")?;
        } else {
            let _ = db.rollback_transaction();
        }
    }
    result?;

    let label = if dry_run { "Would import" } else { "Imported" };
    println!(
        "\n{label}: {} sources ({} new), {} highlights ({} new), {} notes, {} review cards",
        stats.sources_created + stats.sources_existing,
        stats.sources_created,
        stats.highlights_created + stats.highlights_existing,
        stats.highlights_created,
        stats.notes_created,
        stats.review_cards_created,
    );
    Ok(())
}

// ======================================================================
// Readwise import
// ======================================================================

/// Import highlights from a Readwise CSV export.
#[allow(clippy::too_many_lines)]
fn import_readwise(
    db: &Database,
    path: &std::path::Path,
    dry_run: bool,
    enable_review: bool,
) -> Result<()> {
    use pergamon_core::model::HighlightMeta;
    use pergamon_import::readwise::{readwise_highlight_key, readwise_source_key};

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read Readwise CSV: {}", path.display()))?;

    let items = pergamon_import::parse_readwise_csv(&bytes)
        .with_context(|| format!("failed to parse Readwise CSV: {}", path.display()))?;

    if dry_run {
        println!("Dry run — no changes will be made.\n");
    }
    println!(
        "Readwise: {} highlight(s) in {}",
        items.len(),
        path.display()
    );

    let mut stats = HighlightImportStats::default();
    let now = OffsetDateTime::now_utc();

    // Cache of source URL → content item ID.
    let mut source_cache: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();

    if !dry_run {
        db.begin_transaction()
            .context("failed to begin transaction")?;
    }

    let result = (|| -> Result<()> {
        for item in &items {
            let source_url = readwise_source_key(
                item.source_url.as_deref(),
                &item.title,
                item.author.as_deref(),
                item.source_type.as_deref(),
            );

            // Find or create the source content item.
            let source_id = if let Some(&cached_id) = source_cache.get(&source_url) {
                cached_id
            } else if let Some(existing) = db
                .get_content_item_by_url(&source_url)
                .context("failed to look up source")?
            {
                source_cache.insert(source_url.clone(), existing.id);
                stats.sources_existing += 1;
                existing.id
            } else if dry_run {
                let src_id = Uuid::new_v4();
                source_cache.insert(source_url.clone(), src_id);
                stats.sources_created += 1;
                println!("  📖 {} (would create)", item.title);
                src_id
            } else {
                let src_id = Uuid::new_v4();
                let content_type = map_readwise_content_type(
                    item.source_type.as_deref(),
                    item.category.as_deref(),
                );
                let created_at = item.highlighted_at.unwrap_or(now);
                let source_item = ContentItem {
                    id: src_id,
                    url: Some(source_url.clone()),
                    title: item.title.clone(),
                    author: item.author.clone(),
                    content_type,
                    status: DocumentStatus::Reference,
                    content_text: None,
                    excerpt: None,
                    published_at: None,
                    created_at,
                    updated_at: now,
                };
                db.insert_content_item(&source_item)
                    .with_context(|| format!("failed to create source: {}", item.title))?;

                // Apply book-level tags + "readwise" provenance tag.
                let mut all_tags = item.book_tags.clone();
                all_tags.push("readwise".to_owned());
                apply_tags(db, src_id, &all_tags)?;

                source_cache.insert(source_url.clone(), src_id);
                stats.sources_created += 1;
                println!("  📖 {}", item.title);
                src_id
            };

            // Create the highlight.
            let highlight_url = readwise_highlight_key(
                item.uuid.as_deref(),
                item.source_url.as_deref(),
                &item.title,
                item.author.as_deref(),
                item.source_type.as_deref(),
                &item.highlight,
            );

            // Dedup by highlight URL.
            if db
                .get_content_item_by_url(&highlight_url)
                .context("failed to check for duplicate highlight")?
                .is_some()
            {
                stats.highlights_existing += 1;
                continue;
            }

            if dry_run {
                let preview = truncate_str(&item.highlight, 60);
                println!("    + \"{preview}\" (would create)");
                stats.highlights_created += 1;
                continue;
            }

            let highlight_id = Uuid::new_v4();
            let created_at = item.highlighted_at.unwrap_or(now);
            let highlight_item = ContentItem {
                id: highlight_id,
                url: Some(highlight_url),
                title: format!("Highlight from {}", truncate_str(&item.title, 80)),
                author: item.author.clone(),
                content_type: ContentType::Highlight,
                status: DocumentStatus::Reference,
                content_text: Some(item.highlight.clone()),
                excerpt: Some(truncate_str(&item.highlight, 200)),
                published_at: None,
                created_at,
                updated_at: now,
            };
            db.insert_content_item(&highlight_item)
                .context("failed to insert highlight")?;

            let position_start = item.location.as_ref().and_then(|l| {
                l.replace("Page ", "")
                    .replace("Location ", "")
                    .split('-')
                    .next()
                    .and_then(|s| s.trim().parse::<i64>().ok())
            });

            let meta = HighlightMeta {
                content_item_id: highlight_id,
                source_item_id: Some(source_id),
                quote_text: item.highlight.clone(),
                note: item.note.clone(),
                position_start,
                position_end: None,
                color: None,
            };
            db.insert_highlight_meta(&meta)
                .context("failed to insert highlight meta")?;

            // Apply highlight-level tags.
            if !item.tags.is_empty() {
                apply_tags(db, highlight_id, &item.tags)?;
            }

            if enable_review {
                create_review_card(db, highlight_id, now)?;
                stats.review_cards_created += 1;
            }

            stats.highlights_created += 1;
        }
        Ok(())
    })();

    if !dry_run {
        if result.is_ok() {
            db.commit_transaction()
                .context("failed to commit transaction")?;
        } else {
            let _ = db.rollback_transaction();
        }
    }
    result?;

    let label = if dry_run { "Would import" } else { "Imported" };
    println!(
        "\n{label}: {} sources ({} new), {} highlights ({} new), {} review cards",
        stats.sources_created + stats.sources_existing,
        stats.sources_created,
        stats.highlights_created + stats.highlights_existing,
        stats.highlights_created,
        stats.review_cards_created,
    );
    Ok(())
}

/// Map a Readwise source type / category to a pergamon `ContentType`.
fn map_readwise_content_type(source_type: Option<&str>, category: Option<&str>) -> ContentType {
    let key = source_type.or(category).map(|s| s.trim().to_lowercase());

    match key.as_deref() {
        Some("podcast" | "podcasts" | "podcast_episode") => ContentType::PodcastEpisode,
        Some("pdf") => ContentType::Pdf,
        // Books, articles, tweets, and everything else map to Article.
        _ => ContentType::Article,
    }
}

/// Create a new FSRS review card for a highlight.
fn create_review_card(db: &Database, highlight_id: Uuid, now: OffsetDateTime) -> Result<()> {
    let card = pergamon_core::model::ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: highlight_id,
        state: pergamon_core::fsrs::CardState::New,
        stability: None,
        difficulty: None,
        due_at: now,
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now,
        updated_at: now,
    };
    db.insert_review_card(&card)
        .context("failed to create review card")?;
    Ok(())
}

/// Truncate a string to a maximum length, appending "…" if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_owned()
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Apply a folder/collection to a content item, creating the collection if needed.
fn apply_collection(db: &Database, item_id: Uuid, folder_name: &str) -> Result<()> {
    let coll = if let Some(existing) = db
        .get_collection_by_name(folder_name)
        .context("failed to look up collection")?
    {
        existing
    } else {
        let now = OffsetDateTime::now_utc();
        let coll = Collection {
            id: Uuid::new_v4(),
            name: folder_name.to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: false,
            filter_query: None,
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&coll)
            .with_context(|| format!("failed to create collection '{folder_name}'"))?;
        coll
    };
    // Idempotent: ignore if already in collection.
    let _ = db.add_to_collection(item_id, coll.id, 0);
    Ok(())
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
// Obsidian export
// ======================================================================

/// Export highlights and bookmarks to an Obsidian vault.
fn export_obsidian(
    db: &Database,
    vault_path: &std::path::Path,
    folder: &str,
    dry_run: bool,
) -> Result<()> {
    use pergamon_core::content_type::ContentType as CT;
    use pergamon_export::obsidian::{
        BookmarkBundle, ExportConfig, SourceBundle, execute_export, group_highlights_by_source,
        plan_export,
    };

    // 1. Fetch all highlights with their metadata.
    let all_highlights = db
        .list_highlights(None, None, None, None, None)
        .context("failed to list highlights")?;

    // 2. Group by source_item_id.
    let grouped = group_highlights_by_source(&all_highlights);

    // 3. Build source bundles for items that have highlights.
    let mut source_bundles = Vec::new();

    for (source_id_opt, highlights) in &grouped {
        if let Some(source_id) = source_id_opt {
            // Try to load the source content item.
            if let Ok(source) = db.get_content_item(*source_id) {
                let tags: Vec<String> = db
                    .tags_for_item(*source_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.name)
                    .collect();

                let notes = db.list_notes_for_item(*source_id).unwrap_or_default();

                source_bundles.push(SourceBundle {
                    source,
                    tags,
                    highlights: highlights.clone(),
                    notes,
                });
            }
        }
        // Orphan highlights (no source) are skipped for now.
    }

    // 4. Fetch bookmarks without highlights for standalone export.
    let bookmark_items = db
        .list_content_items(Some(CT::Bookmark), None, None, None)
        .context("failed to list bookmarks")?;

    // Filter out bookmarks that already appear as highlight sources.
    let source_ids: std::collections::HashSet<Uuid> =
        source_bundles.iter().map(|b| b.source.id).collect();

    let mut bookmark_bundles = Vec::new();
    for item in bookmark_items {
        if source_ids.contains(&item.id) {
            continue;
        }

        let tags: Vec<String> = db
            .tags_for_item(item.id)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();

        let description = db
            .get_bookmark_meta(item.id)
            .ok()
            .and_then(|m| m.description);

        bookmark_bundles.push(BookmarkBundle {
            item,
            tags,
            description,
        });
    }

    // 5. Plan the export.
    let config = ExportConfig {
        folder_name: folder.to_owned(),
        pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let plan = plan_export(&config, &source_bundles, &bookmark_bundles);

    if dry_run {
        println!("Dry run — no files will be written.\n");
        println!(
            "Would export {} file(s) to {}/{}",
            plan.files.len(),
            vault_path.display(),
            folder,
        );
        println!(
            "  {} source document(s) with highlights",
            source_bundles.len()
        );
        println!("  {} standalone bookmark(s)", bookmark_bundles.len());

        if !plan.files.is_empty() {
            println!("\nFiles:");
            for file in &plan.files {
                println!("  {}", file.relative_path.display());
            }
        }
        return Ok(());
    }

    // 6. Execute the export.
    let result = execute_export(&plan, vault_path).context("failed to write Obsidian export")?;

    println!(
        "Exported {} file(s) to {}/{}",
        result.written,
        vault_path.display(),
        folder,
    );
    println!(
        "  {} source document(s) with highlights",
        source_bundles.len()
    );
    println!("  {} standalone bookmark(s)", bookmark_bundles.len());
    println!(
        "  Manifest: {}/{}/.pergamon/manifest.json",
        vault_path.display(),
        folder,
    );

    Ok(())
}

/// Export content items as Markdown files with YAML frontmatter.
fn export_markdown(
    db: &Database,
    output_dir: &std::path::Path,
    filename_template: &str,
    backlinks: bool,
    tag_format_str: &str,
    type_filter: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    use pergamon_export::markdown::{
        ExportItem, MarkdownExportConfig, TagFormat, execute_markdown_export, plan_markdown_export,
    };
    use pergamon_export::template::SlugTemplate;

    let slug_template = SlugTemplate::parse(filename_template)
        .with_context(|| format!("invalid filename template: {filename_template}"))?;

    if !slug_template.has_id_placeholder() {
        eprintln!("Warning: template does not include {{id}} — duplicate titles may collide.");
    }

    let tag_format = match tag_format_str {
        "yaml" => TagFormat::YamlOnly,
        "hashtag" => TagFormat::Hashtag,
        "both" => TagFormat::Both,
        other => anyhow::bail!("unknown tag format: {other} (expected: yaml, hashtag, both)"),
    };

    let content_type_filter = type_filter
        .map(|s| {
            s.parse::<pergamon_core::content_type::ContentType>()
                .with_context(|| format!("unknown content type: {s}"))
        })
        .transpose()?;

    // Fetch items from the database.
    let items = db
        .list_content_items(content_type_filter, None, None, None)
        .context("failed to list content items")?;

    if items.is_empty() {
        println!("No items to export.");
        return Ok(());
    }

    // Build export items with tags, highlights, and notes.
    let mut export_items = Vec::new();
    for item in &items {
        let tags: Vec<String> = db
            .tags_for_item(item.id)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();

        let highlights = db
            .list_highlights(Some(item.id), None, None, None, None)
            .unwrap_or_default();

        let notes = db.list_notes_for_item(item.id).unwrap_or_default();

        export_items.push(ExportItem {
            item: item.clone(),
            tags,
            highlights,
            notes,
        });
    }

    let config = MarkdownExportConfig {
        slug_template,
        backlinks,
        tag_format,
        pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let files = plan_markdown_export(&config, &export_items);

    if dry_run {
        println!("Dry run — no files will be written.\n");
        println!(
            "Would export {} file(s) to {}",
            files.len(),
            output_dir.display()
        );
        if !files.is_empty() {
            println!("\nFiles:");
            for file in &files {
                println!("  {}", file.relative_path.display());
            }
        }
        return Ok(());
    }

    let result =
        execute_markdown_export(&files, output_dir).context("failed to write Markdown export")?;

    println!(
        "Exported {} file(s) to {}",
        result.written,
        output_dir.display(),
    );

    Ok(())
}

/// Export content items as versioned JSON.
fn export_json(
    db: &Database,
    output: Option<&std::path::Path>,
    pretty: bool,
    include_content: bool,
    type_filter: Option<&str>,
) -> Result<()> {
    use pergamon_export::json::{
        JsonExportConfig, JsonExportItem, build_json_export, serialize_json_export,
    };

    let content_type_filter = type_filter
        .map(|s| {
            s.parse::<pergamon_core::content_type::ContentType>()
                .with_context(|| format!("unknown content type: {s}"))
        })
        .transpose()?;

    let items = db
        .list_content_items(content_type_filter, None, None, None)
        .context("failed to list content items")?;

    if items.is_empty() {
        println!("No items to export.");
        return Ok(());
    }

    let mut export_items = Vec::new();
    for item in &items {
        let tags: Vec<String> = db
            .tags_for_item(item.id)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();

        let highlights = db
            .list_highlights(Some(item.id), None, None, None, None)
            .unwrap_or_default();

        let notes = db.list_notes_for_item(item.id).unwrap_or_default();

        let bookmark_meta = db.get_bookmark_meta(item.id).ok();

        let feed_item_meta = db.get_feed_item_meta(item.id).ok();

        export_items.push(JsonExportItem {
            item: item.clone(),
            tags,
            highlights,
            notes,
            bookmark_meta,
            feed_item_meta,
        });
    }

    let config = JsonExportConfig {
        pretty,
        include_content_text: include_content,
        pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let export = build_json_export(&config, &export_items);
    let json = serialize_json_export(&export, pretty).context("failed to serialize JSON export")?;

    if let Some(path) = output {
        std::fs::write(path, &json)
            .with_context(|| format!("failed to write to {}", path.display()))?;
        println!(
            "Exported {} item(s) to {}",
            export.item_count,
            path.display()
        );
    } else {
        print!("{json}");
    }

    Ok(())
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

/// Build enriched `BookmarkMeta` from HTML bytes.
fn build_enriched_bookmark_meta(
    content_item_id: Uuid,
    bytes: &[u8],
    original_url: &str,
    final_url: &str,
) -> BookmarkMeta {
    let html = String::from_utf8_lossy(bytes);
    let meta = pergamon_extract::extract_metadata(&html);

    let favicon_url = meta
        .favicon_url
        .and_then(|href| pergamon_extract::resolve_favicon_url(&href, final_url));

    BookmarkMeta {
        content_item_id,
        original_url: Some(original_url.to_owned()),
        saved_from: Some("cli".to_owned()),
        thumbnail_url: meta.og_image,
        description: meta.description,
        site_name: meta.site_name,
        favicon_url,
    }
}

/// Save a URL as an article: fetch → extract → store.
///
/// Deduplicates against the canonical form of the final (post-redirect)
/// URL. When a duplicate is found, still applies any requested tags.
/// Creates enriched `BookmarkMeta` with OG image, favicon, and site name.
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

        // If saving as bookmark and no BookmarkMeta exists, create one
        // with enriched metadata from the fetched page.
        if bookmark {
            let meta = build_enriched_bookmark_meta(existing.id, &bytes, &url, &final_url);
            let _ = db.upsert_bookmark_meta(&meta);
        }

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

    // Store enriched bookmark metadata (OG image, favicon, site name).
    let meta = build_enriched_bookmark_meta(item.id, &bytes, &url, &final_url);
    db.insert_bookmark_meta(&meta)
        .context("failed to save bookmark metadata")?;

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

/// Save a search as a smart collection.
#[allow(clippy::too_many_arguments)]
fn save_search(
    db: &Database,
    name: &str,
    query: &str,
    content_type: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    source: Option<&str>,
    since: Option<&str>,
    before: Option<&str>,
) -> Result<()> {
    // Build DSL filter string from search parameters.
    let mut parts = Vec::new();
    parts.push(format!("text:{query}"));
    if let Some(ct) = content_type {
        parts.push(format!("type:{ct}"));
    }
    if let Some(t) = tag {
        parts.push(format!("tag:{t}"));
    }
    if let Some(s) = status {
        parts.push(format!("status:{s}"));
    }
    if let Some(src) = source {
        parts.push(format!("source:{src}"));
    }
    if let Some(s) = since {
        parts.push(format!("since:{s}"));
    }
    if let Some(b) = before {
        parts.push(format!("before:{b}"));
    }
    let filter_str = parts.join(" ");

    // Validate.
    pergamon_core::smart_filter::SmartFilter::parse(&filter_str)
        .map_err(|e| anyhow::anyhow!("invalid filter: {e}"))?;

    let now = OffsetDateTime::now_utc();
    let coll = Collection {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id: None,
        sort_order: 0,
        is_smart: true,
        filter_query: Some(filter_str.clone()),
        created_at: now,
        updated_at: now,
    };
    db.insert_collection(&coll)
        .context("failed to save search")?;
    println!("Saved search '{name}' ({filter_str})");
    Ok(())
}

/// Run a previously saved search by name.
fn run_saved_search(db: &Database, name: &str, limit: u32) -> Result<()> {
    let coll = db
        .get_collection_by_name(name)
        .context("failed to look up saved search")?
        .ok_or_else(|| anyhow::anyhow!("no saved search named '{name}'"))?;
    if !coll.is_smart {
        bail!("'{name}' is not a smart collection / saved search");
    }
    let items = db
        .list_smart_collection_items(coll.id)
        .context("failed to run saved search")?;

    println!("Saved search: {} ({} result(s))", coll.name, items.len());
    if let Some(ref fq) = coll.filter_query {
        println!("Filter: {fq}");
    }
    println!();
    for item in items.iter().take(limit as usize) {
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

/// List all saved searches (smart collections).
fn list_saved_searches(db: &Database) -> Result<()> {
    let colls = db
        .list_collections()
        .context("failed to list collections")?;
    let smart: Vec<_> = colls.iter().filter(|c| c.is_smart).collect();
    if smart.is_empty() {
        println!("No saved searches.");
        return Ok(());
    }
    for coll in &smart {
        let count = db.count_smart_collection_items(coll.id).unwrap_or(0);
        println!("  {} [{}, {} result(s)]", coll.name, coll.id, count);
        if let Some(ref fq) = coll.filter_query {
            println!("    Filter: {fq}");
        }
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
        CollectionAction::Create {
            name,
            parent,
            smart,
        } => collection_create(db, &name, parent.as_deref(), smart.as_deref()),
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
        CollectionAction::EditFilter { collection, filter } => {
            let coll = resolve_collection(db, &collection)?;
            if !coll.is_smart {
                bail!("'{}' is not a smart collection", coll.name);
            }
            db.update_smart_filter(coll.id, &filter)
                .context("failed to update filter")?;
            println!("Updated filter for '{}': {filter}", coll.name);
            Ok(())
        }
    }
}

/// Create a new collection (manual or smart).
fn collection_create(
    db: &Database,
    name: &str,
    parent_ref: Option<&str>,
    smart_filter: Option<&str>,
) -> Result<()> {
    let parent_id = parent_ref
        .map(|p| resolve_collection(db, p).map(|c| c.id))
        .transpose()?;

    let (is_smart, filter_query) = if let Some(filter) = smart_filter {
        // Validate the filter parses correctly before creating.
        pergamon_core::smart_filter::SmartFilter::parse(filter)
            .map_err(|e| anyhow::anyhow!("invalid filter: {e}"))?;
        (true, Some(filter.to_owned()))
    } else {
        (false, None)
    };

    let now = OffsetDateTime::now_utc();
    let coll = Collection {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id,
        sort_order: 0,
        is_smart,
        filter_query,
        created_at: now,
        updated_at: now,
    };
    db.insert_collection(&coll)
        .context("failed to create collection")?;
    if is_smart {
        println!("Created smart collection '{}' ({})", coll.name, coll.id);
    } else {
        println!("Created collection '{}' ({})", coll.name, coll.id);
    }
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
    // Count items per manual collection.
    let all_memberships = db
        .list_all_collection_items()
        .context("failed to load collection memberships")?;
    let mut item_counts: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
    for &(_, coll_id, _) in &all_memberships {
        *item_counts.entry(coll_id).or_default() += 1;
    }

    for coll in &colls {
        let count = if coll.is_smart {
            db.count_smart_collection_items(coll.id).unwrap_or(0)
        } else {
            item_counts.get(&coll.id).copied().unwrap_or(0)
        };
        let kind_label = if coll.is_smart { " [smart]" } else { "" };
        let parent_label = coll
            .parent_id
            .and_then(|pid| colls.iter().find(|c| c.id == pid))
            .map_or(String::new(), |p| format!(" (in {})", p.name));
        println!(
            "  {}{kind_label} [{}, {} item(s)]{parent_label}",
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
    let items = if coll.is_smart {
        db.list_smart_collection_items(coll.id)
            .context("failed to run smart filter")?
    } else {
        db.list_collection_items(coll.id)
            .context("failed to list collection items")?
    };

    let kind_label = if coll.is_smart { " [smart]" } else { "" };
    println!(
        "Collection: {}{kind_label} ({} item(s))",
        coll.name,
        items.len()
    );
    if let Some(ref fq) = coll.filter_query {
        println!("Filter: {fq}");
    }
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
// Doctor command — data quality & dedup
// ======================================================================

/// Dispatch doctor subcommand.
fn handle_doctor(db: &Database, action: DoctorAction) -> Result<()> {
    match action {
        DoctorAction::Dupes => doctor_dupes(db),
        DoctorAction::Merge { keep, discard, yes } => doctor_merge(db, &keep, &discard, yes),
        DoctorAction::Links { stale } => doctor_links(db, stale),
    }
}

/// Scan for duplicate URLs (exact and canonical matches).
fn doctor_dupes(db: &Database) -> Result<()> {
    let urls = db
        .list_all_urls()
        .context("failed to load URLs from database")?;

    if urls.is_empty() {
        println!("No content items with URLs found.");
        return Ok(());
    }

    // Group by canonical URL.
    let mut groups: std::collections::HashMap<String, Vec<(Uuid, String)>> =
        std::collections::HashMap::new();

    for (id, url) in &urls {
        let canonical = pergamon_extract::canonicalize_url(url).unwrap_or_else(|_| url.clone());
        groups
            .entry(canonical)
            .or_default()
            .push((*id, url.clone()));
    }

    // Filter to groups with duplicates.
    let mut dupe_groups: Vec<(String, Vec<(Uuid, String)>)> = groups
        .into_iter()
        .filter(|(_, items)| items.len() > 1)
        .collect();

    if dupe_groups.is_empty() {
        println!("No duplicates found across {} URL(s).", urls.len());
        return Ok(());
    }

    dupe_groups.sort_by(|a, b| a.0.cmp(&b.0));

    println!(
        "Found {} duplicate group(s) across {} URL(s):\n",
        dupe_groups.len(),
        urls.len()
    );

    for (canonical, items) in &dupe_groups {
        let all_exact = items.iter().all(|(_, u)| u == canonical);
        let confidence = if all_exact { "exact" } else { "canonical" };

        println!("  {canonical} ({confidence})");
        for (id, url) in items {
            let label = if url == canonical { "" } else { " (variant)" };
            // Look up the item for its title.
            if let Ok(item) = db.get_content_item(*id) {
                println!(
                    "    {} {id} — {}{label}",
                    item.content_type.as_str(),
                    item.title,
                );
            } else {
                println!("    unknown {id}{label}");
            }
        }
        println!();
    }

    println!("To merge duplicates, run:");
    println!("  pergamon doctor merge <keep-id> <discard-id>");

    Ok(())
}

/// Merge two duplicate items: transfer tags/collections, preserve extension
/// tables, backdate `created_at`, and delete the discarded item.
fn doctor_merge(db: &Database, keep_str: &str, discard_str: &str, yes: bool) -> Result<()> {
    let keep_id = keep_str
        .parse::<Uuid>()
        .with_context(|| format!("invalid keep ID: {keep_str}"))?;
    let discard_id = discard_str
        .parse::<Uuid>()
        .with_context(|| format!("invalid discard ID: {discard_str}"))?;

    if keep_id == discard_id {
        bail!("keep and discard IDs must be different");
    }

    let keep_item = db
        .get_content_item(keep_id)
        .with_context(|| format!("item not found: {keep_id}"))?;
    let discard_item = db
        .get_content_item(discard_id)
        .with_context(|| format!("item not found: {discard_id}"))?;

    println!("Merge plan:");
    println!(
        "  KEEP:    {} [{}] — {} ({})",
        keep_item.title,
        keep_id,
        keep_item.content_type.as_str(),
        keep_item.status.as_str()
    );
    println!(
        "  DISCARD: {} [{}] — {} ({})",
        discard_item.title,
        discard_id,
        discard_item.content_type.as_str(),
        discard_item.status.as_str()
    );

    // Check extension table overlap.
    let keep_has_feed = db
        .has_feed_item_meta(keep_id)
        .context("checking feed_item_meta")?;
    let keep_has_bookmark = db
        .has_bookmark_meta(keep_id)
        .context("checking bookmark_meta")?;
    let discard_has_feed = db
        .has_feed_item_meta(discard_id)
        .context("checking feed_item_meta")?;
    let discard_has_bookmark = db
        .has_bookmark_meta(discard_id)
        .context("checking bookmark_meta")?;

    if discard_has_feed && !keep_has_feed {
        println!("  Note: discard item has feed_item_meta — will be lost in merge.");
    }
    if discard_has_bookmark && !keep_has_bookmark {
        println!("  Note: discard item has bookmark_meta — will be merged into keep item.");
    }

    // Confirm unless --yes.
    if !yes {
        print!("\nProceed? [y/N] ");
        std::io::stdout().flush().context("flush")?;
        let mut input = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut input)
            .context("reading confirmation")?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // 1. Transfer tags from discard → keep.
    db.transfer_tags(discard_id, keep_id)
        .context("transferring tags")?;

    // 2. Transfer collections from discard → keep.
    db.transfer_collections(discard_id, keep_id)
        .context("transferring collections")?;

    // 3. Upsert bookmark_meta if discard has one and keep doesn't.
    if discard_has_bookmark && !keep_has_bookmark {
        if let Ok(bm) = db.get_bookmark_meta(discard_id) {
            let merged = BookmarkMeta {
                content_item_id: keep_id,
                original_url: bm.original_url,
                saved_from: bm.saved_from,
                thumbnail_url: bm.thumbnail_url,
                description: bm.description,
                site_name: bm.site_name,
                favicon_url: bm.favicon_url,
            };
            db.upsert_bookmark_meta(&merged)
                .context("merging bookmark_meta")?;
            println!("Merged bookmark metadata.");
        }
    }

    // 4. Backdate created_at to the earliest of the two.
    if discard_item.created_at < keep_item.created_at {
        db.backdate_created_at(keep_id, discard_item.created_at)
            .context("backdating created_at")?;
        println!(
            "Backdated created_at to {}.",
            discard_item
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".into())
        );
    }

    // 5. Delete the discard item.
    db.delete_content_item(discard_id)
        .context("deleting discard item")?;

    println!(
        "Merged into {} [{}]. Discarded item deleted.",
        keep_item.title, keep_id
    );

    Ok(())
}

/// Check link health by probing saved URLs for dead/broken links.
fn doctor_links(db: &Database, stale_days: Option<u32>) -> Result<()> {
    use std::collections::HashMap;

    let urls = db
        .list_urls_for_health_check(stale_days)
        .context("listing URLs")?;

    if urls.is_empty() {
        println!("No URLs to check.");
        return Ok(());
    }

    println!("Checking {} URL(s)...\n", urls.len());

    // Build a client that does NOT follow redirects so we can track them.
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("pergamon/{}", pergamon_core::VERSION))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("failed to build link-check HTTP client")?;

    // Group by domain for rate-limiting.
    let mut domain_last: HashMap<String, std::time::Instant> = HashMap::new();
    let rate_limit = std::time::Duration::from_millis(200);

    let mut healthy = 0u32;
    let mut redirected = 0u32;
    let mut dead = 0u32;
    let mut server_error = 0u32;
    let mut conn_error = 0u32;

    for (idx, (item_id, url, _title)) in urls.iter().enumerate() {
        // Progress reporting every 50 items.
        if idx > 0 && idx % 50 == 0 {
            println!("  ... checked {idx}/{} URLs", urls.len());
        }

        // Rate-limit per domain.
        if let Ok(parsed) = reqwest::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let host_key = host.to_lowercase();
                if let Some(last) = domain_last.get(&host_key) {
                    let elapsed = last.elapsed();
                    if elapsed < rate_limit {
                        std::thread::sleep(rate_limit.checked_sub(elapsed).unwrap_or_default());
                    }
                }
                domain_last.insert(host_key, std::time::Instant::now());
            }
        }

        let health = check_url(&client, *item_id, url);

        match health.http_status {
            Some(s) if (200..300).contains(&s) => {
                if health.redirect_count > 0 {
                    redirected += 1;
                } else {
                    healthy += 1;
                }
            }
            Some(s) if (400..500).contains(&s) => dead += 1,
            Some(s) if s >= 500 => server_error += 1,
            Some(_) => healthy += 1,
            None => conn_error += 1,
        }

        db.upsert_link_health(&health)
            .with_context(|| format!("saving link health for {item_id}"))?;
    }

    // Summary.
    println!("\nLink health check complete:");
    println!("  ✓ Healthy (2xx):      {healthy}");
    println!("  → Redirected (→2xx):  {redirected}");
    println!("  ✗ Dead (4xx):         {dead}");
    println!("  ! Server error (5xx): {server_error}");
    println!("  ⚠ Connection error:   {conn_error}");

    // List dead/unhealthy links.
    let unhealthy = db
        .list_unhealthy_links()
        .context("listing unhealthy links")?;

    if !unhealthy.is_empty() {
        println!("\nDead / errored links:");
        for (lh, title) in &unhealthy {
            let status_str = lh
                .http_status
                .map_or_else(|| "ERR".into(), |s| s.to_string());
            let err_str = lh.error_message.as_deref().unwrap_or("");
            println!("  [{status_str}] {title}");
            if !err_str.is_empty() {
                println!("        {err_str}");
            }
            println!("        ID: {}", lh.content_item_id);
        }
    }

    Ok(())
}

/// Probe a single URL, following redirects manually up to 10 hops.
///
/// Tries HEAD first; falls back to GET on 405/501 or suspicious 4xx.
fn check_url(client: &reqwest::blocking::Client, item_id: Uuid, url: &str) -> LinkHealth {
    let now = OffsetDateTime::now_utc();
    let max_redirects = 10;

    let mut current_url = url.to_string();
    let mut redirect_count: i32 = 0;

    for _ in 0..=max_redirects {
        // Try HEAD first.
        let resp = match client.head(&current_url).send() {
            Ok(r) => r,
            Err(e) => {
                return LinkHealth {
                    content_item_id: item_id,
                    http_status: None,
                    final_url: Some(current_url),
                    redirect_count,
                    last_checked_at: now,
                    error_message: Some(format!("{e:#}")),
                };
            }
        };

        let status = i32::from(resp.status().as_u16());

        // Follow 3xx redirects.
        if (300..400).contains(&status) {
            if let Some(loc) = resp.headers().get("location") {
                if let Ok(loc_str) = loc.to_str() {
                    // Resolve relative redirects.
                    current_url = reqwest::Url::parse(loc_str).map_or_else(
                        |_| {
                            reqwest::Url::parse(&current_url).map_or_else(
                                |_| loc_str.to_string(),
                                |base| {
                                    base.join(loc_str)
                                        .map_or_else(|_| loc_str.to_string(), |u| u.to_string())
                                },
                            )
                        },
                        |abs| abs.to_string(),
                    );
                    redirect_count += 1;
                    continue;
                }
            }
            // No valid location header — treat as error.
            return LinkHealth {
                content_item_id: item_id,
                http_status: Some(status),
                final_url: Some(current_url),
                redirect_count,
                last_checked_at: now,
                error_message: Some("redirect without Location header".into()),
            };
        }

        // HEAD got 405/501 → retry with GET.
        if status == 405 || status == 501 {
            match client.get(&current_url).send() {
                Ok(r) => {
                    let get_status = i32::from(r.status().as_u16());
                    return LinkHealth {
                        content_item_id: item_id,
                        http_status: Some(get_status),
                        final_url: Some(current_url),
                        redirect_count,
                        last_checked_at: now,
                        error_message: None,
                    };
                }
                Err(e) => {
                    return LinkHealth {
                        content_item_id: item_id,
                        http_status: None,
                        final_url: Some(current_url),
                        redirect_count,
                        last_checked_at: now,
                        error_message: Some(format!("{e:#}")),
                    };
                }
            }
        }

        // Final result for this URL.
        return LinkHealth {
            content_item_id: item_id,
            http_status: Some(status),
            final_url: if current_url == url {
                None
            } else {
                Some(current_url)
            },
            redirect_count,
            last_checked_at: now,
            error_message: None,
        };
    }

    // Exceeded max redirects.
    LinkHealth {
        content_item_id: item_id,
        http_status: None,
        final_url: Some(current_url),
        redirect_count,
        last_checked_at: now,
        error_message: Some(format!("too many redirects (>{max_redirects})")),
    }
}

// ======================================================================
// Highlight commands
// ======================================================================

/// Dispatch highlight subcommand.
fn handle_highlight(db: &Database, action: HighlightAction) -> Result<()> {
    match action {
        HighlightAction::Add {
            source,
            text,
            note,
            color,
            tags,
        } => highlight_add(db, &source, &text, note.as_deref(), color.as_deref(), &tags),
        HighlightAction::List {
            source,
            tag,
            since,
            before,
            limit,
            format,
        } => highlight_list(
            db,
            source.as_deref(),
            tag.as_deref(),
            since.as_deref(),
            before.as_deref(),
            limit,
            &format,
        ),
        HighlightAction::Show { id } => highlight_show(db, &id),
        HighlightAction::Export {
            format,
            output,
            source,
        } => highlight_export(db, &format, output.as_deref(), source.as_deref()),
    }
}

/// Create a new highlight from a source item.
fn highlight_add(
    db: &Database,
    source_id: &str,
    text: &str,
    note: Option<&str>,
    color: Option<&str>,
    tags: &[String],
) -> Result<()> {
    let source_uuid = source_id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {source_id}"))?;

    let item = db
        .create_highlight(source_uuid, text, note, color)
        .context("failed to create highlight")?;

    // Apply tags if any.
    for tag_name in tags {
        let tag = db
            .get_or_create_tag(tag_name)
            .with_context(|| format!("failed to get/create tag: {tag_name}"))?;
        db.tag_content_item(item.id, tag.id)
            .with_context(|| format!("failed to tag highlight with: {tag_name}"))?;
    }

    println!("Highlight created: {}", item.id);
    println!("  Source: {source_uuid}");
    println!("  Text:   {}", truncate_display(text, 80));
    if let Some(n) = note {
        println!("  Note:   {}", truncate_display(n, 80));
    }
    if !tags.is_empty() {
        println!("  Tags:   {}", tags.join(", "));
    }

    Ok(())
}

/// List highlights with optional filters.
fn highlight_list(
    db: &Database,
    source: Option<&str>,
    tag: Option<&str>,
    since_str: Option<&str>,
    before_str: Option<&str>,
    limit: u32,
    format: &str,
) -> Result<()> {
    let source_id = source
        .map(str::parse::<Uuid>)
        .transpose()
        .context("invalid source UUID")?;
    let since = since_str.map(parse_date_arg).transpose()?;
    let before = before_str.map(parse_date_arg).transpose()?;

    let results = db
        .list_highlights(source_id, tag, since, before, Some(limit))
        .context("failed to list highlights")?;

    if results.is_empty() {
        println!("No highlights found.");
        return Ok(());
    }

    if format == "json" {
        let json_items: Vec<serde_json::Value> = results
            .iter()
            .map(|(item, meta)| {
                serde_json::json!({
                    "id": item.id.to_string(),
                    "source_item_id": meta.source_item_id.map(|u| u.to_string()),
                    "quote_text": meta.quote_text,
                    "note": meta.note,
                    "color": meta.color,
                    "position_start": meta.position_start,
                    "position_end": meta.position_end,
                    "created_at": item.created_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                    "url": item.url,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_items).unwrap_or_else(|_| "[]".to_owned())
        );
    } else {
        println!("{} highlight(s):\n", results.len());
        for (item, meta) in &results {
            println!(
                "  {} | {}",
                short_uuid(item.id),
                truncate_display(&meta.quote_text, 60)
            );
            if let Some(n) = &meta.note {
                println!("        note: {}", truncate_display(n, 60));
            }
            if let Some(color) = &meta.color {
                println!("        color: {color}");
            }
        }
    }

    Ok(())
}

/// Show a specific highlight with full details.
fn highlight_show(db: &Database, id: &str) -> Result<()> {
    let uuid = id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {id}"))?;

    let item = db
        .get_content_item(uuid)
        .context("failed to load highlight")?;
    let meta = db
        .get_highlight_meta(uuid)
        .context("failed to load highlight metadata")?;

    println!("Highlight: {}", item.id);
    println!("  URL:      {}", item.url.as_deref().unwrap_or("—"));
    println!(
        "  Source:   {}",
        meta.source_item_id
            .map_or_else(|| "—".to_owned(), |u| u.to_string())
    );
    println!("  Quote:    {}", meta.quote_text);
    if let Some(n) = &meta.note {
        println!("  Note:     {n}");
    }
    if let Some(color) = &meta.color {
        println!("  Color:    {color}");
    }
    if let Some(start) = meta.position_start {
        println!(
            "  Position: {start}..{}",
            meta.position_end.unwrap_or(start)
        );
    }
    println!(
        "  Created:  {}",
        item.created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    );

    // Show tags.
    let tags = db.tags_for_item(uuid).unwrap_or_default();
    if !tags.is_empty() {
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        println!("  Tags:     {}", names.join(", "));
    }

    // Show notes on this highlight.
    let notes = db.list_notes_for_item(uuid).unwrap_or_default();
    if !notes.is_empty() {
        println!("  Notes:");
        for note in &notes {
            println!(
                "    [{}] {}",
                short_uuid(note.id),
                truncate_display(&note.body, 60)
            );
        }
    }

    Ok(())
}

/// Export highlights to Markdown or JSON.
fn highlight_export(
    db: &Database,
    format: &str,
    output: Option<&std::path::Path>,
    source: Option<&str>,
) -> Result<()> {
    let source_id = source
        .map(str::parse::<Uuid>)
        .transpose()
        .context("invalid source UUID")?;

    let results = db
        .list_highlights(source_id, None, None, None, None)
        .context("failed to list highlights")?;

    if results.is_empty() {
        println!("No highlights to export.");
        return Ok(());
    }

    let content = if format == "json" {
        let json_items: Vec<serde_json::Value> = results
            .iter()
            .map(|(item, meta)| {
                serde_json::json!({
                    "id": item.id.to_string(),
                    "source_item_id": meta.source_item_id.map(|u| u.to_string()),
                    "quote_text": meta.quote_text,
                    "note": meta.note,
                    "color": meta.color,
                    "position_start": meta.position_start,
                    "position_end": meta.position_end,
                    "created_at": item.created_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                    "url": item.url,
                })
            })
            .collect();
        serde_json::to_string_pretty(&json_items).unwrap_or_else(|_| "[]".to_owned())
    } else {
        // Markdown export.
        use std::fmt::Write as _;
        let mut md = String::from("# Highlights\n\n");
        for (item, meta) in &results {
            let _ = writeln!(md, "## {}\n", truncate_display(&meta.quote_text, 120));
            let _ = writeln!(md, "> {}\n", meta.quote_text);
            if let Some(n) = &meta.note {
                let _ = writeln!(md, "**Note:** {n}\n");
            }
            let _ = writeln!(
                md,
                "- Source: {}",
                meta.source_item_id
                    .map_or_else(|| "—".to_owned(), |u| u.to_string())
            );
            let _ = writeln!(md, "- URL: {}", item.url.as_deref().unwrap_or("—"));
            let _ = writeln!(
                md,
                "- Created: {}\n",
                item.created_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            );
            md.push_str("---\n\n");
        }
        md
    };

    if let Some(path) = output {
        std::fs::write(path, &content)
            .with_context(|| format!("failed to write to {}", path.display()))?;
        println!(
            "Exported {} highlight(s) to {}",
            results.len(),
            path.display()
        );
    } else {
        print!("{content}");
    }

    Ok(())
}

// ======================================================================
// Note commands
// ======================================================================

/// Dispatch note subcommand.
fn handle_note(db: &Database, action: NoteAction) -> Result<()> {
    match action {
        NoteAction::Add { item, text } => note_add(db, &item, &text),
        NoteAction::List { item, format } => note_list(db, item.as_deref(), &format),
        NoteAction::Edit { id, text } => note_edit(db, &id, &text),
        NoteAction::Delete { id } => note_delete(db, &id),
    }
}

/// Add a note to a content item.
fn note_add(db: &Database, item_id: &str, text: &str) -> Result<()> {
    let uuid = item_id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {item_id}"))?;

    // Verify the content item exists.
    db.get_content_item(uuid)
        .with_context(|| format!("content item not found: {uuid}"))?;

    let now = OffsetDateTime::now_utc();
    let note = pergamon_core::model::Note {
        id: Uuid::new_v4(),
        content_item_id: uuid,
        body: text.to_owned(),
        created_at: now,
        updated_at: now,
    };

    db.insert_note(&note).context("failed to insert note")?;

    println!("Note added: {}", note.id);
    println!("  Item: {uuid}");
    println!("  Body: {}", truncate_display(text, 80));

    Ok(())
}

/// List notes, optionally filtered by content item.
fn note_list(db: &Database, item_id: Option<&str>, format: &str) -> Result<()> {
    let notes = if let Some(id_str) = item_id {
        let uuid = id_str
            .parse::<Uuid>()
            .with_context(|| format!("invalid UUID: {id_str}"))?;
        db.list_notes_for_item(uuid)
            .context("failed to list notes")?
    } else {
        db.list_all_notes().context("failed to list all notes")?
    };

    if notes.is_empty() {
        println!("No notes found.");
        return Ok(());
    }

    if format == "json" {
        let json_items: Vec<serde_json::Value> = notes
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id.to_string(),
                    "content_item_id": n.content_item_id.to_string(),
                    "body": n.body,
                    "created_at": n.created_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                    "updated_at": n.updated_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_items).unwrap_or_else(|_| "[]".to_owned())
        );
    } else {
        println!("{} note(s):\n", notes.len());
        for note in &notes {
            println!(
                "  {} | item {} | {}",
                short_uuid(note.id),
                short_uuid(note.content_item_id),
                truncate_display(&note.body, 60),
            );
        }
    }

    Ok(())
}

/// Edit a note's body text.
fn note_edit(db: &Database, id: &str, text: &str) -> Result<()> {
    let uuid = id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {id}"))?;

    db.update_note(uuid, text)
        .context("failed to update note")?;

    println!("Note updated: {uuid}");

    Ok(())
}

/// Delete a note.
fn note_delete(db: &Database, id: &str) -> Result<()> {
    let uuid = id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {id}"))?;

    let deleted = db.delete_note(uuid).context("failed to delete note")?;

    if deleted {
        println!("Note deleted: {uuid}");
    } else {
        println!("Note not found: {uuid}");
    }

    Ok(())
}

// ------------------------------------------------------------------
// Review (FSRS spaced repetition)
// ------------------------------------------------------------------

fn handle_review(db: &Database, action: ReviewAction) -> Result<()> {
    match action {
        ReviewAction::Enable { id } => review_enable(db, &id),
        ReviewAction::Disable { id } => review_disable(db, &id),
        ReviewAction::Due { limit, format } => review_due(db, limit, &format),
        ReviewAction::Stats { format, tui } => review_stats_cmd(db, format, tui),
        ReviewAction::Start { limit } => review_start(db, limit),
    }
}

/// Enable spaced repetition for a highlight.
fn review_enable(db: &Database, item_id: &str) -> Result<()> {
    let uuid = item_id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {item_id}"))?;

    // Verify it's a highlight.
    let item = db
        .get_content_item(uuid)
        .with_context(|| format!("content item not found: {uuid}"))?;
    if item.content_type != ContentType::Highlight {
        bail!(
            "item {uuid} is a {} — only highlights can be reviewed",
            item.content_type
        );
    }

    // Check if already enabled.
    if let Some(existing) = db
        .get_review_card_for_item(uuid)
        .context("checking existing review card")?
    {
        println!(
            "Review already enabled for {uuid} (card {})",
            short_uuid(existing.id)
        );
        return Ok(());
    }

    let now = OffsetDateTime::now_utc();
    let card = pergamon_core::model::ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: uuid,
        state: pergamon_core::fsrs::CardState::New,
        stability: None,
        difficulty: None,
        due_at: now,
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now,
        updated_at: now,
    };

    db.insert_review_card(&card)
        .context("failed to create review card")?;

    println!("Review enabled for highlight {}", short_uuid(uuid));
    println!("  Card: {}", card.id);

    Ok(())
}

/// Disable spaced repetition for a highlight.
fn review_disable(db: &Database, item_id: &str) -> Result<()> {
    let uuid = item_id
        .parse::<Uuid>()
        .with_context(|| format!("invalid UUID: {item_id}"))?;

    let deleted = db
        .delete_review_card_for_item(uuid)
        .context("failed to delete review card")?;

    if deleted {
        println!("Review disabled for highlight {}", short_uuid(uuid));
    } else {
        println!("No review card found for {}", short_uuid(uuid));
    }

    Ok(())
}

/// List due review cards.
fn review_due(db: &Database, limit: usize, format: &str) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let cards = db
        .list_due_review_cards(now)
        .context("failed to list due cards")?;

    let display: Vec<_> = cards.into_iter().take(limit).collect();

    if display.is_empty() {
        println!("No cards due for review.");
        return Ok(());
    }

    if format == "json" {
        let json_items: Vec<serde_json::Value> = display
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id.to_string(),
                    "content_item_id": c.content_item_id.to_string(),
                    "state": c.state.as_str(),
                    "stability": c.stability,
                    "difficulty": c.difficulty,
                    "due_at": c.due_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                    "review_count": c.review_count,
                    "lapse_count": c.lapse_count,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_items).unwrap_or_else(|_| "[]".to_owned())
        );
    } else {
        println!("{} card(s) due:\n", display.len());
        for card in &display {
            let highlight = db.get_content_item(card.content_item_id).ok();
            let title = highlight.as_ref().map_or_else(
                || "(unknown)".to_owned(),
                |h| truncate_display(&h.title, 50),
            );
            println!(
                "  {} | {} | {} reviews | {}",
                short_uuid(card.id),
                card.state,
                card.review_count,
                title,
            );
        }
    }

    Ok(())
}

/// Route top-level `stats` subcommands.
fn handle_stats(db: &Database, action: &StatsAction) -> Result<()> {
    match action {
        StatsAction::Review { format, tui } => review_stats_cmd(db, *format, *tui),
    }
}

/// Show aggregated review statistics.
fn review_stats_cmd(db: &Database, format: OutputFormat, tui: bool) -> Result<()> {
    if tui {
        return tui::run_stats_tui(db);
    }
    let now = OffsetDateTime::now_utc();
    let report = db
        .review_stats_report(now)
        .context("failed to get review stats")?;

    if format == OutputFormat::Json {
        let json = serde_json::to_string_pretty(&report).context("failed to serialize stats")?;
        println!("{json}");
        return Ok(());
    }

    let stats = &report.stats;

    println!("Review Statistics");
    println!("─────────────────");
    println!("Total cards:     {}", stats.total_cards);
    println!("  New:           {}", stats.new_count);
    println!("  Learning:      {}", stats.learning_count);
    println!("  Review:        {}", stats.review_count);
    println!("  Relearning:    {}", stats.relearning_count);
    println!("Due now:         {}", stats.due_count);
    println!("Today:           {}", stats.reviews_today);
    println!("Total reviews:   {}", stats.total_reviews);
    println!("Retention:       {:.1}%", stats.observed_retention * 100.0);

    println!();
    println!("Streaks");
    println!("───────");
    println!(
        "Current:         {} day{}",
        stats.current_streak,
        if stats.current_streak == 1 { "" } else { "s" }
    );
    println!(
        "Longest:         {} day{}",
        stats.longest_streak,
        if stats.longest_streak == 1 { "" } else { "s" }
    );

    if !report.source_breakdown.is_empty() {
        println!();
        println!("Source Breakdown");
        println!("───────────────");
        for src in &report.source_breakdown {
            println!("  {:<14} {}", format!("{}:", src.origin), src.count);
        }
    }

    if !report.daily_history.is_empty() {
        println!();
        println!("Last 7 Days");
        println!("───────────");
        let recent: Vec<_> = report.daily_history.iter().rev().take(7).collect();
        let max_reviews = recent.iter().map(|d| d.reviews).max().unwrap_or(1).max(1);
        for day in recent.into_iter().rev() {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let bar_len = ((day.reviews as f64 / max_reviews as f64) * 20.0) as usize;
            let bar: String = "█".repeat(bar_len);
            println!("  {} {:>3}  {}", day.date, day.reviews, bar);
        }
    }

    if !report.weekly_history.is_empty() {
        println!();
        println!("Weekly Trend");
        println!("────────────");
        let max_reviews = report
            .weekly_history
            .iter()
            .map(|w| w.reviews)
            .max()
            .unwrap_or(1)
            .max(1);
        for week in &report.weekly_history {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let bar_len = ((week.reviews as f64 / max_reviews as f64) * 20.0) as usize;
            let bar: String = "█".repeat(bar_len);
            println!("  {} {:>3}  {}", week.week, week.reviews, bar);
        }
    }

    Ok(())
}

/// Start an interactive TUI review session.
fn review_start(db: &Database, limit: Option<usize>) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let mut cards = db
        .list_due_review_cards(now)
        .context("failed to list due cards")?;

    if let Some(max) = limit {
        cards.truncate(max);
    }

    if cards.is_empty() {
        println!("No cards due for review. Great work!");
        return Ok(());
    }

    println!("{} card(s) due — launching review session...", cards.len());

    tui::run_review(db, cards).context("review session failed")?;

    Ok(())
}

/// Truncate a string for display, adding "…" if cut.
fn truncate_display(text: &str, max: usize) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.len() <= max {
        first_line.to_owned()
    } else {
        let truncated: String = first_line.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Format a UUID as its first 8 characters.
fn short_uuid(id: Uuid) -> String {
    id.to_string()[..8].to_owned()
}

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
                } else if let tui::app::FilterMode::SmartCollection(id, ref name) = app.filter {
                    let items = db
                        .list_smart_collection_items(id)
                        .with_context(|| format!("failed to run smart collection '{name}'"))?;
                    app.set_status(format!("🔍 {name}: {} result(s)", items.len()));
                    app.items = items;
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
    } else if let tui::app::FilterMode::SmartCollection(id, _) = app.filter {
        app.total_count = db
            .count_smart_collection_items(id)
            .context("failed to count smart collection items")? as u64;
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
    let notes = db.list_all_notes().context("listing notes")?;
    let review_cards = db.list_all_review_cards().context("listing review cards")?;
    let review_logs = db.list_all_review_logs().context("listing review logs")?;
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
    write_json_entry(&mut zip, &opts, "notes.json", &notes)?;
    write_json_entry(&mut zip, &opts, "review_cards.json", &review_cards)?;
    write_json_entry(&mut zip, &opts, "review_logs.json", &review_logs)?;
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
        + notes.len()
        + review_cards.len()
        + review_logs.len()
        + content_item_tags.len()
        + collection_items.len();

    println!("Backup written to {}", output.display());
    println!(
        "  {} feeds, {} items, {} tags, {} collections, {} notes, {} review cards ({total} records total)",
        feeds.len(),
        content_items.len(),
        tags.len(),
        collections.len(),
        notes.len(),
        review_cards.len(),
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
    use pergamon_core::model::{
        BookmarkMeta, Collection, HighlightMeta, Note as NoteModel, ReviewCard as ReviewCardModel,
        ReviewLog as ReviewLogModel, Tag,
    };
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
    let notes: Vec<NoteModel> = read_json_entry(&mut archive, "notes.json").unwrap_or_default();
    let review_cards: Vec<ReviewCardModel> =
        read_json_entry(&mut archive, "review_cards.json").unwrap_or_default();
    let review_logs: Vec<ReviewLogModel> =
        read_json_entry(&mut archive, "review_logs.json").unwrap_or_default();
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
        &notes,
        &review_cards,
        &review_logs,
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
        + notes.len()
        + review_cards.len()
        + review_logs.len()
        + content_item_tags.len()
        + collection_items.len();

    println!("Backup restored from {}", path.display());
    println!(
        "  {} feeds, {} items, {} tags, {} collections, {} notes, {} review cards ({total} records total)",
        feeds.len(),
        content_items.len(),
        tags.len(),
        collections.len(),
        notes.len(),
        review_cards.len(),
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
