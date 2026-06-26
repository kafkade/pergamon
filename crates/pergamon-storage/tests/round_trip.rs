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
        read_at: None,
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
            read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert_content_item failed: {e}"));

    let meta = BookmarkMeta {
        content_item_id: item.id,
        original_url: Some("https://example.com/page?utm_source=twitter".to_owned()),
        saved_from: Some("browser".to_owned()),
        thumbnail_url: Some("https://example.com/thumb.jpg".to_owned()),
        description: Some("An interesting page".to_owned()),
        site_name: Some("Example Site".to_owned()),
        favicon_url: Some("https://example.com/favicon.ico".to_owned()),
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        is_smart: false,
        filter_query: None,
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
        is_smart: false,
        filter_query: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        read_at: None,
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
        site_name: None,
        favicon_url: None,
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
        is_smart: false,
        filter_query: None,
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
        &folders,
        &feeds,
        &items,
        &tags_out,
        &colls,
        &fim,
        &bms,
        &hls,
        &cit,
        &ci,
        &[],
        &[],
        &[],
        &[],
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

    let result = db.restore_backup(
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    );
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
    // We have 11 migrations (V1–V11).
    assert_eq!(version, 11);
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
            read_at: None,
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
            is_smart: false,
            filter_query: None,
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
    fn reorder_collection_items_sets_order() {
        let db = test_db();
        let coll = make_collection(&db, "Ordered", None);
        let a = make_item(&db, "Alpha");
        let b = make_item(&db, "Bravo");
        let c = make_item(&db, "Charlie");

        db.add_to_collection(a.id, coll.id, 0).unwrap();
        db.add_to_collection(b.id, coll.id, 1).unwrap();
        db.add_to_collection(c.id, coll.id, 2).unwrap();

        // Reverse the order.
        db.reorder_collection_items(coll.id, &[c.id, b.id, a.id])
            .unwrap();

        let ids: Vec<Uuid> = db
            .list_collection_items(coll.id)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![c.id, b.id, a.id]);
    }

    #[test]
    fn reorder_smart_collection_rejected() {
        let db = test_db();
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Smart".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("type:article".to_owned()),
            created_at: now(),
            updated_at: now(),
        };
        db.insert_collection(&smart).unwrap();

        assert!(db.reorder_collection_items(smart.id, &[]).is_err());
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
    // Smart collection tests
    // ======================================================================

    #[test]
    fn smart_collection_create_and_query() {
        let db = test_db();

        // Create an article.
        let article = make_item(&db, "Rust Article");
        // make_item creates with ContentType::Article.

        // Create a feed_item (different content type).
        let feed_item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(format!("https://example.com/{}", Uuid::new_v4())),
            title: "Feed Post".to_owned(),
            author: None,
            content_type: ContentType::FeedItem,
            status: DocumentStatus::Inbox,
            content_text: Some("Feed content".to_owned()),
            excerpt: None,
            published_at: None,
            created_at: now(),
            updated_at: now(),
            read_at: None,
        };
        db.insert_content_item(&feed_item).unwrap();

        // Create a smart collection filtering for articles.
        let now = now();
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Articles Only".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("type:article".to_owned()),
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&smart).unwrap();

        // Query should return only the article.
        let items = db.list_smart_collection_items(smart.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, article.id);

        // Count should match.
        let count = db.count_smart_collection_items(smart.id).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn smart_collection_guards_reject_manual_ops() {
        let db = test_db();
        let item = make_item(&db, "Test Item");
        let now = now();
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Smart Guard Test".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("status:inbox".to_owned()),
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&smart).unwrap();

        // Adding to smart collection should fail.
        let err = db.add_to_collection(item.id, smart.id, 0);
        assert!(err.is_err());

        // Removing from smart collection should also fail.
        let err = db.remove_from_collection(item.id, smart.id);
        assert!(err.is_err());
    }

    #[test]
    fn smart_collection_update_filter() {
        let db = test_db();
        let now = now();
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Updatable Smart".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("type:article".to_owned()),
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&smart).unwrap();

        // Update the filter.
        db.update_smart_filter(smart.id, "type:bookmark").unwrap();

        // Verify the filter was updated.
        let got = db.get_collection(smart.id).unwrap();
        assert_eq!(got.filter_query.as_deref(), Some("type:bookmark"));
    }

    #[test]
    fn smart_collection_with_status_filter() {
        let db = test_db();
        let item = make_item(&db, "Inbox Item");
        // Default status is inbox, so this should match.

        let now = now();
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Inbox Items".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("status:inbox".to_owned()),
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&smart).unwrap();

        let items = db.list_smart_collection_items(smart.id).unwrap();
        assert!(items.iter().any(|i| i.id == item.id));
    }

    #[test]
    fn smart_collection_listed_with_is_smart_flag() {
        let db = test_db();
        let now = now();

        let manual = Collection {
            id: Uuid::new_v4(),
            name: "Manual".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: false,
            filter_query: None,
            created_at: now,
            updated_at: now,
        };
        let smart = Collection {
            id: Uuid::new_v4(),
            name: "Smart".to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: true,
            filter_query: Some("type:article".to_owned()),
            created_at: now,
            updated_at: now,
        };
        db.insert_collection(&manual).unwrap();
        db.insert_collection(&smart).unwrap();

        let colls = db.list_collections().unwrap();
        let manual_got = colls.iter().find(|c| c.name == "Manual").unwrap();
        let smart_got = colls.iter().find(|c| c.name == "Smart").unwrap();

        assert!(!manual_got.is_smart);
        assert!(manual_got.filter_query.is_none());
        assert!(smart_got.is_smart);
        assert_eq!(smart_got.filter_query.as_deref(), Some("type:article"));
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
    fn merge_tags_moves_items_and_deletes_source() {
        let db = test_db();
        let item1 = make_item(&db, "Only Source");
        let item2 = make_item(&db, "Both Tags");
        let source = db.get_or_create_tag("js").unwrap();
        let target = db.get_or_create_tag("javascript").unwrap();

        db.tag_content_item(item1.id, source.id).unwrap();
        db.tag_content_item(item2.id, source.id).unwrap();
        db.tag_content_item(item2.id, target.id).unwrap();

        db.merge_tags(source.id, target.id).unwrap();

        // Source tag is gone.
        assert!(db.get_tag_by_name("js").unwrap().is_none());
        // Both items now carry the target tag.
        let t1: Vec<String> = db
            .tags_for_item(item1.id)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(t1, vec!["javascript".to_owned()]);
        let t2: Vec<String> = db
            .tags_for_item(item2.id)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(t2, vec!["javascript".to_owned()]);
    }

    #[test]
    fn merge_tags_refreshes_fts() {
        let db = test_db();
        let item = make_item(&db, "Merge FTS Test");
        let source = db.get_or_create_tag("sourcetag").unwrap();
        let target = db.get_or_create_tag("targettag").unwrap();
        db.tag_content_item(item.id, source.id).unwrap();

        db.merge_tags(source.id, target.id).unwrap();

        // Item is now findable by the target tag, not the source tag.
        assert!(!db.search("targettag").unwrap().is_empty());
        assert!(db.search("sourcetag").unwrap().is_empty());
    }

    #[test]
    fn merge_tags_same_tag_is_noop() {
        let db = test_db();
        let tag = db.get_or_create_tag("solo").unwrap();
        let item = make_item(&db, "Solo Item");
        db.tag_content_item(item.id, tag.id).unwrap();

        db.merge_tags(tag.id, tag.id).unwrap();
        assert!(db.get_tag_by_name("solo").unwrap().is_some());
        assert_eq!(db.tags_for_item(item.id).unwrap().len(), 1);
    }

    #[test]
    fn merge_tags_missing_tag_errors() {
        let db = test_db();
        let tag = db.get_or_create_tag("real").unwrap();
        assert!(db.merge_tags(Uuid::new_v4(), tag.id).is_err());
        assert!(db.merge_tags(tag.id, Uuid::new_v4()).is_err());
    }

    #[test]
    fn link_health_round_trip() {
        use pergamon_core::model::LinkHealth;

        let db = test_db();
        let item = make_item(&db, "Health Item");

        // No record yet.
        assert!(db.get_link_health(item.id).unwrap().is_none());

        let health = LinkHealth {
            content_item_id: item.id,
            http_status: Some(404),
            final_url: Some("https://example.com/gone".to_owned()),
            redirect_count: 1,
            last_checked_at: now(),
            error_message: None,
        };
        db.upsert_link_health(&health).unwrap();

        let got = db.get_link_health(item.id).unwrap().unwrap();
        assert_eq!(got.http_status, Some(404));
        assert_eq!(got.final_url.as_deref(), Some("https://example.com/gone"));
        assert_eq!(got.redirect_count, 1);
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

    #[test]
    fn sort_by_title_ascending() {
        let db = test_db();
        make_item(&db, "Charlie");
        make_item(&db, "alpha");
        make_item(&db, "Bravo");

        let filter = pergamon_storage::ContentItemFilter {
            sort: pergamon_storage::ContentItemSort::TitleAsc,
            ..Default::default()
        };
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn sort_default_is_created_desc() {
        let db = test_db();
        let first = make_item(&db, "First");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = make_item(&db, "Second");

        let filter = pergamon_storage::ContentItemFilter::default();
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        // Newest first.
        assert_eq!(items[0].id, second.id);
        assert_eq!(items[1].id, first.id);
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

// ======================================================================
// Note CRUD tests
// ======================================================================

#[test]
fn note_insert_and_get() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/article".to_owned()),
        title: "Test Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Article body text for testing notes".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert item: {e}"));

    let note = pergamon_core::model::Note {
        id: Uuid::new_v4(),
        content_item_id: item.id,
        body: "This is a great article".to_owned(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_note(&note)
        .unwrap_or_else(|e| unreachable!("insert note: {e}"));

    let got = db
        .get_note(note.id)
        .unwrap_or_else(|e| unreachable!("get note: {e}"));
    assert_eq!(got.id, note.id);
    assert_eq!(got.content_item_id, item.id);
    assert_eq!(got.body, "This is a great article");
}

#[test]
fn note_list_for_item() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/list-notes".to_owned()),
        title: "Note List Test".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    for i in 0..3 {
        let n = pergamon_core::model::Note {
            id: Uuid::new_v4(),
            content_item_id: item.id,
            body: format!("Note {i}"),
            created_at: now(),
            updated_at: now(),
        };
        db.insert_note(&n)
            .unwrap_or_else(|e| unreachable!("insert note {i}: {e}"));
    }

    let notes = db
        .list_notes_for_item(item.id)
        .unwrap_or_else(|e| unreachable!("list: {e}"));
    assert_eq!(notes.len(), 3);
}

#[test]
fn note_update_and_delete() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/note-update".to_owned()),
        title: "Note Update Test".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let note = pergamon_core::model::Note {
        id: Uuid::new_v4(),
        content_item_id: item.id,
        body: "Original".to_owned(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_note(&note)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    // Update.
    db.update_note(note.id, "Updated body")
        .unwrap_or_else(|e| unreachable!("update: {e}"));
    let got = db
        .get_note(note.id)
        .unwrap_or_else(|e| unreachable!("get: {e}"));
    assert_eq!(got.body, "Updated body");

    // Delete.
    let deleted = db
        .delete_note(note.id)
        .unwrap_or_else(|e| unreachable!("delete: {e}"));
    assert!(deleted);

    // Should be gone.
    assert!(db.get_note(note.id).is_err());
}

#[test]
fn notes_cascade_on_content_item_delete() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/cascade".to_owned()),
        title: "Cascade Test".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let note = pergamon_core::model::Note {
        id: Uuid::new_v4(),
        content_item_id: item.id,
        body: "Should be deleted with item".to_owned(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_note(&note)
        .unwrap_or_else(|e| unreachable!("insert note: {e}"));

    // Delete the content item.
    db.delete_content_item(item.id)
        .unwrap_or_else(|e| unreachable!("delete item: {e}"));

    // Note should be gone via CASCADE.
    assert!(db.get_note(note.id).is_err());
}

// ======================================================================
// Highlight listing and creation tests
// ======================================================================

#[test]
fn create_highlight_with_auto_position() {
    let db = test_db();
    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/source".to_owned()),
        title: "Source Article".to_owned(),
        author: Some("Author".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("The quick brown fox jumps over the lazy dog".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source: {e}"));

    let hl = db
        .create_highlight(source.id, "brown fox", Some("great phrase"), Some("yellow"))
        .unwrap_or_else(|e| unreachable!("create highlight: {e}"));

    // Verify content item.
    assert_eq!(hl.content_type, ContentType::Highlight);
    assert_eq!(hl.content_text.as_deref(), Some("brown fox"));

    // Verify metadata.
    let meta = db
        .get_highlight_meta(hl.id)
        .unwrap_or_else(|e| unreachable!("get meta: {e}"));
    assert_eq!(meta.source_item_id, Some(source.id));
    assert_eq!(meta.quote_text, "brown fox");
    assert_eq!(meta.note.as_deref(), Some("great phrase"));
    assert_eq!(meta.color.as_deref(), Some("yellow"));

    // Position should be auto-detected.
    assert_eq!(meta.position_start, Some(10)); // "The quick " = 10 chars
    assert_eq!(meta.position_end, Some(19)); // "brown fox" = 9 chars
}

#[test]
fn create_highlight_ambiguous_position() {
    let db = test_db();
    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/ambig".to_owned()),
        title: "Ambiguous Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("fox fox fox".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let hl = db
        .create_highlight(source.id, "fox", None, None)
        .unwrap_or_else(|e| unreachable!("create: {e}"));

    let meta = db
        .get_highlight_meta(hl.id)
        .unwrap_or_else(|e| unreachable!("get meta: {e}"));
    // Ambiguous — positions should be None.
    assert!(meta.position_start.is_none());
    assert!(meta.position_end.is_none());
}

#[test]
fn list_highlights_with_filters() {
    let db = test_db();

    // Create two source articles.
    let source1 = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/s1".to_owned()),
        title: "Source 1".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("First article content".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    let source2 = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/s2".to_owned()),
        title: "Source 2".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Second article content".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source1)
        .unwrap_or_else(|e| unreachable!("insert s1: {e}"));
    db.insert_content_item(&source2)
        .unwrap_or_else(|e| unreachable!("insert s2: {e}"));

    // Create highlights.
    db.create_highlight(source1.id, "First article", None, None)
        .unwrap_or_else(|e| unreachable!("hl1: {e}"));
    db.create_highlight(source2.id, "Second article", None, None)
        .unwrap_or_else(|e| unreachable!("hl2: {e}"));

    // List all.
    let all = db
        .list_highlights(None, None, None, None, None)
        .unwrap_or_else(|e| unreachable!("list all: {e}"));
    assert_eq!(all.len(), 2);

    // Filter by source.
    let from_s1 = db
        .list_highlights(Some(source1.id), None, None, None, None)
        .unwrap_or_else(|e| unreachable!("list s1: {e}"));
    assert_eq!(from_s1.len(), 1);
    assert_eq!(from_s1[0].1.quote_text, "First article");

    // Limit.
    let limited = db
        .list_highlights(None, None, None, None, Some(1))
        .unwrap_or_else(|e| unreachable!("limited: {e}"));
    assert_eq!(limited.len(), 1);
}

#[test]
fn highlight_searchable_via_fts() {
    let db = test_db();
    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/fts-hl".to_owned()),
        title: "FTS Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Some unique content for highlight FTS test".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    db.create_highlight(source.id, "xylophone_rare_word", None, None)
        .unwrap_or_else(|e| unreachable!("create hl: {e}"));

    let results = db
        .search("xylophone_rare_word")
        .unwrap_or_else(|e| unreachable!("search: {e}"));
    assert!(
        !results.is_empty(),
        "highlight should be searchable via FTS"
    );
}

#[test]
fn backup_restore_includes_notes() {
    let db = test_db();
    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/backup-notes".to_owned()),
        title: "Backup Notes Test".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: None,
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let note = pergamon_core::model::Note {
        id: Uuid::new_v4(),
        content_item_id: item.id,
        body: "A note to backup".to_owned(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_note(&note)
        .unwrap_or_else(|e| unreachable!("insert note: {e}"));

    // Export.
    let notes_out = db
        .list_all_notes()
        .unwrap_or_else(|e| unreachable!("list notes: {e}"));
    let items_out = db
        .list_all_content_items()
        .unwrap_or_else(|e| unreachable!("list items: {e}"));

    // Restore into a fresh database.
    let dst = test_db();
    dst.restore_backup(
        &[],
        &[],
        &items_out,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &notes_out,
        &[],
        &[],
        &[],
    )
    .unwrap_or_else(|e| unreachable!("restore: {e}"));

    let dst_notes = dst
        .list_all_notes()
        .unwrap_or_else(|e| unreachable!("dst notes: {e}"));
    assert_eq!(dst_notes.len(), 1);
    assert_eq!(dst_notes[0].body, "A note to backup");
}

// ======================================================================
// Review card round-trip
// ======================================================================

#[test]
#[allow(clippy::too_many_lines)]
fn review_card_insert_get_and_update() {
    use pergamon_core::fsrs::CardState;
    use pergamon_core::model::ReviewCard;

    let db = test_db();

    // Create source article + highlight (FK requirement).
    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/review-src".to_owned()),
        title: "Review Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("text for review testing".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source: {e}"));

    let highlight = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Review Highlight".to_owned(),
        author: None,
        content_type: ContentType::Highlight,
        status: DocumentStatus::Inbox,
        content_text: Some("review testing".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&highlight)
        .unwrap_or_else(|e| unreachable!("insert highlight: {e}"));

    let meta = HighlightMeta {
        content_item_id: highlight.id,
        source_item_id: Some(source.id),
        quote_text: "review testing".to_owned(),
        note: None,
        position_start: None,
        position_end: None,
        color: None,
    };
    db.insert_highlight_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert meta: {e}"));

    // Insert review card.
    let card = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: highlight.id,
        state: CardState::New,
        stability: None,
        difficulty: None,
        due_at: now(),
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card)
        .unwrap_or_else(|e| unreachable!("insert card: {e}"));

    // Get by id.
    let got = db
        .get_review_card(card.id)
        .unwrap_or_else(|e| unreachable!("get card: {e}"));
    assert_eq!(got.id, card.id);
    assert_eq!(got.content_item_id, highlight.id);
    assert_eq!(got.state, CardState::New);
    assert_eq!(got.review_count, 0);

    // Get by content item.
    let got2 = db
        .get_review_card_for_item(highlight.id)
        .unwrap_or_else(|e| unreachable!("get for item: {e}"));
    assert!(got2.is_some());
    assert_eq!(
        got2.unwrap_or_else(|| unreachable!("already checked")).id,
        card.id
    );

    // Update.
    let due_at = now() + time::Duration::days(5);
    let last_rev = now();
    db.update_review_card(card.id, "review", 5.0, 4.5, due_at, last_rev, 1, 0, 5.0)
        .unwrap_or_else(|e| unreachable!("update card: {e}"));

    let got3 = db
        .get_review_card(card.id)
        .unwrap_or_else(|e| unreachable!("get after update: {e}"));
    assert_eq!(got3.state, CardState::Review);
    assert!(
        (got3
            .stability
            .unwrap_or_else(|| unreachable!("stability should be set"))
            - 5.0)
            .abs()
            < f64::EPSILON
    );
    assert!(
        (got3
            .difficulty
            .unwrap_or_else(|| unreachable!("difficulty should be set"))
            - 4.5)
            .abs()
            < f64::EPSILON
    );
    assert_eq!(got3.review_count, 1);
}

#[test]
fn review_card_list_due() {
    use pergamon_core::fsrs::CardState;
    use pergamon_core::model::ReviewCard;

    let db = test_db();

    // Create two highlights with review cards.
    let items: Vec<_> = (0..2)
        .map(|i| {
            let source = ContentItem {
                id: Uuid::new_v4(),
                url: Some(format!("https://example.com/due-{i}")),
                title: format!("Due Source {i}"),
                author: None,
                content_type: ContentType::Article,
                status: DocumentStatus::Inbox,
                content_text: Some(format!("text {i}")),
                excerpt: None,
                published_at: None,
                created_at: now(),
                updated_at: now(),
                read_at: None,
            };
            db.insert_content_item(&source)
                .unwrap_or_else(|e| unreachable!("insert source {i}: {e}"));
            let hl = ContentItem {
                id: Uuid::new_v4(),
                url: None,
                title: format!("Due Highlight {i}"),
                author: None,
                content_type: ContentType::Highlight,
                status: DocumentStatus::Inbox,
                content_text: Some(format!("highlight {i}")),
                excerpt: None,
                published_at: None,
                created_at: now(),
                updated_at: now(),
                read_at: None,
            };
            db.insert_content_item(&hl)
                .unwrap_or_else(|e| unreachable!("insert hl {i}: {e}"));
            let meta = HighlightMeta {
                content_item_id: hl.id,
                source_item_id: Some(source.id),
                quote_text: format!("highlight {i}"),
                note: None,
                position_start: None,
                position_end: None,
                color: None,
            };
            db.insert_highlight_meta(&meta)
                .unwrap_or_else(|e| unreachable!("insert meta {i}: {e}"));
            hl.id
        })
        .collect();

    // Card 0: due in the past (should be listed).
    let past = now() - time::Duration::hours(1);
    let card0 = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: items[0],
        state: CardState::New,
        stability: None,
        difficulty: None,
        due_at: past,
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card0)
        .unwrap_or_else(|e| unreachable!("insert card0: {e}"));

    // Card 1: due far in the future (should NOT be listed).
    let future = now() + time::Duration::days(30);
    let card1 = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: items[1],
        state: CardState::Review,
        stability: Some(10.0),
        difficulty: Some(5.0),
        due_at: future,
        last_reviewed_at: Some(now()),
        review_count: 3,
        lapse_count: 0,
        scheduled_days: Some(30.0),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card1)
        .unwrap_or_else(|e| unreachable!("insert card1: {e}"));

    let due = db
        .list_due_review_cards(now())
        .unwrap_or_else(|e| unreachable!("list due: {e}"));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, card0.id);

    // list_all should return both.
    let all = db
        .list_all_review_cards()
        .unwrap_or_else(|e| unreachable!("list all: {e}"));
    assert_eq!(all.len(), 2);
}

#[test]
fn review_card_delete_and_cascade() {
    use pergamon_core::fsrs::CardState;
    use pergamon_core::model::ReviewCard;

    let db = test_db();

    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/cascade-review".to_owned()),
        title: "Cascade Review Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("cascade test".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source: {e}"));

    let hl = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Cascade Highlight".to_owned(),
        author: None,
        content_type: ContentType::Highlight,
        status: DocumentStatus::Inbox,
        content_text: Some("cascade".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&hl)
        .unwrap_or_else(|e| unreachable!("insert hl: {e}"));

    let meta = HighlightMeta {
        content_item_id: hl.id,
        source_item_id: Some(source.id),
        quote_text: "cascade".to_owned(),
        note: None,
        position_start: None,
        position_end: None,
        color: None,
    };
    db.insert_highlight_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert meta: {e}"));

    let card = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: hl.id,
        state: CardState::New,
        stability: None,
        difficulty: None,
        due_at: now(),
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card)
        .unwrap_or_else(|e| unreachable!("insert card: {e}"));

    // Delete the highlight content item → card should cascade.
    db.delete_content_item(hl.id)
        .unwrap_or_else(|e| unreachable!("delete hl: {e}"));

    assert!(db.get_review_card(card.id).is_err());
}

#[test]
fn review_log_insert_and_list() {
    use pergamon_core::fsrs::{CardState, Rating};
    use pergamon_core::model::{ReviewCard, ReviewLog};

    let db = test_db();

    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/log-src".to_owned()),
        title: "Log Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("log text".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source: {e}"));

    let hl = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Log Highlight".to_owned(),
        author: None,
        content_type: ContentType::Highlight,
        status: DocumentStatus::Inbox,
        content_text: Some("log hl".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&hl)
        .unwrap_or_else(|e| unreachable!("insert hl: {e}"));

    let meta = HighlightMeta {
        content_item_id: hl.id,
        source_item_id: Some(source.id),
        quote_text: "log hl".to_owned(),
        note: None,
        position_start: None,
        position_end: None,
        color: None,
    };
    db.insert_highlight_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert meta: {e}"));

    let card = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: hl.id,
        state: CardState::New,
        stability: None,
        difficulty: None,
        due_at: now(),
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card)
        .unwrap_or_else(|e| unreachable!("insert card: {e}"));

    // Insert a review log.
    let log = ReviewLog {
        id: Uuid::new_v4(),
        card_id: card.id,
        rating: Rating::Good,
        state_before: CardState::New,
        stability_before: None,
        difficulty_before: None,
        state_after: CardState::Review,
        stability_after: 2.3,
        difficulty_after: 5.5,
        elapsed_days: 0.0,
        scheduled_days: 2.3,
        reviewed_at: now(),
    };
    db.insert_review_log(&log)
        .unwrap_or_else(|e| unreachable!("insert log: {e}"));

    let logs = db
        .list_review_logs_for_card(card.id)
        .unwrap_or_else(|e| unreachable!("list logs: {e}"));
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].rating, Rating::Good);
    assert_eq!(logs[0].state_before, CardState::New);
    assert_eq!(logs[0].state_after, CardState::Review);
    assert!((logs[0].stability_after - 2.3).abs() < f64::EPSILON);
}

#[test]
#[allow(clippy::too_many_lines)]
fn review_stats_computed_correctly() {
    use pergamon_core::fsrs::{CardState, Rating};
    use pergamon_core::model::{ReviewCard, ReviewLog};

    let db = test_db();

    // Stats on empty DB.
    let stats = db
        .review_stats(now())
        .unwrap_or_else(|e| unreachable!("stats empty: {e}"));
    assert_eq!(stats.total_cards, 0);
    assert_eq!(stats.total_reviews, 0);

    // Create a highlight + card.
    let source = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/stats-src".to_owned()),
        title: "Stats Source".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("stats".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&source)
        .unwrap_or_else(|e| unreachable!("insert source: {e}"));

    let hl = ContentItem {
        id: Uuid::new_v4(),
        url: None,
        title: "Stats Highlight".to_owned(),
        author: None,
        content_type: ContentType::Highlight,
        status: DocumentStatus::Inbox,
        content_text: Some("stats hl".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&hl)
        .unwrap_or_else(|e| unreachable!("insert hl: {e}"));

    let meta = HighlightMeta {
        content_item_id: hl.id,
        source_item_id: Some(source.id),
        quote_text: "stats hl".to_owned(),
        note: None,
        position_start: None,
        position_end: None,
        color: None,
    };
    db.insert_highlight_meta(&meta)
        .unwrap_or_else(|e| unreachable!("insert meta: {e}"));

    let card = ReviewCard {
        id: Uuid::new_v4(),
        content_item_id: hl.id,
        state: CardState::New,
        stability: None,
        difficulty: None,
        due_at: now(),
        last_reviewed_at: None,
        review_count: 0,
        lapse_count: 0,
        scheduled_days: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_review_card(&card)
        .unwrap_or_else(|e| unreachable!("insert card: {e}"));

    // Add two review logs: one Good (success), one Again (failure).
    let log1 = ReviewLog {
        id: Uuid::new_v4(),
        card_id: card.id,
        rating: Rating::Good,
        state_before: CardState::New,
        stability_before: None,
        difficulty_before: None,
        state_after: CardState::Review,
        stability_after: 2.3,
        difficulty_after: 5.0,
        elapsed_days: 0.0,
        scheduled_days: 2.3,
        reviewed_at: now(),
    };
    db.insert_review_log(&log1)
        .unwrap_or_else(|e| unreachable!("insert log1: {e}"));

    let log2 = ReviewLog {
        id: Uuid::new_v4(),
        card_id: card.id,
        rating: Rating::Again,
        state_before: CardState::Review,
        stability_before: Some(2.3),
        difficulty_before: Some(5.0),
        state_after: CardState::Relearning,
        stability_after: 0.5,
        difficulty_after: 6.0,
        elapsed_days: 2.0,
        scheduled_days: 0.5,
        reviewed_at: now(),
    };
    db.insert_review_log(&log2)
        .unwrap_or_else(|e| unreachable!("insert log2: {e}"));

    let stats = db
        .review_stats(now())
        .unwrap_or_else(|e| unreachable!("stats: {e}"));
    assert_eq!(stats.total_cards, 1);
    assert_eq!(stats.total_reviews, 2);
    assert_eq!(stats.success_count, 1); // Good counts as success (rating >= 2)
    assert!((stats.observed_retention - 0.5).abs() < f64::EPSILON);
    assert_eq!(stats.new_count, 1); // Card is still in New state in DB
}

// ======================================================================
// Review stats: streaks, source breakdown, daily/weekly history
// ======================================================================

#[allow(clippy::unwrap_used)]
mod review_stats_tests {
    use super::*;

    /// Create a highlight + card + source item scaffolding for stats tests.
    /// Returns `(source_item_id, highlight_item_id, card_id)`.
    fn create_review_scaffold(
        db: &Database,
        source_url: Option<&str>,
        feed_linked: bool,
    ) -> (Uuid, Uuid, Uuid) {
        let source = ContentItem {
            id: Uuid::new_v4(),
            url: source_url.map(std::borrow::ToOwned::to_owned),
            title: "Source".to_owned(),
            author: None,
            content_type: ContentType::Article,
            status: DocumentStatus::Inbox,
            content_text: Some("body".to_owned()),
            excerpt: None,
            published_at: None,
            created_at: now(),
            updated_at: now(),
            read_at: None,
        };
        db.insert_content_item(&source)
            .unwrap_or_else(|e| unreachable!("insert source: {e}"));

        if feed_linked {
            let feed = Feed {
                id: Uuid::new_v4(),
                title: "Feed".to_owned(),
                url: "https://example.com/feed.xml".to_owned(),
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
                .unwrap_or_else(|e| unreachable!("insert feed: {e}"));
            let fmeta = FeedItemMeta {
                content_item_id: source.id,
                feed_id: feed.id,
                guid: Some("test-guid".to_owned()),
                summary: None,
            };
            db.insert_feed_item_meta(&fmeta)
                .unwrap_or_else(|e| unreachable!("insert feed meta: {e}"));
        }

        let hl = ContentItem {
            id: Uuid::new_v4(),
            url: None,
            title: "Highlight".to_owned(),
            author: None,
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            content_text: Some("hl text".to_owned()),
            excerpt: None,
            published_at: None,
            created_at: now(),
            updated_at: now(),
            read_at: None,
        };
        db.insert_content_item(&hl)
            .unwrap_or_else(|e| unreachable!("insert hl: {e}"));

        let meta = HighlightMeta {
            content_item_id: hl.id,
            source_item_id: Some(source.id),
            quote_text: "hl text".to_owned(),
            note: None,
            position_start: None,
            position_end: None,
            color: None,
        };
        db.insert_highlight_meta(&meta)
            .unwrap_or_else(|e| unreachable!("insert hl meta: {e}"));

        let card = pergamon_core::model::ReviewCard {
            id: Uuid::new_v4(),
            content_item_id: hl.id,
            state: pergamon_core::fsrs::CardState::New,
            stability: None,
            difficulty: None,
            due_at: now(),
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
            created_at: now(),
            updated_at: now(),
        };
        db.insert_review_card(&card)
            .unwrap_or_else(|e| unreachable!("insert card: {e}"));

        (source.id, hl.id, card.id)
    }

    /// Insert a review log at a specific timestamp.
    fn insert_log_at(
        db: &Database,
        card_id: Uuid,
        at: OffsetDateTime,
        rating: pergamon_core::fsrs::Rating,
    ) {
        use pergamon_core::fsrs::CardState;
        let log = pergamon_core::model::ReviewLog {
            id: Uuid::new_v4(),
            card_id,
            rating,
            state_before: CardState::New,
            stability_before: None,
            difficulty_before: None,
            state_after: CardState::Review,
            stability_after: 3.0,
            difficulty_after: 5.0,
            elapsed_days: 0.0,
            scheduled_days: 1.0,
            reviewed_at: at,
        };
        db.insert_review_log(&log)
            .unwrap_or_else(|e| unreachable!("insert log: {e}"));
    }

    #[test]
    fn stats_streaks_empty_db() {
        let db = test_db();
        let stats = db.review_stats(now()).unwrap();
        assert_eq!(stats.current_streak, 0);
        assert_eq!(stats.longest_streak, 0);
        assert_eq!(stats.reviews_today, 0);
    }

    #[test]
    fn stats_streaks_consecutive_days() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        // Reviews on 3 consecutive days ending today.
        let today = OffsetDateTime::now_utc();
        let yesterday = today - time::Duration::days(1);
        let day_before = today - time::Duration::days(2);

        insert_log_at(&db, card_id, day_before, Rating::Good);
        insert_log_at(&db, card_id, yesterday, Rating::Hard);
        insert_log_at(&db, card_id, today, Rating::Easy);

        let stats = db.review_stats(today).unwrap();
        assert_eq!(
            stats.current_streak, 3,
            "3 consecutive days including today"
        );
        assert_eq!(stats.longest_streak, 3);
        assert_eq!(stats.reviews_today, 1);
    }

    #[test]
    fn stats_streak_yesterday_no_today() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        let today = OffsetDateTime::now_utc();
        let yesterday = today - time::Duration::days(1);
        let day_before = today - time::Duration::days(2);

        insert_log_at(&db, card_id, day_before, Rating::Good);
        insert_log_at(&db, card_id, yesterday, Rating::Good);
        // No review today — streak should still be preserved (counted from yesterday).

        let stats = db.review_stats(today).unwrap();
        assert_eq!(stats.current_streak, 2, "streak preserved from yesterday");
        assert_eq!(stats.longest_streak, 2);
        assert_eq!(stats.reviews_today, 0);
    }

    #[test]
    fn stats_streak_gap_resets() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        let today = OffsetDateTime::now_utc();
        // Reviews 3 days ago and today, but NOT yesterday → gap resets streak.
        insert_log_at(&db, card_id, today - time::Duration::days(3), Rating::Good);
        insert_log_at(&db, card_id, today, Rating::Good);

        let stats = db.review_stats(today).unwrap();
        assert_eq!(stats.current_streak, 1, "gap resets to 1 (today only)");
        assert_eq!(stats.longest_streak, 1);
    }

    #[test]
    fn stats_longest_streak_exceeds_current() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        let today = OffsetDateTime::now_utc();
        // Old 5-day streak (10-14 days ago), then gap, then single day today.
        for i in (10..=14).rev() {
            insert_log_at(&db, card_id, today - time::Duration::days(i), Rating::Good);
        }
        insert_log_at(&db, card_id, today, Rating::Easy);

        let stats = db.review_stats(today).unwrap();
        assert_eq!(stats.current_streak, 1, "only today");
        assert_eq!(stats.longest_streak, 5, "old 5-day streak is longest");
    }

    #[test]
    fn stats_source_breakdown_manual() {
        let db = test_db();
        let _ = create_review_scaffold(&db, Some("https://example.com/article"), false);

        let breakdown = db.review_source_breakdown().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].origin, "Manual");
        assert_eq!(breakdown[0].count, 1);
    }

    #[test]
    fn stats_source_breakdown_kindle() {
        let db = test_db();
        let _ = create_review_scaffold(&db, Some("kindle://book/abc123"), false);

        let breakdown = db.review_source_breakdown().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].origin, "Kindle");
    }

    #[test]
    fn stats_source_breakdown_readwise() {
        let db = test_db();
        let _ = create_review_scaffold(&db, Some("readwise://source/xyz"), false);

        let breakdown = db.review_source_breakdown().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].origin, "Readwise");
    }

    #[test]
    fn stats_source_breakdown_feed() {
        let db = test_db();
        let _ = create_review_scaffold(&db, Some("https://blog.example.com/post"), true);

        let breakdown = db.review_source_breakdown().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].origin, "Feed");
    }

    #[test]
    fn stats_source_breakdown_mixed() {
        let db = test_db();
        let _ = create_review_scaffold(&db, Some("kindle://book/a"), false);
        let _ = create_review_scaffold(&db, Some("kindle://book/b"), false);
        let _ = create_review_scaffold(&db, Some("readwise://source/x"), false);
        let _ = create_review_scaffold(&db, Some("https://manual.com"), false);

        let breakdown = db.review_source_breakdown().unwrap();
        let kindle = breakdown.iter().find(|s| s.origin == "Kindle");
        let readwise = breakdown.iter().find(|s| s.origin == "Readwise");
        let manual = breakdown.iter().find(|s| s.origin == "Manual");

        assert_eq!(kindle.map(|s| s.count), Some(2));
        assert_eq!(readwise.map(|s| s.count), Some(1));
        assert_eq!(manual.map(|s| s.count), Some(1));
    }

    #[test]
    fn stats_daily_history() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        let today = OffsetDateTime::now_utc();
        // 2 reviews today, 1 yesterday.
        insert_log_at(&db, card_id, today, Rating::Good);
        insert_log_at(
            &db,
            card_id,
            today - time::Duration::seconds(60),
            Rating::Again,
        );
        insert_log_at(&db, card_id, today - time::Duration::days(1), Rating::Hard);

        let daily = db.review_daily_history(30, today).unwrap();
        assert!(daily.len() >= 2, "at least 2 days");

        let today_entry = daily.iter().find(|d| {
            let t = time::format_description::well_known::Rfc3339;
            let s = today.format(&t).unwrap_or_default();
            d.date == s[..10]
        });
        assert!(today_entry.is_some(), "today should be in daily history");
        let te = today_entry.unwrap();
        assert_eq!(te.reviews, 2);
        assert_eq!(te.successes, 1); // Good is success, Again is not
    }

    #[test]
    fn stats_weekly_history() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, None, false);

        let today = OffsetDateTime::now_utc();
        insert_log_at(&db, card_id, today, Rating::Easy);
        insert_log_at(&db, card_id, today - time::Duration::days(8), Rating::Good);

        let weekly = db.review_weekly_history(12, today).unwrap();
        assert!(!weekly.is_empty(), "should have at least 1 week");
    }

    #[test]
    fn stats_report_combines_all() {
        use pergamon_core::fsrs::Rating;
        let db = test_db();
        let (_, _, card_id) = create_review_scaffold(&db, Some("kindle://book/test"), false);

        let today = OffsetDateTime::now_utc();
        insert_log_at(&db, card_id, today, Rating::Good);

        let report = db.review_stats_report(today).unwrap();
        assert_eq!(report.stats.total_cards, 1);
        assert_eq!(report.stats.total_reviews, 1);
        assert_eq!(report.stats.reviews_today, 1);
        assert_eq!(report.stats.current_streak, 1);
        assert!(!report.source_breakdown.is_empty());
        assert_eq!(report.source_breakdown[0].origin, "Kindle");
        assert!(!report.daily_history.is_empty());
    }

    #[test]
    fn stats_report_json_serializable() {
        let db = test_db();
        let report = db.review_stats_report(now()).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("total_cards"));
        assert!(json.contains("source_breakdown"));
        assert!(json.contains("daily_history"));
        assert!(json.contains("weekly_history"));
    }
} // mod review_stats_tests

// ======================================================================
// Content rules
// ======================================================================

#[test]
fn content_rule_crud() {
    use pergamon_core::rule::{ContentRule, RuleAction};

    let db = test_db();
    let now = now();

    let rule = ContentRule {
        id: Uuid::new_v4(),
        name: "Auto-tag Rust".to_owned(),
        enabled: true,
        priority: 10,
        filter_query: "tag:programming".to_owned(),
        actions: vec![RuleAction::AddTag("rust".to_owned())],
        created_at: now,
        updated_at: now,
    };

    db.insert_rule(&rule)
        .unwrap_or_else(|e| unreachable!("insert rule: {e}"));

    let fetched = db
        .get_rule(rule.id)
        .unwrap_or_else(|e| unreachable!("get rule: {e}"));
    assert_eq!(fetched.name, "Auto-tag Rust");
    assert_eq!(fetched.priority, 10);
    assert!(fetched.enabled);
    assert_eq!(fetched.actions.len(), 1);

    let by_name = db
        .get_rule_by_name("auto-tag rust")
        .unwrap_or_else(|e| unreachable!("get by name: {e}"));
    assert!(by_name.is_some());
    assert_eq!(
        by_name.as_ref().map(|r| &r.name),
        Some(&"Auto-tag Rust".to_owned())
    );

    let all = db
        .list_rules()
        .unwrap_or_else(|e| unreachable!("list rules: {e}"));
    assert_eq!(all.len(), 1);

    db.set_rule_enabled(rule.id, false)
        .unwrap_or_else(|e| unreachable!("disable rule: {e}"));
    let disabled = db
        .get_rule(rule.id)
        .unwrap_or_else(|e| unreachable!("get disabled: {e}"));
    assert!(!disabled.enabled);

    db.delete_rule(rule.id)
        .unwrap_or_else(|e| unreachable!("delete rule: {e}"));
    let empty = db
        .list_rules()
        .unwrap_or_else(|e| unreachable!("list after delete: {e}"));
    assert!(empty.is_empty());
}

#[test]
fn content_rule_backup_restore() {
    use pergamon_core::rule::{ContentRule, RuleAction};

    let src = test_db();
    let now = now();

    let rule = ContentRule {
        id: Uuid::new_v4(),
        name: "Mute noisy".to_owned(),
        enabled: true,
        priority: 50,
        filter_query: "source:Noisy".to_owned(),
        actions: vec![RuleAction::Mute],
        created_at: now,
        updated_at: now,
    };
    src.insert_rule(&rule)
        .unwrap_or_else(|e| unreachable!("insert rule: {e}"));

    let rules = src
        .list_rules()
        .unwrap_or_else(|e| unreachable!("list rules: {e}"));
    assert_eq!(rules.len(), 1);

    let dst = test_db();
    dst.restore_backup(
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &rules,
    )
    .unwrap_or_else(|e| unreachable!("restore: {e}"));

    let dst_rules = dst
        .list_rules()
        .unwrap_or_else(|e| unreachable!("dst list rules: {e}"));
    assert_eq!(dst_rules.len(), 1);
    assert_eq!(dst_rules[0].name, "Mute noisy");
    assert_eq!(dst_rules[0].actions, vec![RuleAction::Mute]);
}

#[test]
fn content_rule_multiple_actions() {
    use pergamon_core::rule::{ContentRule, RuleAction};
    use pergamon_core::status::DocumentStatus;

    let db = test_db();
    let now = now();

    let rule = ContentRule {
        id: Uuid::new_v4(),
        name: "Multi-action".to_owned(),
        enabled: true,
        priority: 0,
        filter_query: "type:article".to_owned(),
        actions: vec![
            RuleAction::AddTag("auto".to_owned()),
            RuleAction::SetStatus(DocumentStatus::Later),
            RuleAction::AddToCollection("reading-list".to_owned()),
        ],
        created_at: now,
        updated_at: now,
    };

    db.insert_rule(&rule)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let fetched = db
        .get_rule(rule.id)
        .unwrap_or_else(|e| unreachable!("get: {e}"));
    assert_eq!(fetched.actions.len(), 3);
    assert_eq!(fetched.actions[0], RuleAction::AddTag("auto".to_owned()));
    assert_eq!(
        fetched.actions[1],
        RuleAction::SetStatus(DocumentStatus::Later)
    );
    assert_eq!(
        fetched.actions[2],
        RuleAction::AddToCollection("reading-list".to_owned())
    );
}

// ── Usage stats ──────────────────────────────────────────────────

#[test]
fn usage_stats_report_empty_db() {
    let db = test_db();
    let report = db
        .usage_stats_report(now())
        .unwrap_or_else(|e| unreachable!("usage stats: {e}"));
    assert_eq!(report.overview.total_items, 0);
    assert_eq!(report.overview.archived_count, 0);
    assert_eq!(report.overview.total_reading_minutes, 0);
    assert_eq!(report.overview.reading_streak_days, 0);
    assert!(report.top_sources.is_empty());
    assert!(report.tag_distribution.is_empty());
}

#[test]
fn usage_stats_counts_archived_items() {
    let db = test_db();
    let n = now();

    // Insert 3 articles: 2 archived, 1 inbox.
    for i in 0..3_u32 {
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: Some(format!("https://example.com/a{i}")),
            title: format!("Article {i}"),
            author: None,
            content_type: ContentType::Article,
            status: if i < 2 {
                DocumentStatus::Archived
            } else {
                DocumentStatus::Inbox
            },
            content_text: Some("word ".repeat(238)),
            excerpt: None,
            published_at: None,
            created_at: n,
            updated_at: n,
            read_at: if i < 2 { Some(n) } else { None },
        };
        db.insert_content_item(&item)
            .unwrap_or_else(|e| unreachable!("insert {i}: {e}"));
    }

    let report = db
        .usage_stats_report(n)
        .unwrap_or_else(|e| unreachable!("usage stats: {e}"));
    assert_eq!(report.overview.total_items, 3);
    assert_eq!(report.overview.archived_count, 2);
    assert!(report.overview.total_reading_minutes >= 2);
}

#[test]
#[allow(clippy::too_many_lines)]
fn usage_stats_top_sources_from_feed() {
    let db = test_db();
    let n = now();

    let feed = Feed {
        id: Uuid::new_v4(),
        title: "Example Blog".to_owned(),
        url: "https://blog.example.com/feed.xml".to_owned(),
        site_url: None,
        description: None,
        etag: None,
        last_modified_header: None,
        error_count: 0,
        last_error: None,
        last_fetched_at: None,
        folder_id: None,
        created_at: n,
        updated_at: n,
    };
    db.insert_feed(&feed)
        .unwrap_or_else(|e| unreachable!("insert feed: {e}"));

    for i in 0..3_u32 {
        let item_id = Uuid::new_v4();
        let item = ContentItem {
            id: item_id,
            url: Some(format!("https://blog.example.com/post/{i}")),
            title: format!("Post {i}"),
            author: None,
            content_type: ContentType::FeedItem,
            status: DocumentStatus::Archived,
            content_text: Some("hello world".to_owned()),
            excerpt: None,
            published_at: None,
            created_at: n,
            updated_at: n,
            read_at: Some(n),
        };
        db.insert_content_item(&item)
            .unwrap_or_else(|e| unreachable!("insert item {i}: {e}"));
        let meta = FeedItemMeta {
            content_item_id: item_id,
            feed_id: feed.id,
            guid: Some(format!("guid-{i}")),
            summary: None,
        };
        db.insert_feed_item_meta(&meta)
            .unwrap_or_else(|e| unreachable!("insert meta {i}: {e}"));
    }

    let report = db
        .usage_stats_report(n)
        .unwrap_or_else(|e| unreachable!("usage stats: {e}"));
    assert!(!report.top_sources.is_empty());
    assert_eq!(report.top_sources[0].source_name, "Example Blog");
    assert_eq!(report.top_sources[0].items_read, 3);
}

#[test]
fn usage_stats_tag_distribution() {
    let db = test_db();
    let n = now();

    let item_id = Uuid::new_v4();
    let item = ContentItem {
        id: item_id,
        url: Some("https://example.com/tagged".to_owned()),
        title: "Tagged Article".to_owned(),
        author: None,
        content_type: ContentType::Article,
        status: DocumentStatus::Archived,
        content_text: Some("some text".to_owned()),
        excerpt: None,
        published_at: None,
        created_at: n,
        updated_at: n,
        read_at: Some(n),
    };
    db.insert_content_item(&item)
        .unwrap_or_else(|e| unreachable!("insert: {e}"));

    let rust_tag = Tag {
        id: Uuid::new_v4(),
        name: "rust".to_owned(),
        created_at: n,
    };
    db.insert_tag(&rust_tag)
        .unwrap_or_else(|e| unreachable!("insert tag: {e}"));
    db.tag_content_item(item_id, rust_tag.id)
        .unwrap_or_else(|e| unreachable!("tag: {e}"));

    let prog_tag = Tag {
        id: Uuid::new_v4(),
        name: "programming".to_owned(),
        created_at: n,
    };
    db.insert_tag(&prog_tag)
        .unwrap_or_else(|e| unreachable!("insert tag: {e}"));
    db.tag_content_item(item_id, prog_tag.id)
        .unwrap_or_else(|e| unreachable!("tag: {e}"));

    let report = db
        .usage_stats_report(n)
        .unwrap_or_else(|e| unreachable!("usage stats: {e}"));
    assert_eq!(report.tag_distribution.len(), 2);
    let tag_names: Vec<&str> = report
        .tag_distribution
        .iter()
        .map(|t| t.tag_name.as_str())
        .collect();
    assert!(tag_names.contains(&"rust"));
    assert!(tag_names.contains(&"programming"));
}

#[test]
fn usage_reading_streaks() {
    use pergamon_storage::db::compute_reading_streaks;

    // No dates → zero streaks.
    let (current, longest) = compute_reading_streaks(&[], "2025-07-01");
    assert_eq!(current, 0);
    assert_eq!(longest, 0);

    // 3 consecutive days ending today.
    let dates = vec![
        "2025-07-03".to_owned(),
        "2025-07-02".to_owned(),
        "2025-07-01".to_owned(),
    ];
    let (current, longest) = compute_reading_streaks(&dates, "2025-07-03");
    assert_eq!(current, 3);
    assert_eq!(longest, 3);

    // Streak broken (gap at July 2).
    let dates = vec!["2025-07-03".to_owned(), "2025-07-01".to_owned()];
    let (current, longest) = compute_reading_streaks(&dates, "2025-07-03");
    assert_eq!(current, 1);
    assert_eq!(longest, 1);
}
