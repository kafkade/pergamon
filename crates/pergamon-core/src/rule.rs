//! Content rule engine for automatic organization.
//!
//! Rules match incoming (or existing) content items against a filter and
//! apply actions: tagging, status changes, collection membership, or muting.
//!
//! This module is pure computation — no I/O, no SQL. The storage and CLI
//! layers handle persistence and execution.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::model::ContentItem;
use crate::smart_filter::{FilterPredicate, SmartFilter};
use crate::status::DocumentStatus;

/// A content rule definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRule {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Human-readable rule name.
    pub name: String,
    /// Whether the rule is active.
    pub enabled: bool,
    /// Execution priority (lower runs first).
    pub priority: i32,
    /// Filter query string (DSL syntax).
    pub filter_query: String,
    /// Actions to apply when the filter matches.
    pub actions: Vec<RuleAction>,
    /// When this rule was created.
    pub created_at: OffsetDateTime,
    /// When this rule was last updated.
    pub updated_at: OffsetDateTime,
}

/// An action to apply when a rule matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum RuleAction {
    /// Add a tag to the item.
    AddTag(String),
    /// Set the item's status.
    SetStatus(DocumentStatus),
    /// Add the item to a collection (by name, created if missing).
    AddToCollection(String),
    /// Mute: immediately archive, suppressing from inbox.
    Mute,
}

impl std::fmt::Display for RuleAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddTag(tag) => write!(f, "tag:{tag}"),
            Self::SetStatus(status) => write!(f, "status:{}", status.as_str()),
            Self::AddToCollection(name) => write!(f, "collection:{name}"),
            Self::Mute => f.write_str("mute"),
        }
    }
}

/// Parse a rule action from CLI syntax like `tag:rust`, `status:later`, `mute`.
impl std::str::FromStr for RuleAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "mute" {
            return Ok(Self::Mute);
        }
        let (key, value) = s
            .split_once(':')
            .ok_or_else(|| format!("action must be key:value or 'mute', got: {s}"))?;
        match key {
            "tag" => Ok(Self::AddTag(value.to_owned())),
            "status" => {
                let status: DocumentStatus = value
                    .parse()
                    .map_err(|_| format!("unknown status: {value}"))?;
                Ok(Self::SetStatus(status))
            }
            "collection" => Ok(Self::AddToCollection(value.to_owned())),
            _ => Err(format!("unknown action type: {key}")),
        }
    }
}

/// Context for in-memory rule matching.
///
/// Provides the data needed to evaluate filter predicates without SQL.
/// Tags are mutable because earlier rules may add tags that later rules
/// can match against.
#[derive(Debug)]
pub struct MatchContext<'a> {
    /// The content item being evaluated.
    pub item: &'a ContentItem,
    /// Tags currently on the item (accumulated during rule evaluation).
    pub tags: Vec<String>,
    /// Title of the source feed (if the item came from a feed).
    pub feed_title: Option<String>,
}

/// A planned action from rule evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAction {
    /// Which rule produced this action.
    pub rule_id: Uuid,
    /// The rule's name (for logging).
    pub rule_name: String,
    /// The action to apply.
    pub action: RuleAction,
}

/// Evaluate all enabled rules against a content item.
///
/// Rules are evaluated in priority order (lowest first). Each matched
/// rule's `AddTag` actions update the context so later rules can match
/// on newly-added tags. Multiple `SetStatus` actions: last one wins.
///
/// Returns a list of planned actions in execution order.
pub fn evaluate_rules(ctx: &mut MatchContext<'_>, rules: &[ContentRule]) -> Vec<PlannedAction> {
    let mut planned = Vec::new();
    let mut sorted: Vec<&ContentRule> = rules.iter().filter(|r| r.enabled).collect();
    sorted.sort_by_key(|r| r.priority);

    for rule in sorted {
        let Ok(filter) = SmartFilter::parse(&rule.filter_query) else {
            continue;
        };

        if matches_filter(ctx, &filter) {
            for action in &rule.actions {
                // Update context for chaining: tag additions are visible
                // to subsequent rules.
                if let RuleAction::AddTag(tag) = action {
                    if !ctx.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                        ctx.tags.push(tag.clone());
                    }
                }

                planned.push(PlannedAction {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    action: action.clone(),
                });
            }
        }
    }

    planned
}

/// Check whether all predicates in a filter match the context.
///
/// Uses in-memory evaluation (no SQL). `text:` uses case-insensitive
/// substring matching (not FTS5 MATCH).
#[must_use]
pub fn matches_filter(ctx: &MatchContext<'_>, filter: &SmartFilter) -> bool {
    filter
        .predicates()
        .iter()
        .all(|p| matches_predicate(ctx, p))
}

/// Evaluate a single predicate against the context.
fn matches_predicate(ctx: &MatchContext<'_>, pred: &FilterPredicate) -> bool {
    match pred {
        FilterPredicate::ContentType(types) => types.contains(&ctx.item.content_type),
        FilterPredicate::Tag(tags) => tags
            .iter()
            .any(|t| ctx.tags.iter().any(|ct| ct.eq_ignore_ascii_case(t))),
        FilterPredicate::Status(statuses) => statuses.contains(&ctx.item.status),
        FilterPredicate::ExcludeStatus(statuses) => !statuses.contains(&ctx.item.status),
        FilterPredicate::Source(src) => ctx
            .feed_title
            .as_ref()
            .is_some_and(|ft| ft.to_lowercase().contains(&src.to_lowercase())),
        FilterPredicate::CreatedSince(date) => {
            let threshold = format!("{date}T00:00:00Z");
            format_time(ctx.item.created_at) >= threshold
        }
        FilterPredicate::CreatedBefore(date) => {
            let threshold = format!("{date}T00:00:00Z");
            format_time(ctx.item.created_at) < threshold
        }
        FilterPredicate::Text(query) => {
            let q = query.to_lowercase();
            let title_match = ctx.item.title.to_lowercase().contains(&q);
            let content_match = ctx
                .item
                .content_text
                .as_ref()
                .is_some_and(|t| t.to_lowercase().contains(&q));
            title_match || content_match
        }
    }
}

/// Format a timestamp as ISO 8601 for comparison.
fn format_time(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Statuses considered "protected" — auto-archive skips these.
pub const PROTECTED_STATUSES: &[DocumentStatus] = &[
    DocumentStatus::Later,
    DocumentStatus::Reference,
    DocumentStatus::Reading,
];

/// Check whether an item is protected from auto-archive.
#[must_use]
pub fn is_protected(item: &ContentItem) -> bool {
    PROTECTED_STATUSES.contains(&item.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_type::ContentType;

    fn test_item(title: &str, status: DocumentStatus, ct: ContentType) -> ContentItem {
        let now = OffsetDateTime::now_utc();
        ContentItem {
            id: Uuid::new_v4(),
            url: Some("https://example.com/test".to_owned()),
            title: title.to_owned(),
            author: None,
            content_type: ct,
            status,
            content_text: Some("Rust programming guide".to_owned()),
            excerpt: None,
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        }
    }

    fn test_rule(name: &str, filter: &str, actions: Vec<RuleAction>, priority: i32) -> ContentRule {
        let now = OffsetDateTime::now_utc();
        ContentRule {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            enabled: true,
            priority,
            filter_query: filter.to_owned(),
            actions,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn rule_matches_content_type() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Article);
        let rule = test_rule(
            "Articles get tagged",
            "type:article",
            vec![RuleAction::AddTag("reading".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action, RuleAction::AddTag("reading".to_owned()));
    }

    #[test]
    fn rule_does_not_match_wrong_type() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Bookmark);
        let rule = test_rule(
            "Articles only",
            "type:article",
            vec![RuleAction::AddTag("read".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert!(actions.is_empty());
    }

    #[test]
    fn rule_matches_source() {
        let item = test_item("Post", DocumentStatus::Inbox, ContentType::FeedItem);
        let rule = test_rule(
            "HN tagger",
            "source:hacker",
            vec![RuleAction::AddTag("tech".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: Some("Hacker News".to_owned()),
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn rule_matches_text_in_title() {
        let item = test_item("Rust Guide", DocumentStatus::Inbox, ContentType::Article);
        let rule = test_rule(
            "Rust tagger",
            "text:rust",
            vec![RuleAction::AddTag("rust".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn rule_matches_text_in_content() {
        let item = test_item("Guide", DocumentStatus::Inbox, ContentType::Article);
        let rule = test_rule(
            "Rust content",
            "text:programming",
            vec![RuleAction::AddTag("dev".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn mute_action() {
        let item = test_item("Noisy", DocumentStatus::Inbox, ContentType::FeedItem);
        let rule = test_rule("Mute noisy", "source:noisy", vec![RuleAction::Mute], 0);
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: Some("Noisy Blog".to_owned()),
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action, RuleAction::Mute);
    }

    #[test]
    fn disabled_rules_skipped() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Article);
        let mut rule = test_rule(
            "Disabled",
            "type:article",
            vec![RuleAction::AddTag("x".to_owned())],
            0,
        );
        rule.enabled = false;
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert!(actions.is_empty());
    }

    #[test]
    fn priority_ordering() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Article);
        let rule_low = test_rule(
            "Low priority",
            "type:article",
            vec![RuleAction::AddTag("second".to_owned())],
            10,
        );
        let rule_high = test_rule(
            "High priority",
            "type:article",
            vec![RuleAction::AddTag("first".to_owned())],
            1,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        // Pass in reverse order to verify sorting.
        let actions = evaluate_rules(&mut ctx, &[rule_low, rule_high]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].rule_name, "High priority");
        assert_eq!(actions[1].rule_name, "Low priority");
    }

    #[test]
    fn chained_tag_matching() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Article);
        // Rule 1: articles get tagged "tech".
        let rule1 = test_rule(
            "Tag tech",
            "type:article",
            vec![RuleAction::AddTag("tech".to_owned())],
            0,
        );
        // Rule 2: items tagged "tech" get tagged "priority".
        let rule2 = test_rule(
            "Tech priority",
            "tag:tech",
            vec![RuleAction::AddTag("priority".to_owned())],
            1,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule1, rule2]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].action, RuleAction::AddTag("tech".to_owned()));
        assert_eq!(actions[1].action, RuleAction::AddTag("priority".to_owned()));
    }

    #[test]
    fn exclude_status_works() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::Article);
        let rule = test_rule(
            "Not discarded",
            "-status:discarded",
            vec![RuleAction::AddTag("active".to_owned())],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, std::slice::from_ref(&rule));
        assert_eq!(actions.len(), 1);

        // Discarded item should NOT match.
        let discarded = test_item("Old", DocumentStatus::Discarded, ContentType::Article);
        let mut ctx2 = MatchContext {
            item: &discarded,
            tags: vec![],
            feed_title: None,
        };
        let actions2 = evaluate_rules(&mut ctx2, std::slice::from_ref(&rule));
        assert!(actions2.is_empty());
    }

    #[test]
    fn protected_status_check() {
        let inbox = test_item("Inbox", DocumentStatus::Inbox, ContentType::Article);
        let later = test_item("Later", DocumentStatus::Later, ContentType::Article);
        let reference = test_item("Ref", DocumentStatus::Reference, ContentType::Article);
        let reading = test_item("Reading", DocumentStatus::Reading, ContentType::Article);

        assert!(!is_protected(&inbox));
        assert!(is_protected(&later));
        assert!(is_protected(&reference));
        assert!(is_protected(&reading));
    }

    #[test]
    fn rule_action_parse_round_trip() {
        let cases = [
            ("tag:rust", RuleAction::AddTag("rust".to_owned())),
            ("status:later", RuleAction::SetStatus(DocumentStatus::Later)),
            (
                "collection:reading-list",
                RuleAction::AddToCollection("reading-list".to_owned()),
            ),
            ("mute", RuleAction::Mute),
        ];
        for (input, expected) in &cases {
            let parsed: RuleAction = input.parse().unwrap_or_else(|e| {
                let _ = e;
                unreachable!("failed to parse {input:?}")
            });
            assert_eq!(&parsed, expected);
            let displayed = parsed.to_string();
            let reparsed: RuleAction = displayed.parse().unwrap_or_else(|e| {
                let _ = e;
                unreachable!("failed to reparse {displayed:?}")
            });
            assert_eq!(&reparsed, expected);
        }
    }

    #[test]
    fn rule_action_parse_errors() {
        assert!("".parse::<RuleAction>().is_err());
        assert!("unknown:value".parse::<RuleAction>().is_err());
        assert!("status:invalid".parse::<RuleAction>().is_err());
    }

    #[test]
    fn multiple_actions_per_rule() {
        let item = test_item("Test", DocumentStatus::Inbox, ContentType::FeedItem);
        let rule = test_rule(
            "Multi-action",
            "type:feed_item",
            vec![
                RuleAction::AddTag("news".to_owned()),
                RuleAction::SetStatus(DocumentStatus::Later),
            ],
            0,
        );
        let mut ctx = MatchContext {
            item: &item,
            tags: vec![],
            feed_title: None,
        };
        let actions = evaluate_rules(&mut ctx, &[rule]);
        assert_eq!(actions.len(), 2);
    }
}
