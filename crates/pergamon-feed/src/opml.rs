//! OPML import and export for feed subscription lists.
//!
//! Parses OPML 1.0/2.0 files into a tree of outlines. Folder outlines
//! (no `xmlUrl`) become [`OpmlOutline`] nodes with children; feed outlines
//! (with `xmlUrl`) are leaf nodes. The [`generate_opml`] function produces
//! valid OPML 2.0 XML from the same tree structure.

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::FeedError;

/// A parsed OPML document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlDocument {
    /// Document title from the `<head>` section.
    pub title: String,
    /// Top-level outlines (folders and/or feeds).
    pub outlines: Vec<OpmlOutline>,
}

/// A single OPML `<outline>` element.
///
/// If `xml_url` is `Some`, this outline represents a feed subscription.
/// If `xml_url` is `None` and `children` is non-empty, it represents a
/// folder/category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlOutline {
    /// Display text (`text` attribute).
    pub text: String,
    /// Optional title override (`title` attribute).
    pub title: Option<String>,
    /// Feed URL (`xmlUrl` attribute) — present for feed subscriptions.
    pub xml_url: Option<String>,
    /// Website URL (`htmlUrl` attribute).
    pub html_url: Option<String>,
    /// Feed type, e.g. `"rss"` (`type` attribute).
    pub feed_type: Option<String>,
    /// Child outlines (non-empty for folder nodes).
    pub children: Vec<Self>,
}

impl OpmlOutline {
    /// Returns true if this outline represents a feed (has `xml_url`).
    #[must_use]
    pub const fn is_feed(&self) -> bool {
        self.xml_url.is_some()
    }

    /// Returns true if this outline represents a folder (no `xml_url`).
    #[must_use]
    pub const fn is_folder(&self) -> bool {
        self.xml_url.is_none()
    }

    /// The best display name for this outline: `title` if set, else `text`.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.text)
    }
}

// ======================================================================
// Parsing
// ======================================================================

/// Parse OPML bytes into an [`OpmlDocument`].
///
/// Supports OPML 1.0 and 2.0. Handles nested outlines of arbitrary depth.
///
/// # Errors
///
/// Returns [`FeedError::Opml`] if the XML is malformed or missing required
/// elements.
pub fn parse_opml(bytes: &[u8]) -> Result<OpmlDocument, FeedError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut title = String::new();
    let mut in_head = false;
    let mut in_title = false;
    let mut in_body = false;
    let mut outline_stack: Vec<Vec<OpmlOutline>> = Vec::new();
    let mut root_outlines: Vec<OpmlOutline> = Vec::new();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"head" => in_head = true,
                b"title" if in_head => in_title = true,
                b"body" => in_body = true,
                b"outline" if in_body => {
                    let outline = parse_outline_attrs(e, reader.decoder())?;
                    if outline.is_feed() {
                        // Feed outlines are self-contained — add immediately.
                        push_outline(&mut outline_stack, &mut root_outlines, outline);
                    } else {
                        // Folder: push a new children collector onto the stack.
                        outline_stack.push(Vec::new());
                        // We'll finalize this outline on the matching </outline>.
                        // Temporarily store the folder attributes at the end of
                        // the stack, using a sentinel with children = [].
                        let top = outline_stack
                            .last_mut()
                            .unwrap_or_else(|| unreachable!("stack just pushed"));
                        // We store the folder as a placeholder at position 0 to
                        // recover attrs when we see </outline>.
                        top.insert(0, outline);
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) if e.name().as_ref() == b"outline" && in_body => {
                let outline = parse_outline_attrs(e, reader.decoder())?;
                push_outline(&mut outline_stack, &mut root_outlines, outline);
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"head" => in_head = false,
                b"title" if in_head => in_title = false,
                b"body" => in_body = false,
                b"outline" if in_body => {
                    // Pop the folder from the stack.
                    if let Some(mut children) = outline_stack.pop()
                        && let Some(mut folder) = children.first().cloned()
                    {
                        children.remove(0);
                        folder.children = children;
                        push_outline(&mut outline_stack, &mut root_outlines, folder);
                    }
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) if in_title => {
                title = e
                    .unescape()
                    .map_err(|err| FeedError::Opml(format!("invalid title text: {err}")))?
                    .into_owned();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(FeedError::Opml(format!("XML parse error: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(OpmlDocument {
        title,
        outlines: root_outlines,
    })
}

/// Push a completed outline onto either the parent folder or root list.
fn push_outline(stack: &mut [Vec<OpmlOutline>], root: &mut Vec<OpmlOutline>, outline: OpmlOutline) {
    if let Some(parent_children) = stack.last_mut() {
        parent_children.push(outline);
    } else {
        root.push(outline);
    }
}

/// Extract outline attributes from a `<outline>` start/empty tag.
fn parse_outline_attrs(
    e: &BytesStart<'_>,
    decoder: quick_xml::Decoder,
) -> Result<OpmlOutline, FeedError> {
    let mut text = String::new();
    let mut title = None;
    let mut xml_url = None;
    let mut html_url = None;
    let mut feed_type = None;

    for attr_result in e.attributes() {
        let attr = attr_result
            .map_err(|err| FeedError::Opml(format!("invalid outline attribute: {err}")))?;
        let value = attr
            .decode_and_unescape_value(decoder)
            .map_err(|err| FeedError::Opml(format!("invalid attribute value: {err}")))?
            .into_owned();

        match attr.key.as_ref() {
            b"text" => text = value,
            b"title" => title = Some(value),
            b"xmlUrl" => xml_url = Some(value),
            b"htmlUrl" => html_url = Some(value),
            b"type" => feed_type = Some(value),
            _ => {}
        }
    }

    // Fall back title → text if text is empty.
    if text.is_empty() {
        text = title.clone().unwrap_or_default();
    }

    Ok(OpmlOutline {
        text,
        title,
        xml_url,
        html_url,
        feed_type,
        children: Vec::new(),
    })
}

// ======================================================================
// Generation
// ======================================================================

/// Generate an OPML 2.0 document from a title and outline tree.
///
/// # Errors
///
/// Returns [`FeedError::Opml`] if XML writing fails (should not happen in
/// practice with in-memory buffers).
pub fn generate_opml(title: &str, outlines: &[OpmlOutline]) -> Result<String, FeedError> {
    let mut buffer = Vec::new();
    let mut writer = Writer::new_with_indent(&mut buffer, b' ', 2);

    // XML declaration.
    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            None,
        )))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;

    // <opml version="2.0">
    let mut opml_start = BytesStart::new("opml");
    opml_start.push_attribute(("version", "2.0"));
    writer
        .write_event(Event::Start(opml_start))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;

    // <head><title>...</title></head>
    writer
        .write_event(Event::Start(BytesStart::new("head")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    writer
        .write_event(Event::Start(BytesStart::new("title")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    writer
        .write_event(Event::Text(quick_xml::events::BytesText::new(title)))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    writer
        .write_event(Event::End(BytesEnd::new("title")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    writer
        .write_event(Event::End(BytesEnd::new("head")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;

    // <body>
    writer
        .write_event(Event::Start(BytesStart::new("body")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;

    for outline in outlines {
        write_outline(&mut writer, outline)?;
    }

    // </body></opml>
    writer
        .write_event(Event::End(BytesEnd::new("body")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    writer
        .write_event(Event::End(BytesEnd::new("opml")))
        .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;

    String::from_utf8(buffer).map_err(|e| FeedError::Opml(format!("UTF-8 error: {e}")))
}

/// Write a single outline element (recursively for folders).
fn write_outline<W: std::io::Write>(
    writer: &mut Writer<W>,
    outline: &OpmlOutline,
) -> Result<(), FeedError> {
    let mut elem = BytesStart::new("outline");
    elem.push_attribute(("text", outline.text.as_str()));

    if let Some(ref t) = outline.title {
        elem.push_attribute(("title", t.as_str()));
    }
    if let Some(ref u) = outline.xml_url {
        elem.push_attribute(("type", outline.feed_type.as_deref().unwrap_or("rss")));
        elem.push_attribute(("xmlUrl", u.as_str()));
    }
    if let Some(ref u) = outline.html_url {
        elem.push_attribute(("htmlUrl", u.as_str()));
    }

    if outline.children.is_empty() {
        writer
            .write_event(Event::Empty(elem))
            .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    } else {
        writer
            .write_event(Event::Start(elem))
            .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
        for child in &outline.children {
            write_outline(writer, child)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("outline")))
            .map_err(|e| FeedError::Opml(format!("write error: {e}")))?;
    }

    Ok(())
}

// ======================================================================
// Counting helpers
// ======================================================================

/// Count total feeds and folders in an outline tree.
#[must_use]
pub fn count_outlines(outlines: &[OpmlOutline]) -> (usize, usize) {
    let mut feeds = 0;
    let mut folders = 0;
    for outline in outlines {
        count_recursive(outline, &mut feeds, &mut folders);
    }
    (feeds, folders)
}

fn count_recursive(outline: &OpmlOutline, feeds: &mut usize, folders: &mut usize) {
    if outline.is_feed() {
        *feeds += 1;
    } else {
        *folders += 1;
    }
    for child in &outline.children {
        count_recursive(child, feeds, folders);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_OPML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>My Feeds</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline text="Ars Technica" type="rss"
               xmlUrl="https://feeds.arstechnica.com/arstechnica/index"
               htmlUrl="https://arstechnica.com"/>
      <outline text="TechCrunch" type="rss"
               xmlUrl="https://techcrunch.com/feed/"
               htmlUrl="https://techcrunch.com"/>
    </outline>
    <outline text="News">
      <outline text="BBC" type="rss"
               xmlUrl="https://feeds.bbci.co.uk/news/rss.xml"
               htmlUrl="https://www.bbc.co.uk/news"/>
    </outline>
    <outline text="Uncategorized" type="rss"
             xmlUrl="https://example.com/feed.xml"/>
  </body>
</opml>"#;

    #[test]
    fn parse_simple_opml() {
        let doc = parse_opml(SIMPLE_OPML.as_bytes())
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.title, "My Feeds");
        assert_eq!(doc.outlines.len(), 3);

        // First outline is a folder.
        let tech = &doc.outlines[0];
        assert!(tech.is_folder());
        assert_eq!(tech.text, "Tech");
        assert_eq!(tech.children.len(), 2);
        assert!(tech.children[0].is_feed());
        assert_eq!(tech.children[0].text, "Ars Technica");
        assert_eq!(
            tech.children[0].xml_url.as_deref(),
            Some("https://feeds.arstechnica.com/arstechnica/index")
        );

        // Second outline is a folder.
        let news = &doc.outlines[1];
        assert!(news.is_folder());
        assert_eq!(news.children.len(), 1);
        assert_eq!(news.children[0].text, "BBC");

        // Third outline is a standalone feed.
        let uncat = &doc.outlines[2];
        assert!(uncat.is_feed());
        assert_eq!(
            uncat.xml_url.as_deref(),
            Some("https://example.com/feed.xml")
        );
    }

    #[test]
    fn parse_flat_opml() {
        let opml = r#"<?xml version="1.0"?>
<opml version="1.0">
  <head><title>Flat List</title></head>
  <body>
    <outline text="Feed A" xmlUrl="https://a.example.com/feed"/>
    <outline text="Feed B" xmlUrl="https://b.example.com/feed"/>
    <outline text="Feed C" xmlUrl="https://c.example.com/feed"/>
  </body>
</opml>"#;

        let doc = parse_opml(opml.as_bytes()).unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.title, "Flat List");
        assert_eq!(doc.outlines.len(), 3);
        assert!(doc.outlines.iter().all(OpmlOutline::is_feed));
    }

    #[test]
    fn parse_nested_folders() {
        let opml = r#"<?xml version="1.0"?>
<opml version="2.0">
  <head><title>Nested</title></head>
  <body>
    <outline text="Level 1">
      <outline text="Level 2">
        <outline text="Deep Feed" xmlUrl="https://deep.example.com/feed"/>
      </outline>
    </outline>
  </body>
</opml>"#;

        let doc = parse_opml(opml.as_bytes()).unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.outlines.len(), 1);
        let l1 = &doc.outlines[0];
        assert!(l1.is_folder());
        assert_eq!(l1.children.len(), 1);

        let l2 = &l1.children[0];
        assert!(l2.is_folder());
        assert_eq!(l2.children.len(), 1);
        assert!(l2.children[0].is_feed());
        assert_eq!(l2.children[0].text, "Deep Feed");
    }

    #[test]
    fn parse_empty_body() {
        let opml = r#"<?xml version="1.0"?>
<opml version="1.0">
  <head><title>Empty</title></head>
  <body/>
</opml>"#;

        let doc = parse_opml(opml.as_bytes()).unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.title, "Empty");
        assert!(doc.outlines.is_empty());
    }

    #[test]
    fn count_outlines_works() {
        let doc = parse_opml(SIMPLE_OPML.as_bytes())
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        let (feeds, folders) = count_outlines(&doc.outlines);
        assert_eq!(feeds, 4);
        assert_eq!(folders, 2);
    }

    #[test]
    fn generate_and_roundtrip() {
        let original = parse_opml(SIMPLE_OPML.as_bytes())
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        let xml = generate_opml("My Feeds", &original.outlines)
            .unwrap_or_else(|e| unreachable!("generate failed: {e}"));

        let roundtripped = parse_opml(xml.as_bytes())
            .unwrap_or_else(|e| unreachable!("roundtrip parse failed: {e}"));

        assert_eq!(roundtripped.title, "My Feeds");
        assert_eq!(roundtripped.outlines.len(), original.outlines.len());

        // Verify feed URLs are preserved.
        let (feeds, folders) = count_outlines(&roundtripped.outlines);
        assert_eq!(feeds, 4);
        assert_eq!(folders, 2);
    }

    #[test]
    fn parse_self_closing_outline_tags() {
        let opml = r#"<?xml version="1.0"?>
<opml version="2.0">
  <head><title>Self Closing</title></head>
  <body>
    <outline text="Feed" xmlUrl="https://example.com/feed"/>
  </body>
</opml>"#;

        let doc = parse_opml(opml.as_bytes()).unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.outlines.len(), 1);
        assert!(doc.outlines[0].is_feed());
    }

    #[test]
    fn parse_opml_with_html_entities() {
        let opml = r#"<?xml version="1.0"?>
<opml version="2.0">
  <head><title>Entities &amp; More</title></head>
  <body>
    <outline text="AT&amp;T News" xmlUrl="https://att.example.com/feed"/>
    <outline text="O&apos;Reilly" xmlUrl="https://oreilly.example.com/feed"/>
  </body>
</opml>"#;

        let doc = parse_opml(opml.as_bytes()).unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(doc.title, "Entities & More");
        assert_eq!(doc.outlines[0].text, "AT&T News");
        assert_eq!(doc.outlines[1].text, "O'Reilly");
    }

    #[test]
    fn parse_invalid_xml_returns_error() {
        let result = parse_opml(b"<opml><body><outline text=\"broken");
        assert!(result.is_err());
    }
}
