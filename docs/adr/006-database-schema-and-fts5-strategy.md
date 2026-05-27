# ADR-006: Database Schema and FTS5 Strategy

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon is local-first, offline-capable, and designed around a CLI/TUI workflow. Its storage layer must therefore be easy to install, easy to back up, portable across operating systems, and powerful enough to support full-text search across titles, authors, article bodies, and tags. Because pergamon replaces multiple content tools, search quality is not optional; it is a primary interaction model.

SQLite is the natural fit for a single-user local application, but the exact schema and indexing strategy matter. One major design question is where to store full article bodies. Storing body text in separate filesystem files would reduce database size, but it would complicate search, backup, portability, and consistency. FTS5 works best when the searchable text is already in SQLite. pergamon also needs deterministic migrations and a reliable path strategy across Linux, macOS, and Windows.

At the same time, not everything belongs in the database. Binary assets such as images and favicons can increase DB churn, complicate compaction, and are less important to the core search experience than text. Those files are better handled separately with stable addressing.

pergamon also needs predictable SQLite capabilities across platforms, especially FTS5. Depending on system SQLite would introduce packaging and compatibility variability that is unacceptable for a cross-platform Rust tool.

## Decision

pergamon will use SQLite via `rusqlite` with bundled SQLite enabled to guarantee FTS5 support across platforms.

The database will:

- store structured application data in relational tables
- use an FTS5 virtual table indexing `title`, `author`, `content_text`, and `tags`
- store full article and extracted text in SQLite `TEXT` columns rather than separate files

This choice prioritizes single-file portability, simpler backup, and first-class FTS5 integration, accepting that the database may grow to an estimated 300–500 MB for roughly 50,000 full-text articles.

Binary assets such as images and favicons will use content-addressed filesystem storage, following the pattern used by toku for covers.

Schema migrations will use `refinery` with embedded SQL and will auto-run on startup. Database locations will follow platform conventions:

- Linux: XDG data directory
- macOS: `~/Library/Application Support/pergamon/`
- Windows: `%APPDATA%\pergamon\`

## Consequences

### Positive

- Delivers strong local full-text search through FTS5.
- Keeps text content, metadata, and indexes in one portable database file.
- Simplifies backup, restore, export, and sync packaging.
- Bundled SQLite avoids cross-platform feature drift.
- Embedded migrations support reproducible schema evolution.

### Negative

- Full-text storage increases database size materially.
- Vacuuming and migration operations may take longer on large libraries.
- Storing large text blobs in SQLite may make some inspection tasks less convenient than raw files.
- Asset storage is split between database and filesystem, requiring careful consistency handling.

## Rejected Alternatives

- **Store article bodies as filesystem files and index paths only**: rejected because it complicates FTS5, backup, and portability.
- **Use system SQLite instead of bundled SQLite**: rejected because FTS5 availability and behavior would vary by platform.
- **Adopt a heavier local database such as PostgreSQL**: rejected because pergamon is single-user, local-first, and should not require a service dependency.
- **Store images and favicons inside SQLite blobs**: rejected because binary assets are better managed as content-addressed files and are not primary search data.
