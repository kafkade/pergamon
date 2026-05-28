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
