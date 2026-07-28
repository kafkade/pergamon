// SPDX-License-Identifier: AGPL-3.0-only

//! Per-tenant storage quotas for the AGPL sync server (WP-3d, [#198]).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This module defines the **policy** half of per-tenant storage accounting: the
//! configured limits and the pure predicate that decides whether a projected
//! usage would exceed them. The **measurement** half — summing the actual stored
//! ciphertext bytes/counts and enforcing the limit inside the write transaction —
//! lives in [`crate::store`], so accounting stays transactionally consistent with
//! the content it meters.
//!
//! ## Content-blindness (design §2.5)
//! Quotas are measured on **ciphertext size and object counts only**
//! (`blobs.byte_len` + `events.payload_bytes`). Sizes and counts are metadata the
//! operator necessarily learns by storing the bytes; metering them does **not**
//! inspect or decode any ciphertext, so the blind-relay invariant (ADR-026) is
//! preserved.
//!
//! ## `0` means unlimited
//! Each limit is disabled when it is `0`. The default ([`QuotaConfig::default`])
//! is fully unlimited, so a blind single-account self-host and every existing
//! multi-tenant deployment are byte-for-byte unchanged unless an operator
//! explicitly configures a cap (mirrors the WP-4 [`crate::abuse::AbuseConfig`]
//! opt-in pattern).
//!
//! [#198]: https://github.com/kafkade/pergamon/issues/198

/// Which configured limit an over-quota write would violate.
///
/// Carried by [`crate::store::StoreError::QuotaExceeded`] so the HTTP layer can
/// tell the caller *which* cap was hit (bytes vs objects) in the 507 response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaLimit {
    /// The per-account **ciphertext byte** cap ([`QuotaConfig::max_account_bytes`]).
    Bytes,
    /// The per-account **object-count** cap ([`QuotaConfig::max_account_objects`]).
    Objects,
}

impl std::fmt::Display for QuotaLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes => f.write_str("storage bytes"),
            Self::Objects => f.write_str("object count"),
        }
    }
}

/// Per-tenant storage quota policy (WP-3d, #198).
///
/// Both limits are surfaced as CLI flags / environment variables by the server
/// binary. A limit of `0` means **unlimited** (the layer/check is simply not
/// applied for that dimension); the [`Default`] is fully unlimited.
///
/// The limits are enforced against the **combined** ciphertext usage of an
/// account: `total_bytes = blob_bytes + event_bytes` and
/// `total_objects = blob_count + event_count` (see [`crate::store`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    /// Maximum total ciphertext bytes (blob + event payload) an account may
    /// store. `0` disables the byte cap.
    pub max_account_bytes: u64,
    /// Maximum total stored objects (blobs + events) an account may hold. `0`
    /// disables the object-count cap.
    pub max_account_objects: u64,
}

impl QuotaConfig {
    /// A fully-unlimited quota (both dimensions off). This is the default and the
    /// value that keeps blind / existing multi-tenant behavior byte-for-byte.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_account_bytes: 0,
            max_account_objects: 0,
        }
    }

    /// `true` when **neither** dimension is capped, i.e. enforcement is a no-op.
    ///
    /// The store uses this to take an unchanged fast path (no usage computation)
    /// so the default deployment does no extra work.
    #[must_use]
    pub const fn is_unlimited(&self) -> bool {
        self.max_account_bytes == 0 && self.max_account_objects == 0
    }

    /// Check a **projected** total usage against the configured caps.
    ///
    /// Returns `Err(limit)` naming the first dimension that would be exceeded, or
    /// `Ok(())` when the projected usage is within every configured cap. A `0`
    /// cap is treated as unlimited and never triggers. Equality with a cap is
    /// allowed (being exactly at the cap is not "over").
    ///
    /// # Errors
    /// Returns the [`QuotaLimit`] that `total_bytes`/`total_objects` would exceed.
    pub const fn check(&self, total_bytes: u64, total_objects: u64) -> Result<(), QuotaLimit> {
        if self.max_account_bytes != 0 && total_bytes > self.max_account_bytes {
            return Err(QuotaLimit::Bytes);
        }
        if self.max_account_objects != 0 && total_objects > self.max_account_objects {
            return Err(QuotaLimit::Objects);
        }
        Ok(())
    }
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self::unlimited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unlimited() {
        let q = QuotaConfig::default();
        assert!(q.is_unlimited());
        assert_eq!(q, QuotaConfig::unlimited());
        // Unlimited never rejects, even for huge projected usage.
        assert!(q.check(u64::MAX, u64::MAX).is_ok());
    }

    #[test]
    fn byte_cap_rejects_only_over_the_limit() {
        let q = QuotaConfig {
            max_account_bytes: 100,
            max_account_objects: 0,
        };
        assert!(!q.is_unlimited());
        // At the cap is allowed; over is rejected as Bytes.
        assert!(q.check(100, 999).is_ok());
        assert_eq!(q.check(101, 0), Err(QuotaLimit::Bytes));
        // Objects unlimited (0): never the cause.
        assert!(q.check(0, u64::MAX).is_ok());
    }

    #[test]
    fn object_cap_rejects_only_over_the_limit() {
        let q = QuotaConfig {
            max_account_bytes: 0,
            max_account_objects: 3,
        };
        assert!(q.check(u64::MAX, 3).is_ok());
        assert_eq!(q.check(0, 4), Err(QuotaLimit::Objects));
    }

    #[test]
    fn bytes_checked_before_objects() {
        // When both would be exceeded, the byte limit is reported first.
        let q = QuotaConfig {
            max_account_bytes: 10,
            max_account_objects: 1,
        };
        assert_eq!(q.check(11, 2), Err(QuotaLimit::Bytes));
    }

    #[test]
    fn limit_display_is_human_readable() {
        assert_eq!(QuotaLimit::Bytes.to_string(), "storage bytes");
        assert_eq!(QuotaLimit::Objects.to_string(), "object count");
    }
}
