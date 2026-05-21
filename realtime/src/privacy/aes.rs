//! AES-256-GCM with a freshly random 12-byte nonce per call.
//!
//! The nonce is drawn from the OS CSPRNG (`OsRng`). 96-bit random nonces give
//! collision probability ~2⁻³² after 2³² messages, far beyond Surge's lifetime.
//! Re-using a (key, nonce) pair under AES-GCM is catastrophic, so we MUST never
//! derive the nonce deterministically.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

/// Length of the AES-GCM nonce in bytes.
pub const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AesError {
    Encrypt,
}

/// Encrypts `plaintext` under `key` and returns the inner blob payload
/// `[nonce(12B) || ciphertext || tag(16B)]` (without the outer scheme byte).
pub fn encrypt_random_nonce(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, AesError> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    encrypt_with_nonce(plaintext, key, &nonce)
}

/// Encrypts under an explicit nonce. Exposed for deterministic tests; production code
/// goes through `encrypt_random_nonce`.
pub fn encrypt_with_nonce(
    plaintext: &[u8],
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, AesError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let ct = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        .map_err(|_| AesError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Key, Nonce};

    #[test]
    fn roundtrip_via_aes_gcm_decrypt() {
        // Encrypt with our helper, decrypt with a stock AES-GCM call to verify layout.
        let key = [0x42u8; 32];
        let nonce = [0x37u8; NONCE_LEN];
        let m = b"compressed manifest goes here";
        let inner = encrypt_with_nonce(m, &key, &nonce).unwrap();

        // [nonce(12) || ct(len(m)) || tag(16)]
        assert_eq!(&inner[..12], &nonce);
        assert_eq!(inner.len(), 12 + m.len() + 16);

        // Decrypt the ct+tag tail with a fresh AES-GCM instance.
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &inner[12..],
                    aad: &[],
                },
            )
            .unwrap();
        assert_eq!(pt, m);
    }

    #[test]
    fn random_nonce_changes_per_call() {
        let key = [0u8; 32];
        let a = encrypt_random_nonce(b"x", &key).unwrap();
        let b = encrypt_random_nonce(b"x", &key).unwrap();
        assert_ne!(&a[..12], &b[..12]);
    }
}
