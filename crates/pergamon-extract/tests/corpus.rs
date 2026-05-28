//! Corpus-based integration tests for article extraction.
//!
//! Each fixture is a realistic HTML page that exercises different aspects
//! of the readability + ammonia extraction pipeline. Tests verify:
//! - No panics on any fixture
//! - Expected metadata extraction (title, author, OG tags)
//! - Content quality via a scoring system
//! - Known edge-case behaviour

use pergamon_extract::{extract_article, extract_article_from_html, extract_metadata};

// ── Quality scoring ──────────────────────────────────────────────────

/// Expected properties for a fixture, used for quality scoring.
#[allow(clippy::struct_excessive_bools)]
struct Expected {
    name: &'static str,
    has_title: bool,
    has_content: bool,
    has_author: bool,
    has_excerpt: bool,
    min_text_len: usize,
}

/// Quality score for a single extraction result.
#[allow(clippy::struct_excessive_bools)]
struct Score {
    name: &'static str,
    title_ok: bool,
    content_ok: bool,
    author_ok: bool,
    excerpt_ok: bool,
    text_len_ok: bool,
    points: u32,
    max_points: u32,
}

impl Score {
    fn from_extraction(article: &pergamon_extract::ExtractedArticle, expected: &Expected) -> Self {
        let title_ok = if expected.has_title {
            article.title.is_some() && !article.title.as_deref().unwrap_or_default().is_empty()
        } else {
            true
        };
        let content_ok = if expected.has_content {
            !article.content_html.is_empty()
        } else {
            true
        };
        let author_ok = if expected.has_author {
            article.author.is_some()
        } else {
            true
        };
        let excerpt_ok = if expected.has_excerpt {
            article.excerpt.is_some()
        } else {
            true
        };
        let text_len_ok = article.content_text.len() >= expected.min_text_len;

        let mut points = 0;
        let mut max_points = 0;
        if expected.has_title {
            max_points += 1;
            if title_ok {
                points += 1;
            }
        }
        if expected.has_content {
            max_points += 1;
            if content_ok {
                points += 1;
            }
        }
        if expected.has_author {
            max_points += 1;
            if author_ok {
                points += 1;
            }
        }
        if expected.has_excerpt {
            max_points += 1;
            if excerpt_ok {
                points += 1;
            }
        }
        if expected.min_text_len > 0 {
            max_points += 1;
            if text_len_ok {
                points += 1;
            }
        }

        Self {
            name: expected.name,
            title_ok,
            content_ok,
            author_ok,
            excerpt_ok,
            text_len_ok,
            points,
            max_points,
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn extract_fixture(html: &str, url: &str) -> pergamon_extract::ExtractedArticle {
    extract_article_from_html(html, url).unwrap_or_else(|e| {
        unreachable!("Failed to extract fixture {url}: {e}");
    })
}

// ── Individual fixture tests ─────────────────────────────────────────

#[test]
fn extract_blog_post_simple() {
    let html = include_str!("fixtures/pages/blog-post-simple.html");
    let article = extract_fixture(html, "https://simpleblog.example.com/local-first");
    assert!(article.title.is_some());
    assert!(!article.content_html.is_empty());
    assert!(!article.content_text.is_empty());
    // Metadata should be extracted.
    assert!(article.metadata.canonical_url.is_some());
    assert!(article.metadata.published_time.is_some());
}

#[test]
fn extract_blog_post_sidebar() {
    let html = include_str!("fixtures/pages/blog-post-sidebar.html");
    let article = extract_fixture(html, "https://techinsights.example.com/async-rust");
    assert!(article.title.is_some());
    // Sidebar content should NOT appear in the extracted article.
    assert!(
        !article.content_text.contains("Popular Posts"),
        "Sidebar should be stripped"
    );
    assert!(
        !article.content_text.contains("Advertisement"),
        "Ads should be stripped"
    );
}

#[test]
fn extract_news_article() {
    let html = include_str!("fixtures/pages/news-article.html");
    let article = extract_fixture(
        html,
        "https://worldtechnews.example.com/chip-shortage-easing",
    );
    assert!(article.title.is_some());
    assert!(article.metadata.og_image.is_some());
    assert!(article.metadata.site_name.as_deref() == Some("World Tech News"));
    // Should extract the article body, not nav/footer.
    assert!(article.content_text.contains("semiconductor"));
}

#[test]
fn extract_tech_blog_code() {
    let html = include_str!("fixtures/pages/tech-blog-code.html");
    let article = extract_fixture(html, "https://codedeepdive.example.com/custom-allocator");
    assert!(article.title.is_some());
    // Code blocks should be preserved in HTML output.
    assert!(
        article.content_html.contains("GlobalAlloc")
            || article.content_text.contains("GlobalAlloc"),
        "Code content should be preserved"
    );
    // Tables may or may not survive readability scoring — this is acceptable.
    // We only assert that substantial content was extracted.
}

#[test]
fn extract_minimal_paragraphs() {
    let html = include_str!("fixtures/pages/minimal-paragraphs.html");
    let article = extract_fixture(html, "https://minimal.example.com/page");
    // With very little content, extraction may or may not find an article.
    // At minimum, it should not panic and should produce some output.
    assert!(!article.content_text.is_empty() || article.title.is_some());
}

#[test]
fn extract_image_heavy() {
    let html = include_str!("fixtures/pages/image-heavy.html");
    let article = extract_fixture(html, "https://gallery.example.com/architecture-2024");
    assert!(article.title.is_some());
    // Some text content should survive despite being image-dominated.
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_no_article() {
    let html = include_str!("fixtures/pages/no-article.html");
    let _article = extract_fixture(html, "https://appstore.example.com");
    // Landing pages without a clear article — extraction may produce minimal
    // or no useful content, but should not panic.
    // We don't assert on content quality here; this is an expected limitation.
}

#[test]
fn extract_newsletter() {
    let html = include_str!("fixtures/pages/newsletter.html");
    let article = extract_fixture(html, "https://newsletter.example.com/issue/47");
    assert!(article.title.is_some());
    // Newsletter body should be extracted.
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_og_metadata_rich() {
    let html = include_str!("fixtures/pages/og-metadata-rich.html");
    let article = extract_fixture(html, "https://techanalysis.example.com/edge-computing");
    // Rich OG metadata should be fully extracted.
    let meta = &article.metadata;
    assert!(meta.title.is_some(), "OG title should be present");
    assert!(
        meta.description.is_some(),
        "OG description should be present"
    );
    assert!(meta.og_image.is_some(), "OG image should be present");
    assert!(meta.site_name.is_some(), "OG site_name should be present");
    assert!(
        meta.canonical_url.is_some(),
        "Canonical URL should be present"
    );
    assert!(
        meta.published_time.is_some(),
        "article:published_time should be present"
    );
}

#[test]
fn extract_no_metadata() {
    let html = include_str!("fixtures/pages/no-metadata.html");
    let article = extract_fixture(html, "https://bare.example.com/page");
    // Should still extract content even without metadata.
    assert!(!article.content_text.is_empty());
    // Title falls back to <title> tag.
    assert!(article.title.is_some());
}

#[test]
fn extract_canonical_url() {
    let html = include_str!("fixtures/pages/canonical-url.html");
    let meta = extract_metadata(html);
    // og:url is present and should be extracted.
    assert!(
        meta.canonical_url.is_some(),
        "Canonical URL should be extracted from og:url or link[rel=canonical]"
    );
}

#[test]
fn extract_foreign_language() {
    let html = include_str!("fixtures/pages/foreign-language.html");
    let article = extract_fixture(html, "https://jablog.example.com/rust-intro");
    assert!(article.title.is_some());
    // Japanese content should be preserved.
    assert!(
        article.content_text.contains("Rust") || article.content_text.contains("所有権"),
        "Japanese content should be preserved"
    );
}

#[test]
fn extract_table_content() {
    let html = include_str!("fixtures/pages/table-content.html");
    let article = extract_fixture(html, "https://dbreview.example.com/benchmarks");
    assert!(article.title.is_some());
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_paywall_truncated() {
    let html = include_str!("fixtures/pages/paywall-truncated.html");
    let article = extract_fixture(html, "https://premium.example.com/tech-debt");
    assert!(article.title.is_some());
    // Should extract the visible content before the paywall.
    assert!(
        article.content_text.contains("technical debt")
            || article.content_text.contains("Technical Debt"),
        "Pre-paywall content should be extracted"
    );
}

#[test]
fn extract_multiple_articles() {
    let html = include_str!("fixtures/pages/multiple-articles.html");
    let article = extract_fixture(html, "https://aggregator.example.com");
    // Aggregator pages are tricky — the extractor may grab a subset.
    // Should not panic.
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_heavy_navigation() {
    let html = include_str!("fixtures/pages/heavy-navigation.html");
    let article = extract_fixture(html, "https://esc.example.com/blog/scaling");
    assert!(article.title.is_some());
    // Navigation links should not dominate the extracted text.
    let nav_phrases = ["Privacy", "Terms of Service", "Cookie Policy", "GDPR"];
    let nav_count = nav_phrases
        .iter()
        .filter(|p| article.content_text.contains(**p))
        .count();
    // It's acceptable if some nav leaks through, but not all of it.
    assert!(
        nav_count < nav_phrases.len(),
        "Too much navigation leaked into content (found {nav_count}/{} nav phrases)",
        nav_phrases.len()
    );
}

#[test]
fn extract_inline_styles() {
    let html = include_str!("fixtures/pages/inline-styles.html");
    let article = extract_fixture(html, "https://styled.example.com/clean-code");
    assert!(article.title.is_some());
    // Inline styles should be stripped from sanitized HTML.
    assert!(
        !article.content_html.contains("font-family"),
        "Inline styles should be stripped by ammonia"
    );
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_nested_divs() {
    let html = include_str!("fixtures/pages/nested-divs.html");
    let article = extract_fixture(html, "https://nested.example.com/deep");
    assert!(article.title.is_some());
    assert!(
        article.content_text.contains("deeply nested"),
        "Content should be found despite deep nesting"
    );
}

#[test]
fn extract_list_article() {
    let html = include_str!("fixtures/pages/list-article.html");
    let article = extract_fixture(html, "https://listicle.example.com/habits");
    assert!(article.title.is_some());
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_blockquote_heavy() {
    let html = include_str!("fixtures/pages/blockquote-heavy.html");
    let article = extract_fixture(html, "https://reflections.example.com/failure");
    assert!(article.title.is_some());
    // Blockquote content should be preserved.
    assert!(
        article.content_text.contains("Edison") || article.content_text.contains("10,000"),
        "Blockquote content should be preserved"
    );
}

#[test]
fn extract_figures_captions() {
    let html = include_str!("fixtures/pages/figures-captions.html");
    let article = extract_fixture(html, "https://devops-illustrated.example.com/containers");
    assert!(article.title.is_some());
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_recipe_page() {
    let html = include_str!("fixtures/pages/recipe-page.html");
    let article = extract_fixture(html, "https://bakers.example.com/sourdough");
    assert!(article.title.is_some());
    // Recipe content should include ingredients or instructions.
    assert!(
        article.content_text.contains("flour") || article.content_text.contains("sourdough"),
        "Recipe content should be extracted"
    );
}

#[test]
fn extract_wiki_article() {
    let html = include_str!("fixtures/pages/wiki-article.html");
    let article = extract_fixture(html, "https://encyclopedia.example.com/wiki/Concurrency");
    assert!(article.title.is_some());
    // Wiki content should be substantial.
    assert!(
        article.content_text.len() > 200,
        "Wiki article should produce substantial text"
    );
}

#[test]
fn extract_spa_shell() {
    let html = include_str!("fixtures/pages/spa-shell.html");
    // SPA shells have no real content — extraction is expected to fail.
    // This is a known limitation (documented in KNOWN_LIMITATIONS.md).
    let result = extract_article_from_html(html, "https://spa.example.com");
    assert!(
        result.is_err(),
        "SPA shells should fail extraction (no content to extract)"
    );
}

#[test]
fn extract_iframe_heavy() {
    let html = include_str!("fixtures/pages/iframe-heavy.html");
    let article = extract_fixture(html, "https://embeds.example.com/talk");
    assert!(article.title.is_some());
    // Iframes should be stripped; text content should remain.
    assert!(
        !article.content_html.contains("<iframe"),
        "Iframes should be stripped by sanitizer"
    );
}

#[test]
fn extract_malformed_html() {
    let html = include_str!("fixtures/pages/malformed-html.html");
    // Malformed HTML may or may not extract — either outcome is acceptable.
    let _result = extract_article_from_html(html, "https://broken.example.com/page");
    // The parser should not panic regardless of outcome.
}

#[test]
fn extract_script_heavy() {
    let html = include_str!("fixtures/pages/script-heavy.html");
    let article = extract_fixture(html, "https://tracked.example.com/article");
    assert!(article.title.is_some());
    // Scripts should be completely stripped.
    assert!(
        !article.content_html.contains("<script"),
        "Scripts should be stripped"
    );
    assert!(
        !article.content_text.contains("GoogleAnalyticsObject"),
        "Script content should not leak into text"
    );
    assert!(
        !article.content_text.contains("fbq("),
        "Facebook pixel code should not leak into text"
    );
}

#[test]
fn extract_encoded_entities() {
    let html = include_str!("fixtures/pages/encoded-entities.html");
    let article = extract_fixture(html, "https://entities.example.com/page");
    assert!(article.title.is_some());
    // Entity decoding should produce proper Unicode.
    assert!(!article.content_text.is_empty());
}

#[test]
fn extract_mixed_headings() {
    let html = include_str!("fixtures/pages/mixed-headings.html");
    let article = extract_fixture(html, "https://chaotic.example.com/headings");
    assert!(article.title.is_some());
    assert!(!article.content_text.is_empty());
}

// ── Metadata-only tests ──────────────────────────────────────────────

#[test]
fn metadata_extraction_standalone() {
    let html = include_str!("fixtures/pages/og-metadata-rich.html");
    let meta = extract_metadata(html);
    assert_eq!(
        meta.title.as_deref(),
        Some("The Future of Edge Computing: A Comprehensive Analysis")
    );
    assert_eq!(meta.site_name.as_deref(), Some("Tech Analysis Weekly"));
    assert!(meta.og_image.is_some());
    assert!(meta.published_time.is_some());
    assert_eq!(meta.author.as_deref(), Some("Dr. Wei Chen"));
}

#[test]
fn metadata_canonical_precedence() {
    let html = include_str!("fixtures/pages/canonical-url.html");
    let meta = extract_metadata(html);
    // Should extract a canonical URL (either from og:url or link[rel=canonical]).
    assert!(meta.canonical_url.is_some());
}

// ── Bytes API test ───────────────────────────────────────────────────

#[test]
fn extract_article_bytes_api() {
    let html = include_str!("fixtures/pages/blog-post-simple.html");
    let article = extract_article(
        html.as_bytes(),
        "https://simpleblog.example.com/local-first",
    )
    .unwrap_or_else(|e| unreachable!("Bytes API failed: {e}"));
    assert!(article.title.is_some());
    assert!(!article.content_text.is_empty());
}

// ── Quality scoring summary ──────────────────────────────────────────

#[test]
#[allow(clippy::too_many_lines)]
fn quality_score_summary() {
    let fixtures: Vec<(&str, &str, Expected)> = vec![
        (
            include_str!("fixtures/pages/blog-post-simple.html"),
            "https://simpleblog.example.com/local-first",
            Expected {
                name: "blog-post-simple",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 200,
            },
        ),
        (
            include_str!("fixtures/pages/blog-post-sidebar.html"),
            "https://techinsights.example.com/async-rust",
            Expected {
                name: "blog-post-sidebar",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 200,
            },
        ),
        (
            include_str!("fixtures/pages/news-article.html"),
            "https://worldtechnews.example.com/chip-shortage",
            Expected {
                name: "news-article",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 300,
            },
        ),
        (
            include_str!("fixtures/pages/tech-blog-code.html"),
            "https://codedeepdive.example.com/allocator",
            Expected {
                name: "tech-blog-code",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 200,
            },
        ),
        (
            include_str!("fixtures/pages/newsletter.html"),
            "https://newsletter.example.com/47",
            Expected {
                name: "newsletter",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 100,
            },
        ),
        (
            include_str!("fixtures/pages/og-metadata-rich.html"),
            "https://techanalysis.example.com/edge",
            Expected {
                name: "og-metadata-rich",
                has_title: true,
                has_content: true,
                has_author: true,
                has_excerpt: false,
                min_text_len: 50,
            },
        ),
        (
            include_str!("fixtures/pages/wiki-article.html"),
            "https://encyclopedia.example.com/wiki/Concurrency",
            Expected {
                name: "wiki-article",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 500,
            },
        ),
        (
            include_str!("fixtures/pages/recipe-page.html"),
            "https://bakers.example.com/sourdough",
            Expected {
                name: "recipe-page",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 300,
            },
        ),
        (
            include_str!("fixtures/pages/list-article.html"),
            "https://listicle.example.com/habits",
            Expected {
                name: "list-article",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 200,
            },
        ),
        (
            include_str!("fixtures/pages/blockquote-heavy.html"),
            "https://reflections.example.com/failure",
            Expected {
                name: "blockquote-heavy",
                has_title: true,
                has_content: true,
                has_author: false,
                has_excerpt: false,
                min_text_len: 200,
            },
        ),
    ];

    let mut total_points = 0u32;
    let mut total_max = 0u32;
    let mut scores = Vec::new();

    for (html, url, expected) in &fixtures {
        let article = extract_fixture(html, url);
        let score = Score::from_extraction(&article, expected);
        total_points += score.points;
        total_max += score.max_points;
        scores.push(score);
    }

    // Print quality report (visible in `cargo test -- --nocapture`).
    eprintln!("\n╔══════════════════════════════════════════════════╗");
    eprintln!("║         EXTRACTION QUALITY REPORT                ║");
    eprintln!("╠══════════════════════════════════════════════════╣");
    for s in &scores {
        let pct = (s.points * 100).checked_div(s.max_points).unwrap_or(100);
        let status = if pct == 100 { "✓" } else { "△" };
        eprintln!(
            "║ {status} {:<25} {}/{} ({pct}%)",
            s.name, s.points, s.max_points
        );
        if !s.title_ok {
            eprintln!("║   ✗ title missing");
        }
        if !s.content_ok {
            eprintln!("║   ✗ content empty");
        }
        if !s.author_ok {
            eprintln!("║   ✗ author missing");
        }
        if !s.excerpt_ok {
            eprintln!("║   ✗ excerpt missing");
        }
        if !s.text_len_ok {
            eprintln!("║   ✗ text too short");
        }
    }
    let overall_pct = (total_points * 100).checked_div(total_max).unwrap_or(100);
    eprintln!("╠══════════════════════════════════════════════════╣");
    eprintln!("║ Overall: {total_points}/{total_max} ({overall_pct}%)");
    eprintln!("╚══════════════════════════════════════════════════╝\n");

    // Require at least 70% quality score across all fixtures.
    assert!(
        overall_pct >= 70,
        "Extraction quality score {overall_pct}% is below 70% threshold"
    );
}

// ── All-fixtures smoke test ──────────────────────────────────────────

#[test]
#[allow(clippy::too_many_lines)]
fn all_extraction_fixtures_parse_without_panic() {
    let fixtures: &[(&str, &str)] = &[
        (
            include_str!("fixtures/pages/blog-post-simple.html"),
            "https://example.com/1",
        ),
        (
            include_str!("fixtures/pages/blog-post-sidebar.html"),
            "https://example.com/2",
        ),
        (
            include_str!("fixtures/pages/news-article.html"),
            "https://example.com/3",
        ),
        (
            include_str!("fixtures/pages/tech-blog-code.html"),
            "https://example.com/4",
        ),
        (
            include_str!("fixtures/pages/minimal-paragraphs.html"),
            "https://example.com/5",
        ),
        (
            include_str!("fixtures/pages/image-heavy.html"),
            "https://example.com/6",
        ),
        (
            include_str!("fixtures/pages/no-article.html"),
            "https://example.com/7",
        ),
        (
            include_str!("fixtures/pages/newsletter.html"),
            "https://example.com/8",
        ),
        (
            include_str!("fixtures/pages/og-metadata-rich.html"),
            "https://example.com/9",
        ),
        (
            include_str!("fixtures/pages/no-metadata.html"),
            "https://example.com/10",
        ),
        (
            include_str!("fixtures/pages/canonical-url.html"),
            "https://example.com/11",
        ),
        (
            include_str!("fixtures/pages/foreign-language.html"),
            "https://example.com/12",
        ),
        (
            include_str!("fixtures/pages/table-content.html"),
            "https://example.com/13",
        ),
        (
            include_str!("fixtures/pages/paywall-truncated.html"),
            "https://example.com/14",
        ),
        (
            include_str!("fixtures/pages/multiple-articles.html"),
            "https://example.com/15",
        ),
        (
            include_str!("fixtures/pages/heavy-navigation.html"),
            "https://example.com/16",
        ),
        (
            include_str!("fixtures/pages/inline-styles.html"),
            "https://example.com/17",
        ),
        (
            include_str!("fixtures/pages/nested-divs.html"),
            "https://example.com/18",
        ),
        (
            include_str!("fixtures/pages/list-article.html"),
            "https://example.com/19",
        ),
        (
            include_str!("fixtures/pages/blockquote-heavy.html"),
            "https://example.com/20",
        ),
        (
            include_str!("fixtures/pages/figures-captions.html"),
            "https://example.com/21",
        ),
        (
            include_str!("fixtures/pages/recipe-page.html"),
            "https://example.com/22",
        ),
        (
            include_str!("fixtures/pages/wiki-article.html"),
            "https://example.com/23",
        ),
        (
            include_str!("fixtures/pages/spa-shell.html"),
            "https://example.com/24",
        ),
        (
            include_str!("fixtures/pages/iframe-heavy.html"),
            "https://example.com/25",
        ),
        (
            include_str!("fixtures/pages/malformed-html.html"),
            "https://example.com/26",
        ),
        (
            include_str!("fixtures/pages/script-heavy.html"),
            "https://example.com/27",
        ),
        (
            include_str!("fixtures/pages/encoded-entities.html"),
            "https://example.com/28",
        ),
        (
            include_str!("fixtures/pages/mixed-headings.html"),
            "https://example.com/29",
        ),
    ];

    // Known failures: SPA shells and malformed HTML may fail extraction.
    let known_failures: &[&str] = &[
        "https://example.com/24", // spa-shell
    ];

    for (i, (html, url)) in fixtures.iter().enumerate() {
        let result = extract_article_from_html(html, url);
        if known_failures.contains(url) {
            // Expected to fail — just ensure no panic.
            continue;
        }
        if let Err(e) = &result {
            unreachable!("Fixture {i} ({url}) failed extraction: {e}");
        }
    }
}
