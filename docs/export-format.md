# Export Formats

pergamon provides two general-purpose export formats — **Markdown** and
**JSON** — designed for interoperability with other tools, scripts, and
knowledge-management workflows. Both formats carry a schema version number so
consumers can detect breaking changes.

## Schema versioning

Every exported file (Markdown frontmatter or JSON envelope) includes a
`schema_version` (JSON) or `export_schema` (Markdown frontmatter) field.

- **Patch-level changes** (adding optional fields, expanding enums) do **not**
  bump the schema version.
- **Breaking changes** (renaming fields, removing fields, changing value
  semantics) bump the schema version.

Current schema version: **1**.

---

## Markdown export

### CLI usage

```sh
# Export all items as Markdown files
pergamon export markdown --output ./export

# Custom filename template
pergamon export markdown --output ./export --filename "{date} - {title}"

# Filter by content type
pergamon export markdown --output ./export --type article

# With backlinks and hashtag-style tags
pergamon export markdown --output ./export --backlinks --tag-format both

# Dry run (preview without writing)
pergamon export markdown --output ./export --dry-run
```

### Filename templates

The `--filename` flag accepts a template string with placeholders:

| Placeholder | Description                          | Example output  |
|-------------|--------------------------------------|-----------------|
| `{title}`   | Slugified title (max 60 chars)       | `my-article`    |
| `{date}`    | Created date in `YYYY-MM-DD` format  | `2025-01-15`    |
| `{id}`      | First 8 characters of the UUID       | `a1b2c3d4`      |
| `{type}`    | Slugified content type               | `bookmark`      |

Default template: `{title}--{id}`

Templates that omit `{id}` may produce filename collisions when items share
the same title and date. When a collision is detected, pergamon automatically
appends `--{id}` to the second file.

Unknown placeholders (e.g., `{author}`) are rejected at parse time.

### File structure

Each exported file has four sections:

1. **YAML frontmatter** — structured metadata
2. **Body** — title heading, metadata block, excerpt, content
3. **Annotations** — highlights and notes sections
4. **Related** — wikilink backlinks (when `--backlinks` is enabled)

### Frontmatter fields (schema v1)

```yaml
---
pergamon_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
export_schema: 1
title: "Article Title"
content_type: "article"
author: "Jane Doe"              # optional
url: "https://example.com/post" # optional
tags:                           # optional, present when item has tags
  - rust
  - programming
status: "archived"
highlight_count: 3              # optional, only when highlights exist
created: "2025-01-15"
updated: "2025-01-20"
---
```

All fields are strings or lists of strings, except:

- `export_schema` — integer
- `highlight_count` — integer

### Body format

```markdown
# Article Title

**Author:** Jane Doe
**URL:** [example.com](https://example.com/post)
**Type:** article
**Status:** archived

#rust #programming          ← only with --tag-format hashtag or both

> Excerpt text here

## Highlights

> Highlighted quote text
> spanning multiple lines

*Annotation note about this highlight*

Color: yellow

---

## Notes

- Note body text *(2025-01-15)*

## Related

- [[other-article--b2c3d4e5|Other Article Title]]
```

### Tag format options

| `--tag-format` | YAML frontmatter | Body hashtags |
|----------------|------------------|---------------|
| `yaml` (default) | ✓ | ✗ |
| `hashtag`      | ✗ | ✓ |
| `both`         | ✓ | ✓ |

### Backlinks

When `--backlinks` is enabled, pergamon builds a cross-reference index using
the `source_item_id` field on highlights. If item A has highlights whose source
is item B, item A's Markdown file will include a `## Related` section with a
wikilink to item B's exported file.

Wikilink format: `[[filename-stem|Display Title]]`

---

## JSON export

### CLI usage

```sh
# Export to stdout (compact)
pergamon export json

# Pretty-printed to a file
pergamon export json --output items.json --pretty

# Include full content text
pergamon export json --output items.json --include-content

# Filter by type
pergamon export json --type bookmark
```

### Top-level structure

```json
{
  "schema_version": 1,
  "exported_at": "2025-01-15T10:30:00Z",
  "pergamon_version": "0.3.0",
  "item_count": 42,
  "items": [...]
}
```

### Item structure

```json
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "title": "Article Title",
  "content_type": "article",
  "status": "archived",
  "url": "https://example.com/post",
  "author": "Jane Doe",
  "excerpt": "Short summary...",
  "content_text": "Full article text...",
  "tags": ["rust", "programming"],
  "created_at": "2025-01-15T10:00:00Z",
  "updated_at": "2025-01-20T14:30:00Z",
  "highlights": [...],
  "notes": [...],
  "bookmark_meta": {...},
  "feed_item_meta": {...}
}
```

### Field reference

#### Item fields

| Field | Type | Presence | Description |
|-------|------|----------|-------------|
| `id` | string (UUID) | always | Stable item identifier |
| `title` | string | always | Item title |
| `content_type` | string | always | One of: `feed_item`, `article`, `bookmark`, `highlight`, `pdf`, `podcast_episode` |
| `status` | string | always | One of: `inbox`, `later`, `reading`, `reference`, `archived`, `discarded` |
| `url` | string | optional | Source URL |
| `author` | string | optional | Author name |
| `excerpt` | string | optional | Short summary or description |
| `content_text` | string | optional | Full extracted text (opt-in via `--include-content`) |
| `tags` | string[] | optional | Tag names (omitted when empty) |
| `created_at` | string (ISO 8601) | always | Creation timestamp |
| `updated_at` | string (ISO 8601) | always | Last update timestamp |
| `highlights` | object[] | optional | Highlight annotations (omitted when empty) |
| `notes` | object[] | optional | Notes (omitted when empty) |
| `bookmark_meta` | object | optional | Bookmark-specific metadata |
| `feed_item_meta` | object | optional | Feed item-specific metadata |

#### Highlight fields

| Field | Type | Presence | Description |
|-------|------|----------|-------------|
| `id` | string (UUID) | always | Highlight item ID |
| `quote_text` | string | always | Highlighted text |
| `note` | string | optional | Annotation on the highlight |
| `color` | string | optional | Highlight color |
| `source_item_id` | string (UUID) | optional | ID of the source document |
| `position_start` | integer | optional | Start position in source |
| `position_end` | integer | optional | End position in source |
| `created_at` | string (ISO 8601) | always | Creation timestamp |

#### Note fields

| Field | Type | Presence | Description |
|-------|------|----------|-------------|
| `id` | string (UUID) | always | Note ID |
| `body` | string | always | Note text |
| `created_at` | string (ISO 8601) | always | Creation timestamp |

#### Bookmark metadata fields

| Field | Type | Presence | Description |
|-------|------|----------|-------------|
| `folder` | string | optional | Bookmark folder |
| `description` | string | optional | User description |
| `is_favorite` | boolean | always | Favorite flag |

#### Feed item metadata fields

| Field | Type | Presence | Description |
|-------|------|----------|-------------|
| `feed_title` | string | optional | Parent feed title |
| `feed_url` | string | optional | Feed URL |
| `published_at` | string (ISO 8601) | optional | Publication date |
| `guid` | string | optional | Feed item GUID |

### Optional fields

Fields marked "optional" are omitted from the output when their value is
`null` or empty (empty arrays, empty strings). This keeps the JSON compact.
The `content_text` field is additionally opt-in: it is only included when
`--include-content` is passed.

---

## Stability guarantees

1. **Schema version 1 fields will not be removed or renamed** without bumping
   the schema version.
2. **New optional fields may be added** to any object without a version bump.
3. **Enum values may be extended** (e.g., new content types) without a version
   bump.
4. Consumers should ignore unknown fields for forward compatibility.
5. The `schema_version` / `export_schema` field is the canonical indicator of
   format compatibility.
