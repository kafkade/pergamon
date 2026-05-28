# Known Extraction Limitations

This document tracks accepted limitations of the article extraction pipeline.
These are cases where the extractor produces suboptimal or empty results by
design, and are not considered bugs.

## SPA Shells (`spa-shell.html`)

**Behavior:** Empty or near-empty extraction.

Single-page applications render content via JavaScript. Since the extractor
operates on static HTML (no JS execution), SPA shells without server-side
rendering will produce no meaningful content.

**Mitigation:** Users should use the raw HTML snapshot for SPA content, or
rely on a headless browser to pre-render the page before extraction.

## Landing Pages / Marketing Pages (`no-article.html`)

**Behavior:** Content may be fragmented or include marketing copy.

Pages without a clear article structure (product pages, homepages, app
download pages) don't have a single dominant content block for the
readability algorithm to latch onto.

**Mitigation:** These pages are typically bookmarked, not read. The bookmark
metadata (title, URL, description) is usually sufficient.

## Paywall-Truncated Articles (`paywall-truncated.html`)

**Behavior:** Only the visible portion of the article is extracted.

The extractor captures whatever content is present in the HTML. For
paywalled articles, this is typically the first few paragraphs plus the
paywall prompt. The paywall overlay div may or may not be stripped.

**Mitigation:** None — the full content simply isn't in the HTML.

## Multiple Articles on One Page (`multiple-articles.html`)

**Behavior:** The extractor may select one article or combine content.

Aggregator pages, index pages, and "top stories" pages contain multiple
`<article>` elements. The readability algorithm selects the most likely
"main content" block, which may not match user intent.

**Mitigation:** Users should save individual article links, not index pages.

## Table-Heavy Content (`table-content.html`)

**Behavior:** Tables may be stripped or poorly formatted in plain text.

The readability algorithm sometimes scores table-heavy content as
navigation or layout elements rather than article content. When tables
survive, the plain-text representation loses column alignment.

**Mitigation:** The HTML output (`content_html`) preserves tables when the
readability algorithm keeps them. Use the HTML version for structured data.

## Newsletter HTML (`newsletter.html`)

**Behavior:** Table-based email layouts may partially extract.

Newsletters use `<table>` for layout, which can confuse readability scoring.
Content extraction quality varies significantly between newsletter formats.

**Mitigation:** Newsletter content is typically ingested via feed entries,
not via page extraction. The feed entry's content is used directly.

## Heavy Navigation Pages (`heavy-navigation.html`)

**Behavior:** Some navigation links may leak into extracted content.

Pages with extensive navigation menus, mega-menus, or footer link blocks
may have navigation content leak into the extracted article, especially
when the article content is relatively short compared to the navigation.

**Mitigation:** The quality is generally acceptable. Navigation content
is reduced but may not be completely eliminated.
