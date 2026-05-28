//! Round-trip integration tests for the pergamon storage layer.
//!
//! These tests verify that every entity can be inserted, queried, and returned
//! with all fields intact. They also validate FTS5 search behaviour, tag/collection
//! associations, and extension-table round-trips.

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{
    BookmarkMeta, Collection, ContentItem, Feed, FeedItemMeta, HighlightMeta, Tag,
};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::Database;
use pergamon_storage::SearchFilter;
use time::OffsetDateTime;
use uuid::Uuid;

/// Helper: create an in-memory database.
fn test_db() -> Database {
    Database::open_in_memory().unwrap_or_else(|e| unreachable!("failed to open in-memory DB: {e}"))
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

// ======================================================================
// Feed round-trip
// ======================================================================

#[test]
fn feed_insert_and_get() {
    let db = test_db();
    let feed = Feed {
        id: Uuid::new_v4(),
        title: "Rust Blog".to_owned(),
        url: "https://blog.rust-lang.org/feed.xml".to_owned(),
        site_url: Some("https://blog.rust-lang.org".to_owned()),
        description: Some("The official Rust blog".to_owned()),
        etag: None,
        last_modified_header: None,
        error_count: 0,
        last_error: None,
        last_fetched_at: Some(now()),
        folder_id: None,
        created_at: now(),
        updated_at: now(),
    };

    db.insert_feed(&feed)
        .unwrap_or_else(|e| unreachable!("insert_feed failed: {e}"));
    let got = db
        .get_feed(feed.id)
        .unwrap_or_else(|e| unreachable!("get_feed failed: {e}"));

    assert_eq!(got.id, feed.id);
    assert_eq!(got.title, feed.title);
    assert_eq!(got.url, feed.url);
    assert_eq!(got.site_url, feed.site_url);
    assert_eq!(got.description, feed.description);
    assert!(got.last_fetched_at.is_some());
}

// ======================================================================
// Content item round-trip
// ======================================================================

#[test]
fn content_item_insert_and_get() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/article".to_owned()),
        title: "Test Article".to_owned(),
        author: Some("Jane Doe".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("This is the full article text for FTS testing.".to_owned()),
        excerpt: Some("A short excerpt.".to_owned()),
        published_at: Some(now()),
        created_at: now(),
        updated_at: now(),
    };

    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));
    let got = db
        .get_content_item(item.id)
        .unwrap_or_else(|e| unreachable!("get_content_item failed: {e}"));

    assert_eq!(got.id, item.id);
    assert_eq!(got.url, item.url);
    assert_eq!(got.title, item.title);
    assert_eq!(got.author, item.author);
    assert_eq!(got.content_type, item.content_type);
    assert_eq!(got.status, item.status);
    assert_eq!(got.content_text, item.content_text);
    assert_eq!(got.excerpt, item.excerpt);
    assert!(got.published_at.is_some());
}

// ======================================================================
// All content types round-trip
// ======================================================================

#[test]
fn all_content_types_round_trip() {
    let db = test_db();
    let types = [
        ContentType::FeedItem,
        ContentType::Article,
        ContentType::Bookmark,
        ContentType::Highlight,
        ContentType::Pdf,
        ContentType::PodcastEpisode,
    ];

    for ct in types {
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(format!("https://example.com/{ct}")),
            title: format!("Item of type {ct}"),
            author: None,
            content_type: ct,
            status: DocumentStatus::Inbox,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: now(),
            updated_at: now(),
        };
        db.insert_content_item(&item)
            .unwrap_or_else(|e| unreachable!("insert failed for {ct}: {e}"));
        let got = db
            .get_content_item(item.id)
            .unwrap_or_else(|e| unreachable!("get failed for {ct}: {e}"));
        assert_eq!(got.content_type, ct);
    }
}

// ======================================================================
// List with filters
// ======================================================================

#[test]
fn list_content_items_with_filters() {
    let db = test_db();

    let article = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "An Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Reading,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    let bookmark = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "A Bookmark".to_owned(),
        author: None,
        content_type: ContentType::Bookmark,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };

    db.insert_content_item(&article)
        .unwrap_or_else(|e| unreachable!("insert article failed: {e}"));
    db.insert_content_item(&bookmark)
        .unwrap_or_else(|e| unreachable!("insert bookmark failed: {e}"));

    let articles = db
        .list_content_items(Some(ContentType::Article), None, None, None)
        .unwrap_or_else(|e| unreachable!("list articles failed: {e}"));
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0].title, "An Article");

    let reading = db
        .list_content_items(None, Some(DocumentStatus::Reading), None, None)
        .unwrap_or_else(|e| unreachable!("list reading failed: {e}"));
    assert_eq!(reading.len(), 1);
    assert_eq!(reading[0].title, "An Article");

    let all = db
        .list_content_items(None, None, None, None)
        .unwrap_or_else(|e| unreachable!("list all failed: {e}"));
    assert_eq!(all.len(), 2);
}

// ======================================================================
// Feed item meta round-trip
// ======================================================================

#[test]
fn feed_item_meta_round_trip() {
    let db = test_db();

    let feed = Feed {
        id: Uuid::new_v4(),
        title: "Test Feed".to_owned(),
        url: "https://feed.example.com/rss".to_owned(),
        site_url: None,
        description: None,
        etag: None,
        last_modified_header: None,
        error_count: 0,
        last_error: None,
        last_fetched_at: None,
        folder_id: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_feed(&feed)
        .unwrap_or_else(|e| unreachable!("insert_feed failed: {e}"));

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://feed.example.com/post/1".to_owned()),
        title: "Feed Post 1".to_owned(),
        author: Some("Alice".to_owned()),
        content_type: ContentType::FeedItem,
        status: DocumentStatus::Inbox,
        content_text: Some("Feed post body text".to_owned()),
        excerpt: None,
        published_at: Some(now()),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let meta = FeedItemMeta {
        content_item_id: item.id,
        feed_id: feed.id,
        guid: Some("urn:uuid:12345".to_owned()),
        summary: Some("A short summary from the feed.".to_owned()),
    };
    db.insert_feed_item_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert_feed_item_meta failed: {e}"));

    let got = db
        .get_feed_item_meta(item.id)
        .unwrap_or_else(|e| unreachable!("get_feed_item_meta failed: {e}"));
    assert_eq!(got.content_item_id, item.id);
    assert_eq!(got.feed_id, feed.id);
    assert_eq!(got.guid, meta.guid);
    assert_eq!(got.summary, meta.summary);
}

// ======================================================================
// Bookmark meta round-trip
// ======================================================================

#[test]
fn bookmark_meta_round_trip() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/page".to_owned()),
        title: "Bookmarked Page".to_owned(),
        author: None,
        content_type: ContentType::Bookmark,
        status: DocumentStatus::Reference,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let meta = BookmarkMeta {
        content_item_id: item.id,
        original_url: Some("https://example.com/page?utm_source=twitter".to_owned()),
        saved_from: Some("browser".to_owned()),
        thumbnail_url: Some("https://example.com/thumb.jpg".to_owned()),
        description: Some("An interesting page".to_owned()),
    };
    db.insert_bookmark_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert_bookmark_meta failed: {e}"));

    let got = db
        .get_bookmark_meta(item.id)
        .unwrap_or_else(|e| unreachable!("get_bookmark_meta failed: {e}"));
    assert_eq!(got.content_item_id, item.id);
    assert_eq!(got.original_url, meta.original_url);
    assert_eq!(got.saved_from, meta.saved_from);
    assert_eq!(got.thumbnail_url, meta.thumbnail_url);
    assert_eq!(got.description, meta.description);
}

// ======================================================================
// Highlight meta round-trip
// ======================================================================

#[test]
fn highlight_meta_round_trip() {
    let db = test_db();

    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/source-article".to_owned()),
        title: "Source Article".to_owned(),
        author: Some("Bob".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Archived,
        content_text: Some("Full article text with highlightable content.".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source failed: {e}"));

    let highlight = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Highlight from Source Article".to_owned(),
        author: None,
        content_type: ContentType::Highlight,
        status: DocumentStatus::Inbox,
        content_text: Some("highlightable content".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&highlight)
        .unwrap_or_else(|e| unreachable!("insert highlight failed: {e}"));

    let meta = HighlightMeta {
        content_item_id: highlight.id,
        source_item_id: Some(source.id),
        quote_text: "highlightable content".to_owned(),
        note: Some("This is important.".to_owned()),
        position_start: Some(30),
        position_end: Some(51),
        color: Some("yellow".to_owned()),
    };
    db.insert_highlight_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert_highlight_meta failed: {e}"));

    let got = db
        .get_highlight_meta(highlight.id)
        .unwrap_or_else(|e| unreachable!("get_highlight_meta failed: {e}"));
    assert_eq!(got.content_item_id, highlight.id);
    assert_eq!(got.source_item_id, Some(source.id));
    assert_eq!(got.quote_text, meta.quote_text);
    assert_eq!(got.note, meta.note);
    assert_eq!(got.position_start, meta.position_start);
    assert_eq!(got.position_end, meta.position_end);
    assert_eq!(got.color, meta.color);
}

// ======================================================================
// Tags round-trip and association
// ======================================================================

#[test]
fn tag_insert_get_and_associate() {
    let db = test_db();

    let tag = Tag {
        id: Uuid::new_v4(),
        name: "rust".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&tag)
        .unwrap_or_else(|e| unreachable!("insert_tag failed: {e}"));

    let got = db
        .get_tag(tag.id)
        .unwrap_or_else(|e| unreachable!("get_tag failed: {e}"));
    assert_eq!(got.id, tag.id);
    assert_eq!(got.name, tag.name);

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Rust Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("An article about Rust.".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    db.tag_content_item(item.id, tag.id)
        .unwrap_or_else(|e| unreachable!("tag_content_item failed: {e}"));

    // Tagging again should be idempotent
    db.tag_content_item(item.id, tag.id)
        .unwrap_or_else(|e| unreachable!("duplicate tag_content_item failed: {e}"));
}

// ======================================================================
// Collections round-trip and association
// ======================================================================

#[test]
fn collection_insert_get_and_associate() {
    let db = test_db();

    let parent = Collection {
        id: Uuid::new_v4(),
        name: "Tech".to_owned(),
        parent_id: None,
        sort_order: 0,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_collection(&parent)
        .unwrap_or_else(|e| unreachable!("insert parent collection failed: {e}"));

    let child = Collection {
        id: Uuid::new_v4(),
        name: "Rust".to_owned(),
        parent_id: Some(parent.id),
        sort_order: 1,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_collection(&child)
        .unwrap_or_else(|e| unreachable!("insert child collection failed: {e}"));

    let got_parent = db
        .get_collection(parent.id)
        .unwrap_or_else(|e| unreachable!("get parent collection failed: {e}"));
    assert_eq!(got_parent.name, "Tech");
    assert!(got_parent.parent_id.is_none());

    let got_child = db
        .get_collection(child.id)
        .unwrap_or_else(|e| unreachable!("get child collection failed: {e}"));
    assert_eq!(got_child.name, "Rust");
    assert_eq!(got_child.parent_id, Some(parent.id));

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Rust Tips".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    db.add_to_collection(item.id, child.id, 0)
        .unwrap_or_else(|e| unreachable!("add_to_collection failed: {e}"));
}

// ======================================================================
// FTS5 search
// ======================================================================

#[test]
fn fts_search_by_title() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Understanding Borrow Checking in Rust".to_owned(),
        author: Some("Alice".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some(
            "The borrow checker is a key part of Rust's safety guarantees.".to_owned(),
        ),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let results = db
        .search("borrow")
        .unwrap_or_else(|e| unreachable!("search failed: {e}"));
    assert!(!results.is_empty(), "expected at least one search result");
    assert_eq!(results[0].content_item_id, item.id);
}

#[test]
fn fts_search_by_author() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Some Article".to_owned(),
        author: Some("Archimedes".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let results = db
        .search("Archimedes")
        .unwrap_or_else(|e| unreachable!("search failed: {e}"));
    assert!(!results.is_empty());
    assert_eq!(results[0].content_item_id, item.id);
}

#[test]
fn fts_search_by_content() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Untitled".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Quantum entanglement defies classical intuition.".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let results = db
        .search("entanglement")
        .unwrap_or_else(|e| unreachable!("search failed: {e}"));
    assert!(!results.is_empty());
    assert_eq!(results[0].content_item_id, item.id);
}

#[test]
fn fts_search_by_tags() {
    let db = test_db();

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Generic Title".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let tag = Tag {
        id: Uuid::new_v4(),
        name: "distributed-systems".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&tag)
        .unwrap_or_else(|e| unreachable!("insert_tag failed: {e}"));

    db.tag_content_item(item.id, tag.id)
        .unwrap_or_else(|e| unreachable!("tag_content_item failed: {e}"));

    let results = db
        .search("distributed-systems")
        .unwrap_or_else(|e| unreachable!("search failed: {e}"));
    assert!(
        !results.is_empty(),
        "expected tag-based search to return results"
    );
    assert_eq!(results[0].content_item_id, item.id);
}

// ======================================================================
// FTS search returns no results for unrelated query
// ======================================================================

#[test]
fn fts_search_no_results() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Cooking Tips".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("How to make the perfect sourdough.".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let results = db
        .search("blockchain")
        .unwrap_or_else(|e| unreachable!("search failed: {e}"));
    assert!(
        results.is_empty(),
        "expected no search results for unrelated term"
    );
}

// ======================================================================
// Migration idempotency
// ======================================================================

#[test]
fn migrations_are_idempotent() {
    let db = test_db();
    drop(db);
    // Opening a second in-memory DB should also work (tests that migration
    // tracking doesn't crash on a fresh DB, though each in-memory DB is
    // independent).
    let _ = Database::open_in_memory().unwrap_or_else(|e| unreachable!("second open failed: {e}"));
}

// ======================================================================
// Not-found errors
// ======================================================================

#[test]
fn get_nonexistent_content_item_returns_not_found() {
    let db = test_db();
    let result = db.get_content_item(Uuid::new_v4());
    match result {
        Ok(_) => unreachable!("expected an error for missing item"),
        Err(e) => assert!(
            e.to_string().contains("not found"),
            "expected not-found error, got: {e}"
        ),
    }
}

#[test]
fn get_nonexistent_feed_returns_not_found() {
    let db = test_db();
    let result = db.get_feed(Uuid::new_v4());
    match result {
        Ok(_) => unreachable!("expected an error for missing feed"),
        Err(e) => assert!(
            e.to_string().contains("not found"),
            "expected not-found error, got: {e}"
        ),
    }
}

// ======================================================================
// Filtered FTS search (search_filtered)
// ======================================================================

/// Helper: insert a content item and return it.
fn insert_item(db: &Database, title: &str, ct: ContentType, status: DocumentStatus) -> ContentItem {
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: title.to_owned(),
        author: Some("Author".to_owned()),
        content_type: ct,
        status,
        content_text: Some(format!("Body text for {title}")),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));
    item
}

#[test]
fn fts_search_filtered_basic() {
    let db = test_db();
    insert_item(
        &db,
        "Rust Ownership Guide",
        ContentType::Article,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Cooking Pasta",
        ContentType::Article,
        DocumentStatus::Inbox,
    );

    let filter = SearchFilter::default();
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.title, "Rust Ownership Guide");
    assert!(hits[0].rank < 0.0, "BM25 rank should be negative");
}

#[test]
fn fts_search_filtered_by_type() {
    let db = test_db();
    insert_item(
        &db,
        "Rust News Feed",
        ContentType::FeedItem,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Rust Bookmark",
        ContentType::Bookmark,
        DocumentStatus::Inbox,
    );

    let filter = SearchFilter {
        content_type: Some(ContentType::Bookmark),
        ..SearchFilter::default()
    };
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.content_type, ContentType::Bookmark);
}

#[test]
fn fts_search_filtered_by_status() {
    let db = test_db();
    insert_item(
        &db,
        "Rust in Inbox",
        ContentType::Article,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Rust Archived",
        ContentType::Article,
        DocumentStatus::Archived,
    );

    let filter = SearchFilter {
        status: Some(DocumentStatus::Archived),
        ..SearchFilter::default()
    };
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.status, DocumentStatus::Archived);
}

#[test]
fn fts_search_filtered_by_tag() {
    let db = test_db();
    let item = insert_item(
        &db,
        "Tagged Rust Item",
        ContentType::Article,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Untagged Rust Item",
        ContentType::Article,
        DocumentStatus::Inbox,
    );

    let tag = Tag {
        id: Uuid::new_v4(),
        name: "programming".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&tag)
        .unwrap_or_else(|e| unreachable!("insert_tag failed: {e}"));
    db.tag_content_item(item.id, tag.id)
        .unwrap_or_else(|e| unreachable!("tag_content_item failed: {e}"));

    let filter = SearchFilter {
        tag_name: Some("programming".to_owned()),
        ..SearchFilter::default()
    };
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, item.id);
}

#[test]
fn fts_search_filtered_by_date() {
    let db = test_db();

    // Insert two items with different timestamps.
    let old = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Old Rust Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Rust old stuff".to_owned()),
        excerpt: None,
        published_at: Some(
            OffsetDateTime::from_unix_timestamp(1_600_000_000)
                .unwrap_or_else(|_| unreachable!("valid timestamp")),
        ),
        created_at: OffsetDateTime::from_unix_timestamp(1_600_000_000)
            .unwrap_or_else(|_| unreachable!("valid timestamp")),
        updated_at: now(),
    };
    let recent = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Recent Rust Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Rust recent stuff".to_owned()),
        excerpt: None,
        published_at: Some(now()),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&old)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));
    db.insert_content_item(&recent)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));

    // Filter: only items since 2024-01-01.
    let cutoff = OffsetDateTime::from_unix_timestamp(1_704_067_200)
        .unwrap_or_else(|_| unreachable!("valid timestamp"));
    let filter = SearchFilter {
        since: Some(cutoff),
        ..SearchFilter::default()
    };
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, recent.id);
}

#[test]
fn fts_search_filtered_combined() {
    let db = test_db();
    insert_item(
        &db,
        "Rust Article Inbox",
        ContentType::Article,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Rust Bookmark Inbox",
        ContentType::Bookmark,
        DocumentStatus::Inbox,
    );
    insert_item(
        &db,
        "Rust Article Archived",
        ContentType::Article,
        DocumentStatus::Archived,
    );

    let filter = SearchFilter {
        content_type: Some(ContentType::Article),
        status: Some(DocumentStatus::Inbox),
        ..SearchFilter::default()
    };
    let hits = db
        .search_filtered("Rust", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.title, "Rust Article Inbox");
}

#[test]
fn fts_search_filtered_with_limit() {
    let db = test_db();
    for i in 0..5 {
        insert_item(
            &db,
            &format!("Rust Item {i}"),
            ContentType::Article,
            DocumentStatus::Inbox,
        );
    }

    let filter = SearchFilter::default();
    let hits = db
        .search_filtered("Rust", &filter, Some(3))
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 3);
}

#[test]
fn fts_search_filtered_snippet_returned() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Quantum Computing Overview".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some(
            "Quantum computing uses qubits which can be in superposition of states.".to_owned(),
        ),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));

    let filter = SearchFilter::default();
    let hits = db
        .search_filtered("superposition", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert_eq!(hits.len(), 1);
    // Snippet should exist and contain the search term context.
    assert!(hits[0].snippet.is_some(), "expected a snippet");
}

#[test]
fn fts_search_filtered_no_results() {
    let db = test_db();
    insert_item(
        &db,
        "Cooking Pasta",
        ContentType::Article,
        DocumentStatus::Inbox,
    );

    let filter = SearchFilter::default();
    let hits = db
        .search_filtered("blockchain", &filter, None)
        .unwrap_or_else(|e| unreachable!("search_filtered failed: {e}"));

    assert!(hits.is_empty());
}

#[test]
fn list_tags_round_trip() {
    let db = test_db();

    // Initially empty.
    let tags = db
        .list_tags()
        .unwrap_or_else(|e| unreachable!("list_tags failed: {e}"));
    assert!(tags.is_empty());

    // Insert two tags.
    let t1 = Tag {
        id: Uuid::new_v4(),
        name: "rust".to_owned(),
        created_at: now(),
    };
    let t2 = Tag {
        id: Uuid::new_v4(),
        name: "python".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&t1)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));
    db.insert_tag(&t2)
        .unwrap_or_else(|e| unreachable!("insert failed: {e}"));

    let tags = db
        .list_tags()
        .unwrap_or_else(|e| unreachable!("list_tags failed: {e}"));
    assert_eq!(tags.len(), 2);
    let names: Vec<_> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"rust"));
    assert!(names.contains(&"python"));
}

// ======================================================================
// Backup round-trip
// ======================================================================

use pergamon_core::model::FeedFolder;

#[test]
#[allow(clippy::too_many_lines)]
fn backup_round_trip() {
    let src = test_db();

    // Populate source database with sample data across all tables.
    let folder = FeedFolder {
        id: Uuid::new_v4(),
        name: "Tech".to_owned(),
        parent_id: None,
        created_at: now(),
        updated_at: now(),
    };
    src.insert_feed_folder(&folder)
        .unwrap_or_else(|e| unreachable!("insert folder failed: {e}"));

    let feed = Feed {
        id: Uuid::new_v4(),
        url: "https://example.com/feed.xml".to_owned(),
        title: "Example Feed".to_owned(),
        site_url: Some("https://example.com".to_owned()),
        description: Some("An example feed".to_owned()),
        folder_id: Some(folder.id),
        last_fetched_at: None,
        created_at: now(),
        updated_at: now(),
        etag: Some("W/\"abc\"".to_owned()),
        last_modified_header: None,
        error_count: 0,
        last_error: None,
    };
    src.insert_feed(&feed)
        .unwrap_or_else(|e| unreachable!("insert feed failed: {e}"));

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/post".to_owned()),
        title: "Test Post".to_owned(),
        author: Some("Author".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Post body".to_owned()),
        excerpt: Some("Post excerpt".to_owned()),
        published_at: Some(now()),
        created_at: now(),
        updated_at: now(),
    };
    src.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert item failed: {e}"));

    let meta = FeedItemMeta {
        content_item_id: item.id,
        feed_id: feed.id,
        guid: Some("guid-123".to_owned()),
        summary: Some("Summary".to_owned()),
    };
    src.insert_feed_item_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert meta failed: {e}"));

    let bm = BookmarkMeta {
        content_item_id: item.id,
        original_url: Some("https://example.com/original".to_owned()),
        saved_from: Some("cli".to_owned()),
        thumbnail_url: None,
        description: Some("A bookmark".to_owned()),
    };
    src.insert_bookmark_meta(&bm)
        .unwrap_or_else(|e| unreachable!("insert bookmark meta failed: {e}"));

    let hl = HighlightMeta {
        content_item_id: item.id,
        source_item_id: None,
        quote_text: "highlighted text".to_owned(),
        note: Some("my note".to_owned()),
        position_start: Some(10),
        position_end: Some(25),
        color: Some("yellow".to_owned()),
    };
    src.insert_highlight_meta(&hl)
        .unwrap_or_else(|e| unreachable!("insert highlight meta failed: {e}"));

    let tag = Tag {
        id: Uuid::new_v4(),
        name: "testing".to_owned(),
        created_at: now(),
    };
    src.insert_tag(&tag)
        .unwrap_or_else(|e| unreachable!("insert tag failed: {e}"));
    src.tag_content_item(item.id, tag.id)
        .unwrap_or_else(|e| unreachable!("tag item failed: {e}"));

    let coll = Collection {
        id: Uuid::new_v4(),
        name: "Reading List".to_owned(),
        parent_id: None,
        sort_order: 1,
        created_at: now(),
        updated_at: now(),
    };
    src.insert_collection(&coll)
        .unwrap_or_else(|e| unreachable!("insert collection failed: {e}"));
    src.add_to_collection(item.id, coll.id, 0)
        .unwrap_or_else(|e| unreachable!("add to collection failed: {e}"));

    // Export everything from source.
    let folders = src
        .list_feed_folders()
        .unwrap_or_else(|e| unreachable!("list folders: {e}"));
    let feeds = src
        .list_feeds()
        .unwrap_or_else(|e| unreachable!("list feeds: {e}"));
    let items = src
        .list_all_content_items()
        .unwrap_or_else(|e| unreachable!("list items: {e}"));
    let tags_out = src
        .list_tags()
        .unwrap_or_else(|e| unreachable!("list tags: {e}"));
    let colls = src
        .list_collections()
        .unwrap_or_else(|e| unreachable!("list collections: {e}"));
    let fim = src
        .list_all_feed_item_meta()
        .unwrap_or_else(|e| unreachable!("list fim: {e}"));
    let bms = src
        .list_all_bookmark_meta()
        .unwrap_or_else(|e| unreachable!("list bm: {e}"));
    let hls = src
        .list_all_highlight_meta()
        .unwrap_or_else(|e| unreachable!("list hl: {e}"));
    let cit = src
        .list_all_content_item_tags()
        .unwrap_or_else(|e| unreachable!("list cit: {e}"));
    let ci = src
        .list_all_collection_items()
        .unwrap_or_else(|e| unreachable!("list ci: {e}"));

    // Restore into a fresh database.
    let dst = test_db();
    dst.restore_backup(
        &folders, &feeds, &items, &tags_out, &colls, &fim, &bms, &hls, &cit, &ci,
    )
    .unwrap_or_else(|e| unreachable!("restore failed: {e}"));

    // Verify all records are present.
    let dst_feeds = dst
        .list_feeds()
        .unwrap_or_else(|e| unreachable!("dst feeds: {e}"));
    assert_eq!(dst_feeds.len(), 1);
    assert_eq!(dst_feeds[0].title, "Example Feed");

    let dst_items = dst
        .list_all_content_items()
        .unwrap_or_else(|e| unreachable!("dst items: {e}"));
    assert_eq!(dst_items.len(), 1);
    assert_eq!(dst_items[0].title, "Test Post");

    let dst_tags = dst
        .list_tags()
        .unwrap_or_else(|e| unreachable!("dst tags: {e}"));
    assert_eq!(dst_tags.len(), 1);
    assert_eq!(dst_tags[0].name, "testing");

    let dst_cit = dst
        .list_all_content_item_tags()
        .unwrap_or_else(|e| unreachable!("dst cit: {e}"));
    assert_eq!(dst_cit.len(), 1);

    let dst_colls = dst
        .list_collections()
        .unwrap_or_else(|e| unreachable!("dst colls: {e}"));
    assert_eq!(dst_colls.len(), 1);
    assert_eq!(dst_colls[0].name, "Reading List");

    let dst_ci = dst
        .list_all_collection_items()
        .unwrap_or_else(|e| unreachable!("dst ci: {e}"));
    assert_eq!(dst_ci.len(), 1);

    // Verify FTS was rebuilt — search should find the restored item.
    let results = dst
        .search("Test Post")
        .unwrap_or_else(|e| unreachable!("search: {e}"));
    assert_eq!(results.len(), 1);
}

#[test]
fn restore_rejects_nonempty_database() {
    let db = test_db();

    // Insert a tag to make the database non-empty.
    let tag = Tag {
        id: Uuid::new_v4(),
        name: "existing".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&tag)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let result = db.restore_backup(&[], &[], &[], &[], &[], &[], &[], &[], &[], &[]);
    match result {
        Ok(()) => unreachable!("expected restore to fail on non-empty DB"),
        Err(e) => {
            let msg = format!("{e}");
            assert!(msg.contains("not empty"), "unexpected error message: {msg}");
        }
    }
}

#[test]
fn schema_version_returns_latest() {
    let db = test_db();
    let version = db
        .schema_version()
        .unwrap_or_else(|e| unreachable!("version: {e}"));
    // We have 4 migrations (V1–V4).
    assert_eq!(version, 4);
}

#[test]
fn is_empty_on_fresh_database() {
    let db = test_db();
    let empty = db
        .is_empty()
        .unwrap_or_else(|e| unreachable!("is_empty: {e}"));
    assert!(empty);
}

#[allow(clippy::unwrap_used)]
mod collection_tag_bulk {
    use super::*;

    // ======================================================================
    // Collection management tests
    // ======================================================================

    fn make_item(db: &Database, title: &str) -> ContentItem {
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(format!("https://example.com/{}", Uuid::new_v4())),
            title: title.to_owned(),
            author: None,
            content_type: ContentType::Article,
            status: DocumentStatus::Inbox,
            content_text: Some(format!("Content of {title}")),
            excerpt: None,
            published_at: None,
            created_at: now(),
            updated_at: now(),
        };
        db.insert_content_item(&item).unwrap();
        item
    }

    fn make_collection(db: &Database, name: &str, parent_id: Option<Uuid>) -> Collection {
        let coll = Collection {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            parent_id,
            sort_order: 0,
            created_at: now(),
            updated_at: now(),
        };
        db.insert_collection(&coll).unwrap();
        coll
    }

    #[test]
    fn collection_rename() {
        let db = test_db();
        let coll = make_collection(&db, "Original", None);

        db.rename_collection(coll.id, "Renamed").unwrap();
        let got = db.get_collection(coll.id).unwrap();
        assert_eq!(got.name, "Renamed");
    }

    #[test]
    fn collection_delete() {
        let db = test_db();
        let coll = make_collection(&db, "ToDelete", None);
        assert!(db.delete_collection(coll.id).unwrap());
        assert!(db.get_collection(coll.id).is_err());
    }

    #[test]
    fn collection_delete_nonexistent() {
        let db = test_db();
        assert!(!db.delete_collection(Uuid::new_v4()).unwrap());
    }

    #[test]
    fn collection_move_basic() {
        let db = test_db();
        let parent = make_collection(&db, "Parent", None);
        let child = make_collection(&db, "Child", None);

        db.move_collection(child.id, Some(parent.id)).unwrap();
        let got = db.get_collection(child.id).unwrap();
        assert_eq!(got.parent_id, Some(parent.id));
    }

    #[test]
    fn collection_move_to_root() {
        let db = test_db();
        let parent = make_collection(&db, "Parent", None);
        let child = make_collection(&db, "Child", Some(parent.id));

        db.move_collection(child.id, None).unwrap();
        let got = db.get_collection(child.id).unwrap();
        assert_eq!(got.parent_id, None);
    }

    #[test]
    fn collection_move_cycle_self() {
        let db = test_db();
        let coll = make_collection(&db, "Self", None);
        let result = db.move_collection(coll.id, Some(coll.id));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("under itself"));
    }

    #[test]
    fn collection_move_cycle_descendant() {
        let db = test_db();
        let grandparent = make_collection(&db, "Grandparent", None);
        let parent = make_collection(&db, "Parent", Some(grandparent.id));
        let child = make_collection(&db, "Child", Some(parent.id));

        // Try to move grandparent under child — should fail.
        let result = db.move_collection(grandparent.id, Some(child.id));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("descendants"));
    }

    #[test]
    fn collection_get_by_name() {
        let db = test_db();
        make_collection(&db, "MyCollection", None);

        let found = db.get_collection_by_name("mycollection").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "MyCollection");

        let not_found = db.get_collection_by_name("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn collection_add_remove_items() {
        let db = test_db();
        let coll = make_collection(&db, "Reading", None);
        let item1 = make_item(&db, "Item 1");
        let item2 = make_item(&db, "Item 2");

        db.add_to_collection(item1.id, coll.id, 0).unwrap();
        db.add_to_collection(item2.id, coll.id, 1).unwrap();

        let items = db.list_collection_items(coll.id).unwrap();
        assert_eq!(items.len(), 2);

        // Remove one.
        assert!(db.remove_from_collection(item1.id, coll.id).unwrap());
        let items = db.list_collection_items(coll.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item2.id);

        // Remove non-member returns false.
        assert!(!db.remove_from_collection(item1.id, coll.id).unwrap());
    }

    #[test]
    fn collection_delete_promotes_children() {
        let db = test_db();
        let parent = make_collection(&db, "Parent", None);
        let child = make_collection(&db, "Child", Some(parent.id));

        db.delete_collection(parent.id).unwrap();

        // Child should now have no parent (promoted).
        let got = db.get_collection(child.id).unwrap();
        assert_eq!(got.parent_id, None);
    }

    // ======================================================================
    // Tag management tests
    // ======================================================================

    #[test]
    fn tag_get_by_name() {
        let db = test_db();
        db.get_or_create_tag("rust").unwrap();

        let found = db.get_tag_by_name("Rust").unwrap(); // case-insensitive
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "rust");

        let not_found = db.get_tag_by_name("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn untag_content_item() {
        let db = test_db();
        let item = make_item(&db, "Tagged Article");
        let tag = db.get_or_create_tag("rust").unwrap();

        db.tag_content_item(item.id, tag.id).unwrap();
        let tags = db.tags_for_item(item.id).unwrap();
        assert_eq!(tags.len(), 1);

        assert!(db.untag_content_item(item.id, tag.id).unwrap());
        let tags = db.tags_for_item(item.id).unwrap();
        assert!(tags.is_empty());

        // Second untag returns false.
        assert!(!db.untag_content_item(item.id, tag.id).unwrap());
    }

    #[test]
    fn untag_refreshes_fts() {
        let db = test_db();
        let item = make_item(&db, "FTS Tag Test");
        let tag = db.get_or_create_tag("searchable").unwrap();

        db.tag_content_item(item.id, tag.id).unwrap();
        let results = db.search("searchable").unwrap();
        assert!(!results.is_empty());

        db.untag_content_item(item.id, tag.id).unwrap();
        let results = db.search("searchable").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn delete_tag() {
        let db = test_db();
        let item = make_item(&db, "Will Lose Tag");
        let tag = db.get_or_create_tag("ephemeral").unwrap();
        db.tag_content_item(item.id, tag.id).unwrap();

        assert!(db.delete_tag(tag.id).unwrap());
        let tags = db.tags_for_item(item.id).unwrap();
        assert!(tags.is_empty());
        assert!(db.get_tag_by_name("ephemeral").unwrap().is_none());
    }

    #[test]
    fn delete_tag_refreshes_fts() {
        let db = test_db();
        let item = make_item(&db, "Delete Tag FTS");
        let tag = db.get_or_create_tag("removeme").unwrap();
        db.tag_content_item(item.id, tag.id).unwrap();

        let results = db.search("removeme").unwrap();
        assert!(!results.is_empty());

        db.delete_tag(tag.id).unwrap();
        let results = db.search("removeme").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn rename_tag() {
        let db = test_db();
        let tag = db.get_or_create_tag("oldname").unwrap();
        let item = make_item(&db, "Rename Tag Test");
        db.tag_content_item(item.id, tag.id).unwrap();

        db.rename_tag(tag.id, "newname").unwrap();
        let got = db.get_tag(tag.id).unwrap();
        assert_eq!(got.name, "newname");
    }

    #[test]
    fn rename_tag_refreshes_fts() {
        let db = test_db();
        let tag = db.get_or_create_tag("original").unwrap();
        let item = make_item(&db, "Rename FTS Test");
        db.tag_content_item(item.id, tag.id).unwrap();

        // Search by old name should match.
        let results = db.search("original").unwrap();
        assert!(!results.is_empty());

        db.rename_tag(tag.id, "renamed").unwrap();

        // Search by new name should match.
        let results = db.search("renamed").unwrap();
        assert!(!results.is_empty());

        // Old name should no longer match.
        let results = db.search("original").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn list_items_by_tag() {
        let db = test_db();
        let tag = db.get_or_create_tag("tagged").unwrap();
        let item1 = make_item(&db, "Tagged 1");
        let item2 = make_item(&db, "Tagged 2");
        let _untagged = make_item(&db, "Untagged");

        db.tag_content_item(item1.id, tag.id).unwrap();
        db.tag_content_item(item2.id, tag.id).unwrap();

        let items = db.list_items_by_tag(tag.id).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn tags_for_item() {
        let db = test_db();
        let item = make_item(&db, "Multi Tag");
        let t1 = db.get_or_create_tag("alpha").unwrap();
        let t2 = db.get_or_create_tag("beta").unwrap();
        db.tag_content_item(item.id, t1.id).unwrap();
        db.tag_content_item(item.id, t2.id).unwrap();

        let tags = db.tags_for_item(item.id).unwrap();
        assert_eq!(tags.len(), 2);
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn list_tags_matching_prefix() {
        let db = test_db();
        db.get_or_create_tag("rust").unwrap();
        db.get_or_create_tag("rust-async").unwrap();
        db.get_or_create_tag("python").unwrap();

        let matches = db.list_tags_matching("rust").unwrap();
        assert_eq!(matches.len(), 2);

        let matches = db.list_tags_matching("py").unwrap();
        assert_eq!(matches.len(), 1);
    }

    // ======================================================================
    // Filter tests (tag_id, collection_id, uncollected)
    // ======================================================================

    #[test]
    fn filter_by_tag_id() {
        let db = test_db();
        let tag = db.get_or_create_tag("filterme").unwrap();
        let tagged = make_item(&db, "Has Tag");
        let _untagged = make_item(&db, "No Tag");
        db.tag_content_item(tagged.id, tag.id).unwrap();

        let filter = pergamon_storage::ContentItemFilter {
            tag_id: Some(tag.id),
            ..Default::default()
        };
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, tagged.id);
    }

    #[test]
    fn filter_by_collection_id() {
        let db = test_db();
        let coll = make_collection(&db, "FilterColl", None);
        let in_coll = make_item(&db, "In Collection");
        let _outside = make_item(&db, "Outside");
        db.add_to_collection(in_coll.id, coll.id, 0).unwrap();

        let filter = pergamon_storage::ContentItemFilter {
            collection_id: Some(coll.id),
            ..Default::default()
        };
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, in_coll.id);
    }

    #[test]
    fn filter_uncollected() {
        let db = test_db();
        let coll = make_collection(&db, "SomeColl", None);
        let in_coll = make_item(&db, "In Collection");
        let uncollected = make_item(&db, "Uncollected");
        db.add_to_collection(in_coll.id, coll.id, 0).unwrap();

        let filter = pergamon_storage::ContentItemFilter {
            uncollected: true,
            ..Default::default()
        };
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, uncollected.id);
    }

    // ======================================================================
    // Bulk operation tests
    // ======================================================================

    #[test]
    fn bulk_tag_items() {
        let db = test_db();
        let tag = db.get_or_create_tag("bulk").unwrap();
        let item1 = make_item(&db, "Bulk 1");
        let item2 = make_item(&db, "Bulk 2");
        let item3 = make_item(&db, "Bulk 3");

        let ids = vec![item1.id, item2.id, item3.id];
        let count = db.bulk_tag(&ids, tag.id).unwrap();
        assert_eq!(count, 3);

        // All should have the tag.
        for id in &ids {
            let tags = db.tags_for_item(*id).unwrap();
            assert_eq!(tags.len(), 1);
        }

        // Re-tagging should return 0 (no new associations).
        let count = db.bulk_tag(&ids, tag.id).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn bulk_add_to_collection() {
        let db = test_db();
        let coll = make_collection(&db, "Bulk Coll", None);
        let item1 = make_item(&db, "BC 1");
        let item2 = make_item(&db, "BC 2");

        let ids = vec![item1.id, item2.id];
        let count = db.bulk_add_to_collection(&ids, coll.id).unwrap();
        assert_eq!(count, 2);

        let items = db.list_collection_items(coll.id).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn bulk_archive() {
        let db = test_db();
        let item1 = make_item(&db, "Archive 1");
        let item2 = make_item(&db, "Archive 2");

        let count = db.bulk_archive(&[item1.id, item2.id]).unwrap();
        assert_eq!(count, 2);

        let got1 = db.get_content_item(item1.id).unwrap();
        let got2 = db.get_content_item(item2.id).unwrap();
        assert_eq!(got1.status, DocumentStatus::Archived);
        assert_eq!(got2.status, DocumentStatus::Archived);
    }

    #[test]
    fn bulk_discard() {
        let db = test_db();
        let item1 = make_item(&db, "Discard 1");
        let item2 = make_item(&db, "Discard 2");

        let count = db.bulk_discard(&[item1.id, item2.id]).unwrap();
        assert_eq!(count, 2);

        let got1 = db.get_content_item(item1.id).unwrap();
        let got2 = db.get_content_item(item2.id).unwrap();
        assert_eq!(got1.status, DocumentStatus::Discarded);
        assert_eq!(got2.status, DocumentStatus::Discarded);
    }

    #[test]
    fn bulk_tag_updates_fts() {
        let db = test_db();
        let tag = db.get_or_create_tag("bulkfts").unwrap();
        let item = make_item(&db, "Bulk FTS Item");

        db.bulk_tag(&[item.id], tag.id).unwrap();
        let results = db.search("bulkfts").unwrap();
        assert!(!results.is_empty());
    }
} // mod collection_tag_bulk
