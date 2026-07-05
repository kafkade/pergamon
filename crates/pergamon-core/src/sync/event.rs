//! The decrypted **event body** — the plaintext semantics of one synced change.
//!
//! This is exactly what lives inside the ADR-022 ciphertext: the server never
//! sees any of it. A local mutation writes a canonical row and one of these
//! bodies (as an outbox row) in the same transaction; the sync engine encrypts
//! it, appends it to the log, and — on pull — decrypts it and applies it
//! through the ADR-023 merge policy.
//!
//! The body is deliberately **schema-generic**: fields are a name → JSON value
//! map rather than a typed struct per entity, so the same envelope, encryption,
//! and merge machinery serves every entity class. The typed mapping between a
//! domain entity and this field map lives in the storage / sync-apply layers.

use serde::{Deserialize, Serialize};

use super::hlc::Hlc;

/// The class of entity a change targets. Mirrors the ADR-022 `entity_type`
/// enum and selects the ADR-023 conflict strategy per field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// A unified content item / document (`ContentItem`). Per-field mixed strategy.
    Document,
    /// A tag entity (`name`). Per-field LWW.
    Tag,
    /// A collection entity (`name`, `parent_id`, `sort_order`, `filter_query`). Per-field LWW.
    Collection,
    /// A document↔tag membership edge. Set-union with observed-remove tombstone.
    TagEdge,
    /// A document↔collection membership edge. Set-union with observed-remove tombstone.
    CollectionEdge,
    /// A highlight annotation. Creation auto-merges; body/color is conflict-copy.
    Highlight,
    /// A note annotation. Creation auto-merges; body is conflict-copy.
    Note,
    /// A review card lifecycle flag; scheduling state is derived, never merged.
    ReviewCard,
    /// An append-only review event / log entry. Always auto-merges by id.
    ReviewLog,
    /// A feed subscription (mutable config). Per-field LWW with audit.
    FeedSubscription,
    /// An application setting (mutable config). Per-field LWW with audit.
    Settings,
}

impl EntityType {
    /// The canonical wire / storage string for this entity type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Tag => "tag",
            Self::Collection => "collection",
            Self::TagEdge => "tag_edge",
            Self::CollectionEdge => "collection_edge",
            Self::Highlight => "highlight",
            Self::Note => "note",
            Self::ReviewCard => "review_card",
            Self::ReviewLog => "review_log",
            Self::FeedSubscription => "feed_subscription",
            Self::Settings => "settings",
        }
    }

    /// Parse a canonical entity-type string.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "document" => Self::Document,
            "tag" => Self::Tag,
            "collection" => Self::Collection,
            "tag_edge" => Self::TagEdge,
            "collection_edge" => Self::CollectionEdge,
            "highlight" => Self::Highlight,
            "note" => Self::Note,
            "review_card" => Self::ReviewCard,
            "review_log" => Self::ReviewLog,
            "feed_subscription" => Self::FeedSubscription,
            "settings" => Self::Settings,
            _ => return None,
        })
    }

    /// Whether this entity class is an append-only, auto-merging log entry
    /// (ADR-023): review logs and annotation *creation*. Idempotent by id.
    #[must_use]
    pub const fn is_append_only(self) -> bool {
        matches!(self, Self::ReviewLog)
    }
}

/// The kind of mutation a change represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Create-or-replace the entity with the carried `fields`.
    Upsert,
    /// Soft-delete (tombstone) the entity.
    Delete,
    /// Patch only the carried `fields`, leaving others untouched. Enables
    /// per-field LWW so two devices editing different fields never conflict.
    FieldPatch,
}

impl Op {
    /// The canonical wire string for this op.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
            Self::FieldPatch => "field_patch",
        }
    }

    /// Parse a canonical op string.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "upsert" => Self::Upsert,
            "delete" => Self::Delete,
            "field_patch" => Self::FieldPatch,
            _ => return None,
        })
    }
}

/// One entry of an event's blob manifest: how to locate and decrypt a large
/// immutable blob the event references (ADR-022 `blob_manifest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobManifestEntry {
    /// Ciphertext hash — the content address the blob is stored under.
    pub ct_hash: String,
    /// What role the blob plays for the entity (e.g. `raw_html`, `pdf`,
    /// `extracted_text`), so the client knows which field it satisfies.
    pub role: String,
    /// BLAKE3 hash of the *plaintext*, lowercase hex — the key input for the
    /// convergent blob key (`pergamon-crypto`), so the puller can decrypt it.
    pub plaintext_hash: String,
    /// Plaintext length in bytes, for allocation and integrity sanity checks.
    pub plaintext_len: u64,
}

/// The decrypted body of one sync event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBody {
    /// Which entity class this change targets.
    pub entity_type: EntityType,
    /// The stable domain id of the target entity (UUID string, or a composite
    /// edge id like `document_id:tag_id` for membership edges).
    pub entity_id: String,
    /// The mutation kind.
    pub op: Op,
    /// The HLC stamp of this change — its position in the total order.
    pub clock: Hlc,
    /// The entity/field version the writer observed before making this change.
    /// Concurrency (a *conflict*) is detected when this does not match the
    /// version the change is applied onto (ADR-023). `None` for a create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_version: Option<Hlc>,
    /// The changed fields (all fields for `upsert`, only the patched ones for
    /// `field_patch`, empty for `delete`).
    #[serde(default)]
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// Blobs this event depends on, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blob_manifest: Vec<BlobManifestEntry>,
}

impl ChangeBody {
    /// Serialize the body to canonical JSON bytes for encryption.
    ///
    /// # Errors
    /// Returns a [`serde_json::Error`] only if serialization fails, which does
    /// not happen for this type in practice.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize a body from its JSON bytes after decryption.
    ///
    /// # Errors
    /// Returns a [`serde_json::Error`] if the bytes are not a valid encoding of
    /// a [`ChangeBody`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The ciphertext hashes of every blob this event references, in order.
    #[must_use]
    pub fn blob_refs(&self) -> Vec<String> {
        self.blob_manifest
            .iter()
            .map(|b| b.ct_hash.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn clock() -> Hlc {
        Hlc::new(1234, 2, "device-a".to_owned())
    }

    #[test]
    fn entity_type_wire_round_trips() {
        for et in [
            EntityType::Document,
            EntityType::Tag,
            EntityType::Collection,
            EntityType::TagEdge,
            EntityType::CollectionEdge,
            EntityType::Highlight,
            EntityType::Note,
            EntityType::ReviewCard,
            EntityType::ReviewLog,
            EntityType::FeedSubscription,
            EntityType::Settings,
        ] {
            assert_eq!(EntityType::from_wire(et.as_str()), Some(et));
        }
        assert_eq!(EntityType::from_wire("nope"), None);
    }

    #[test]
    fn op_wire_round_trips() {
        for op in [Op::Upsert, Op::Delete, Op::FieldPatch] {
            assert_eq!(Op::from_wire(op.as_str()), Some(op));
        }
        assert_eq!(Op::from_wire("nope"), None);
    }

    #[test]
    fn body_round_trips_through_bytes() {
        let mut fields = serde_json::Map::new();
        fields.insert("status".to_owned(), serde_json::json!("archived"));
        fields.insert("title".to_owned(), serde_json::json!("Hello"));
        let body = ChangeBody {
            entity_type: EntityType::Document,
            entity_id: "doc-1".to_owned(),
            op: Op::FieldPatch,
            clock: clock(),
            base_version: Some(Hlc::zero("device-a".to_owned())),
            fields,
            blob_manifest: vec![BlobManifestEntry {
                ct_hash: "abc".to_owned(),
                role: "raw_html".to_owned(),
                plaintext_hash: "def".to_owned(),
                plaintext_len: 42,
            }],
        };
        let bytes = body.to_bytes().unwrap();
        let restored = ChangeBody::from_bytes(&bytes).unwrap();
        assert_eq!(body, restored);
        assert_eq!(restored.blob_refs(), vec!["abc".to_owned()]);
    }

    #[test]
    fn only_review_log_is_append_only() {
        assert!(EntityType::ReviewLog.is_append_only());
        assert!(!EntityType::Document.is_append_only());
        assert!(!EntityType::Highlight.is_append_only());
    }
}
