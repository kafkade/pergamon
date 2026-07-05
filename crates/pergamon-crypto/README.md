# pergamon-crypto

Client-side **end-to-end encryption** for pergamon's optional multi-device sync.
It implements the key scheme fixed by [ADR-024] on top of the [ADR-022] wire
envelope, so the sync server only ever stores and relays opaque ciphertext.

This crate is **Apache-2.0**, like every other client/core crate. The AGPL-3.0
`pergamon-sync-server` never links it — the server is blind and interprets none
of the bytes this crate produces. Keeping all cryptography here (and out of
`pergamon-core`) preserves [ADR-001]'s zero-I/O core: derivations are pure, and
only key/nonce generation touches the OS CSPRNG.

## Primitives (ADR-024)

| Purpose | Primitive | Crate |
| --- | --- | --- |
| Key agreement / sealed box | X25519 ECDH | `x25519-dalek` |
| Signatures (attestation, trust, revocation) | Ed25519 | `ed25519-dalek` |
| AEAD (events, blobs, key-wrap) | XChaCha20-Poly1305 | `chacha20poly1305` |
| KDF (hierarchy, per-message keys) | HKDF-SHA-256 | `hkdf` + `sha2` |
| `entity_ref` blinding | HMAC-SHA-256 | `hmac` + `sha2` |
| Recovery stretching | Argon2id | `argon2` |
| Content hashing / SAS / convergent input | BLAKE3 | `blake3` |
| CSPRNG | OS RNG | `rand_core` (getrandom) |
| Secret hygiene | zeroize | `zeroize` |

The sealed box is built in-house from X25519 (ephemeral key) + HKDF +
XChaCha20-Poly1305 so it uses exactly these primitives and is fully
deterministic and testable.

## Key hierarchy

All levels are domain-separated HKDF derivations from a single 256-bit Account
Root Key (ARK):

```text
ARK (256-bit random)
├── account_stream_key = HKDF(ARK, "pergamon/v1/entity-ref")        -> entity_ref HMAC key
├── ACK_e             = HKDF(ARK, "pergamon/v1/account-content"||e)  -> per-epoch content key
│   ├── event key     = HKDF(ACK_e, "pergamon/v1/event"||change_id)
│   └── blob key      = HKDF(ACK_e, "pergamon/v1/blob"||BLAKE3(plaintext))  (convergent)
└── recovery = XChaCha20-Poly1305 wrap of ARK under Argon2id(passphrase)
```

`account_id` is an independent 128-bit random handle, **not** derived from the
ARK.

## Modules

- `primitives` — typed, zeroizing wrappers over the primitives above; AEAD
  seal/open with AAD, HKDF-expand, HMAC, BLAKE3, Argon2id KEK, sealed box,
  Ed25519 sign/verify, CSPRNG helpers.
- `hierarchy` — ARK / `account_id` generation and the full key schedule.
- `envelope` — authenticated encryption of ADR-022 event bodies (the
  server-visible header is bound as associated data) plus `entity_ref` blinding.
- `blob` — convergent blob encryption; identical plaintext yields identical
  ciphertext under a content-derived key, so ADR-022's ciphertext-hash dedup
  keeps working under E2EE.
- `device` — per-device X25519 + Ed25519 keypairs and signed device records.
- `enrollment` — device-to-device onboarding: Short Authentication String (SAS)
  verification and sealed enrollment bundles.
- `attestation` — Ed25519 trust and revocation attestations.
- `recovery` — the optional, opt-in Argon2id-wrapped recovery blob.
- `rotation` — key-epoch rotation on device revocation (re-wraps the new content
  key to the remaining devices).

## Security notes

- **Convergent encryption is a deliberate trade-off** (ADR-024): the convergent
  key is scoped under the secret `ACK_e`, so only account key-holders can run a
  confirmation oracle. This is documented and intentional.
- **Revocation is honest, not retroactive**: rotation protects *new* content
  only; a revoked device keeps the epoch keys it already held. There is no
  library-wide re-key.
- Every derivation is covered by known-answer test vectors
  (`tests/vectors.rs`); everything that needs randomness takes it from the OS
  CSPRNG.

[ADR-001]: ../../docs/adr/001-zero-io-core-library.md
[ADR-022]: ../../docs/adr/022-sync-protocol-and-envelope-model.md
[ADR-024]: ../../docs/adr/024-device-onboarding-and-key-lifecycle.md
