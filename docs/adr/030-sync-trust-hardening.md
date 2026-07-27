# ADR-030: Sync Trust Hardening — Event Authenticity, AAD Identity Binding, Rollback Detection, and Forward-Secret Revocation

**Status**: Accepted  
**Date**: 2026-07-27  
**Deciders**: kafkade

## Context

Phase 7 (epic #35) shipped **optional, end-to-end encrypted multi-device sync**:
ADR-022 (#121) fixed the wire contract (an append-only, server-sequenced log of
encrypted event envelopes plus content-addressed blobs), ADR-023 (#122) fixed
conflict resolution, and ADR-024 (#125) fixed the key hierarchy, device
onboarding, and revocation/rotation. ADR-026 deploys the server as a **blind
relay** that stores and echoes ciphertext and opaque onboarding artifacts only.

That stack gives content **confidentiality** — the server never sees plaintext.
But a review of the event path (epic #185) found it does *not* yet give event
**authenticity** or **authorship integrity**. Two concrete gaps exist in the
shipped code:

1. **No per-device authorship proof.** Event bodies are encrypted with an
   account-wide content key `ACK_e = HKDF(ARK, "pergamon/v1/account-content" ‖ e)`
   (ADR-024 hierarchy, `pergamon-crypto` `hierarchy::content_key`). *Any* holder
   of the Account Root Key (ARK) can therefore produce a ciphertext that every
   other device will accept and apply. A **revoked** device — which still knows
   every historical ARK and epoch it ever held — can forge events indefinitely,
   and one compromised device is indistinguishable from any other. There is no
   signature that ties an event to the specific device that authored it.

2. **Routing identity is unauthenticated.** The server-visible header carries
   `device_id` (origin device) and `entity_ref` (the blinded per-entity grouping
   token) as plaintext fields, but ADR-022's AEAD **AAD** binds only
   `protocol_version`, `account_id`, `change_id`, `key_epoch`, and `blob_refs`
   (`pergamon-crypto` `envelope::EventHeader::aad_bytes`). Because `device_id`
   and `entity_ref` are *outside* the AAD, a hostile or rolled-back relay can
   **re-attribute** an event to a different device or **re-route** it to a
   different entity without breaking decryption — the body still decrypts, only
   its provenance and grouping are silently rewritten.

This ADR decides the **full hardened trust model** for the sync event path for
the whole of epic #185, and marks which parts are implemented now versus
deferred to sibling children of the epic. It **amends ADR-022's envelope**: the
envelope now carries a per-device signature and an expanded AAD.

### Threat model

The adversary is the relay operator (or anyone who has taken over the relay, or
replays a stale relay snapshot) — i.e. a **hostile, equivocating, or
rolled-back server** — plus a **revoked or compromised device**. The relay is
untrusted for integrity: it may reorder within the bounds the protocol allows,
withhold, replay, duplicate, re-attribute, re-route, or roll back to an earlier
state. It cannot read plaintext (that is ADR-022/024 and out of scope to
re-litigate). What we add here is: **a pulling device must be able to detect any
event it did not — or a still-trusted peer did not — genuinely author.**

## Decision

Adopt a five-part hardened model. Parts **(a)** and **(b)** are implemented in
this issue (WP-A); parts **(c)**, **(d)**, and **(e)** are specified here as the
target design and deferred to sibling issues.

### (a) Per-device event signing + authorship authorization — IMPLEMENTED HERE

Every event is signed by the **Ed25519 device key** of the device that authored
it (the same keypair ADR-024 already provisions per device, whose `device_id` is
a hash of its Ed25519 public key). The signature is verified on pull against the
account's **device roster** (`SignedDeviceRecord` → `DeviceRecord`, mapping
`device_id → ed25519_pub`, published during ADR-024 onboarding).

**Canonical signature digest.** Signing is over a domain-tagged, unambiguous
serialization of the *entire* server-visible envelope — header and body:

```text
EVENT_SIG_TAG ‖ header.aad_bytes() ‖ u32_be(len(ciphertext)) ‖ ciphertext
```

where `EVENT_SIG_TAG = "pergamon/v1/event-sig"`. Signing over `aad_bytes()`
(see (b)) transitively covers every routing field — `protocol_version`,
`account_id`, `device_id`, `change_id`, `key_epoch`, `entity_ref`, `blob_refs`
— and the length prefix on the ciphertext makes the header/ciphertext boundary
unambiguous. The distinct tag domain-separates this signature from the AEAD's
own use of the same AAD, so a signature can never be confused with, or replayed
as, any other protocol artifact. The primitives are pure and live in
`pergamon-crypto` (`sign_event`, `verify_event`, `event_signing_bytes`).

**Wire.** The 64-byte signature travels as an opaque, standard-base64
`sig_b64` header field on the event envelope. The **blind relay stores and
echoes it verbatim and never inspects it** — identical in spirit to how it
treats `ciphertext_b64`. Authenticity is enforced **entirely client-side**.

**Pull-time policy (implemented).** For each pulled event, after the existing
echo-suppression (`device_id == self.device_id`) and applied-dedupe checks and
*before* decrypting or applying:

- **Valid signature from a known device →** apply as normal.
- **Known device, signature does not verify →** reject with a new, **fatal**
  (non-retryable) `SyncError::BadEventSignature { change_id, device_id }`; the
  event is not applied and the cursor is not advanced past it. Retrying cannot
  make a forged signature valid.
- **Unknown signer (device absent from the local roster) →** reject with a new,
  **retryable** `SyncError::UnknownSigner { device_id }`; the roster may simply
  be stale, so the caller refreshes it (re-runs ADR-024 `roster()`) and retries
  rather than applying an unverifiable event.

To keep the sync engine transport-generic and unit-testable, verification uses a
small in-memory `device_id → ed25519_pub` **directory** the caller builds from a
*verified* roster and passes into the engine, rather than giving the pure engine
network access.

**Scope boundary.** This part proves *who authored* an event and that a
still-enrolled device did so. It does **not** yet reject an event from a device
that is *known but has since been revoked* — that requires the anchored trust
chain in (c). Until (c) lands, a revoked-but-still-on-roster device's signature
still verifies; the forward-secrecy limit in (e) is the other half of that gap.

### (b) Bind `device_id` + `entity_ref` into the event AAD — IMPLEMENTED HERE

`EventHeader` is extended to carry `device_id: String` and
`entity_ref: Option<String>`, and `aad_bytes()` now binds **both**, using the
existing domain-tagged, length-prefixed framing so that no two distinct headers
can ever produce the same AAD. Concretely the AAD is the concatenation, in a
fixed order, of a domain tag and each field length-prefixed with a big-endian
`u32`:

```text
AAD = EVENT_AAD_TAG
    ‖ u32_be(protocol_version)
    ‖ lp(account_id)
    ‖ lp(device_id)
    ‖ lp(change_id)
    ‖ u32_be(key_epoch)
    ‖ entity_ref_frame
    ‖ u32_be(len(blob_refs)) ‖ lp(blob_ref[0]) ‖ lp(blob_ref[1]) ‖ …
```

where `EVENT_AAD_TAG = "pergamon/v1/event-aad"` and `lp(x) = u32_be(len(x)) ‖ x`.
The **`entity_ref` presence is encoded unambiguously** so that `None` can never
collide with `Some("")`:

```text
entity_ref_frame = 0x00                       when entity_ref is None
                 = 0x01 ‖ lp(entity_ref)      when entity_ref is Some(s)
```

Because `device_id` and `entity_ref` are now inside the AAD, a hostile server
that re-attributes an event to a different device, or re-routes it to a
different entity, makes AEAD decryption **fail** on every honest client — the
tamper is detected even independently of the signature in (a). The sync crypto
glue (`CryptoContext::encrypt_change` / `decrypt_change`) populates these header
fields on **both** encrypt and decrypt from the origin `device_id` and the
blinded `entity_ref` it already computes, so the body is cryptographically bound
to its routing in both directions.

### (c) Anchored trust-chain validation — DESIGN ONLY, deferred

**Target design.** Parts (a)/(b) verify that an event was signed by *a* device
whose key appears on the roster. They do not verify that the device is *still
trusted*. The full model validates each signer against an **anchored trust
chain** built from ADR-024's signed device records and **trust/revocation
attestations**: starting from the account's founding device (the trust anchor
established at bootstrap), a device is trusted iff there is an unbroken chain of
enrollment attestations to it and **no** revocation attestation for it at or
before the event's epoch. On pull, an event whose signer is *known but revoked*
(as of that event's `key_epoch`) is rejected as a forgery rather than applied.

This turns today's retryable `UnknownSigner` / fatal `BadEventSignature` split
into a three-way decision (trusted-and-valid → apply; untrusted/revoked signer →
fatal reject; genuinely unknown → refresh-and-retry). It is deferred because it
requires evaluating the attestation graph at a specific epoch and defining the
revocation-visibility rules against the log, which is a self-contained unit of
work. **Cross-references ADR-024** (device records, trust and revocation
attestations, the founding-device anchor).

### (d) Signed monotonic checkpoints → rollback & equivocation detection — DESIGN ONLY, deferred

**Target design.** A hostile relay can still **roll back** (serve a truncated
prefix of the log) or **equivocate** (serve different devices divergent logs)
without breaking any per-event signature — every event it serves is individually
authentic; the *set* is dishonest. The full model has each device periodically
publish a **signed checkpoint** committing to `(max server_seq observed, a
rolling hash/Merkle root over the applied prefix, epoch)`. Peers cross-check
checkpoints: a server that later serves a prefix inconsistent with a
previously-signed checkpoint (rollback), or serves two peers
mutually-inconsistent checkpoints (equivocation/fork), is **detected**. This
gives the append-log a tamper-evident, monotonic spine on top of the
server-assigned `server_seq` (which ADR-022 explicitly does not trust for
integrity). Deferred as its own issue: it adds a new signed artifact type, a
publish/verify cadence, and a fork-response policy. **Cross-references ADR-022**
(`server_seq` is assigned by the untrusted server and is not an integrity
anchor).

### (e) True forward secrecy on revocation — DESIGN ONLY, deferred

**Problem.** ADR-024 derives every epoch's content key deterministically from
the long-lived ARK:
`ACK_e = HKDF(ARK, "pergamon/v1/account-content" ‖ e)`. Revocation advances
`key_epoch` and re-wraps the *new* epoch key to the remaining devices — but any
party that ever held the ARK (including the revoked device) can **re-derive
every epoch key, past and future**, simply by evaluating the HKDF. So revocation
today provides no real forward secrecy against a departed device: it can still
decrypt anything encrypted under any epoch it can derive.

**Target design.** Replace the ARK-derived epoch chain with a **random
per-epoch content root** that is *not* a deterministic function of the ARK. On
rotation, a fresh epoch root is generated and distributed **only to the
remaining devices** (wrapped to each remaining device's key), while historical
epoch keys are retained by continuing devices and wrapped forward **separately**
(so existing content stays readable) but are **never re-derivable from the
ARK**. A revoked device, holding only the roots it was given while enrolled,
cannot compute any post-revocation epoch key. This is the only part that
**rewrites the ADR-024 key hierarchy** and its wrapped-key distribution, so it is
the largest and most carefully-sequenced deferral. **Cross-references ADR-024**
(the key hierarchy and revocation/rotation this replaces).

## Consequences

### Positive

- **Authorship is provable and tamper-evident (now).** Every applied event is
  signed by a device on the roster; a hostile server cannot forge an event,
  re-attribute it to another device, or re-route it to another entity without an
  honest client rejecting it — via signature failure (a/b) and, redundantly for
  routing, via AEAD decryption failure (b).
- **Defense in depth.** Routing identity is protected by *both* the signature
  and the AEAD AAD, so neither a signature-forgery nor an AAD-only attack
  succeeds in isolation.
- **The blind relay stays blind.** The signature is opaque bytes the server
  stores and echoes verbatim; no server-side verification, no new plaintext
  exposure, and ADR-026's blind-relay property is preserved.
- **Clean, staged path to the full model.** (c)/(d)/(e) are specified against
  the same envelope and roster, so the remaining hardening lands incrementally
  without re-opening the wire contract again.

### Negative / trade-offs

- **Residual gaps until (c)–(e) land.** A *revoked-but-still-on-roster* device
  can still author accepted events (closed by (c)), a hostile relay can still
  roll back or equivocate (closed by (d)), and revocation is not yet
  forward-secret against a departed device (closed by (e)). This ADR makes those
  boundaries explicit rather than implying the event path is fully hardened.
- **Roster dependency on pull.** Verifying signatures requires an up-to-date
  `device_id → ed25519_pub` directory. A stale roster surfaces as a retryable
  `UnknownSigner`, adding a refresh round-trip the first time a new device's
  events are seen. Push is unaffected (it never consults the directory), and a
  single-device account never populates it (its own echoes are suppressed before
  verification).
- **Slightly larger events and a signing/verifying cost.** Each event grows by a
  base64-encoded 64-byte signature and costs one Ed25519 sign on encrypt and one
  verify on apply. Negligible relative to the AEAD and network costs.

### Neutral

- **Envelope amendment, not a new protocol version.** Remote sync is not yet
  released, so `sig_b64` is added as a `#[serde(default)]` field (empty ⇒ fails
  verification) and no data migration is required for existing rows; the
  `protocol_version` is unchanged. The server's `events` table gains an opaque
  `signature` column.
- **License boundaries preserved.** All new cryptographic logic lives in the
  Apache-2.0 crates (`pergamon-crypto`, `pergamon-sync`); the AGPL relay
  (`pergamon-sync-server`) only gains opaque passthrough of the signature bytes.
  `pergamon-crypto` remains zero-I/O — timestamps and keys are passed in by
  callers.

## Implementation notes (WP-A, this issue)

- `pergamon-crypto` (`envelope.rs`): `EventHeader` gains `device_id` and
  `entity_ref`; `aad_bytes()` binds both with the framing above;
  `EVENT_SIG_TAG`, `event_signing_bytes`, `sign_event`, and `verify_event` are
  added as pure helpers with unit tests (valid verifies; tampered
  header/ciphertext/wrong-key fail; `None` vs `Some("")` entity_ref differ).
- `pergamon-sync`: `EventInput`/`StoredEvent` gain an opaque `sig_b64`
  (`#[serde(default)]`); `CryptoContext` gains the device Ed25519 signing key and
  signs in `encrypt_change`; a `DeviceKeyDirectory` (built from a verified
  `roster()`) is passed into `SyncEngine`, which verifies each pulled event and
  applies the pull-time policy above; new `SyncError::BadEventSignature`
  (fatal) and `SyncError::UnknownSigner` (retryable) variants.
- `pergamon-sync-server` (AGPL blind relay): opaque `sig_b64` passthrough on the
  envelope and an opaque `signature` column on the `events` table — stored and
  echoed verbatim, never inspected.
- `pergamon-cli` / `pergamon-server` / `pergamon-uniffi`: supply the device
  signing key into the crypto context and build the verified device-key
  directory from `roster()` where a sync session is constructed.

## References

- [ADR-022: Sync Protocol and Envelope Model](022-sync-protocol-and-envelope-model.md)
  — amended here: the envelope now carries a signature and an expanded AAD.
- [ADR-024: Device Onboarding and Key Lifecycle](024-device-onboarding-and-key-lifecycle.md)
  — device keys, roster, trust/revocation attestations, and the key hierarchy
  that (c) and (e) build on / replace.
- [ADR-023: Conflict Policy by Entity Type](023-conflict-policy-by-entity-type.md)
  — unchanged; signed events are reconciled by the same policy.
- [ADR-026: Sync Server Deployment](026-sync-server-deployment.md) — the blind
  relay whose "store and echo opaque bytes" property this preserves.
- Epic #185 — hostile-server & device-loss hardening (this ADR governs the epic;
  (c), (d), (e) are its sibling children).
