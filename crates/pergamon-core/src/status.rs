//! Document lifecycle status.
//!
//! Documents move through a triage workflow:
//! `inbox → later/reference → reading → archived/discarded`.
//!
//! See the roadmap Section 3 for the full status model.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Lifecycle status of a content item in the triage workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Newly captured, awaiting triage.
    Inbox,
    /// Marked for later reading.
    Later,
    /// Saved as a reference (not intended for deep reading).
    Reference,
    /// Currently being read.
    Reading,
    /// Finished / processed.
    Archived,
    /// Explicitly discarded.
    Discarded,
}

impl DocumentStatus {
    /// Returns the canonical string representation used in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Later => "later",
            Self::Reference => "reference",
            Self::Reading => "reading",
            Self::Archived => "archived",
            Self::Discarded => "discarded",
        }
    }
}

impl fmt::Display for DocumentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DocumentStatus {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inbox" => Ok(Self::Inbox),
            "later" => Ok(Self::Later),
            "reference" => Ok(Self::Reference),
            "reading" => Ok(Self::Reading),
            "archived" => Ok(Self::Archived),
            "discarded" => Ok(Self::Discarded),
            other => Err(CoreError::UnknownDocumentStatus(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_variants() {
        let variants = [
            DocumentStatus::Inbox,
            DocumentStatus::Later,
            DocumentStatus::Reference,
            DocumentStatus::Reading,
            DocumentStatus::Archived,
            DocumentStatus::Discarded,
        ];
        for status in variants {
            let s = status.to_string();
            let parsed: DocumentStatus = s.parse().unwrap_or_else(|e| {
                let _ = e;
                unreachable!("failed to parse DocumentStatus from {s:?}")
            });
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn invalid_status() {
        let result = "unknown".parse::<DocumentStatus>();
        assert!(result.is_err());
    }
}
