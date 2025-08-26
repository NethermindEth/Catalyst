/// Ethereum ECDSA signer with fixed K value, implemented by Chainbound https://chainbound.io/
// https://github.com/chainbound/taiko-mk1/blob/74a780af3463efecaded93d36620bd310bf6a834/crates/primitives/src/taiko/deterministic_signer.rs
use anyhow::Error;
use std::str::FromStr;

use alloy::{
    primitives::{B256, U256},
    signers::Signature,
};
use k256::{
    FieldBytes, ProjectivePoint, Scalar,
    elliptic_curve::{
        PrimeField, bigint::Uint, point::AffineCoordinates, scalar::FromUintUnchecked,
    },
};

/// Half of the secp256k1 (a.k.a k256) curve order `N`. Used to check if the
/// signature scalar `s` satisfies `s > N/2`, for signature malleability concerns.
///
/// Here is a more detailed explanation:
/// <https://github.com/OpenZeppelin/openzeppelin-sdk/blob/7d96de7248ae2e7e81a743513ccc617a2e6bba21/packages/lib/contracts/cryptography/ECDSA.sol#L41-L52>
const HALF_ORDER_HEX: &str = "7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0";
const HALF_ORDER: Uint<4> = Uint::from_be_hex(HALF_ORDER_HEX);

/// Signs a message hash using fixed k nonces (tries k=1, falls back to k=2).
///
/// This function tries to follow the reference Go implementation at:
/// <https://github.com/taikoxyz/taiko-mono/blob/d6f39b0c5308e6a1d3ab8ea58ab42d8ef6df4773/packages/taiko-client/driver/anchor_tx_constructor/anchor_tx_constructor.go#L203>
pub fn sign_hash_deterministic(secret_key: B256, hash: B256) -> Result<Signature, Error> {
    // Try with k = 1
    if let Ok(sig) = sign_hash_with_fixed_k(secret_key, hash, 1) {
        return Ok(Signature::try_from(sig.as_slice()).expect("infallible"));
    }

    // Try with k = 2
    if let Ok(sig) = sign_hash_with_fixed_k(secret_key, hash, 2) {
        return Ok(Signature::try_from(sig.as_slice()).expect("infallible"));
    }

    Err(anyhow::anyhow!("Failed to sign hash"))
}

/// Sign a message using a fixed k value.
///
/// NOTE: this is intended for use with **small** k values.
/// `r_scalar` overflows are not handled gracefully.
fn sign_hash_with_fixed_k(sk: B256, message: B256, k: u128) -> Result<[u8; 65], Error> {
    let k_scalar = Scalar::from_u128(k);

    // Convert message to scalar
    let (message_scalar, _of) = scalar_from_bytes(message.as_slice());

    // Get the secret key as scalar
    let (secret_key_scalar, _of) = scalar_from_bytes(sk.as_slice());

    // Compute r = k*G using a projective point which is more efficient
    // and doesn't require modular inversion
    let r_point = ProjectivePoint::GENERATOR * k_scalar;
    let r_affine = r_point.to_affine();

    // r = x-coordinate of R
    let (r_scalar, overflow) = scalar_from_bytes(r_affine.x().as_ref());
    if overflow {
        return Err(anyhow::anyhow!("K too large: {}", k));
    }

    // Determine recovery ID based on y-coordinate
    let mut recovery_id = r_affine.y_is_odd().unwrap_u8() & 1;

    // Calculate k^-1 (inverse of k)
    let Some(k_inv) = k_scalar.invert().into_option() else {
        return Err(anyhow::anyhow!("Cannot invert zero"));
    };

    // s = (sk * r + message) * k^-1
    let mut s_scalar = (message_scalar + r_scalar * secret_key_scalar) * k_inv;

    // Check if s > N/2 and negate if needed, to comply to signature verification
    // algorithms that avoid signature malleability
    if s_scalar > Scalar::from_uint_unchecked(HALF_ORDER) {
        s_scalar = s_scalar.negate();
        recovery_id ^= 1;
    }

    // Create the 65-byte signature: [r (32 bytes) | s (32 bytes) | v (1 byte)]
    let mut signature = [0u8; 65];
    signature[..32].copy_from_slice(r_scalar.to_bytes().as_ref());
    signature[32..64].copy_from_slice(s_scalar.to_bytes().as_ref());
    signature[64] = recovery_id;

    Ok(signature)
}

/// Convert a byte slice to a scalar, also returning `true` if there is overflow.
fn scalar_from_bytes(bytes: &[u8]) -> (Scalar, bool) {
    let field_bytes = FieldBytes::from_slice(bytes);

    // returns Some(Scalar) if there is no overflow
    let scalar = Scalar::from_repr(*field_bytes);

    if scalar.is_some().unwrap_u8() == 1 {
        // There is no overflow, can safely unwrap
        (scalar.unwrap(), false)
    } else {
        // There is no way to unwrap a CtOption when is_some is false
        // even though the value is actually present inside it.
        // Thus we need to compute the modulus by hand.

        let alloy_field_bytes = U256::from_be_slice(bytes);
        let modulus_bytes = B256::from_str(Scalar::MODULUS).expect("assert: modulus fits B256");
        let modulus_bytes = U256::from_be_slice(modulus_bytes.as_slice());

        // Compute the reduced scalar modulo the field order.
        let reduced = alloy_field_bytes % modulus_bytes;
        let reduced_slice = reduced.to_be_bytes::<32>();
        let reduced_bytes = FieldBytes::from_slice(&reduced_slice);

        // Unwrap is safe because the scalar has been reduced to fit within the field order.
        let scalar_reduced = Scalar::from_repr(*FieldBytes::from_slice(reduced_bytes)).unwrap();

        (scalar_reduced, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{hex, keccak256};

    #[test]
    fn test_fixed_k_signature_generation() {
        // Test private key
        let private_key_hex = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let private_key_bytes = hex::decode(private_key_hex).unwrap();
        let private_key = B256::from_slice(&private_key_bytes);

        // Create a test message
        let message = b"This is a test message for signing";
        let message_hash = keccak256(message);

        // Sign the message with fixed k=1
        let signature =
            sign_hash_with_fixed_k(private_key, message_hash, 1).expect("Signing failed");

        // The signature should be deterministic with fixed k, so signing again should produce the
        // same result
        let signature2 =
            sign_hash_with_fixed_k(private_key, message_hash, 1).expect("Second signing failed");

        // Signatures should be identical since we used the same k value
        assert_eq!(signature, signature2);

        // Now try a different k value k=2
        let signature3 =
            sign_hash_with_fixed_k(private_key, message_hash, 2).expect("Third signing failed");

        // Signatures should be different with different k values
        assert_ne!(signature, signature3);
    }

    #[test]
    fn test_anchor_transaction_signing() {
        // Test private key
        let private_key_hex = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let private_key_bytes = hex::decode(private_key_hex).unwrap();
        let private_key = B256::from_slice(&private_key_bytes);

        // Create a test transaction hash
        let tx_hash = keccak256(b"Anchor transaction data");

        // Use the convenience method to sign the transaction
        let signature =
            sign_hash_deterministic(private_key, tx_hash).expect("Anchor tx signing failed");

        println!("signature: {}", hex::encode(signature.as_bytes()));
    }

    #[test]
    fn test_sign_with_k_go_compatibility() {
        // Exactly the same test as the Go implementation of TestSignWithK
        // to verify compatibility between implementations
        //
        // Reference: <https://github.com/taikoxyz/taiko-mono/blob/d6f39b0c5308e6a1d3ab8ea58ab42d8ef6df4773/packages/taiko-client/driver/signer/fixed_k_signer_test.go#L14>

        // Private key from the Go test
        let private_key_hex = "92954368afd3caa1f3ce3ead0069c1af414054aefe1ef9aeacc1bf426222ce38";
        let private_key_bytes = hex::decode(private_key_hex).unwrap();
        let private_key = B256::from_slice(&private_key_bytes);

        // Test case 1
        let payload1_hex = "44943399d1507f3ce7525e9be2f987c3db9136dc759cb7f92f742154196868b9";
        let payload1_bytes = hex::decode(payload1_hex).unwrap();
        let payload1 = B256::from_slice(&payload1_bytes);

        let expected_r1 = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
        let expected_s1 = "38940d69b21d5b088beb706e9ebabe6422307e12863997a44239774467e240d5";
        let expected_v1 = 1;

        // Sign the first payload with k=2
        let sig1 = sign_hash_with_fixed_k(private_key, payload1, 2).expect("Signing failed");
        let (r1, s1, v1) = decode_sig(&sig1);

        // Verify they match the expected values from the Go test
        assert_eq!(r1, expected_r1, "r value mismatch in test case 1");
        assert_eq!(s1, expected_s1, "s value mismatch in test case 1");
        assert_eq!(v1, expected_v1, "v value mismatch in test case 1");

        // Test case 2
        let payload2_hex = "663d210fa6dba171546498489de1ba024b89db49e21662f91bf83cdffe788820";
        let payload2_bytes = hex::decode(payload2_hex).unwrap();
        let payload2 = B256::from_slice(&payload2_bytes);

        let expected_r2 = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
        let expected_s2 = "5840695138a83611aa9dac67beb95aba7323429787a78df993f1c5c7f2c0ef7f";
        let expected_v2 = 0;

        // Sign the second payload with k=2
        let sig2 = sign_hash_with_fixed_k(private_key, payload2, 2).expect("Signing failed");
        let (r2, s2, v2) = decode_sig(&sig2);

        // Verify they match the expected values from the Go test
        assert_eq!(r2, expected_r2, "r value mismatch in test case 2");
        assert_eq!(s2, expected_s2, "s value mismatch in test case 2");
        assert_eq!(v2, expected_v2, "v value mismatch in test case 2");
    }

    fn decode_sig(sig: &[u8]) -> (String, String, u8) {
        let r = hex::encode(&sig[0..32]);
        let s = hex::encode(&sig[32..64]);
        let v = sig[64];
        (r, s, v)
    }
}
