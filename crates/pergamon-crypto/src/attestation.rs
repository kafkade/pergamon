// SPDX-License-Identifier: Apache-2.0

//! Ed25519 trust and revocation attestations (ADR-024).
//!
//! Device trust is established by **attestation**: an already-trusted device
//! signs a statement about another device. A **trust** attestation vouches for a
//! newly enrolled device; a **revocation** attestation withdraws trust and
//! records the key epoch from which the revoked device is excluded.
//!
//! Attestations are self-authenticating public statements — the server relays
//! them opaquely. Verifying one proves *who signed it*; deciding whether that
//! signer is authorized (i.e. itself trusted) is the caller's policy.

use crate::device::{DeviceKeypairs, DeviceRecord, device_id_from_ed25519};
use crate::error::{CryptoError, Result};
use crate::primitives::{self, KEY_LEN, SIG_LEN};
use crate::wire::Cursor;

/// Domain tag prefixed to the bytes an attestation signs over.
const ATTESTATION_TAG: &[u8] = b"pergamon/v1/attestation";

/// Magic prefix identifying a serialized [`SignedAttestation`] on the wire.
const ATTESTATION_WIRE_TAG: &[u8] = b"pergamon/v1/attestation-wire";

/// What an attestation asserts about its subject device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationKind {
    /// The signer vouches for the subject device (grant trust).
    Trust,
    /// The signer revokes the subject device from `key_epoch` onward.
    Revoke,
}

impl AttestationKind {
    /// One-byte wire tag for canonical signing.
    const fn tag(self) -> u8 {
        match self {
            Self::Trust => 1,
            Self::Revoke => 2,
        }
    }

    /// Reconstruct a kind from its one-byte wire tag.
    const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Trust),
            2 => Some(Self::Revoke),
            _ => None,
        }
    }
}

/// A signed statement by one device about another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// Whether trust is granted or revoked.
    pub kind: AttestationKind,
    /// The signing (attesting) device's opaque handle.
    pub signer_device_id: String,
    /// The signing device's Ed25519 verifying key.
    pub signer_ed25519_pub: [u8; KEY_LEN],
    /// The subject device's opaque handle.
    pub subject_device_id: String,
    /// The subject device's Ed25519 verifying key.
    pub subject_ed25519_pub: [u8; KEY_LEN],
    /// For [`AttestationKind::Trust`], the current epoch; for
    /// [`AttestationKind::Revoke`], the new epoch the subject is excluded from.
    pub key_epoch: u32,
    /// Issue time in epoch milliseconds (supplied by the platform; this crate is
    /// clock-free per ADR-001).
    pub issued_at: i64,
}

impl Attestation {
    /// Canonical, unambiguous bytes this attestation is signed over (all
    /// variable-length fields length-prefixed).
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(ATTESTATION_TAG);
        b.push(self.kind.tag());
        push_lp(&mut b, self.signer_device_id.as_bytes());
        b.extend_from_slice(&self.signer_ed25519_pub);
        push_lp(&mut b, self.subject_device_id.as_bytes());
        b.extend_from_slice(&self.subject_ed25519_pub);
        b.extend_from_slice(&self.key_epoch.to_be_bytes());
        b.extend_from_slice(&self.issued_at.to_be_bytes());
        b
    }
}

/// An [`Attestation`] together with the signer's Ed25519 signature over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedAttestation {
    /// The attestation statement.
    pub attestation: Attestation,
    /// The signer's Ed25519 signature over [`Attestation::signing_bytes`].
    pub signature: [u8; SIG_LEN],
}

impl SignedAttestation {
    /// Verify internal consistency and the signature.
    ///
    /// Checks that both device handles match their embedded Ed25519 keys and
    /// that the signature verifies under the signer's key. Does **not** decide
    /// whether the signer is authorized — that is the caller's trust policy.
    ///
    /// # Errors
    /// [`CryptoError::Malformed`] if a `device_id` does not match its key;
    /// [`CryptoError::BadSignature`] if the signature does not verify.
    pub fn verify(&self) -> Result<()> {
        let a = &self.attestation;
        if device_id_from_ed25519(&a.signer_ed25519_pub) != a.signer_device_id {
            return Err(CryptoError::Malformed(
                "signer device_id does not match its ed25519 key",
            ));
        }
        if device_id_from_ed25519(&a.subject_ed25519_pub) != a.subject_device_id {
            return Err(CryptoError::Malformed(
                "subject device_id does not match its ed25519 key",
            ));
        }
        primitives::ed25519_verify(&a.signer_ed25519_pub, &a.signing_bytes(), &self.signature)
    }

    /// Serialize to opaque, self-describing wire bytes for relaying: a magic
    /// prefix, the kind tag, both length-prefixed device handles, both public
    /// keys, the epoch, `issued_at`, and the signature. Deterministic.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let a = &self.attestation;
        let signer = a.signer_device_id.as_bytes();
        let subject = a.subject_device_id.as_bytes();
        let mut b = Vec::with_capacity(
            ATTESTATION_WIRE_TAG.len()
                + 1
                + 4
                + signer.len()
                + KEY_LEN
                + 4
                + subject.len()
                + KEY_LEN
                + 4
                + 8
                + SIG_LEN,
        );
        b.extend_from_slice(ATTESTATION_WIRE_TAG);
        b.push(a.kind.tag());
        push_lp(&mut b, signer);
        b.extend_from_slice(&a.signer_ed25519_pub);
        push_lp(&mut b, subject);
        b.extend_from_slice(&a.subject_ed25519_pub);
        b.extend_from_slice(&a.key_epoch.to_be_bytes());
        b.extend_from_slice(&a.issued_at.to_be_bytes());
        b.extend_from_slice(&self.signature);
        b
    }

    /// Parse a [`SignedAttestation`] from its opaque wire encoding.
    ///
    /// Reconstructs the structure only; call [`Self::verify`] afterwards to
    /// authenticate it.
    ///
    /// # Errors
    /// [`CryptoError::Malformed`] if the input is truncated, has the wrong magic
    /// prefix, or an unknown kind tag.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut cur = Cursor::new(bytes);
        cur.expect_tag(ATTESTATION_WIRE_TAG)?;
        let kind = AttestationKind::from_tag(cur.read_u8()?)
            .ok_or(CryptoError::Malformed("unknown attestation kind"))?;
        let signer_len = cur.read_u32()? as usize;
        let signer_device_id = cur.read_string(signer_len)?;
        let signer_ed25519_pub = cur.read_array::<KEY_LEN>()?;
        let subject_len = cur.read_u32()? as usize;
        let subject_device_id = cur.read_string(subject_len)?;
        let subject_ed25519_pub = cur.read_array::<KEY_LEN>()?;
        let key_epoch = cur.read_u32()?;
        let issued_at = cur.read_i64()?;
        let signature = cur.read_array::<SIG_LEN>()?;
        cur.expect_end()?;
        Ok(Self {
            attestation: Attestation {
                kind,
                signer_device_id,
                signer_ed25519_pub,
                subject_device_id,
                subject_ed25519_pub,
                key_epoch,
                issued_at,
            },
            signature,
        })
    }
}

/// Sign a **trust** attestation vouching for `subject` at the current
/// `key_epoch`.
#[must_use]
pub fn attest_trust(
    signer: &DeviceKeypairs,
    subject: &DeviceRecord,
    key_epoch: u32,
    issued_at: i64,
) -> SignedAttestation {
    sign(
        signer,
        AttestationKind::Trust,
        subject,
        key_epoch,
        issued_at,
    )
}

/// Sign a **revocation** attestation excluding `subject` from `new_key_epoch`
/// onward.
#[must_use]
pub fn attest_revoke(
    signer: &DeviceKeypairs,
    subject: &DeviceRecord,
    new_key_epoch: u32,
    issued_at: i64,
) -> SignedAttestation {
    sign(
        signer,
        AttestationKind::Revoke,
        subject,
        new_key_epoch,
        issued_at,
    )
}

fn sign(
    signer: &DeviceKeypairs,
    kind: AttestationKind,
    subject: &DeviceRecord,
    key_epoch: u32,
    issued_at: i64,
) -> SignedAttestation {
    let attestation = Attestation {
        kind,
        signer_device_id: signer.device_id().to_owned(),
        signer_ed25519_pub: *signer.ed25519_verifying(),
        subject_device_id: subject.device_id.clone(),
        subject_ed25519_pub: subject.ed25519_pub,
        key_epoch,
        issued_at,
    };
    let signature = signer.sign(&attestation.signing_bytes());
    SignedAttestation {
        attestation,
        signature,
    }
}

/// Append a big-endian u32 length prefix followed by `bytes`.
fn push_lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn record(keys: &DeviceKeypairs) -> DeviceRecord {
        keys.sign_record(1_700_000_000_000).record
    }

    #[test]
    fn trust_attestation_verifies() {
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        let att = attest_trust(&signer, &record(&subject), 0, 1_700_000_000_001);
        assert_eq!(att.attestation.kind, AttestationKind::Trust);
        att.verify().unwrap();
    }

    #[test]
    fn revoke_attestation_verifies_and_records_epoch() {
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        let att = attest_revoke(&signer, &record(&subject), 5, 1_700_000_000_002);
        assert_eq!(att.attestation.kind, AttestationKind::Revoke);
        assert_eq!(att.attestation.key_epoch, 5);
        att.verify().unwrap();
    }

    #[test]
    fn tampered_attestation_fails() {
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        let mut att = attest_trust(&signer, &record(&subject), 0, 1);
        att.attestation.key_epoch = 9; // signature no longer covers this
        assert!(matches!(att.verify(), Err(CryptoError::BadSignature)));
    }

    #[test]
    fn forged_signer_device_id_fails() {
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        let mut att = attest_trust(&signer, &record(&subject), 0, 1);
        att.attestation.signer_device_id = "deadbeef".to_owned();
        assert!(matches!(att.verify(), Err(CryptoError::Malformed(_))));
    }

    #[test]
    fn attestation_wire_roundtrips_and_verifies() {
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        for att in [
            attest_trust(&signer, &record(&subject), 2, 1_700_000_000_001),
            attest_revoke(&signer, &record(&subject), 7, 1_700_000_000_002),
        ] {
            let bytes = att.to_bytes();
            let parsed = SignedAttestation::from_bytes(&bytes).unwrap();
            assert_eq!(parsed, att);
            parsed.verify().unwrap();
            assert_eq!(parsed.to_bytes(), bytes);
        }
    }

    #[test]
    fn attestation_wire_rejects_bad_input() {
        assert!(matches!(
            SignedAttestation::from_bytes(b"garbage"),
            Err(CryptoError::Malformed(_))
        ));
        let signer = DeviceKeypairs::generate().unwrap();
        let subject = DeviceKeypairs::generate().unwrap();
        let mut bytes = attest_trust(&signer, &record(&subject), 0, 1).to_bytes();
        bytes.push(0xff); // trailing garbage
        assert!(matches!(
            SignedAttestation::from_bytes(&bytes),
            Err(CryptoError::Malformed(_))
        ));
    }
}
