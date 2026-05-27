# ADR-010: Bookmark vs Article — Unified Content Model

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

A common problem in read-later and bookmarking systems is conceptual duplication. A saved URL may begin life as a bookmark, later become an extracted article, and also appear as a feed item or a highlighted source. If the system treats bookmarks and articles as fundamentally different entities, users are forced into an artificial distinction that does not match intent. They ask, “Did I save this as a bookmark or as an article?” instead of simply asking whether the content exists in their library.

For pergamon, this distinction is especially harmful because the project merges the roles of Raindrop.io, Readwise Reader, and feed ingestion. Users should be able to tag, search, collect, deduplicate, and enrich content without creating parallel records. If a bookmark is enriched with extracted text, it should not become a different logical item. If a feed item is bookmarked, that should not create unnecessary duplication either.

The unified `content_items` architecture already provides a shared identity model. The open question is how to represent bookmark and article behavior within that model. pergamon needs a design that preserves origin metadata while allowing progressive enrichment from URL-only capture to fully extracted content.

## Decision

pergamon will treat bookmarks and articles as the same top-level entity type family within `content_items`, distinguished by `content_type` and state rather than by separate root models.

Specifically:

- a bookmark is a `content_item` with `content_type = 'bookmark'` and optional extracted content
- a read-later article is a `content_item` with `content_type = 'article'` and extracted content

When a user enriches a bookmark by extracting full content, the item gains article-like properties without losing bookmark-related metadata. The system may update status, metadata completeness, and extraction fields, but it will preserve identity. This avoids the need to create a second record for the same URL.

If a `feed_item` is bookmarked by the user, pergamon will link that user action to the existing feed item rather than duplicate the underlying content. The feed item gains a `bookmarked` flag or equivalent linkage, preserving provenance and preventing duplicate library entries.

## Consequences

### Positive

- Eliminates confusing user-facing distinctions between “bookmark” and “article.”
- Simplifies tagging, collections, deduplication, and search.
- Supports progressive enrichment from saved link to extracted reading object.
- Preserves provenance when content originates from a feed.
- Reduces duplicate records and synchronization complexity.

### Negative

- Some UI and reporting logic must carefully explain state versus type.
- Edge cases may arise when bookmark metadata and article extraction metadata disagree.
- Migration from imported tools may require normalization rules to fit the unified model.
- The feed-item linkage model needs clear constraints to avoid accidental duplication.

## Rejected Alternatives

- **Separate root entities for bookmarks and articles**: rejected because it creates user confusion, duplicates logic, and complicates enrichment.
- **Always convert bookmarks into articles on extraction**: rejected because it discards useful bookmark provenance and implies an unnecessary identity change.
- **Duplicate feed items when bookmarked**: rejected because it increases storage, weakens deduplication, and fragments user history.
- **Model bookmarks as only tags on articles**: rejected because some saved items may never be extracted and still need first-class identity and metadata.
