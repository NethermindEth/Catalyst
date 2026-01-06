use alloy::primitives::Bytes;
use anyhow::Error;

/// Encode the extra data field for a Shasta block header.
pub fn encode_extra_data(basefee_sharing_pctg: u8, proposal_id: u64) -> Result<Bytes, Error> {
    if proposal_id > 0xFFFFFFFFFFFF {
        return Err(anyhow::anyhow!(
            "encode_extra_data: proposal_id exceeds 48 bits"
        ));
    }
    if basefee_sharing_pctg > 100 {
        return Err(anyhow::anyhow!(
            "encode_extra_data: basefee_sharing_pctg exceeds 100"
        ));
    }
    let mut data = [0u8; 7];
    data[0] = basefee_sharing_pctg;
    let proposal_bytes = proposal_id.to_be_bytes();
    data[1..7].copy_from_slice(&proposal_bytes[2..8]);
    Ok(Bytes::from(data.to_vec()))
}

/// Decode the extra data field of a Shasta block header.
///
/// The expected format of `extra_data` is exactly 7 bytes:
/// - byte 0: `basefee_sharing_pctg` (the base fee sharing percentage)
/// - bytes 1..7: the most significant 6 bytes of the big-endian `proposal_id`
///
/// On success, returns a tuple `(basefee_sharing_pctg, proposal_id)`.
/// Returns an error if `extra_data` is not exactly 7 bytes long.
pub fn decode_extra_data(extra_data: &[u8]) -> Result<(u8, u64), Error> {
    if extra_data.len() != 7 {
        return Err(anyhow::anyhow!(
            "Invalid extra data length: expected 7, got {}",
            extra_data.len()
        ));
    }
    let basefee_sharing_pctg = extra_data[0];
    let proposal_bytes = &extra_data[1..7];
    let mut full_proposal_bytes = [0u8; 8];
    full_proposal_bytes[2..8].copy_from_slice(proposal_bytes);
    let proposal_id = u64::from_be_bytes(full_proposal_bytes);
    Ok((basefee_sharing_pctg, proposal_id))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_encode_decode_extra_data() {
        use super::*;

        // Test encoding with basefee_sharing_pctg=30, proposal_id=1
        let extra_data = encode_extra_data(30, 1).unwrap();
        let expected_bytes = vec![30, 0, 0, 0, 0, 0, 1];
        assert_eq!(extra_data.as_ref(), expected_bytes.as_slice());

        // Test decoding
        let (decoded_pctg, decoded_proposal_id) = decode_extra_data(&extra_data).unwrap();
        assert_eq!(decoded_pctg, 30);
        assert_eq!(decoded_proposal_id, 1);

        // Test encoding with basefee_sharing_pctg=50, proposal_id=0
        let extra_data = encode_extra_data(50, 0).unwrap();
        let expected_bytes = vec![50, 0, 0, 0, 0, 0, 0];
        assert_eq!(extra_data.as_ref(), expected_bytes.as_slice());

        // Test decoding
        let (decoded_pctg, decoded_proposal_id) = decode_extra_data(&extra_data).unwrap();
        assert_eq!(decoded_pctg, 50);
        assert_eq!(decoded_proposal_id, 0);

        // Test with larger proposal_id
        let extra_data = encode_extra_data(100, 0x123456789ABC).unwrap();
        let (decoded_pctg, decoded_proposal_id) = decode_extra_data(&extra_data).unwrap();
        assert_eq!(decoded_pctg, 100);
        assert_eq!(decoded_proposal_id, 0x123456789ABC);

        // Test with invalid basefee_sharing_pctg
        let extra_data = encode_extra_data(101, 0x123456789ABC);
        assert!(extra_data.is_err());

        // Test with invalid proposal_id
        let extra_data = encode_extra_data(100, 0x123456789ABCDEF);
        assert!(extra_data.is_err());
    }
}
