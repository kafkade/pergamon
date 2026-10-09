// SPDX-License-Identifier: AGPL-3.0-only

//! Trusted-host web orchestration over the existing v2 client and V15 authority.

// The single-owner guard spans operation/error recording; status has its own lock.
#![allow(clippy::significant_drop_tightening)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use pergamon_core::account_flow::{
    LocalAccountState, guard_attach_existing, guard_create_new, guard_join_new_device,
};
use pergamon_crypto::{AccountId, AccountRootKey, DeviceKeypairs, SignedDeviceRecord};
use pergamon_keystore::DeviceKeyStore;
use pergamon_storage::{Database, sync::RemoteAccountBinding, web_sync::WebSyncSetup};
use pergamon_sync::{
    CryptoContext, HttpRelay, RelayTransport, SyncError,
    account_binding::{self as binding, RefreshingSession, RemoteSession, SessionScope},
    credential::AccessTokenProvider,
    http::HttpTransport,
    http_auth::HttpAuth,
    onboarding::{self as client, PendingRecovery},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

use crate::sync_worker::{self, AuthenticatedWorkerConfig, WorkerEvent, WorkerHandle};

/// Nonsecret fixed deployment paths and policy; never browser-supplied paths.
#[derive(Clone)]
pub struct SyncConfig {
    /// Canonical library.
    pub db_path: PathBuf,
    /// Encrypted file used by the headless trusted client.
    pub key_file: PathBuf,
    /// Durable plaintext blob store.
    pub blob_dir: PathBuf,
    /// Local keystore label.
    pub account: String,
    /// Healthy worker cadence.
    pub interval_secs: u64,
    /// Explicit development-only loopback cleartext allowance.
    pub allow_insecure_loopback: bool,
}

fn peer_id(value: &str) -> Result<&str> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        bail!("device ID must be 32 lowercase hexadecimal characters");
    }
    Ok(value)
}

/// Safe renderable state, kept independent of long-running service operations.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// No live worker owns the configured account.
    Stopped,
    /// A worker has been started and has not reported termination.
    Running,
}

/// Safe renderable state, kept independent of long-running service operations.
#[derive(Clone)]
pub struct SyncSnapshot {
    /// Human-readable worker/setup state.
    pub label: String,
    /// Nonsecret guidance or error.
    pub message: String,
    /// Current nonsecret wizard progress.
    pub setup: Option<WebSyncSetup>,
    /// Existing V15 authority, distinct from web presentation state.
    pub binding: Option<RemoteAccountBinding>,
    /// Whether this process holds an unlocked keystore.
    pub unlocked: bool,
    /// Persisted requirement for fresh authentication rather than refresh replay.
    pub needs_login: bool,
    /// Whether unlock is opening an existing encrypted file.
    pub key_file_exists: bool,
    /// Live worker state, not merely unlocked keys or saved settings.
    pub worker: WorkerState,
    /// Last real successful push count.
    pub pushed: usize,
    /// Last real successful apply count.
    pub applied: usize,
    /// Last successful sync's UTC timestamp.
    pub last_success: String,
    /// SAS for the explicitly selected peer.
    pub sas: String,
    /// Peer/device that SAS names.
    pub sas_device: String,
    /// Earliest manual retry after a server rate limit.
    pub retry_at_millis: i64,
}

impl SyncSnapshot {
    /// Expose controls only for a live worker, never for settings-only progress.
    pub const fn can_trigger(&self) -> bool {
        matches!(self.worker, WorkerState::Running)
    }
}

/// Protected form operations; no debug representation for submitted secrets.
#[derive(Deserialize)]
pub struct SyncCommand {
    /// Requested operation.
    pub action: String,
    /// Last rendered wizard revision.
    pub revision: i64,
    /// Destination for initial selection.
    pub server: Option<String>,
    /// OPAQUE identity.
    pub identity: Option<String>,
    /// Explicit create, attach, or join intent.
    pub flow: Option<String>,
    /// Password consumed during one OPAQUE operation.
    pub password: Option<String>,
    /// Unlock input, never retained after deriving the KEK.
    pub unlock_passphrase: Option<String>,
    /// Existing recovery secret.
    pub recovery_code: Option<String>,
    /// Optional self-contained encrypted key package, standard base64.
    pub recovery_package: Option<String>,
    /// Selected enrollment peer/subject.
    pub device: Option<String>,
    /// Human-verified out-of-band SAS.
    pub expect_sas: Option<String>,
    /// Explicit nonempty-library creation confirmation.
    pub confirm_create: Option<String>,
    /// Explicit offline recovery capture acknowledgement.
    pub confirm_recovery: Option<String>,
    /// Register a relay identity instead of only logging in.
    pub register: Option<String>,
}

impl Drop for SyncCommand {
    fn drop(&mut self) {
        for secret in [
            &mut self.password,
            &mut self.unlock_passphrase,
            &mut self.recovery_code,
        ] {
            if let Some(secret) = secret.as_mut() {
                secret.zeroize();
            }
        }
    }
}

impl SyncCommand {
    /// Construct an operation with no retained secret inputs.
    pub fn new(action: &str, revision: i64) -> Self {
        Self {
            action: action.into(),
            revision,
            server: None,
            identity: None,
            flow: None,
            password: None,
            unlock_passphrase: None,
            recovery_code: None,
            recovery_package: None,
            device: None,
            expect_sas: None,
            confirm_create: None,
            confirm_recovery: None,
            register: None,
        }
    }
}

struct Runtime {
    store: Option<Arc<Mutex<DeviceKeyStore>>>,
    provider: Option<Arc<dyn AccessTokenProvider>>,
    worker: Option<WorkerHandle>,
}

/// One serialized writer/lifecycle owner, with a separately readable status.
pub struct SyncService {
    config: SyncConfig,
    runtime: Mutex<Runtime>,
    snapshot: Arc<Mutex<SyncSnapshot>>,
}

fn millis() -> i64 {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .unwrap_or(i64::MAX)
}

/// Decode only the existing canonical lowercase-hex identity, never an alias.
pub fn account_id(value: &str) -> Result<AccountId> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        bail!("canonical content identity is not 32 lowercase hexadecimal characters");
    }
    let mut bytes = [0; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    Ok(AccountId::from_bytes(bytes))
}

fn field<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str> {
    let value = value.context(format!("{name} is required"))?;
    if value.is_empty() || value.len() > 4_096 {
        bail!("{name} is empty or too long");
    }
    Ok(value)
}

fn normalize_sas(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
}

impl SyncService {
    /// Initialize observable locked state without unlocking keys or syncing.
    pub fn new(config: SyncConfig) -> Result<Self> {
        let db = Database::open(&config.db_path)?;
        let setup = db.web_sync_setup()?;
        let binding = db.remote_account_binding()?;
        let has_binding = binding.is_some();
        let key_file_exists = config.key_file.exists();
        Ok(Self {
            config,
            runtime: Mutex::new(Runtime { store: None, provider: None, worker: None }),
            snapshot: Arc::new(Mutex::new(SyncSnapshot {
                label: if has_binding || setup.is_some() { "Locked" } else { "Local only" }.into(),
                message: "Local library use is available without an account. Unlock the encrypted key file to configure sync.".into(),
                setup, binding, unlocked: false, needs_login: db.web_sync_needs_login()?,
                key_file_exists,
                worker: WorkerState::Stopped, pushed: 0, applied: 0, last_success: String::new(),
                sas: String::new(), sas_device: String::new(), retry_at_millis: 0,
            })),
        })
    }

    /// Read status without contending with network/crypto operations.
    pub fn snapshot(&self) -> Result<SyncSnapshot> {
        Ok(self
            .snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .clone())
    }

    fn status(&self, label: &str, message: &str) -> Result<()> {
        let mut status = self
            .snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?;
        status.label = label.into();
        status.message = message.into();
        Ok(())
    }

    fn progress(&self, db: &Database, setup: &mut WebSyncSetup, phase: &str) -> Result<()> {
        let old = setup.revision;
        setup.revision = old.checked_add(1).context("setup revision exhausted")?;
        setup.phase = phase.into();
        db.save_web_sync_setup(setup, old)?;
        let mut status = self
            .snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?;
        status.setup = Some(setup.clone());
        status.binding = db.remote_account_binding()?;
        Ok(())
    }

    fn stop(&self, runtime: &mut Runtime) -> Result<()> {
        if let Some(mut worker) = runtime.worker.take() {
            worker.stop()?;
        }
        runtime.provider = None;
        self.snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .worker = WorkerState::Stopped;
        Ok(())
    }

    /// Stop and join the worker at shutdown, preserving durable local state.
    pub fn shutdown(&self) -> Result<()> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow!("sync service lock poisoned"))?;
        self.stop(&mut runtime)
    }

    /// Preserve explicit legacy startup only when no authenticated intent/session exists.
    pub fn start_legacy(&self, passphrase: &str) -> Result<()> {
        let db = Database::open(&self.config.db_path)?;
        let Some(server) = db.sync_state()?.server_url else {
            return Ok(());
        };
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow!("sync service lock poisoned"))?;
        if db.remote_account_binding()?.is_some()
            || self
                .store(&runtime)?
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?
                .load_remote_session(&self.config.account, &server)?
                .is_some()
        {
            bail!("authenticated intent/session prevents legacy transport fallback");
        }
        self.stop(&mut runtime)?;
        runtime.worker = Some(sync_worker::spawn(sync_worker::SyncWorkerConfig {
            db_path: self.config.db_path.clone(),
            account: self.config.account.clone(),
            key_file: self.config.key_file.clone(),
            passphrase: passphrase.into(),
            interval_secs: self.config.interval_secs,
        })?);
        self.snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .worker = WorkerState::Running;
        self.status("Legacy sync", "Legacy external transport is running; guided setup uses v2 authentication. Inspect server logs for legacy round outcomes.")
    }

    /// Execute on a blocking thread; never uses the request-handler DB mutex.
    #[allow(clippy::too_many_lines)]
    pub fn execute(&self, command: &SyncCommand) -> Result<()> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow!("sync service lock poisoned"))?;
        let db = Database::open(&self.config.db_path)?;
        let current = db.web_sync_setup()?;
        if current.as_ref().map_or(0, |s| s.revision) != command.revision {
            bail!("stale sync setup form; reload before retrying");
        }
        let result = (|| -> Result<()> {
            match command.action.as_str() {
                "unlock" => {
                    let passphrase =
                        field(command.unlock_passphrase.as_deref(), "key-file password")?;
                    self.stop(&mut runtime)?;
                    let store = DeviceKeyStore::encrypted_file(
                        &self.config.key_file,
                        passphrase.as_bytes(),
                    )?;
                    runtime.store = Some(Arc::new(Mutex::new(store)));
                    self.snapshot
                        .lock()
                        .map_err(|_| anyhow!("sync status lock poisoned"))?
                        .unlocked = true;
                    self.status(
                        "Unlocked",
                        "Keys are unlocked in this process; no relay password is retained.",
                    )?;
                    if db
                        .remote_account_binding()?
                        .is_some_and(|b| b.state == "active")
                    {
                        self.start(&db, &mut runtime)?;
                    }
                    Ok(())
                }
                "select" => self.select(&db, command),
                "authenticate" => {
                    self.stop(&mut runtime)?;
                    let mut setup = current.context("select a server first")?;
                    self.authenticate(&db, &mut runtime, &mut setup, command)
                }
                "renew" => {
                    self.stop(&mut runtime)?;
                    self.renew(&db, &mut runtime, command)
                }
                "recover" => {
                    let mut setup = current.context("no pending join")?;
                    self.recover(&db, &mut runtime, &mut setup, command)
                }
                "enroll" => {
                    let mut setup = current.context("no pending join")?;
                    self.enroll(&db, &mut runtime, &mut setup, command)
                }
                "accept" => {
                    let mut setup = current.context("no pending join")?;
                    self.accept(&db, &mut runtime, &mut setup, command)
                }
                "acknowledge" => {
                    let mut setup = current.context("no recovery capture is pending")?;
                    if setup.phase != "recovery"
                        || command.confirm_recovery.as_deref() != Some("yes")
                    {
                        bail!(
                            "save recovery material offline and explicitly acknowledge it before syncing"
                        );
                    }
                    self.capture(&runtime, &setup)?;
                    setup.recovery_ack = true;
                    self.progress(&db, &mut setup, "ready")?;
                    self.store(&runtime)?
                        .lock()
                        .map_err(|_| anyhow!("secure store lock poisoned"))?
                        .remove_bootstrap_recovery(&self.config.account, &setup.relay_url)?;
                    self.activate(&db, &mut runtime, &mut setup)?;
                    Ok(())
                }
                "start" => {
                    self.stop(&mut runtime)?;
                    if let Some(mut setup) = current {
                        self.activate(&db, &mut runtime, &mut setup)
                    } else {
                        self.start(&db, &mut runtime)
                    }
                }
                "trigger" => {
                    if self.snapshot()?.retry_at_millis > millis() {
                        bail!("the relay retry delay has not elapsed");
                    }
                    if !runtime
                        .worker
                        .as_ref()
                        .context("sync worker is not running")?
                        .trigger()
                    {
                        bail!("sync worker has stopped; restart or sign in again");
                    }
                    self.status(
                        "Sync queued",
                        "The worker accepted a trigger; this is not a completed sync.",
                    )?;
                    Ok(())
                }
                "pause" | "cancel" => {
                    self.stop(&mut runtime)?;
                    self.status("Paused", "Sync is paused. Local content, keys and safe setup progress are preserved.")
                }
                "sas" => self.show_sas(&db, &mut runtime, command),
                "approve" => self.approve(&db, &mut runtime, command),
                _ => bail!("unknown sync setup action"),
            }
        })();
        if let Err(error) = &result {
            if matches!(
                error.downcast_ref::<SyncError>(),
                Some(
                    SyncError::SessionNeedsLogin { .. }
                        | SyncError::AuthRefused { status: 401, .. }
                )
            ) {
                db.set_web_sync_needs_login(true)?;
                self.snapshot
                    .lock()
                    .map_err(|_| anyhow!("sync status lock poisoned"))?
                    .needs_login = true;
            }
            tracing::warn!(error=%public_error(error), "protected sync setup operation failed");
            self.status("Setup incomplete", &public_error(error))?;
        }
        result
    }

    fn select(&self, db: &Database, command: &SyncCommand) -> Result<()> {
        let url = url::Url::parse(field(command.server.as_deref(), "relay URL")?)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.host().is_none()
            || !(url.scheme() == "https"
                || (url.scheme() == "http"
                    && self.config.allow_insecure_loopback
                    && crate::operator_session::is_loopback(&url)))
        {
            bail!(
                "relay URL requires HTTPS and no embedded credentials/query/fragment; loopback HTTP requires the development opt-in"
            );
        }
        let server = url.as_str().trim_end_matches('/').to_owned();
        let flow = field(command.flow.as_deref(), "account flow")?;
        if !matches!(flow, "create" | "attach" | "join") {
            bail!("choose create, attach or join explicitly");
        }
        if let Some(binding) = db.remote_account_binding()?
            && (binding.relay_url != server
                || binding.local_label != self.config.account
                || (binding.state == "pending" && binding.flow != flow))
        {
            bail!(
                "another relay/account flow is active or pending; automatic migration or account replacement is not supported"
            );
        }
        let mut setup = db.web_sync_setup()?.unwrap_or_else(|| WebSyncSetup {
            relay_url: server.clone(),
            identity_handle: String::new(),
            flow: flow.into(),
            phase: String::new(),
            revision: 0,
            recovery_ack: false,
            publication_millis: millis(),
            approver_device_id: None,
        });
        setup.relay_url = server;
        setup.flow = flow.into();
        self.progress(db, &mut setup, "credentials")?;
        self.status(
            "Server selected",
            "Selection is not authentication or successful sync.",
        )
    }

    fn store(&self, runtime: &Runtime) -> Result<Arc<Mutex<DeviceKeyStore>>> {
        runtime.store.clone().with_context(|| {
            format!(
                "unlock the encrypted key file at {} first",
                self.config.key_file.display()
            )
        })
    }

    fn local_state(&self, db: &Database, store: &DeviceKeyStore) -> Result<LocalAccountState> {
        let state = db.sync_state()?;
        Ok(LocalAccountState {
            has_device_keys: store.load_device_keys(&self.config.account)?.is_some(),
            has_ark: store.load_ark(&self.config.account)?.is_some(),
            has_account_id: store.load_account_id(&self.config.account)?.is_some(),
            is_sync_bound: state.server_url.is_some() || state.account_id.is_some(),
            has_local_content: db.count_content_items(None)? > 0,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn authenticate(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
        command: &SyncCommand,
    ) -> Result<()> {
        let password = field(command.password.as_deref(), "relay password")?;
        let identity = field(command.identity.as_deref(), "relay identity")?;
        let store = self.store(runtime)?;
        let pending = db.remote_account_binding()?;
        let auth = HttpAuth::new(&setup.relay_url)?;
        let keys = {
            let mut store = store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?;
            let state = self.local_state(db, &store)?;
            match setup.flow.as_str() {
                "create" => {
                    let resume = pending
                        .as_ref()
                        .is_some_and(|b| b.state == "pending" && b.flow == "create");
                    if !resume {
                        guard_create_new(&state, command.confirm_create.as_deref() == Some("yes"))?;
                    } else if !state.has_ark || !state.has_account_id || !state.has_device_keys {
                        bail!(
                            "pending creation keys are missing; restore them instead of creating replacement keys"
                        );
                    }
                }
                "attach" => guard_attach_existing(&state)?,
                "join" => guard_join_new_device(
                    &state,
                    pending
                        .as_ref()
                        .is_some_and(|b| b.state == "pending" && b.flow == "join"),
                )?,
                _ => bail!("invalid stored account flow"),
            }
            if pending.is_none() {
                self.status("Checking relay support", "Checking the real v2 OPAQUE endpoint before allocating local account material.")?;
                auth.probe()?;
            }
            let keys = match store.load_device_keys(&self.config.account)? {
                Some(keys) => keys,
                None => DeviceKeypairs::generate()?,
            };
            if setup.flow == "create" {
                let ark = match store.load_ark(&self.config.account)? {
                    Some(ark) => ark,
                    None => AccountRootKey::generate()?,
                };
                let id = match store.load_account_id(&self.config.account)? {
                    Some(id) => id,
                    None => AccountId::generate()?,
                };
                store.save_account_material(&self.config.account, &keys, &ark, &id)?;
            } else if setup.flow == "join" && !state.has_device_keys {
                store.save_device_keys(&self.config.account, &keys)?;
            }
            keys
        };
        setup.identity_handle = identity.into();
        self.progress(db, setup, "authenticating")?;
        self.status(
            "Authenticating",
            "OPAQUE login proves relay authority, not possession of content keys.",
        )?;
        let mut intent = if setup.flow == "join" {
            None
        } else {
            let id = {
                let mut store = store
                    .lock()
                    .map_err(|_| anyhow!("secure store lock poisoned"))?;

                if let Some(id) = store.load_account_id(&self.config.account)? {
                    id
                } else {
                    if store
                        .load_remote_session(&self.config.account, &setup.relay_url)?
                        .is_some()
                        || pending.is_some()
                    {
                        bail!(
                            "canonical identity is missing despite authenticated metadata; restore it"
                        );
                    }
                    let id = match db.sync_state()?.account_id {
                        Some(id) => account_id(&id)?,
                        None => AccountId::generate()?,
                    };
                    store.save_account_id(&self.config.account, &id)?;
                    id
                }
            };
            Some(db.begin_remote_binding(
                &setup.relay_url,
                &self.config.account,
                &id.to_hex(),
                keys.device_id(),
                &uuid::Uuid::new_v4().to_string(),
                &setup.flow,
            )?)
        };
        if command.register.as_deref() == Some("yes") {
            binding::register_identity(&auth, identity, password.as_bytes())?;
        }
        let session = binding::login_device(&auth, identity, password.as_bytes(), &keys)?;
        let status = binding::binding_status(&auth, &session)?;
        let session = if let Some(intent) = intent.as_mut() {
            if status.binding.as_ref().is_some_and(|b| {
                b.content_account_id != intent.content_account_id && b.state != "legacy_reserved"
            }) {
                bail!(
                    "relay identity belongs to different content; this local library will not be replaced"
                );
            }
            db.identify_remote_binding(&session.auth_tenant_id, &status.server_instance_id)?;
            let receipt = binding::bind_empty_namespace(
                &auth,
                &session,
                &keys,
                &intent.content_account_id,
                &intent.operation_id,
            )?;
            let session = binding::login_device(&auth, identity, password.as_bytes(), &keys)?;
            if session.scope != SessionScope::Content
                || session.content_account_id.as_deref() != Some(intent.content_account_id.as_str())
                || session.auth_tenant_id != receipt.auth_tenant_id
                || session.binding_version != receipt.binding_version
            {
                bail!("content session does not match the binding receipt");
            }
            session
        } else {
            let content = session
                .content_account_id
                .as_deref()
                .context("this identity is unbound; use create or attach, not join")?;
            let id = account_id(content)?;
            if session.scope != SessionScope::Content {
                bail!("join has no content authority");
            }
            let intent = db.begin_remote_binding(
                &setup.relay_url,
                &self.config.account,
                content,
                keys.device_id(),
                &uuid::Uuid::new_v4().to_string(),
                "join",
            )?;
            db.identify_remote_binding(&session.auth_tenant_id, &status.server_instance_id)?;
            store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?
                .save_account_id(&self.config.account, &id)?;
            drop(intent);
            session
        };
        if setup.flow != "join" {
            self.verify_history(db, runtime, setup, &session)?;
        }
        store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .save_remote_session(
                &self.config.account,
                &setup.relay_url,
                &serde_json::to_vec(&session)?,
            )?;
        db.set_web_sync_needs_login(false)?;
        self.snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .needs_login = false;
        runtime.provider = Some(self.provider(db, runtime)?);
        if setup.flow == "join" {
            self.progress(db, setup, "keys")?;
            self.status("Content keys required", "Login succeeded, but no ARK was generated. Enroll with a trusted device or supply existing recovery material.")
        } else {
            self.prepare_recovery(db, runtime, setup)
        }
    }

    fn provider(&self, db: &Database, runtime: &Runtime) -> Result<Arc<dyn AccessTokenProvider>> {
        let intent = db
            .remote_account_binding()?
            .context("authenticated relay intent is missing; no blind fallback")?;
        if intent.local_label != self.config.account {
            bail!("configured keystore label conflicts with relay intent");
        }
        let store = self.store(runtime)?;
        let session = {
            let store = store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?;
            let raw = store
                .load_remote_session(&self.config.account, &intent.relay_url)?
                .context(
                    "secure relay session is missing; sign in again, without a blind fallback",
                )?;
            let session: RemoteSession = serde_json::from_slice(&raw)?;
            let keys = store
                .load_device_keys(&self.config.account)?
                .context("device keys are missing")?;
            let id = store
                .load_account_id(&self.config.account)?
                .context("canonical content ID is missing")?;
            if session.scope != SessionScope::Content
                || session.content_account_id.as_deref() != Some(intent.content_account_id.as_str())
                || id.to_hex() != intent.content_account_id
                || session.device_id != keys.device_id()
                || session.device_id != intent.device_id
                || intent.auth_tenant_id.as_deref() != Some(session.auth_tenant_id.as_str())
                || intent
                    .binding_version
                    .is_some_and(|v| v != session.binding_version)
            {
                bail!("secure session does not match canonical content/device/tenant authority");
            }
            (session, keys)
        };
        let label = self.config.account.clone();
        let relay = intent.relay_url.clone();
        let installation = intent
            .server_instance_id
            .clone()
            .context("authenticated installation is missing")?;
        let status_auth = HttpAuth::new(&relay)?;
        let persist = move |next: &RemoteSession| {
            let status = binding::binding_status(&status_auth, next)?;
            if status.server_instance_id != installation {
                return Err(SyncError::SessionNeedsLogin {
                    reason: "relay installation changed",
                });
            }
            store
                .lock()
                .map_err(|_| SyncError::Protocol("secure store lock poisoned".into()))?
                .save_remote_session(&label, &relay, &serde_json::to_vec(next)?)
                .map_err(|_| SyncError::SessionNeedsLogin {
                    reason: "rotated credentials could not be persisted",
                })
        };
        Ok(Arc::new(RefreshingSession::new(
            HttpAuth::new(&intent.relay_url)?,
            session.1,
            session.0,
            persist,
        )?))
    }

    fn relay(&self, db: &Database, runtime: &mut Runtime) -> Result<HttpRelay> {
        if runtime.provider.is_none() {
            runtime.provider = Some(self.provider(db, runtime)?);
        }
        let intent = db
            .remote_account_binding()?
            .context("authenticated intent missing")?;
        Ok(HttpRelay::new(&intent.relay_url)?.with_token_provider(
            runtime
                .provider
                .clone()
                .context("content provider missing")?,
        ))
    }

    fn verify_history(
        &self,
        db: &Database,
        runtime: &Runtime,
        setup: &WebSyncSetup,
        session: &RemoteSession,
    ) -> Result<()> {
        let store = self.store(runtime)?;
        let store = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?;
        let keys = store
            .load_device_keys(&self.config.account)?
            .context("device keys missing")?;
        let ark = store
            .load_ark(&self.config.account)?
            .context("account root key missing")?;
        let id = store
            .load_account_id(&self.config.account)?
            .context("canonical ID missing")?;
        drop(store);
        let credential = Some(pergamon_sync::TransportCredential::Bearer {
            token: session.access_token.clone(),
        });
        let transport = HttpTransport::with_credential(&setup.relay_url, credential.clone())?;
        let relay = HttpRelay::with_credential(&setup.relay_url, credential)?;
        let crypto = CryptoContext::new(
            ark,
            id.to_hex(),
            keys.device_id().into(),
            *keys.ed25519_signing(),
            db.sync_state()?.key_epoch,
        )?;
        client::verify_existing_content(&transport, &relay, &id, &crypto)?;
        Ok(())
    }

    fn material(&self, runtime: &Runtime) -> Result<(DeviceKeypairs, AccountRootKey, AccountId)> {
        let store = self.store(runtime)?;
        let store = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?;
        Ok((
            store
                .load_device_keys(&self.config.account)?
                .context("device keys missing")?,
            store
                .load_ark(&self.config.account)?
                .context("account root key missing; login is not recovery")?,
            store
                .load_account_id(&self.config.account)?
                .context("canonical content ID missing")?,
        ))
    }

    fn renew(&self, db: &Database, runtime: &mut Runtime, command: &SyncCommand) -> Result<()> {
        let intent = db
            .remote_account_binding()?
            .context("no authenticated intent; choose create, attach or join explicitly")?;
        if intent.state != "active" && intent.flow != "attach" {
            bail!("login renewal cannot complete pending creation or adopt a pending join");
        }
        let (_, _, id) = self.material(runtime)?;
        let store = self.store(runtime)?;
        let keys = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .load_device_keys(&self.config.account)?
            .context("device keys missing")?;
        if id.to_hex() != intent.content_account_id || keys.device_id() != intent.device_id {
            bail!("existing local keys/identity do not match relay intent");
        }
        let identity = field(command.identity.as_deref(), "relay identity")?;
        let auth = HttpAuth::new(&intent.relay_url)?;
        let session = binding::login_device(
            &auth,
            identity,
            field(command.password.as_deref(), "relay password")?.as_bytes(),
            &keys,
        )?;
        let status = binding::binding_status(&auth, &session)?;
        if session.scope != SessionScope::Content
            || session.content_account_id.as_deref() != Some(intent.content_account_id.as_str())
            || intent.auth_tenant_id.as_deref() != Some(session.auth_tenant_id.as_str())
            || intent.server_instance_id.as_deref() != Some(status.server_instance_id.as_str())
            || intent
                .binding_version
                .is_some_and(|v| v != session.binding_version)
        {
            bail!("login changed established authority; local adoption refused");
        }
        let mut setup = db.web_sync_setup()?.unwrap_or_else(|| WebSyncSetup {
            relay_url: intent.relay_url,
            identity_handle: identity.into(),
            flow: "attach".into(),
            phase: String::new(),
            revision: 0,
            recovery_ack: false,
            publication_millis: millis(),
            approver_device_id: None,
        });
        self.verify_history(db, runtime, &setup, &session)?;
        store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .save_remote_session(
                &self.config.account,
                &setup.relay_url,
                &serde_json::to_vec(&session)?,
            )?;
        db.set_web_sync_needs_login(false)?;
        self.snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .needs_login = false;
        runtime.provider = Some(self.provider(db, runtime)?);
        setup.identity_handle = identity.into();
        self.progress(db, &mut setup, "ready")?;
        self.activate(db, runtime, &mut setup)
    }

    fn ensure_device(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &WebSyncSetup,
        root: bool,
    ) -> Result<()> {
        let (keys, _, id) = self.material(runtime)?;
        let relay = self.relay(db, runtime)?;
        let record = if let Some(bytes) = relay.device_get(&id.to_hex(), keys.device_id())? {
            let record = SignedDeviceRecord::from_bytes(&bytes)?;
            record.verify()?;
            if record.record.ed25519_pub != *keys.ed25519_verifying()
                || record.record.x25519_pub != *keys.x25519_public()
            {
                bail!("remote device record does not match local keys");
            }
            record
        } else {
            let record = keys.sign_record(setup.publication_millis);
            relay.device_put(&id.to_hex(), keys.device_id(), &record.to_bytes())?;
            record
        };
        if root {
            let attestations = relay.attestations_list(&id.to_hex(), 0)?;
            let already_rooted = attestations.iter().any(|row| {
                pergamon_crypto::SignedAttestation::from_bytes(&row.attestation).is_ok_and(|a| {
                    a.verify().is_ok()
                        && a.attestation.kind == pergamon_crypto::AttestationKind::Trust
                        && a.attestation.signer_device_id == keys.device_id()
                        && a.attestation.subject_device_id == keys.device_id()
                })
            });
            if !already_rooted {
                let bytes = pergamon_crypto::attest_trust(
                    &keys,
                    &record.record,
                    db.sync_state()?.key_epoch,
                    setup.publication_millis,
                )
                .to_bytes();
                if !attestations.iter().any(|a| a.attestation == bytes) {
                    relay.attestation_append(&id.to_hex(), &bytes)?;
                }
            }
        }
        Ok(())
    }

    fn prepare_recovery(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
    ) -> Result<()> {
        self.ensure_device(db, runtime, setup, true)?;
        let (_, ark, id) = self.material(runtime)?;
        let relay = self.relay(db, runtime)?;
        let remote = relay.recovery_get(&id.to_hex())?;
        let store = self.store(runtime)?;
        if setup.recovery_ack {
            if remote.is_none() {
                bail!("acknowledged recovery publication is missing; do not replace it silently");
            }
            self.progress(db, setup, "ready")?;
            return self.status(
                "Ready to sync",
                "The existing capture acknowledgement and account identity are preserved.",
            );
        }
        let mut store = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?;
        let pending = if let Some(raw) =
            store.load_bootstrap_recovery(&self.config.account, &setup.relay_url)?
        {
            let pending: PendingRecovery = serde_json::from_slice(&raw)?;
            pending.validate(&ark, &id)?;
            pending
        } else {
            if remote.is_some() {
                if setup.flow == "attach" {
                    drop(store);
                    self.progress(db, setup, "ready")?;
                    return self.status(
                        "Ready to sync",
                        "Existing recovery material was preserved, not overwritten.",
                    );
                }
                bail!(
                    "recovery already exists but its capture secret is missing locally; restore saved material rather than overwrite it"
                );
            }
            let pending = PendingRecovery::new(&ark, &id)?;
            store.save_bootstrap_recovery(
                &self.config.account,
                &setup.relay_url,
                &serde_json::to_vec(&pending)?,
            )?;
            pending
        };
        drop(store);
        if remote.as_ref().is_some_and(|blob| *blob != pending.blob) {
            bail!("the relay has different recovery material; refusing to overwrite it");
        }
        relay.recovery_put(&id.to_hex(), &pending.blob)?;
        if relay.recovery_get(&id.to_hex())?.as_ref() != Some(&pending.blob) {
            bail!("recovery publication could not be verified");
        }
        self.progress(db, setup, "recovery")?;
        self.status("Save recovery material", "Recovery is published, but content sync is blocked until you save and acknowledge the code.")
    }

    fn capture(&self, runtime: &Runtime, setup: &WebSyncSetup) -> Result<PendingRecovery> {
        if setup.recovery_ack || setup.phase != "recovery" {
            bail!("recovery capture is not pending");
        }
        let (_, ark, id) = self.material(runtime)?;
        let store = self.store(runtime)?;
        let bytes = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .load_bootstrap_recovery(&self.config.account, &setup.relay_url)?
            .context("capture material missing")?;
        let pending: PendingRecovery = serde_json::from_slice(&bytes)?;
        pending.validate(&ark, &id)?;
        Ok(pending)
    }

    /// Read an owner-only capture for a no-store view/download on a blocking thread.
    pub fn recovery_capture(&self) -> Result<PendingRecovery> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow!("sync service lock poisoned"))?;
        let setup = Database::open(&self.config.db_path)?
            .web_sync_setup()?
            .context("no capture pending")?;
        self.capture(&runtime, &setup)
    }

    fn recover(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
        command: &SyncCommand,
    ) -> Result<()> {
        if setup.flow != "join" || !matches!(setup.phase.as_str(), "keys" | "enrollment") {
            bail!("recovery is only for an explicit pending join");
        }
        let intent = db
            .remote_account_binding()?
            .context("authenticated join intent missing")?;
        let id = account_id(&intent.content_account_id)?;
        let secret = field(command.recovery_code.as_deref(), "existing recovery secret")?;
        let relay = self.relay(db, runtime)?;
        let ark = if let Some(package) = command
            .recovery_package
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD.decode(package.trim())?;
            let package = pergamon_crypto::KeyPackage::from_bytes(&bytes)?;
            if package.account_id != id {
                bail!("key package belongs to another content account");
            }
            pergamon_crypto::import_key_package(&package, secret.as_bytes())?
        } else {
            client::recover_ark(&relay, &id, secret.as_bytes())?
        };
        self.adopt_join_keys(db, runtime, setup, &ark, &id, true)
    }

    fn enroll(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
        command: &SyncCommand,
    ) -> Result<()> {
        if setup.flow != "join" || !matches!(setup.phase.as_str(), "keys" | "enrollment") {
            bail!("no pending join for enrollment");
        }
        let peer = peer_id(field(command.device.as_deref(), "trusted approver device")?)?;
        let intent = db
            .remote_account_binding()?
            .context("authenticated join intent missing")?;
        let id = account_id(&intent.content_account_id)?;
        let store = self.store(runtime)?;
        let keys = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .load_device_keys(&self.config.account)?
            .context("join device keys missing")?;
        let relay = self.relay(db, runtime)?;
        client::enroll_publish(&relay, &id, &keys, setup.publication_millis)?;
        let sas = client::sas_against(&relay, &id, &keys, peer)?.digits();
        setup.approver_device_id = Some(peer.into());
        self.progress(db, setup, "enrollment")?;
        self.display_sas(peer, &sas)?;
        self.status(
            "Waiting for trusted-device approval",
            "Compare the SAS on both devices. Login alone still has not supplied an ARK.",
        )
    }

    fn display_sas(&self, device: &str, sas: &str) -> Result<()> {
        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?;
        snapshot.sas = sas.into();
        snapshot.sas_device = device.into();
        Ok(())
    }

    fn accept(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
        command: &SyncCommand,
    ) -> Result<()> {
        if setup.flow != "join" || setup.phase != "enrollment" {
            bail!("no enrollment is pending");
        }
        let peer = setup
            .approver_device_id
            .as_deref()
            .context("trusted approver is not selected")?;
        let intent = db
            .remote_account_binding()?
            .context("authenticated join intent missing")?;
        let id = account_id(&intent.content_account_id)?;
        let keys = self
            .store(runtime)?
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .load_device_keys(&self.config.account)?
            .context("join device keys missing")?;
        let relay = self.relay(db, runtime)?;
        let sas = client::sas_against(&relay, &id, &keys, peer)?.digits();
        if normalize_sas(field(command.expect_sas.as_deref(), "verified SAS")?)
            != normalize_sas(&sas)
        {
            bail!("SAS mismatch; do not accept this device");
        }
        let accepted = client::accept(&relay, &id, &keys)?;
        if accepted.bundle.account_id != id || accepted.approver_device_id.as_deref() != Some(peer)
        {
            bail!(
                "enrollment bundle or approving device does not match the authenticated selection"
            );
        }
        self.adopt_join_keys(db, runtime, setup, &accepted.bundle.ark, &id, false)
    }

    fn adopt_join_keys(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
        ark: &AccountRootKey,
        id: &AccountId,
        recovery_root: bool,
    ) -> Result<()> {
        let relay = self.relay(db, runtime)?;
        let epoch = client::current_epoch(&relay, id)?;
        let store = self.store(runtime)?;
        let keys = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .load_device_keys(&self.config.account)?
            .context("join device keys missing")?;
        let state = {
            let unlocked = store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?;
            self.local_state(db, &unlocked)?
        };
        guard_join_new_device(&state, true)?;
        let crypto = CryptoContext::new(
            AccountRootKey::from_bytes(*ark.expose_bytes()),
            id.to_hex(),
            keys.device_id().into(),
            *keys.ed25519_signing(),
            epoch,
        )?;
        let transport = HttpTransport::new(&setup.relay_url)?.with_token_provider(
            runtime
                .provider
                .clone()
                .context("authenticated provider missing")?,
        );
        client::verify_existing_content(&transport, &relay, id, &crypto)?;
        store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?
            .save_account_material(&self.config.account, &keys, ark, id)?;
        db.set_key_epoch(epoch)?;
        self.ensure_device(db, runtime, setup, recovery_root)?;
        self.progress(db, setup, "ready")?;
        self.activate(db, runtime, setup)
    }

    fn activate(
        &self,
        db: &Database,
        runtime: &mut Runtime,
        setup: &mut WebSyncSetup,
    ) -> Result<()> {
        if db.web_sync_needs_login()? {
            return Err(SyncError::SessionNeedsLogin {
                reason: "fresh authentication is required before adoption or restart",
            }
            .into());
        }
        if !matches!(setup.phase.as_str(), "ready" | "active")
            || (setup.flow == "create" && !setup.recovery_ack)
        {
            bail!("keys and required recovery capture must be complete before attaching/syncing");
        }
        let (keys, _, id) = self.material(runtime)?;
        let mut intent = db
            .remote_account_binding()?
            .context("authenticated binding intent missing")?;
        if intent.content_account_id != id.to_hex() || intent.device_id != keys.device_id() {
            bail!("local content/device identity changed before adoption");
        }
        let provider = self.provider(db, runtime)?;
        let session = {
            let store = self.store(runtime)?;
            let store = store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?;
            let bytes = store
                .load_remote_session(&self.config.account, &setup.relay_url)?
                .context("secure content session missing")?;
            let mut session: RemoteSession = serde_json::from_slice(&bytes)?;
            drop(store);
            session.access_token = provider.access_token()?;
            session
        };
        let status = binding::binding_status(&HttpAuth::new(&setup.relay_url)?, &session)?;
        db.identify_remote_binding(&session.auth_tenant_id, &status.server_instance_id)?;
        intent.auth_tenant_id = Some(session.auth_tenant_id);
        intent.server_instance_id = Some(status.server_instance_id);
        intent.binding_version = Some(session.binding_version);
        if setup.recovery_ack {
            self.store(runtime)?
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?
                .remove_bootstrap_recovery(&self.config.account, &setup.relay_url)?;
        }
        db.activate_remote_binding(&intent, u64::try_from(millis()).context("invalid clock")?)?;
        self.progress(db, setup, "active")?;
        runtime.provider = Some(provider);
        self.start(db, runtime)
    }

    #[allow(clippy::too_many_lines)]
    fn start(&self, db: &Database, runtime: &mut Runtime) -> Result<()> {
        if db.web_sync_needs_login()? {
            return Err(SyncError::SessionNeedsLogin { reason: "fresh authentication is required; consumed refresh credentials will not be replayed" }.into());
        }
        if db
            .web_sync_setup()?
            .is_some_and(|s| s.flow == "create" && !s.recovery_ack)
        {
            bail!("new web account recovery capture has not been acknowledged");
        }
        if let Some(mut worker) = runtime.worker.take() {
            worker.stop()?;
        }
        let (keys, ark, id) = self.material(runtime)?;
        let provider = match runtime.provider.clone() {
            Some(provider) => provider,
            None => self.provider(db, runtime)?,
        };
        let intent = db
            .remote_account_binding()?
            .context("active authenticated binding missing")?;
        let current = {
            let store = self.store(runtime)?;
            let store = store
                .lock()
                .map_err(|_| anyhow!("secure store lock poisoned"))?;
            let bytes = store
                .load_remote_session(&self.config.account, &intent.relay_url)?
                .context("secure session missing; no blind fallback")?;
            let mut session: RemoteSession = serde_json::from_slice(&bytes)?;
            drop(store);
            session.access_token = provider.access_token()?;
            binding::binding_status(&HttpAuth::new(&intent.relay_url)?, &session)?
        };
        if intent.server_instance_id.as_deref() != Some(current.server_instance_id.as_str()) {
            bail!("relay installation changed; refusing to start sync");
        }
        let snapshot = self.snapshot.clone();
        let db_path = self.config.db_path.clone();
        let observe = Arc::new(move |event| {
            if matches!(&event, WorkerEvent::Failed(_, true)) {
                match Database::open(&db_path).and_then(|db| db.set_web_sync_needs_login(true)) {
                    Ok(()) => {}
                    Err(error) => {
                        tracing::error!(error=%error, "could not persist sync reauthentication requirement");
                    }
                }
            }
            let Ok(mut status) = snapshot.lock() else {
                tracing::error!("sync status observer lock poisoned");
                return;
            };
            match event {
                WorkerEvent::Syncing => {
                    status.label = "Syncing".into();
                    status.message =
                        "Exchanging real changes and required blobs with the selected relay."
                            .into();
                }
                WorkerEvent::Synced(counters) => {
                    status.label = "Connected".into();
                    status.message = "Push, pull and upload completeness verified.".into();
                    status.pushed = counters.pushed;
                    status.applied = counters.applied;
                    status.last_success = time::OffsetDateTime::now_utc().to_string();
                    status.retry_at_millis = 0;
                }
                WorkerEvent::Offline(message, seconds) => {
                    status.label = "Offline / retrying".into();
                    status.message = message;
                    status.retry_at_millis = millis().saturating_add(
                        i64::try_from(seconds)
                            .unwrap_or(i64::MAX)
                            .saturating_mul(1_000),
                    );
                }
                WorkerEvent::Failed(message, login) => {
                    status.worker = WorkerState::Stopped;
                    status.needs_login = login;
                    status.label = if login {
                        "Sign in again"
                    } else {
                        "Sync incomplete"
                    }
                    .into();
                    status.message = message;
                }
                WorkerEvent::Stopped => {
                    status.worker = WorkerState::Stopped;
                    status.label = "Paused".into();
                    status.message = "Worker stopped; local data is preserved.".into();
                }
            }
        });
        self.status(
            "Starting sync",
            "The worker is starting; connection success has not been claimed.",
        )?;
        self.snapshot
            .lock()
            .map_err(|_| anyhow!("sync status lock poisoned"))?
            .worker = WorkerState::Running;
        let worker = sync_worker::spawn_authenticated(AuthenticatedWorkerConfig {
            db_path: self.config.db_path.clone(),
            blob_dir: self.config.blob_dir.clone(),
            server: intent.relay_url,
            content_id: id,
            ark,
            keys,
            epoch: db.sync_state()?.key_epoch,
            provider: provider.clone(),
            interval_secs: self.config.interval_secs,
            observe,
        })
        .inspect_err(|_| {
            if let Ok(mut snapshot) = self.snapshot.lock() {
                snapshot.worker = WorkerState::Stopped;
            }
        })?;
        runtime.provider = Some(provider);
        runtime.worker = Some(worker);
        Ok(())
    }

    fn show_sas(&self, db: &Database, runtime: &mut Runtime, command: &SyncCommand) -> Result<()> {
        let device = peer_id(field(command.device.as_deref(), "peer device")?)?;
        let (keys, _, id) = self.material(runtime)?;
        let relay = self.relay(db, runtime)?;
        let sas = client::sas_against(&relay, &id, &keys, device)?.digits();
        self.display_sas(device, &sas)
    }

    fn approve(&self, db: &Database, runtime: &mut Runtime, command: &SyncCommand) -> Result<()> {
        let intent = db
            .remote_account_binding()?
            .context("authenticated binding missing")?;
        if intent.state != "active" {
            bail!("only an active trusted device can approve another device");
        }
        let device = peer_id(field(command.device.as_deref(), "new device")?)?;
        let (keys, ark, id) = self.material(runtime)?;
        let relay = self.relay(db, runtime)?;
        let peer = client::fetch_device_record(&relay, &id, device)?;
        let sas = client::sas_against(&relay, &id, &keys, device)?.digits();
        if normalize_sas(field(command.expect_sas.as_deref(), "verified SAS")?)
            != normalize_sas(&sas)
        {
            bail!("SAS mismatch; do not approve this device");
        }
        let epoch = db.sync_state()?.key_epoch;
        let operation = format!("approve:{device}:{epoch}");
        let store = self.store(runtime)?;
        let mut store = store
            .lock()
            .map_err(|_| anyhow!("secure store lock poisoned"))?;
        let publication = if let Some(bytes) =
            store.load_onboarding_artifact(&self.config.account, &intent.relay_url, &operation)?
        {
            let publication: ApprovalPublication = serde_json::from_slice(&bytes)?;
            if publication.content_id != id.to_hex()
                || publication.device_id != device
                || publication.peer != peer.to_bytes()
                || publication.epoch != epoch
                || publication.sas != sas
            {
                bail!("pending approval identity changed; refusing to reuse its sealed material");
            }
            publication
        } else {
            let publication = ApprovalPublication {
                content_id: id.to_hex(),
                device_id: device.into(),
                epoch,
                sas,
                peer: peer.to_bytes(),
                bundle: pergamon_crypto::seal_enrollment_bundle(
                    &peer.record.x25519_pub,
                    device,
                    &ark,
                    &id,
                    epoch,
                )?,
                attestation: pergamon_crypto::attest_trust(&keys, &peer.record, epoch, millis())
                    .to_bytes(),
            };
            store.save_onboarding_artifact(
                &self.config.account,
                &intent.relay_url,
                &operation,
                &serde_json::to_vec(&publication)?,
            )?;
            publication
        };
        drop(store);
        if !relay
            .wraps_list(&id.to_hex(), device, 0)?
            .iter()
            .any(|w| w.bundle == publication.bundle)
        {
            relay.wrap_put(&id.to_hex(), device, &publication.bundle)?;
        }
        if !relay
            .attestations_list(&id.to_hex(), 0)?
            .iter()
            .any(|a| a.attestation == publication.attestation)
        {
            relay.attestation_append(&id.to_hex(), &publication.attestation)?;
        }
        self.status("Device approved", "The same sealed bundle and trust attestation were published. The new device can verify and accept them.")
    }
}

#[derive(Serialize, Deserialize)]
struct ApprovalPublication {
    content_id: String,
    device_id: String,
    epoch: u32,
    sas: String,
    peer: Vec<u8>,
    bundle: Vec<u8>,
    attestation: Vec<u8>,
}

/// Public errors never echo submitted passwords or an arbitrary relay response.
pub fn public_error(error: &anyhow::Error) -> String {
    if let Some(error) = error.downcast_ref::<SyncError>() {
        return match error {
            SyncError::AuthRefused { status: 401, .. } => "Authentication failed. Check the relay identity and password.".into(),
            SyncError::AuthRefused { status: 404, .. } => "This server does not support v2 authenticated sync. A healthy blind relay is not enough.".into(),
            SyncError::AuthRefused { code, .. } if code == "CONTENT_NAMESPACE_UNAVAILABLE" => "This content namespace cannot be allocated. Existing local keys/data were preserved.".into(),
            SyncError::RateLimited { retry_after_seconds } => format!("Too many requests. Retry after {} seconds.", retry_after_seconds.unwrap_or(60)),
            SyncError::SessionNeedsLogin { .. } => "Sign in again. The session expired, was revoked, or a refresh outcome could not be safely persisted.".into(),
            SyncError::Transport(_) => "The relay is unreachable or temporarily unavailable. Safe local setup progress was preserved.".into(),
            SyncError::Protocol(message) if message.contains("OPAQUE") => "Authentication failed. Check the relay identity and password.".into(),
            SyncError::Serialization(_) | SyncError::Base64(_) | SyncError::Protocol(_) =>
                "Relay or saved setup data failed validation; no account or key replacement was performed.".into(),
            SyncError::BadEventSignature { .. } => "An event signature failed verification; sync stopped without applying it.".into(),
            SyncError::UnknownSigner { .. } => "A signer is absent from the verified roster; sync will refresh and retry.".into(),
            SyncError::MissingBlob(_) => "A required blob is missing; sync is incomplete.".into(),
            SyncError::Storage(_) => "Local sync storage failed; setup remains incomplete.".into(),
            _ => error.to_string(),
        };
    }
    if error.downcast_ref::<serde_json::Error>().is_some() {
        "Saved setup/session data is malformed; restore the correct encrypted file or sign in again.".into()
    } else {
        error.to_string()
    }
}
