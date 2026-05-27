# ADR-002: Content Type Taxonomy and Unified Data Model

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon replaces multiple information tools at once: Inoreader for feed items, Readwise for highlights and review, Readwise Reader for read-later articles, and Raindrop.io for bookmarks. These sources have different shapes, but users expect a single system. They want one search box, one tagging model, one collection system, one deduplication story, and one consistent status model across saved links, feed entries, extracted articles, PDFs, highlights, and podcast episodes.

A naive schema would create a separate top-level table for each content category: feed items, articles, bookmarks, PDFs, highlights, and podcast episodes. That is superficially simple, but it fragments core product behavior. Cross-type search becomes multiple queries merged in application code. Tags and collections must be duplicated or abstracted awkwardly. Deduplication across types becomes harder because the same URL or canonical document may appear in multiple tables. Feature work also becomes repetitive: each new field or workflow may require updating many tables and queries.

pergamon needs a model that reflects both similarity and difference. Most content items share a stable common core: identifier, URL, title, author, lifecycle status, timestamps, and tags. At the same time, some types need type-specific metadata such as feed-level details, bookmark capture details, or highlight anchoring information.

The system also relies on SQLite FTS5. A unified search experience is easier to implement and maintain if searchable content shares a primary identity and indexing strategy. That strongly favors a central table with common columns and extension tables for specialization.

## Decision

pergamon will use a unified content model centered on a single `content_items` table with a `content_type` discriminator column.

Supported initial content types are:
- `feed_item`
- `article`
- `bookmark`
- `highlight`
- `pdf`
- `podcast_episode`

The shared table will include common fields such as:
- `id`
- `url`
- `title`
- `author`
- `content_type`
- `status`
- `created_at`
- `tags`

Type-specific attributes will live in extension tables such as:
- `feed_item_meta`
- `bookmark_meta`
- `highlight_meta`

The unified table will be the primary anchor for tagging, collections, search, deduplication, and lifecycle state. Extension tables will add specialized metadata without splitting the system into disconnected silos.

## Consequences

### Positive
- Enables one cross-type search experience and a single FTS5 indexing strategy.
- Simplifies tagging and collections because all content shares one primary identity.
- Makes deduplication across feeds, bookmarks, and articles more reliable.
- Reduces schema and query duplication for common behaviors.
- Makes future types easier to add without redesigning the whole model.

### Negative
- Requires discipline around nullable fields and type-specific validation.
- Some queries need joins to extension tables for specialized metadata.
- Incorrect use of the discriminator could lead to inconsistent rows if constraints are weak.
- The unified table may become large and central to many workflows, increasing migration sensitivity.

## Rejected Alternatives

- **Fully separate tables for each content type**: rejected because it fragments search, duplicates tag and collection logic, and makes cross-type deduplication awkward.
- **Store everything as opaque JSON in one table**: rejected because it weakens relational integrity, complicates indexing, and makes migrations harder to reason about.
- **Use per-type inheritance through only extension tables without a common root**: rejected because pergamon needs a first-class shared identity for search, tagging, and state.
- **Start with separate tables and unify later**: rejected because migration cost would be high and the core product depends on unified behavior from day one.
