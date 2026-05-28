//! # pergamon CLI
//!
//! Command-line interface for pergamon — unified personal information
//! system. Combines RSS reader, read-later, bookmark manager, and
//! knowledge retention engine into a single CLI + ratatui TUI.

mod tui;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use pergamon_core::content_type::ContentType;
use pergamon_core::model::{ContentItem, Feed, FeedFolder, FeedItemMeta};
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
        /// URL to save.
        url: String,
    },
    /// Open the TUI inbox / reader.
    Read,
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
        Command::Save { url } => save_url(&db, &url),
        Command::Read => run_tui(&db),
        Command::Import { action } => handle_import(&db, action),
        Command::Export { action } => handle_export(&db, action),
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
    }
}

/// Dispatch export subcommand.
fn handle_export(db: &Database, action: ExportAction) -> Result<()> {
    match action {
        ExportAction::Opml { output } => export_opml(db, output.as_deref()),
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

/// Save a URL as an article: fetch → extract → store.
fn save_url(db: &Database, url: &str) -> Result<()> {
    let client = http_client()?;
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to fetch {url}"))?;

    let final_url = response.url().to_string();

    if !response.status().is_success() {
        bail!("HTTP {} for {url}", response.status());
    }

    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response from {url}"))?;

    let now = OffsetDateTime::now_utc();

    // Try full article extraction; fall back to metadata-only.
    let (title, author, content_text, excerpt, published_at) =
        if let Ok(article) = pergamon_extract::extract_article(&bytes, &final_url) {
            (
                article.title.unwrap_or_else(|| final_url.clone()),
                article.author,
                Some(article.content_text),
                article.excerpt,
                article.published_at,
            )
        } else {
            // Fall back to metadata extraction only.
            let html = String::from_utf8_lossy(&bytes);
            let meta = pergamon_extract::extract_metadata(&html);
            (
                meta.title.unwrap_or_else(|| final_url.clone()),
                meta.author,
                None,
                meta.description,
                None,
            )
        };

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some(final_url),
        title,
        author,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text,
        excerpt,
        published_at,
        created_at: now,
        updated_at: now,
    };

    db.insert_content_item(&item)
        .context("failed to save article")?;

    println!("Saved: {} [{}]", item.title, item.id);
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
                let filter = tui::event::build_filter(&app.filter);
                app.items = db
                    .list_content_items_filtered(&filter, Some(ITEM_LIMIT), None)
                    .context("failed to reload content items")?;
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
    let current_filter = tui::event::build_filter(&app.filter);
    app.total_count = db
        .count_content_items_filtered(&current_filter)
        .context("failed to count items")?;

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
