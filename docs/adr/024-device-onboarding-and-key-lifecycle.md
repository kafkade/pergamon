# ADR-024: Device Onboarding and Key Lifecycle

**Status**: Accepted  
**Date**: 2026-07-03  
**Deciders**: kafkade

## Context

Phase 7 (epic #35) adds **optional, end-to-end encrypted multi-device sync**.
ADR-022 (#121) fixed the **wire contract** — an append-only, server-sequenced
log of encrypted event envelopes, content-addressed blobs, and cursor-based pull
— and ADR-023 (#122) fixed **conflict resolution**. Both deliberately deferred
*the cryptography and key management* to this ADR. ADR-022 states it plainly: the
server only ever holds ciphertext, and "the specific AEAD construction and key
schedule are ADR-024/#125"; the envelope merely *carries* the crypto-relevant
fields (`account_id`, `device_id`, `key_epoch`, blinded `entity_ref`, blob
`ct_hash`, and AAD binding) without deciding how any of them are produced.

This ADR decides the **key management scheme and its lifecycle** so those fields
have concrete meaning:

1. **Account bootstrap and identity** — how an account is created and what
   opaque `account_id` the server sees, with no email, no server-side identity,
   and no plaintext content ever leaving a device.
2. **Device keypairs, enrollment, and trust** — how each device generates keys,
   how an existing trusted device authorizes a new one via public-key exchange,
   and how a man-in-the-middle during enrollment is prevented.
3. **Key derivation for content encryption** — the hierarchy from an account
   root secret down to the per-epoch keys that encrypt ADR-022 events and blobs,
   including the `account_stream_key` that produces ADR-022's `entity_ref` HMAC
   and the content-derived (convergent) keys that keep ADR-022's ciphertext-hash
   blob dedup working.
4. **Recovery** — an optional, opt-in recovery path for a user who has lost all
   trusted devices.
5. **Revocation and rotation** — how removing a device advances `key_epoch`,
   re-wraps the new epoch key to the remaining devices, and what
   forward/backward-secrecy guarantees that implies.

It deliberately does **not** decide:

- **The wire contract** — envelope schema, event/blob split, cursors,
  versioning. That is ADR-022, and this ADR introduces **no new wire fields**; it
  only supplies the semantics of fields ADR-022 already defined.
- **Conflict resolution** — that is ADR-023. Wrapped-key envelopes, device
  records, and revocation attestations sync as ordinary entities/events and are
  reconciled by the same policy.
- **The web-app login boundary** — ADR-017's single owner password protects the
  self-hosted *web UI*. It is unrelated to the E2EE *account/device identity*
  introduced here; a reader must not conflate the two (see "Relationship to
  ADR-017" below).
- **Implementation** — the concrete crypto module, OS-keychain integration, and
  the enrollment/recovery HTTP endpoints are #125. This ADR fixes the scheme and
  the primitives it is built from.

### Dependencies and constraints

- **ADR-022 (envelope):** the server sees only opaque `account_id` / `device_id`,
  a monotonic `server_seq`, the idempotency key `change_id`, a blinded
  `entity_ref = HMAC(account_stream_key, entity_type‖entity_id)`, an integer
  `key_epoch`, blob ciphertext hashes `ct_hash`, and sizes. The AEAD **AAD** must
  cover at least `protocol_version, account_id, change_id, key_epoch, blob_refs`.
  Blobs are content-addressed by *ciphertext* hash, so identical plaintext must
  encrypt to identical ciphertext to dedup — which requires **content-derived
  keys** for immutable content. This ADR must satisfy exactly those requirements.
- **ADR-007 / ADR-001 (zero-I/O core):** the key *schedule* — every derivation,
  wrap/unwrap, and AAD construction — is pure computation and lives in
  `pergamon-core` (or the Apache-2.0 client crypto module), exhaustively
  unit-testable and reusable across CLI, iOS, and web. Key *storage* (OS
  keychain) and *transport* (enrollment/recovery over HTTP) live in
  platform/client code and the server, never in the core.
- **ADR-008 (licensing):** the AGPL `pergamon-sync-server` stores wrapped keys,
  device records, revocation attestations, and the recovery blob as **opaque
  ciphertext** and relays them; it never unwraps or derives anything. No crypto
  logic is pulled into `pergamon-core`.
- **ADR-020 (mobile storage):** device private keys live in the platform secure
  enclave / Keychain on iOS; the CLI stores them in the OS keychain (Keychain,
  Secret Service, Windows Credential Manager) or, as a fallback, an
  Argon2id-passphrase-encrypted key file. Private keys never sync and never
  leave the device.
- **Roadmap §2.5 / Decisions #20, #21:** device onboarding via public-key
  exchange, sync batches and blobs encrypted client-side, server stores
  ciphertext only. Local SQLite stays plaintext (OS disk encryption); E2EE is a
  **sync-boundary** property, not local at-rest field encryption in v1.

## Decision

### Cryptographic primitives

One modern, well-reviewed primitive per job; all are available as audited Rust
crates and compile to WASM (ADR-016).

| Purpose | Primitive |
|---------|-----------|
| Key agreement (device enrollment, key wrapping) | **X25519** ECDH, used as a libsodium-style sealed box / authenticated ECDH |
| Signatures (device attestation, revocation, trust) | **Ed25519** |
| Authenticated encryption (events, blobs, key-wrap) | **XChaCha20-Poly1305** (192-bit random nonce ⇒ safe random nonces without a counter) |
| Key derivation (hierarchy, per-message keys) | **HKDF-SHA-256** with distinct, hard-coded `info` domain-separation labels |
| Recovery-passphrase stretching | **Argon2id** (OWASP-tier parameters, same as ADR-017) |
| Content hashing (`ct_hash`, convergent-key input) | **BLAKE3** |

Rationale: this is the standard modern E2EE toolbox (the NaCl/libsodium and
Signal lineage). XChaCha20-Poly1305's extended nonce removes the operational
hazard of nonce reuse across many messages and devices without a shared counter,
which matters for a multi-writer log. All keys are 256-bit.

### Account identity and bootstrap

An account is **a secret, not a login**. Creating an account (first device)
generates:

- **Account Root Key (ARK):** a 256-bit CSPRNG secret. The ARK is the root of
  the entire key hierarchy and is the thing device enrollment and recovery
  transfer. It is never sent to the server in any form except wrapped ciphertext.
- **`account_id`:** a separate, independent 128-bit random opaque handle — **not
  derived from the ARK** — so that the identifier the server indexes leaks
  nothing about the key material. It is the ADR-022 `account_id`.

There is no email, username, or server-side identity record beyond this opaque
handle plus whatever ciphertext the account uploads. The server cannot enumerate
who owns an account or link it to a person.

### Key hierarchy

Everything is derived from the ARK by HKDF-SHA-256 with domain-separating
labels, so no two purposes ever share key material:

```text
Account Root Key (ARK, 256-bit random, never leaves a device in plaintext)
├── account_stream_key   = HKDF(ARK, "pergamon/v1/entity-ref")
│      └── used only as the HMAC key for ADR-022 entity_ref blinding
├── ACK_e  (per key epoch e)= HKDF(ARK, "pergamon/v1/account-content" ‖ epoch=e)
│      the account content key for epoch e; the KEK/derivation root for content
│      ├── event key   = HKDF(ACK_e, "pergamon/v1/event" ‖ change_id)
│      │      per-event key; AEAD-encrypts the event body, AAD = ADR-022 header
│      └── blob key    = HKDF(ACK_e, "pergamon/v1/blob" ‖ BLAKE3(plaintext))
│             convergent key for an immutable blob (see below)
└── recovery is a *wrapping of the ARK*, not a hierarchy branch (see Recovery)
```

- **`account_stream_key`** feeds ADR-022's `entity_ref` HMAC exactly. Because it
  is a stable per-account key, the same entity always blinds to the same token,
  which is what lets the server coalesce and targeted-fetch per entity without
  learning identity.
- **`ACK_e`** is named by the envelope's **`key_epoch`**. A device that holds the
  set `{ACK_0 … ACK_n}` can decrypt any event/blob regardless of which epoch
  encrypted it; rotation adds a new epoch without invalidating the ability to
  read history.
- **Event bodies** use a fresh per-event key derived from `ACK_e` and the
  `change_id`, so the derivation is deterministic (any holder of `ACK_e`
  reproduces it) yet unique per event. The AEAD AAD is the ADR-022 header
  (`protocol_version, account_id, change_id, key_epoch, blob_refs`), binding the
  ciphertext to its routing so a server cannot re-target or replay it.
- **Immutable blobs use convergent (content-derived) keys.** The blob key is
  derived from `ACK_e` and `BLAKE3(plaintext)`. Identical plaintext therefore
  produces an identical key, an identical nonce (also derived from the content
  hash), and thus **identical ciphertext** — so ADR-022's content-addressed blob
  store deduplicates by `ct_hash` as designed. The nonce is content-derived
  rather than random precisely to make encryption deterministic. This is
  scoped per epoch, so the same plaintext under a new epoch re-encrypts to a new
  blob (acceptable; blobs are immutable and reference-counted in ADR-022).

  Convergent encryption has a known trade-off: an attacker who *guesses* a
  plaintext can confirm the account stores it (a "confirmation-of-file" oracle),
  and equal plaintexts are linkable. Scoping the key under the secret `ACK_e`
  (rather than a pure hash of the plaintext) means only a party who already holds
  the account key can run that check — the blind server cannot — so the residual
  leak is limited to cross-account "do two accounts share this exact blob"
  observations. For a personal library of articles and PDFs this dedup win
  outweighs the narrow leak; an account may disable convergent keys and fall
  back to random per-blob keys (losing dedup) as a future opt-in.

### Device keypairs

On first run, every device generates two keypairs and stores the private halves
in the platform secure store (ADR-020); the private keys never sync:

- **X25519 keypair** — for key agreement: receiving wrapped account keys during
  enrollment and recovery.
- **Ed25519 keypair** — for attestation: signing this device's own record, and
  (for a trusting device) signing trust and revocation statements.

A device publishes a self-signed **device record** — `{device_id,
x25519_pub, ed25519_pub, created_at}`, signed by its Ed25519 key — as an ordinary
synced entity. `device_id` is the ADR-022 opaque origin handle. Device records
form the account's roster of known devices; trust between them is asserted
separately (below).

### Enrollment: device-to-device trust with out-of-band verification

The **primary** way a second (or later) device joins an account is by being
**authorized by an existing trusted device**, via public-key exchange with an
out-of-band human check to defeat a man-in-the-middle. The server only relays
opaque ciphertext and never participates in trust.

```text
New device N                         Existing trusted device E        Server
------------                         -------------------------        ------
generate X25519/Ed25519
publish device record (N) ------------------------------------------> store
show enrollment request:
  {N.x25519_pub, N.ed25519_pub}
  + Short Auth String (SAS)  ---- out-of-band (QR / 6-word code) ----> shown to user

                                    user compares SAS on both screens
                                    (defeats MITM: a swapped key => SAS mismatch)
                                    user approves N

                                    seal { ARK, {ACK_0..ACK_n},
                                           account_id, current key_epoch }
                                    to N.x25519_pub (X25519 sealed box)   -> store
                                    sign trust attestation over N's
                                    device record with E.ed25519 key      -> store

fetch sealed bundle <------------------------------------------------- relay
open with N.x25519_priv
=> N now holds ARK, all ACKs, account_id
verify E's trust attestation
N is fully enrolled; begins ADR-022 pull from snapshot/cursor
```

- The **Short Authentication String (SAS)** is derived from both devices'
  enrollment public keys (e.g. a BLAKE3 commitment rendered as a QR code and a
  short word list). Verifying it out-of-band binds the two real public keys, so
  a server that swaps in its own key to intercept the sealed bundle is detected
  because the SAS will not match. This is the standard defense against
  enrollment MITM.
- The sealed bundle is an **X25519 sealed box** — only the holder of N's private
  key can open it — carrying the ARK and the full set of epoch keys so N can read
  all history. The server stores and relays it blind.
- The **trust attestation** is E's Ed25519 signature over N's device record. The
  set of attestations is the account's web of trust; a device is "trusted" when
  attested by an already-trusted device, rooted at the account's first device.

### Bootstrap-from-nothing and recovery

The **first** device creates the account (generates ARK, `account_id`, its own
keypairs) with no external input. Every later device needs the ARK, obtained
either from a trusted device (above) or — if none remain — from **recovery**.

**Recovery is optional and opt-in, off by default.** Enabling it presents an
explicit warning that the passphrase's strength is the *entire* protection on the
account key stored (as ciphertext) on the server.

```text
Enable recovery:
  user chooses a recovery passphrase (or generates a printable recovery code)
  recovery KEK = Argon2id(passphrase, per-account random salt)   [client-side]
  recovery blob = XChaCha20-Poly1305_wrap(ARK, key = recovery KEK,
                    AAD = account_id ‖ current key_epoch)
  upload opaque recovery blob to server                          [server stores ciphertext]

Recover on a fresh device with no trusted device available:
  fetch recovery blob (by account_id)
  recovery KEK = Argon2id(passphrase, salt)
  ARK = unwrap(recovery blob)  => derive all ACK_e, account_id checks out
  device self-enrolls: publishes its device record + a self-trust rooted at recovery
```

- The KEK source is either a **user passphrase** (Argon2id-stretched, same
  parameter tier as ADR-017) or a **printable high-entropy recovery code**
  (offered as the stronger, phishing-resistant alternative for users who would
  otherwise pick a weak passphrase). Both wrap the same ARK.
- The recovery blob is re-wrapped whenever `key_epoch` advances so that the AAD's
  epoch stays current; the ARK itself is stable, so a rotation does not force the
  user to re-enter the passphrase (the client re-wraps using the ARK it holds).
- Because recovery uploads a ciphertext derived from a human-memorable secret, it
  is the account's weakest link by construction — hence off by default, warned,
  and offered alongside the stronger recovery-code option.

### Revocation and key rotation

Removing a device (lost, sold, compromised) is a **key-epoch rotation**:

1. A trusted device increments `key_epoch` from `e` to `e+1` and derives
   `ACK_{e+1} = HKDF(ARK, "pergamon/v1/account-content" ‖ e+1)`.
2. It **re-wraps** the updated epoch-key set to **every remaining trusted
   device** (X25519 sealed boxes) — but **not** to the revoked device.
3. It publishes a **revocation attestation** (Ed25519-signed) removing the
   device from the trusted roster; other devices honor it and drop the revoked
   device record.
4. All **new** content is encrypted under `ACK_{e+1}`; `key_epoch` in each new
   envelope names the epoch, and (per ADR-022) `key_epoch` is inside the AAD, so
   a server cannot silently downgrade a client to an older epoch.
5. If recovery is enabled, the recovery blob is re-wrapped for epoch `e+1`.

**Secrecy boundary — stated honestly.** A revoked device *keeps whatever epoch
keys it already held* (`ACK_0 … ACK_e`) and *keeps any plaintext it already
downloaded*. Rotation therefore protects **future** content (encrypted under
`ACK_{e+1}`, which the revoked device never receives) but does **not**
retroactively re-protect content the device could already read. Because ADR-022
blobs are immutable and content-addressed, true forward-secrecy for *old* content
would require re-encrypting and re-uploading the entire library under the new
epoch and pruning the old blobs — expensive and explicitly **out of scope** here;
it is offered as a future "full re-key" operation. This bounded guarantee — new
content is protected immediately, old content is assumed already-seen by a device
that held its keys — is the pragmatic and honest choice for a personal library.

### Wrapped-key envelope AAD binding

Every wrapped-key artifact (enrollment bundle, per-device re-wrap on rotation,
recovery blob) binds its recipient and epoch in the AEAD AAD — at minimum
`account_id`, recipient `device_id` (for device-targeted wraps), and `key_epoch`.
A server that reorders, replays, or re-targets a wrapped bundle to the wrong
device or epoch causes the unwrap to fail, so it cannot trick a device into
adopting a bundle meant for another device or epoch.

### Relationship to ADR-017

ADR-017's single owner password authenticates a browser to the **self-hosted web
app** and is stored (Argon2id-hashed) in that instance's database. It is a
*local access-control* boundary for one deployment. ADR-024's ARK, device keys,
and recovery passphrase are the *end-to-end encryption identity* shared across a
user's devices through the sync server. They are independent: unlocking the web
UI does not reveal the ARK, and holding the ARK is not how one logs into the web
UI. A deployment may use both; neither substitutes for the other.

### Scope split with #125

This ADR fixes the **scheme and primitives**. Issue #125 (E2E encryption work)
implements: the pure key-schedule module in `pergamon-core`/the client crypto
crate (derivations, wrap/unwrap, AAD, convergent-blob encryption), OS-keychain
integration per platform (ADR-020), and the AGPL server's opaque relay endpoints
for device records, wrapped bundles, attestations, and the recovery blob. No
ADR-022 wire fields change.

## Consequences

### Positive

- **The server stays blind.** It stores and relays opaque handles, ciphertext
  bundles, signatures, and one recovery blob; it never holds the ARK, any epoch
  key, or any plaintext. "The server never sees plaintext" now holds for keys as
  well as content, by construction and by AAD binding.
- **A clean, standard key hierarchy.** One ARK, HKDF domain separation, per-epoch
  content keys, per-event and convergent-blob keys — every ADR-022 crypto field
  (`key_epoch`, `entity_ref`, `ct_hash`, AAD) is produced by an explicit,
  testable derivation with no I/O, satisfying ADR-001/007.
- **Blob dedup survives encryption.** Convergent keys keep ADR-022's
  content-addressed, hash-deduplicated blob store working under E2EE, which is
  what makes syncing a large library affordable.
- **Trust-on-first-use with a human check.** Device-to-device enrollment with an
  out-of-band SAS gives strong MITM resistance without a certificate authority
  or cloud identity, matching the local-first, no-accounts ethos.
- **Rotation is cheap and immediate for new content.** Revocation is an epoch
  bump plus a re-wrap to remaining devices — no library-wide re-encryption on the
  hot path — and downgrade-proof because `key_epoch` is authenticated in AAD.
- **Recovery is honest.** Off by default, opt-in, warned, with a stronger
  recovery-code alternative — the user consciously chooses the passphrase risk
  rather than having it imposed.
- **Modern, WASM-friendly primitives** (X25519/Ed25519/XChaCha20-Poly1305/
  HKDF/Argon2id/BLAKE3) are all audited Rust crates that build for CLI, iOS, and
  web (ADR-016), and BLAKE3/Argon2id reuse choices already made in ADR-017/022.

### Negative

- **No retroactive forward secrecy without a full re-key.** A revoked device
  keeps the epoch keys and plaintext it already had; protecting *old* content
  from it requires an expensive, out-of-scope library-wide re-encryption.
- **Convergent encryption leaks equality/existence.** Equal blobs are linkable
  and a plaintext guess can be confirmed by a party holding the account key;
  mitigated by scoping the key under `ACK_e` (the blind server cannot exploit it)
  and by an opt-out that sacrifices dedup.
- **Enrollment needs two live devices and a human.** Onboarding a device without
  an existing trusted device *and* without recovery enabled is impossible by
  design — the safety property that the server cannot mint access is exactly what
  makes lost-all-devices-without-recovery unrecoverable.
- **Recovery-passphrase risk.** Enabling recovery uploads a ciphertext whose only
  guard is the passphrase; a weak passphrase plus server compromise is the
  account's worst case. This is the deliberate cost of offering any recovery at
  all.
- **Client complexity.** Sealed-box enrollment, SAS verification, epoch re-wraps,
  and keychain integration land entirely client-side (#125), on top of the
  already-substantial ADR-022/023 sync engine.
- **Key storage depends on the platform.** Security reduces to the OS keychain /
  secure enclave; on a headless CLI host without a keychain the fallback is an
  Argon2id-encrypted key file, which is weaker than hardware-backed storage.

## Rejected Alternatives

- **Password-derived account key (no per-device keys).** Deriving the ARK
  directly from a single account password (à la some password managers) was
  rejected: it makes the password the permanent single point of failure, offers
  no per-device revocation (you can only rotate by changing the password
  everywhere), and provides no MITM-resistant device enrollment. Per-device
  keypairs give real revocation and trust attestation.
- **Server-assisted key escrow / recoverable-by-provider.** Rejected: any scheme
  where the server can recover the key breaks E2EE. Recovery here is a
  *client-encrypted* blob the server cannot open.
- **Random (non-convergent) blob keys.** Rejected as the default: random per-blob
  keys defeat ADR-022's ciphertext-hash dedup, inflating storage and upload cost
  for a library with many shared/immutable artifacts. Convergent keys scoped
  under the secret `ACK_e` keep dedup while limiting the confirmation oracle to
  key-holders; random keys remain a future opt-out for the privacy-maximalist.
- **AES-GCM instead of XChaCha20-Poly1305.** Rejected for the multi-writer log:
  GCM's 96-bit nonce demands careful non-reuse coordination across devices and
  epochs; XChaCha20-Poly1305's 192-bit random nonce removes that footgun with no
  practical downside on the target platforms.
- **A certificate authority / server-vouched device identity.** Rejected: it
  reintroduces a trusted third party and cloud identity, contradicting the
  no-accounts, local-first model. Ed25519 device attestations rooted at the first
  device give a self-contained web of trust.
- **QR/SAS-free "just click approve" enrollment.** Rejected: without an
  out-of-band comparison, a malicious or compromised relay can MITM the key
  exchange. The SAS check is the property that lets an untrusted server broker
  enrollment safely.
- **Full local at-rest field encryption of SQLite in v1.** Rejected per roadmap
  §2.5 / Decision #20: local search, indexing, and portability matter more than
  local-vault complexity on day one; OS disk encryption plus E2EE sync is the
  right first compromise. Local encryption can be layered later without changing
  this scheme.
- **Deciding the wire contract or conflict policy here.** Rejected as scope:
  those are ADR-022 and ADR-023. This ADR adds no wire fields and lets keys,
  envelope, and conflict resolution evolve independently.
