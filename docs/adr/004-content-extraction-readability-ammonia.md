# ADR-004: Content Extraction — readability + ammonia

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon aims to replace read-later and article-centric workflows currently handled by tools such as Readwise Reader. Saving a URL is not enough; users need a durable, searchable local representation of article content that remains useful even if the original page changes or disappears. At the same time, the web is noisy. Pages include navigation chrome, ads, scripts, tracking pixels, broken markup, lazy-loaded content, and inconsistent metadata conventions.

The extraction stack must therefore do three things well: isolate reader-worthy content, sanitize unsafe or noisy HTML, and capture structured metadata such as title, author, site name, images, and canonical URLs. It must also fit pergamon’s architecture: no extraction logic in pergamon-core, deterministic behavior when given raw bytes, and graceful degradation when extraction is incomplete.

A lightweight custom extractor would not be robust enough across modern sites. Storing raw HTML alone would preserve too much noise and create unpleasant TUI and Obsidian rendering. Relying entirely on metadata without content would weaken full-text search and offline use. PDF handling is also important, but OCR introduces significant complexity and external dependency risk that does not fit the initial scope.

pergamon needs a practical extraction stack that works now, stays within Rust, and allows quality improvements over time without compromising local-first behavior.

## Decision

pergamon will implement content extraction in a dedicated `pergamon-extract` crate using:

- a Rust readability implementation (`readability` or `readability-rs`) for main-article extraction
- `ammonia` for HTML sanitization
- `scraper` for metadata extraction from Open Graph tags, Twitter Cards, and JSON-LD
- `lopdf` for PDF text-layer extraction

The extraction flow will be:

1. `pergamon-cli` fetches article bytes over HTTP.
2. Raw bytes are passed to `pergamon-extract`.
3. Readability extracts the primary article content.
4. Ammonia sanitizes the result, removing scripts, tracking elements, and unsafe markup while preserving useful formatting.
5. Scraper extracts metadata and canonical information.
6. If extraction fails, pergamon stores the URL plus available metadata only and links back to the original page.
7. For PDFs, pergamon extracts embedded text when present. OCR is explicitly deferred until after 1.0.

## Consequences

### Positive

- Produces a durable local representation suitable for search, TUI reading, and Obsidian export.
- Reduces noise and unsafe markup while preserving readable structure.
- Captures metadata from common web conventions without bespoke parsers per site.
- Keeps extraction outside pergamon-core and reusable across interfaces.
- Provides a pragmatic fallback when full extraction is not possible.

### Negative

- Readability quality varies across sites and may require tuning or fallback heuristics.
- Sanitization can occasionally remove formatting users expected to keep.
- Metadata sources may disagree, requiring precedence rules.
- PDF support without OCR will miss scanned or image-only documents in early releases.

## Rejected Alternatives

- **Store raw HTML only**: rejected because it retains too much noise, hurts readability, and complicates safe rendering.
- **Build a custom article extractor**: rejected because it is high effort and unnecessary compared with mature readability-based approaches.
- **Use a headless browser extraction pipeline by default**: rejected because it adds operational complexity and is too heavy for a CLI-first local tool.
- **Require OCR for all PDF ingestion before release**: rejected because it would delay shipping and expand scope beyond the initial architecture.
