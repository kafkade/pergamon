// SPDX-License-Identifier: Apache-2.0

//! Thin, typed wrappers over the audited cryptographic primitives ADR-024
//! selected.
//!
//! Nothing here knows about pergamon's key hierarchy or wire format;
//! these are the building blocks the rest of the crate composes.
//!
//! | Job | Primitive |
//! |-----|-----------|
//! | AEAD (events, blobs, key-wrap) | XChaCha20-Poly1305 |
//! | Key derivation | HKDF-SHA-256 |
//! | Keyed hashing (`entity_ref`) | HMAC-SHA-256 |
//! | Content hashing / commitments | BLAKE3 |
//! | Passphrase stretching (recovery) | Argon2id |
//! | Key agreement / sealed box | X25519 |
//! | Signatures | Ed25519 |
//!
//! All secret key material is held in [`Zeroizing`] buffers so it is wiped from
//! memory on drop.

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::error::{CryptoError, Result};

/// Length in bytes of every symmetric key, X25519 scalar/point, and 256-bit
/// digest used in the system.
pub const KEY_LEN: usize = 32;
/// Length in bytes of the XChaCha20-Poly1305 nonce (extended, 192-bit).
pub const NONCE_LEN: usize = 24;
/// Length in bytes of the Poly1305 authentication tag.
pub const TAG_LEN: usize = 16;
/// Length in bytes of an Ed25519 signature.
pub const SIG_LEN: usize = 64;

/// A 256-bit symmetric key, zeroized on drop.
pub type SymmetricKey = Zeroizing<[u8; KEY_LEN]>;

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

/// Fill `buf` with cryptographically secure random bytes from the OS CSPRNG.
///
/// # Errors
/// Returns [`CryptoError::Random`] if the OS CSPRNG fails.
pub fn fill_random(buf: &mut [u8]) -> Result<()> {
    OsRng.try_fill_bytes(buf).map_err(|_| CryptoError::Random)
}

/// Return `N` cryptographically secure random bytes.
///
/// # Errors
/// Returns [`CryptoError::Random`] if the OS CSPRNG fails.
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    fill_random(&mut out)?;
    Ok(out)
}

/// Return a fresh random 256-bit symmetric key (zeroized on drop).
///
/// # Errors
/// Returns [`CryptoError::Random`] if the OS CSPRNG fails.
pub fn random_key() -> Result<SymmetricKey> {
    Ok(Zeroizing::new(random_array::<KEY_LEN>()?))
}

// ---------------------------------------------------------------------------
// HKDF-SHA-256
// ---------------------------------------------------------------------------

/// HKDF-SHA-256 expand into an `N`-byte key.
///
/// Uses `ikm` as input key material and `info` as the domain-separation label.
/// No salt is used: every caller supplies a distinct, hard-coded `info` label,
/// which is what separates the derived keys (ADR-024 "distinct, hard-coded
/// `info` domain-separation
/// labels").
///
/// # Errors
/// Returns [`CryptoError::KeyDerivation`] only if `N` exceeds HKDF's output
/// limit (255·32 bytes), which no caller in this crate approaches.
pub fn hkdf_sha256<const N: usize>(ikm: &[u8], info: &[u8]) -> Result<[u8; N]> {
    let hk = Hkdf::<Sha256>::new(None, ikm);
    let mut okm = [0u8; N];
    hk.expand(info, &mut okm)
        .map_err(|_| CryptoError::KeyDerivation)?;
    Ok(okm)
}

/// Derive a 256-bit key with HKDF-SHA-256 (zeroized on drop).
///
/// # Errors
/// Propagates [`hkdf_sha256`] errors.
pub fn hkdf_key(ikm: &[u8], info: &[u8]) -> Result<SymmetricKey> {
    Ok(Zeroizing::new(hkdf_sha256::<KEY_LEN>(ikm, info)?))
}

// ---------------------------------------------------------------------------
// HMAC-SHA-256
// ---------------------------------------------------------------------------

/// Compute HMAC-SHA-256 over `msg` under `key`, returning the 32-byte tag.
///
/// Used for ADR-022 `entity_ref` blinding. HMAC-SHA-256 accepts a key of any
/// length, so this never fails for our fixed 32-byte keys.
///
/// # Errors
/// Returns [`CryptoError::KeyDerivation`] only in the impossible case that the
/// HMAC backend rejects the key length.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Result<[u8; 32]> {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).map_err(|_| CryptoError::KeyDerivation)?;
    mac.update(msg);
    Ok(mac.finalize().into_bytes().into())
}

// ---------------------------------------------------------------------------
// BLAKE3
// ---------------------------------------------------------------------------

/// BLAKE3 hash of `data` (32-byte output). Used for content addressing inputs,
/// convergent-key material, and enrollment commitments.
#[must_use]
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

// ---------------------------------------------------------------------------
// Argon2id
// ---------------------------------------------------------------------------

/// Stretch a recovery passphrase (or recovery code) into a 256-bit
/// key-wrapping key.
///
/// Uses Argon2id with the library's OWASP-tier default parameters and the
/// per-account `salt` (which must be at least 8 bytes).
///
/// # Errors
/// Returns [`CryptoError::KeyDerivation`] if Argon2id rejects the parameters
/// (e.g. a salt shorter than 8 bytes).
pub fn argon2id_kek(passphrase: &[u8], salt: &[u8]) -> Result<SymmetricKey> {
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::default()
        .hash_password_into(passphrase, salt, out.as_mut_slice())
        .map_err(|_| CryptoError::KeyDerivation)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// XChaCha20-Poly1305 AEAD
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` under `key` and 24-byte `nonce`, binding `aad`.
///
/// The returned buffer is `ciphertext ‖ tag`
/// (the nonce is **not** prepended — the caller decides how to transport it).
///
/// Prefer [`aead_seal`] for random-nonce encryption; this deterministic form
/// exists for convergent blob encryption (ADR-024), where the nonce is derived
/// from the content so identical plaintext yields identical ciphertext.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the AEAD backend rejects the inputs.
pub fn aead_encrypt_with_nonce(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let xnonce = XNonce::try_from(&nonce[..]).map_err(|_| CryptoError::Malformed("bad nonce"))?;
    cipher
        .encrypt(
            &xnonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Malformed("aead encryption failed"))
}

/// Decrypt a `ciphertext ‖ tag` buffer produced by [`aead_encrypt_with_nonce`]
/// under `key`, `nonce`, and `aad`.
///
/// # Errors
/// Returns [`CryptoError::Decryption`] if authentication fails (wrong key,
/// nonce, AAD, or a tampered ciphertext).
pub fn aead_decrypt_with_nonce(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let xnonce = XNonce::try_from(&nonce[..]).map_err(|_| CryptoError::Malformed("bad nonce"))?;
    cipher
        .decrypt(
            &xnonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

/// Encrypt `plaintext` under `key` with a fresh random 24-byte nonce, binding
/// `aad`.
///
/// The returned buffer is `nonce ‖ ciphertext ‖ tag`, self-describing so
/// [`aead_open`] can decrypt it with only the key and AAD.
///
/// This is the workhorse for event bodies, sealed boxes, key-wraps, and the
/// recovery blob — everything whose ciphertext need not be deterministic.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if encryption fails.
pub fn aead_seal(key: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce_bytes = random_array::<NONCE_LEN>()?;
    let xnonce =
        XNonce::try_from(&nonce_bytes[..]).map_err(|_| CryptoError::Malformed("bad nonce"))?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let ct = cipher
        .encrypt(
            &xnonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Malformed("aead encryption failed"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a `nonce ‖ ciphertext ‖ tag` buffer produced by [`aead_seal`].
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the buffer is too short to contain a
/// nonce and tag, or [`CryptoError::Decryption`] if authentication fails.
pub fn aead_open(key: &[u8; KEY_LEN], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Malformed("aead payload too short"));
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    let xnonce = XNonce::try_from(nonce).map_err(|_| CryptoError::Malformed("bad nonce"))?;
    cipher
        .decrypt(&xnonce, Payload { msg: ct, aad })
        .map_err(|_| CryptoError::Decryption)
}

// ---------------------------------------------------------------------------
// X25519 sealed box (anonymous, authenticated-to-recipient encryption)
// ---------------------------------------------------------------------------

/// Domain-separation label for the sealed-box KDF.
const SEALED_BOX_INFO: &[u8] = b"pergamon/v1/sealed-box";

/// Derive the sealed-box symmetric key from an X25519 shared secret, binding
/// both public keys so a swapped key produces a different key.
fn sealed_box_key(
    shared: &[u8; KEY_LEN],
    eph_pub: &[u8; KEY_LEN],
    recip_pub: &[u8; KEY_LEN],
) -> Result<SymmetricKey> {
    let mut info = Vec::with_capacity(SEALED_BOX_INFO.len() + 2 * KEY_LEN);
    info.extend_from_slice(SEALED_BOX_INFO);
    info.extend_from_slice(eph_pub);
    info.extend_from_slice(recip_pub);
    hkdf_key(shared, &info)
}

/// Seal `plaintext` to an X25519 public key so only the holder of the matching
/// private key can open it, binding `aad` into the AEAD.
///
/// Implements a libsodium-style sealed box from our own primitives: an
/// ephemeral X25519 keypair, an HKDF-SHA-256 key over the ECDH shared secret
/// (bound to both
/// public keys), and XChaCha20-Poly1305.
///
/// Output layout: `eph_pub(32) ‖ nonce(24) ‖ ciphertext ‖ tag(16)`.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] or [`CryptoError::KeyDerivation`] on
/// failure.
pub fn seal_to(recipient_pub: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let eph_scalar = Zeroizing::new(random_array::<KEY_LEN>()?);
    let eph_secret = StaticSecret::from(*eph_scalar);
    let eph_pub = XPublicKey::from(&eph_secret);
    let recip = XPublicKey::from(*recipient_pub);
    let shared = eph_secret.diffie_hellman(&recip);
    let key = sealed_box_key(shared.as_bytes(), eph_pub.as_bytes(), recipient_pub)?;

    let mut sealed = aead_seal(&key, aad, plaintext)?;
    let mut out = Vec::with_capacity(KEY_LEN + sealed.len());
    out.extend_from_slice(eph_pub.as_bytes());
    out.append(&mut sealed);
    Ok(out)
}

/// Open a sealed box produced by [`seal_to`] using the recipient's 32-byte
/// X25519 secret scalar and the same `aad`.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the buffer is too short, or
/// [`CryptoError::Decryption`] if the box was not sealed to this key or was
/// tampered with (including a mismatched `aad`).
pub fn open_sealed(recipient_secret: &[u8; KEY_LEN], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < KEY_LEN + NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Malformed("sealed box too short"));
    }
    let (eph_pub_bytes, sealed) = data.split_at(KEY_LEN);
    let eph_pub: [u8; KEY_LEN] = eph_pub_bytes
        .try_into()
        .map_err(|_| CryptoError::Malformed("bad ephemeral public key"))?;

    let secret = StaticSecret::from(*recipient_secret);
    let recip_pub = XPublicKey::from(&secret);
    let shared = secret.diffie_hellman(&XPublicKey::from(eph_pub));
    let key = sealed_box_key(shared.as_bytes(), &eph_pub, recip_pub.as_bytes())?;
    aead_open(&key, aad, sealed)
}

// ---------------------------------------------------------------------------
// X25519 keypairs
// ---------------------------------------------------------------------------

/// Generate an X25519 keypair, returning `(secret_scalar, public_key)` bytes.
/// The secret is zeroized on drop.
///
/// # Errors
/// Returns [`CryptoError::Random`] if key generation fails.
pub fn x25519_generate() -> Result<(SymmetricKey, [u8; KEY_LEN])> {
    let secret = StaticSecret::from(random_array::<KEY_LEN>()?);
    let public = XPublicKey::from(&secret);
    let secret_bytes = Zeroizing::new(secret.to_bytes());
    Ok((secret_bytes, public.to_bytes()))
}

/// Derive the X25519 public key for a given secret scalar.
#[must_use]
pub fn x25519_public(secret: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    XPublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

// ---------------------------------------------------------------------------
// Ed25519 signatures
// ---------------------------------------------------------------------------

/// Generate an Ed25519 keypair, returning `(signing_key, verifying_key)` bytes.
/// The signing key is zeroized on drop.
///
/// # Errors
/// Returns [`CryptoError::Random`] if key generation fails.
pub fn ed25519_generate() -> Result<(SymmetricKey, [u8; KEY_LEN])> {
    let seed = Zeroizing::new(random_array::<KEY_LEN>()?);
    let signing = SigningKey::from_bytes(&seed);
    let verifying = signing.verifying_key();
    let signing_bytes = Zeroizing::new(signing.to_bytes());
    Ok((signing_bytes, verifying.to_bytes()))
}

/// Derive the Ed25519 verifying (public) key for a signing-key seed.
#[must_use]
pub fn ed25519_public(signing_key: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    SigningKey::from_bytes(signing_key)
        .verifying_key()
        .to_bytes()
}

/// Sign `msg` with an Ed25519 signing-key seed, returning a 64-byte signature.
#[must_use]
pub fn ed25519_sign(signing_key: &[u8; KEY_LEN], msg: &[u8]) -> [u8; SIG_LEN] {
    SigningKey::from_bytes(signing_key).sign(msg).to_bytes()
}

/// Verify a 64-byte Ed25519 signature over `msg` against a verifying key.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the verifying key is invalid, or
/// [`CryptoError::BadSignature`] if the signature does not verify.
pub fn ed25519_verify(
    verifying_key: &[u8; KEY_LEN],
    msg: &[u8],
    sig: &[u8; SIG_LEN],
) -> Result<()> {
    let vk = VerifyingKey::from_bytes(verifying_key)
        .map_err(|_| CryptoError::Malformed("invalid ed25519 verifying key"))?;
    let signature = ed25519_dalek::Signature::from_bytes(sig);
    vk.verify(msg, &signature)
        .map_err(|_| CryptoError::BadSignature)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn aead_seal_open_roundtrip() {
        let key = [7u8; KEY_LEN];
        let sealed = aead_seal(&key, b"aad", b"hello world").unwrap();
        let opened = aead_open(&key, b"aad", &sealed).unwrap();
        assert_eq!(opened, b"hello world");
    }

    #[test]
    fn aead_open_rejects_wrong_aad() {
        let key = [7u8; KEY_LEN];
        let sealed = aead_seal(&key, b"aad", b"data").unwrap();
        assert!(matches!(
            aead_open(&key, b"other", &sealed),
            Err(CryptoError::Decryption)
        ));
    }

    #[test]
    fn aead_deterministic_nonce_is_reproducible() {
        let key = [3u8; KEY_LEN];
        let nonce = [9u8; NONCE_LEN];
        let a = aead_encrypt_with_nonce(&key, &nonce, b"", b"same").unwrap();
        let b = aead_encrypt_with_nonce(&key, &nonce, b"", b"same").unwrap();
        assert_eq!(a, b, "same key+nonce+plaintext must be deterministic");
        let opened = aead_decrypt_with_nonce(&key, &nonce, b"", &a).unwrap();
        assert_eq!(opened, b"same");
    }

    #[test]
    fn hkdf_is_deterministic_and_label_separated() {
        let ikm = [1u8; 32];
        let a: [u8; 32] = hkdf_sha256(&ikm, b"label-a").unwrap();
        let a2: [u8; 32] = hkdf_sha256(&ikm, b"label-a").unwrap();
        let b: [u8; 32] = hkdf_sha256(&ikm, b"label-b").unwrap();
        assert_eq!(a, a2);
        assert_ne!(a, b, "different labels must derive different keys");
    }

    #[test]
    fn sealed_box_roundtrip_and_wrong_recipient() {
        let (secret, public) = x25519_generate().unwrap();
        let sealed = seal_to(&public, b"aad", b"secret bundle").unwrap();
        let opened = open_sealed(&secret, b"aad", &sealed).unwrap();
        assert_eq!(opened, b"secret bundle");

        let (other_secret, _) = x25519_generate().unwrap();
        assert!(open_sealed(&other_secret, b"aad", &sealed).is_err());
    }

    #[test]
    fn ed25519_sign_verify() {
        let (signing, verifying) = ed25519_generate().unwrap();
        let sig = ed25519_sign(&signing, b"attestation");
        assert!(ed25519_verify(&verifying, b"attestation", &sig).is_ok());
        assert!(matches!(
            ed25519_verify(&verifying, b"tampered", &sig),
            Err(CryptoError::BadSignature)
        ));
    }

    #[test]
    fn argon2id_is_deterministic() {
        let salt = [42u8; 16];
        let a = argon2id_kek(b"passphrase", &salt).unwrap();
        let b = argon2id_kek(b"passphrase", &salt).unwrap();
        assert_eq!(*a, *b);
        let c = argon2id_kek(b"different", &salt).unwrap();
        assert_ne!(*a, *c);
    }
}
