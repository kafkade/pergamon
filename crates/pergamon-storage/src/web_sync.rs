//! Local-only progress for the trusted web host's encrypted relay setup.

use rusqlite::{OptionalExtension, params};

use crate::{Database, StorageError};

/// Nonsecret web progress; authority remains in `RemoteAccountBinding`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSyncSetup {
    /// Normalized relay destination.
    pub relay_url: String,
    /// OPAQUE identity handle, never its password.
    pub identity_handle: String,
    /// Explicit create, attach, or join intent.
    pub flow: String,
    /// Resumable web step.
    pub phase: String,
    /// Form revision used to reject stale submissions.
    pub revision: i64,
    /// Whether the operator acknowledged saving recovery material.
    pub recovery_ack: bool,
    /// Stable timestamp for retrying deterministic signed publications.
    pub publication_millis: i64,
    /// Selected trusted enrollment peer, if any.
    pub approver_device_id: Option<String>,
}

impl Database {
    /// Read the persisted refusal to replay consumed/uncertain credentials.
    ///
    /// # Errors
    /// Returns a storage error if runtime state cannot be read.
    pub fn web_sync_needs_login(&self) -> Result<bool, StorageError> {
        Ok(self.connection().query_row(
            "SELECT needs_login FROM web_sync_runtime WHERE id=1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Latch refresh failures until an explicit successful authentication.
    ///
    /// # Errors
    /// Returns a storage error if the state cannot be committed.
    pub fn set_web_sync_needs_login(&self, value: bool) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE web_sync_runtime SET needs_login=?1 WHERE id=1",
            [value],
        )?;
        Ok(())
    }

    /// Read local-only web setup without exposing any credential.
    ///
    /// # Errors
    /// Returns a storage error if the record cannot be read.
    pub fn web_sync_setup(&self) -> Result<Option<WebSyncSetup>, StorageError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT relay_url,identity_handle,flow,phase,revision,recovery_ack,
                    publication_millis,approver_device_id FROM web_sync_setup WHERE id=1",
                [],
                |row| {
                    Ok(WebSyncSetup {
                        relay_url: row.get(0)?,
                        identity_handle: row.get(1)?,
                        flow: row.get(2)?,
                        phase: row.get(3)?,
                        revision: row.get(4)?,
                        recovery_ack: row.get(5)?,
                        publication_millis: row.get(6)?,
                        approver_device_id: row.get(7)?,
                    })
                },
            )
            .optional()?)
    }

    /// Commit progress only when the submitted revision is still current.
    ///
    /// # Errors
    /// Returns an explicit conflict for a stale form or a storage error.
    pub fn save_web_sync_setup(
        &self,
        setup: &WebSyncSetup,
        expected_revision: i64,
    ) -> Result<(), StorageError> {
        self.in_transaction(|db| {
            let current = db.web_sync_setup()?.map_or(0, |s| s.revision);
            if current != expected_revision || current.checked_add(1) != Some(setup.revision) {
                return Err(StorageError::Constraint("stale sync setup form; reload before retrying".into()));
            }
            db.connection().execute(
                "INSERT INTO web_sync_setup
                 (id,relay_url,identity_handle,flow,phase,revision,recovery_ack,publication_millis,approver_device_id)
                 VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET relay_url=excluded.relay_url,
                 identity_handle=excluded.identity_handle,flow=excluded.flow,phase=excluded.phase,
                 revision=excluded.revision,recovery_ack=excluded.recovery_ack,
                 publication_millis=excluded.publication_millis,approver_device_id=excluded.approver_device_id",
                params![setup.relay_url, setup.identity_handle, setup.flow, setup.phase,
                    setup.revision, setup.recovery_ack, setup.publication_millis, setup.approver_device_id],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn web_progress_is_local_only_and_rejects_stale_forms() {
        let db = Database::open_in_memory().unwrap();
        let setup = WebSyncSetup {
            relay_url: "https://relay.example".into(),
            identity_handle: "owner".into(),
            flow: "create".into(),
            phase: "credentials".into(),
            revision: 1,
            recovery_ack: false,
            publication_millis: 1,
            approver_device_id: None,
        };
        db.save_web_sync_setup(&setup, 0).unwrap();
        assert_eq!(db.web_sync_setup().unwrap(), Some(setup.clone()));
        assert!(db.save_web_sync_setup(&setup, 0).is_err());
        assert_eq!(db.pending_outbox_count().unwrap(), 0);
        assert!(db.remote_account_binding().unwrap().is_none());
    }
}
