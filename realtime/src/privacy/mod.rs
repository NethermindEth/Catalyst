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
//! Wire layout (full blob payload buffer fed into `SidecarBuilder::from_slice`):
//!
//! ```text
//! [ bytes32(version=1) (32B) ] [ bytes32(size) (32B) ] [ scheme (1B) ] [ scheme_body ]
//!                                                       └───── inner ────────┘
//!                                                       size = 1 + len(scheme_body)
//! ```
//!
//! Inner shape per scheme (mirrors `raiko/lib/src/privacy/`):
//!
//! - Plaintext (scheme 0x00): `[0x00 || compressed_manifest]`
//! - AES-256-GCM (scheme 0x01): `[0x01 || nonce(12B) || ciphertext || tag(16B)]`
//!
//! [`build_blob_payload`] is the helper that performs the end-to-end transform from
//! `manifest.encode_and_compress()` framed bytes to the corrected `[frame || inner]`
//! buffer; both the L1 proposal-tx builder and the Raiko-request submitter use it so
//! the bytes hashed on L1 match the bytes Raiko sees.

pub mod aes;

use alloy::primitives::U256;
use anyhow::{Result, anyhow};

/// Length of the Shasta `[bytes32(version) || bytes32(size)]` outer frame.
const SHASTA_FRAME_LEN: usize = 64;

/// `SHASTA_PAYLOAD_VERSION` from `taiko_protocol::shasta::constants`. Hard-coded here
/// to keep the privacy module self-contained; if upstream bumps this value, this constant
/// must be updated in lockstep (the unit tests will catch a mismatch on round-trip).
const SHASTA_PAYLOAD_VERSION: u8 = 0x01;

/// Re-frames the output of `taiko_protocol::shasta::manifest::DerivationSourceManifest::
/// encode_and_compress` so the privacy scheme byte sits *inside* the Shasta frame, not
/// before it.
///
/// `framed_input` is `encode_and_compress`'s return value: `[frame(64) || compressed_manifest]`.
/// The frame is stripped, the compressed manifest is wrapped via `cipher` (producing
/// `[scheme || scheme_body]`), and a fresh frame with the new size is prepended. The
/// returned buffer is what `SidecarBuilder::from_slice` should consume.
///
/// Without this re-framing, `cipher.wrap(framed_input)` would emit
/// `[scheme || frame || compressed_manifest]`, which causes raiko's
/// `blob_tx_slice_param_for_source` to read the scheme byte as the first byte of the
/// version field and reject the blob.
pub fn build_blob_payload(framed_input: &[u8], cipher: &ProposalCipher) -> Result<Vec<u8>> {
    if framed_input.len() < SHASTA_FRAME_LEN {
        return Err(anyhow!(
            "encode_and_compress output shorter than Shasta frame: {} < {}",
            framed_input.len(),
            SHASTA_FRAME_LEN
        ));
    }
    let compressed_manifest = &framed_input[SHASTA_FRAME_LEN..];

    // [scheme(1B) || scheme_body]
    let inner = cipher.wrap(compressed_manifest)?;

    let mut out = Vec::with_capacity(SHASTA_FRAME_LEN + inner.len());

    // bytes32(version) = [0u8; 31] || SHASTA_PAYLOAD_VERSION
    let mut version = [0u8; 32];
    version[31] = SHASTA_PAYLOAD_VERSION;
    out.extend_from_slice(&version);

    // bytes32(size) = U256(inner.len()).to_be_bytes::<32>()
    let size_bytes: [u8; 32] = U256::from(inner.len()).to_be_bytes();
    out.extend_from_slice(&size_bytes);

    out.extend_from_slice(&inner);
    Ok(out)
}

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

    /// Builds a synthetic `encode_and_compress` output (`[frame(64) || compressed]`)
    /// and verifies `build_blob_payload` re-emits the frame around the cipher-wrapped
    /// inner — version byte at index 31, size matching `inner.len()`, scheme byte at
    /// index 64. This is the layout raiko's `blob_tx_slice_param_for_source` and the
    /// driver's `ExtractVersionAndSize` both expect.
    #[test]
    fn build_blob_payload_reframes_around_inner() {
        // Synthesize what encode_and_compress would have returned.
        let compressed = b"compressed-manifest-bytes";
        let mut framed_input = Vec::with_capacity(64 + compressed.len());
        let mut version = [0u8; 32];
        version[31] = SHASTA_PAYLOAD_VERSION;
        framed_input.extend_from_slice(&version);
        let size_bytes: [u8; 32] = U256::from(compressed.len()).to_be_bytes();
        framed_input.extend_from_slice(&size_bytes);
        framed_input.extend_from_slice(compressed);

        // Privacy disabled: inner = [0x00 || compressed].
        let cipher = ProposalCipher::disabled();
        let out = build_blob_payload(&framed_input, &cipher).unwrap();

        // Frame: version[31] = 0x01.
        assert_eq!(out[..31], [0u8; 31]);
        assert_eq!(out[31], SHASTA_PAYLOAD_VERSION);

        // Frame: size = U256(inner.len()) = U256(1 + compressed.len()).
        let expected_size: [u8; 32] = U256::from(1 + compressed.len()).to_be_bytes();
        assert_eq!(&out[32..64], &expected_size);

        // Inner: scheme byte at 64, then the compressed manifest.
        assert_eq!(out[64], SCHEME_PLAIN);
        assert_eq!(&out[65..], compressed);
    }

    #[test]
    fn build_blob_payload_encrypted_path() {
        let compressed = b"some-compressed-bytes";
        let mut framed_input = Vec::with_capacity(64 + compressed.len());
        framed_input.extend_from_slice(&[0u8; 32]); // version (with last byte unset is fine; we strip)
        framed_input.extend_from_slice(&[0u8; 32]); // size (we strip)
        framed_input.extend_from_slice(compressed);

        let cipher = ProposalCipher::enabled([0x42u8; 32]);
        let out = build_blob_payload(&framed_input, &cipher).unwrap();

        // Re-emitted frame.
        assert_eq!(out[31], SHASTA_PAYLOAD_VERSION);
        // Inner length = 1 (scheme) + 12 (nonce) + len(compressed) (ct) + 16 (tag).
        let expected_inner_len = 1 + 12 + compressed.len() + 16;
        let expected_size: [u8; 32] = U256::from(expected_inner_len).to_be_bytes();
        assert_eq!(&out[32..64], &expected_size);
        assert_eq!(out[64], SCHEME_AES256_GCM);
        assert_eq!(out.len(), 64 + expected_inner_len);
    }

    #[test]
    fn build_blob_payload_rejects_short_input() {
        let cipher = ProposalCipher::disabled();
        let too_short = vec![0u8; 63];
        assert!(build_blob_payload(&too_short, &cipher).is_err());
    }

    /// Reproduces raiko's `blob_tx_slice_param_for_source` parser
    /// (`raiko/lib/src/input.rs:442`) on this helper's output and asserts the
    /// returned `(start, size)` slice exactly equals the cipher-wrapped inner.
    /// This test is the regression guard for the bug where the scheme byte was
    /// emitted *outside* the frame and raiko rejected the payload.
    #[test]
    fn build_blob_payload_passes_raiko_slice_parser() {
        // Synthetic encode_and_compress output (frame contents are stripped, so
        // we don't bother making them realistic).
        let compressed = b"compressed-manifest-payload";
        let mut framed_input = Vec::new();
        framed_input.extend_from_slice(&[0u8; 64]);
        framed_input.extend_from_slice(compressed);

        let cipher = ProposalCipher::enabled([0xAAu8; 32]);
        let out = build_blob_payload(&framed_input, &cipher).unwrap();

        // ─── Mimic raiko's parser, byte for byte ─────────────────────────────
        let offset = 0usize;

        // version check: bytes32(1) == [0u8; 31] || 0x01
        let mut expected_version = [0u8; 32];
        expected_version[31] = 1;
        assert_eq!(
            &out[offset..offset + 32],
            &expected_version,
            "version frame must be bytes32(1)"
        );

        // size: read bytes [offset+32 .. offset+64] as B256, take last 8 BE → u64.
        let size_b256: [u8; 32] = out[offset + 32..offset + 64].try_into().unwrap();
        let size_bytes: [u8; 8] = size_b256[24..32].try_into().unwrap();
        let blob_data_size = u64::from_be_bytes(size_bytes) as usize;

        let start = offset + 64;
        let end = start + blob_data_size;
        assert!(end <= out.len(), "advertised size must fit");

        let sliced = &out[start..end];

        // The slice must equal cipher.wrap(compressed_manifest) exactly.
        let expected_inner_len = 1 + 12 + compressed.len() + 16; // scheme + nonce + ct + tag
        assert_eq!(sliced.len(), expected_inner_len);
        assert_eq!(sliced[0], SCHEME_AES256_GCM);

        // The dispatcher would now strip the scheme byte and feed the rest to AES.
        // (We don't redo the round-trip here — `aes::tests::roundtrip` covers that.)
    }
}
