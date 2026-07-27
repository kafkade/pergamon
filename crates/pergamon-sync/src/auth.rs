// SPDX-License-Identifier: Apache-2.0

//! OPAQUE client helpers for hosted-sync server auth (WP-3a, #189).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This is the **client** half of the OPAQUE flows whose server half lives in
//! the AGPL `pergamon-sync-server` crate. It is gated behind the `auth` feature
//! so the core sync engine stays crypto-light. `opaque-ke` is dual
//! MIT/Apache-2.0, so it is fine in this Apache-2.0 crate; nothing here crosses
//! the AGPL/Apache boundary.
//!
//! The password (optionally folded with a high-entropy Secret Key by a higher
//! layer — the server is agnostic, design §Part 4 Q2) never leaves the device:
//! the OPRF blinds it before it is sent.
//!
//! ## Cross-crate cipher-suite parity
//! [`PergamonCipherSuite`] must stay byte-for-byte parameter-identical to the
//! server's definition in `pergamon-sync-server::auth::cipher_suite`. The two
//! are deliberately duplicated to respect the AGPL/Apache split; the server↔
//! client round-trip integration test guards against drift. **Keep them in
//! sync.**
//!
//! Because `opaque-ke 4.x` is built on the `digest 0.10` generation, the AKE
//! hash is `sha2 0.10`'s `Sha512`, imported here via the `sha2-opaque` rename.

use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse, Ristretto255,
    TripleDh,
};
use rand::rngs::OsRng;

/// The project OPAQUE cipher suite — identical to the server's (see module docs).
#[derive(Debug, Clone, Copy)]
pub struct PergamonCipherSuite;

impl CipherSuite for PergamonCipherSuite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2_opaque::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}

/// Errors from the client OPAQUE helpers.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The `opaque-ke` protocol rejected a message (e.g. an invalid login).
    #[error("OPAQUE protocol error: {0}")]
    Protocol(String),
    /// A server message could not be decoded.
    #[error("failed to decode OPAQUE server message")]
    Decode,
}

/// An in-progress client registration awaiting the server's response.
///
/// Hold this between [`ClientRegistrationFlow::start`] and
/// [`ClientRegistrationFlow::finish`].
pub struct ClientRegistrationFlow {
    state: ClientRegistration<PergamonCipherSuite>,
}

impl ClientRegistrationFlow {
    /// Begin registration for `password`. Returns the flow to persist and the
    /// serialized `RegistrationRequest` to send to `register/start`.
    ///
    /// # Errors
    /// Returns [`AuthError::Protocol`] if the OPRF blinding fails.
    pub fn start(password: &[u8]) -> Result<(Self, Vec<u8>), AuthError> {
        let mut rng = OsRng;
        let result = ClientRegistration::<PergamonCipherSuite>::start(&mut rng, password)
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok((
            Self {
                state: result.state,
            },
            result.message.serialize().to_vec(),
        ))
    }

    /// Finish registration given the server's `RegistrationResponse` bytes.
    /// Returns the serialized `RegistrationUpload` to send to `register/finish`.
    ///
    /// # Errors
    /// Returns [`AuthError::Decode`] if the response is malformed, or
    /// [`AuthError::Protocol`] if finalization fails.
    pub fn finish(self, password: &[u8], response_bytes: &[u8]) -> Result<Vec<u8>, AuthError> {
        let response = RegistrationResponse::<PergamonCipherSuite>::deserialize(response_bytes)
            .map_err(|_| AuthError::Decode)?;
        let mut rng = OsRng;
        let result = self
            .state
            .finish(
                &mut rng,
                password,
                response,
                ClientRegistrationFinishParameters::default(),
            )
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok(result.message.serialize().to_vec())
    }
}

/// An in-progress client login awaiting the server's KE2 response.
///
/// Hold this between [`ClientLoginFlow::start`] and [`ClientLoginFlow::finish`].
pub struct ClientLoginFlow {
    state: ClientLogin<PergamonCipherSuite>,
}

/// The successful result of [`ClientLoginFlow::finish`].
pub struct ClientLoginFinished {
    /// Serialized `CredentialFinalization` (KE3) to send to `login/finish`.
    pub finalization: Vec<u8>,
    /// The mutually-authenticated session key (matches the server's on success).
    pub session_key: Vec<u8>,
}

impl ClientLoginFlow {
    /// Begin login for `password`. Returns the flow to persist and the
    /// serialized `CredentialRequest` (KE1) to send to `login/start`.
    ///
    /// # Errors
    /// Returns [`AuthError::Protocol`] if the OPRF blinding fails.
    pub fn start(password: &[u8]) -> Result<(Self, Vec<u8>), AuthError> {
        let mut rng = OsRng;
        let result = ClientLogin::<PergamonCipherSuite>::start(&mut rng, password)
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok((
            Self {
                state: result.state,
            },
            result.message.serialize().to_vec(),
        ))
    }

    /// Finish login given the server's `CredentialResponse` (KE2) bytes.
    ///
    /// # Errors
    /// Returns [`AuthError::Decode`] if the response is malformed, or
    /// [`AuthError::Protocol`] on an invalid login (wrong password / unknown
    /// identity dummy path) — the two are indistinguishable by design.
    pub fn finish(
        self,
        password: &[u8],
        response_bytes: &[u8],
    ) -> Result<ClientLoginFinished, AuthError> {
        let response = CredentialResponse::<PergamonCipherSuite>::deserialize(response_bytes)
            .map_err(|_| AuthError::Decode)?;
        let mut rng = OsRng;
        let result = self
            .state
            .finish(
                &mut rng,
                password,
                response,
                ClientLoginFinishParameters::default(),
            )
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok(ClientLoginFinished {
            finalization: result.message.serialize().to_vec(),
            session_key: result.session_key.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A local client-only sanity check that the helpers compose (the full
    /// cross-crate round trip against the server is proven in the server crate's
    /// integration test). Here we only drive the client side against the
    /// library's own server types to keep this crate self-contained.
    #[test]
    fn client_helpers_produce_messages() {
        use opaque_ke::{ServerRegistration, ServerSetup};

        let (reg_flow, request) = ClientRegistrationFlow::start(b"pw").unwrap();
        assert!(!request.is_empty());

        // Drive the server side with the library directly to get a response.
        let mut rng = OsRng;
        let setup = ServerSetup::<PergamonCipherSuite>::new(&mut rng);
        let req =
            opaque_ke::RegistrationRequest::<PergamonCipherSuite>::deserialize(&request).unwrap();
        let s_start =
            ServerRegistration::<PergamonCipherSuite>::start(&setup, req, b"alice").unwrap();
        let upload = reg_flow
            .finish(b"pw", &s_start.message.serialize())
            .unwrap();
        assert!(!upload.is_empty());
    }
}
