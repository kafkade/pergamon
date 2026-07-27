//! Explicit account-onboarding flow model (ADR-024, ADR-029).
//!
//! An account in pergamon is **a client-side secret, not a login** (ADR-024):
//! the first device generates a random Account Root Key (ARK) and an independent
//! random `account_id`. ADR-029 (Decision 3) then insists the onboarding state
//! machine models **three separate flows** and must never collapse them, because
//! each moves different key material:
//!
//! - [`AccountFlow::CreateNew`] — this device's local data *becomes* a brand-new
//!   account: generate the ARK + `account_id`, save a recovery code, publish.
//! - [`AccountFlow::AttachExisting`] — this device already holds a local
//!   `account_id` (used offline first) and only binds it to a server. **No new
//!   ARK.** A transport change, not a crypto change.
//! - [`AccountFlow::JoinOnNewDevice`] — a fresh device with no data obtains the
//!   *existing* ARK via SAS enrollment or a recovery package. It never invents
//!   its own ARK.
//!
//! The dangerous, ADR-029-forbidden footgun is a fresh-but-not-empty device
//! silently taking the "create" path and minting a **second, different** account
//! for a user who actually meant to *join* the account they already have on
//! another device. This module encodes that guard as **pure logic** so the CLI
//! today — and web/iOS later (WP-5/WP-6) — enforce the same rule from one place.
//!
//! This crate is zero-I/O (ADR-001): callers gather the [`LocalAccountState`]
//! from their own key store and database, then ask this module to decide.

use std::fmt;

use thiserror::Error;

/// The three mutually-exclusive account-onboarding flows (ADR-029, Decision 3).
///
/// They are represented explicitly, and never collapsed into a single
/// "sign up / log in", precisely so a caller cannot accidentally run the wrong
/// key movement for the device's actual state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountFlow {
    /// Create a brand-new account on this (first) device: generate the ARK and
    /// `account_id`, save a recovery code, publish this device.
    CreateNew,
    /// Attach an already-existing *local* account to a server. No new ARK is
    /// generated; only the transport/binding changes.
    AttachExisting,
    /// Join an already-existing account on a fresh device by obtaining the
    /// existing ARK (SAS enrollment from a trusted device, or a recovery
    /// package). This device never invents its own ARK.
    JoinOnNewDevice,
}

impl AccountFlow {
    /// The `pergamon` subcommand that performs this flow, for guidance messages.
    #[must_use]
    pub const fn cli_command(self) -> &'static str {
        match self {
            Self::CreateNew => "sync-device bootstrap",
            Self::AttachExisting => "sync-remote enable",
            Self::JoinOnNewDevice => "sync-device enroll / accept / recover",
        }
    }

    /// A one-line human description of what the flow does.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::CreateNew => "create a new account (first device)",
            Self::AttachExisting => "attach this existing local account to a server",
            Self::JoinOnNewDevice => "join an existing account on a new device",
        }
    }
}

impl fmt::Display for AccountFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// A snapshot of this device's local account state, gathered by the caller.
///
/// Every field is derived from local reads only (key store + database); the
/// guard logic in this module never performs I/O. Fields are deliberately
/// independent booleans rather than a single derived enum so callers can supply
/// exactly what they can cheaply observe — this is the reusable shape ADR-029's
/// three-flow model is built on for CLI, web, and iOS.
#[allow(clippy::struct_excessive_bools)] // independent observations, not a state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalAccountState {
    /// This device already has its X25519/Ed25519 device keypairs.
    pub has_device_keys: bool,
    /// An Account Root Key is present for this account handle.
    pub has_ark: bool,
    /// An `account_id` is present for this account handle.
    pub has_account_id: bool,
    /// The local database is already bound to a server / account (sync enabled).
    pub is_sync_bound: bool,
    /// The local library already contains user content (documents, etc.).
    pub has_local_content: bool,
}

impl LocalAccountState {
    /// A pristine device: no keys, no account, no content. Convenience for
    /// tests and callers that want to start from empty and set fields.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            has_device_keys: false,
            has_ark: false,
            has_account_id: false,
            is_sync_bound: false,
            has_local_content: false,
        }
    }

    /// Whether this device already belongs to an account and therefore must not
    /// create another. True when an ARK exists or sync is already bound.
    #[must_use]
    pub const fn belongs_to_account(self) -> bool {
        self.has_ark || self.is_sync_bound
    }
}

/// Why a [`AccountFlow::CreateNew`] request was refused by [`guard_create_new`].
///
/// Both variants steer the user toward the correct, non-destructive flow rather
/// than silently minting a duplicate account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CreateAccountBlock {
    /// This device already belongs to an account (it has an ARK or sync is
    /// bound). Creating another would fork the library into a second, unrelated
    /// account. Refused unconditionally — an explicit confirmation cannot
    /// override it, because the safe intent here is to *join*, not create.
    #[error(
        "this device already belongs to an account; creating a new one would make a \
         second, unrelated account. To use this device with an existing account, join it \
         with `pergamon sync-device enroll` (SAS from a trusted device) or \
         `pergamon sync-device recover` (recovery code)"
    )]
    AlreadyHasAccount,

    /// This device has local content but no account yet, so the intent is
    /// ambiguous: the user may have meant to *join* an account that already
    /// exists on another device. Requires an explicit opt-in to proceed.
    #[error(
        "this device has local data but no account yet — creating a NEW account will not \
         merge that data with an account on another device. To join an existing account, \
         use `pergamon sync-device enroll` or `pergamon sync-device recover`. To create a \
         brand-new account from this data, re-run with `--create-new-account`"
    )]
    NeedsExplicitConfirmation,
}

impl CreateAccountBlock {
    /// The flow(s) the user most likely wanted instead of creating a new
    /// account, for guidance messages.
    #[must_use]
    pub const fn suggested_flow(self) -> AccountFlow {
        AccountFlow::JoinOnNewDevice
    }
}

/// Guard the [`AccountFlow::CreateNew`] path against silently forking or
/// duplicating an account (ADR-029, Decision 3).
///
/// Decision table:
///
/// | state | `explicit_confirm` | result |
/// |-------|--------------------|--------|
/// | has ARK **or** sync-bound | any | [`Err`]([`CreateAccountBlock::AlreadyHasAccount`]) |
/// | has local content, no account | `false` | [`Err`]([`CreateAccountBlock::NeedsExplicitConfirmation`]) |
/// | has local content, no account | `true`  | [`Ok`] |
/// | clean / empty device | any | [`Ok`] |
///
/// The "already has an account" refusal is **not** overridable by
/// `explicit_confirm`: if a device already holds an ARK or is sync-bound, the
/// correct move is always to join, never to create a second account.
///
/// # Errors
/// Returns a [`CreateAccountBlock`] describing why creation was refused.
pub const fn guard_create_new(
    state: &LocalAccountState,
    explicit_confirm: bool,
) -> Result<(), CreateAccountBlock> {
    // Already part of an account: refuse unconditionally. Even an explicit
    // confirmation must not fork the library into a second account.
    if state.belongs_to_account() {
        return Err(CreateAccountBlock::AlreadyHasAccount);
    }
    // Ambiguous: local data present but no account yet. The user may have meant
    // to join an existing account, so require an explicit opt-in.
    if state.has_local_content && !explicit_confirm {
        return Err(CreateAccountBlock::NeedsExplicitConfirmation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_device_may_create_without_confirmation() {
        let state = LocalAccountState::empty();
        assert!(guard_create_new(&state, false).is_ok());
        assert!(guard_create_new(&state, true).is_ok());
    }

    #[test]
    fn device_keys_alone_do_not_block_creation() {
        // `device-key init` may create keypairs before bootstrap; that alone is
        // not "belonging to an account".
        let state = LocalAccountState {
            has_device_keys: true,
            ..LocalAccountState::empty()
        };
        assert!(guard_create_new(&state, false).is_ok());
    }

    #[test]
    fn account_id_alone_does_not_block_creation() {
        // Per ADR-029 the guard keys on ARK presence / sync binding, not on the
        // opaque handle existing on its own.
        let state = LocalAccountState {
            has_account_id: true,
            ..LocalAccountState::empty()
        };
        assert!(guard_create_new(&state, false).is_ok());
    }

    #[test]
    fn existing_ark_refuses_creation_even_with_confirmation() {
        let state = LocalAccountState {
            has_ark: true,
            ..LocalAccountState::empty()
        };
        assert_eq!(
            guard_create_new(&state, false),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
        // Confirmation must NOT override an already-owned account.
        assert_eq!(
            guard_create_new(&state, true),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
    }

    #[test]
    fn sync_bound_refuses_creation_even_with_confirmation() {
        let state = LocalAccountState {
            is_sync_bound: true,
            ..LocalAccountState::empty()
        };
        assert_eq!(
            guard_create_new(&state, false),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
        assert_eq!(
            guard_create_new(&state, true),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
    }

    #[test]
    fn ark_and_sync_bound_together_refuse_creation() {
        let state = LocalAccountState {
            has_ark: true,
            is_sync_bound: true,
            has_account_id: true,
            ..LocalAccountState::empty()
        };
        assert_eq!(
            guard_create_new(&state, true),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
    }

    #[test]
    fn local_content_without_account_needs_confirmation() {
        let state = LocalAccountState {
            has_local_content: true,
            ..LocalAccountState::empty()
        };
        assert_eq!(
            guard_create_new(&state, false),
            Err(CreateAccountBlock::NeedsExplicitConfirmation)
        );
    }

    #[test]
    fn local_content_creation_proceeds_with_confirmation() {
        let state = LocalAccountState {
            has_local_content: true,
            has_device_keys: true,
            ..LocalAccountState::empty()
        };
        assert!(guard_create_new(&state, true).is_ok());
    }

    #[test]
    fn already_has_account_takes_precedence_over_needs_confirmation() {
        // Both conditions true: the un-overridable refusal wins.
        let state = LocalAccountState {
            has_ark: true,
            has_local_content: true,
            ..LocalAccountState::empty()
        };
        assert_eq!(
            guard_create_new(&state, false),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
        assert_eq!(
            guard_create_new(&state, true),
            Err(CreateAccountBlock::AlreadyHasAccount)
        );
    }

    #[test]
    fn belongs_to_account_reflects_ark_or_binding() {
        assert!(!LocalAccountState::empty().belongs_to_account());
        assert!(
            LocalAccountState {
                has_ark: true,
                ..LocalAccountState::empty()
            }
            .belongs_to_account()
        );
        assert!(
            LocalAccountState {
                is_sync_bound: true,
                ..LocalAccountState::empty()
            }
            .belongs_to_account()
        );
    }

    #[test]
    fn flow_metadata_is_stable() {
        assert_eq!(
            AccountFlow::CreateNew.cli_command(),
            "sync-device bootstrap"
        );
        assert_eq!(
            AccountFlow::AttachExisting.cli_command(),
            "sync-remote enable"
        );
        assert_eq!(
            AccountFlow::JoinOnNewDevice.cli_command(),
            "sync-device enroll / accept / recover"
        );
        assert_eq!(
            CreateAccountBlock::AlreadyHasAccount.suggested_flow(),
            AccountFlow::JoinOnNewDevice
        );
        // Display delegates to describe().
        assert_eq!(
            AccountFlow::CreateNew.to_string(),
            "create a new account (first device)"
        );
    }

    #[test]
    fn block_messages_point_to_join_commands() {
        let already = CreateAccountBlock::AlreadyHasAccount.to_string();
        assert!(already.contains("sync-device enroll"));
        assert!(already.contains("sync-device recover"));
        let needs = CreateAccountBlock::NeedsExplicitConfirmation.to_string();
        assert!(needs.contains("--create-new-account"));
        assert!(needs.contains("sync-device enroll"));
    }
}
