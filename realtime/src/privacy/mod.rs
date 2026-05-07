//! Blob privacy: encrypts compressed manifests before they become EIP-4844 blobs.
//!
//! Catalyst is the *encrypting* side: it generates AES-256-GCM ciphertexts under a
//! shared symmetric key and emits the inner blob payload `[scheme || ...]` that gets
//! packed by `SidecarBuilder`. The same scheme bytes and layouts are decoded by the
//! driver and the raiko prover.
//!
//! This crate intentionally does NOT include an ECIES implementation: forced-inclusion
//! blobs are encrypted by their submitters off-system and posted directly to L1 via
//! `RealTimeInbox.saveForcedInclusion`. Catalyst only references their blob hashes via
//! `numForcedInclusions` on the propose input — it never re-encrypts FI payloads.
//!
//! Byte layout (mirrors `raiko/lib/src/privacy/`):
//!
//! - Plaintext (scheme 0x00): `[0x00 || compressed_manifest]`
//! - AES-256-GCM (scheme 0x01): `[0x01 || nonce(12B) || ciphertext || tag(16B)]`

pub mod aes;

use anyhow::{anyhow, Result};

/// Plaintext (no encryption).
pub const SCHEME_PLAIN: u8 = 0x00;
/// AES-256-GCM with a shared symmetric key.
pub const SCHEME_AES256_GCM: u8 = 0x01;

/// Encrypts (or pass-through-wraps) the compressed manifest into the inner blob payload
/// that `SidecarBuilder::from_slice` will pack into the EIP-4844 sidecar.
///
/// In privacy mode (`cipher.is_some()`), produces a scheme-0x01 payload with a fresh
/// random AES-GCM nonce. Otherwise, produces a scheme-0x00 payload (plaintext).
#[derive(Clone)]
pub struct ProposalCipher {
    symmetric_key: Option<[u8; 32]>,
}

impl ProposalCipher {
    /// Construct a privacy-disabled cipher (emits scheme 0x00 / plaintext).
    pub fn disabled() -> Self {
        Self {
            symmetric_key: None,
        }
    }

    /// Construct a privacy-enabled cipher with the given 32-byte AES-256-GCM key.
    pub fn enabled(symmetric_key: [u8; 32]) -> Self {
        Self {
            symmetric_key: Some(symmetric_key),
        }
    }

    /// True if this cipher will produce scheme 0x01 (encrypted) blobs.
    pub fn is_enabled(&self) -> bool {
        self.symmetric_key.is_some()
    }

    /// Wraps `compressed_manifest` (`M`) into the scheme-prefixed inner blob payload.
    pub fn wrap(&self, compressed_manifest: &[u8]) -> Result<Vec<u8>> {
        match self.symmetric_key {
            None => {
                let mut out = Vec::with_capacity(1 + compressed_manifest.len());
                out.push(SCHEME_PLAIN);
                out.extend_from_slice(compressed_manifest);
                Ok(out)
            }
            Some(key) => {
                let inner = aes::encrypt_random_nonce(compressed_manifest, &key)
                    .map_err(|e| anyhow!("AES-GCM encrypt failed: {:?}", e))?;
                let mut out = Vec::with_capacity(1 + inner.len());
                out.push(SCHEME_AES256_GCM);
                out.extend_from_slice(&inner);
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_plaintext_prepends_scheme_byte() {
        let cipher = ProposalCipher::disabled();
        let m = b"the quick brown fox";
        let out = cipher.wrap(m).unwrap();
        assert_eq!(out[0], SCHEME_PLAIN);
        assert_eq!(&out[1..], m);
    }

    #[test]
    fn wrap_encrypted_layout() {
        let cipher = ProposalCipher::enabled([0x42u8; 32]);
        let m = b"the quick brown fox";
        let out = cipher.wrap(m).unwrap();
        // [scheme(1) || nonce(12) || ct(=len(m)) || tag(16)]
        assert_eq!(out[0], SCHEME_AES256_GCM);
        assert_eq!(out.len(), 1 + 12 + m.len() + 16);
    }

    #[test]
    fn wrap_encrypted_uses_fresh_nonce_each_call() {
        let cipher = ProposalCipher::enabled([0x42u8; 32]);
        let m = b"abc";
        let a = cipher.wrap(m).unwrap();
        let b = cipher.wrap(m).unwrap();
        // Same scheme + length, but the nonce (and therefore ciphertext + tag) must differ.
        assert_eq!(a.len(), b.len());
        assert_ne!(a, b);
        assert_ne!(&a[1..13], &b[1..13]); // nonces differ
    }
}
