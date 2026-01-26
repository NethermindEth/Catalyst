use alloy::primitives::Bytes;
use anyhow::{Error, bail};

/// Size (in bytes) of Shasta extra data.
const EXTRA_DATA_LEN: usize = 7;

/// Maximum allowed proposal ID (48 bits).
const MAX_PROPOSAL_ID: u64 = 0xFFFFFFFFFFFF;

/// Maximum allowed base fee sharing percentage.
const MAX_BASEFEE_PCTG: u8 = 100;

/// Structured representation of Shasta block header extra data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraData {
    pub basefee_sharing_pctg: u8,
    pub proposal_id: u64,
}

impl ExtraData {
    /// Encode the extra data field for a Shasta block header.
    ///
    /// Layout (7 bytes total):
    /// - byte 0: basefee_sharing_pctg
    /// - bytes 1..7: most significant 6 bytes of big-endian proposal_id
    pub fn encode(&self) -> Result<Bytes, Error> {
        if self.basefee_sharing_pctg > MAX_BASEFEE_PCTG {
            bail!("basefee_sharing_pctg exceeds 100");
        }

        if self.proposal_id > MAX_PROPOSAL_ID {
            bail!("proposal_id exceeds 48 bits");
        }

        let mut data = [0u8; EXTRA_DATA_LEN];
        data[0] = self.basefee_sharing_pctg;

        let proposal_bytes = self.proposal_id.to_be_bytes();
        data[1..7].copy_from_slice(&proposal_bytes[2..8]);

        Ok(Bytes::copy_from_slice(&data))
    }

    /// Decode the extra data field of a Shasta block header.
    pub fn decode(extra_data: &[u8]) -> Result<Self, Error> {
        if extra_data.len() != EXTRA_DATA_LEN {
            bail!(
                "Invalid extra data length: expected {}, got {}",
                EXTRA_DATA_LEN,
                extra_data.len()
            );
        }

        let basefee_sharing_pctg = extra_data[0];
        if basefee_sharing_pctg > MAX_BASEFEE_PCTG {
            bail!("Invalid basefee_sharing_pctg: {}", basefee_sharing_pctg);
        }

        let mut proposal_bytes = [0u8; 8];
        proposal_bytes[2..].copy_from_slice(&extra_data[1..]);

        let proposal_id = u64::from_be_bytes(proposal_bytes);

        Ok(Self {
            basefee_sharing_pctg,
            proposal_id,
        })
    }
}

impl TryFrom<&[u8]> for ExtraData {
    type Error = Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::decode(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_tests() {
        // basefee_sharing_pctg = 30, proposal_id = 1
        let extra = ExtraData {
            basefee_sharing_pctg: 30,
            proposal_id: 1,
        };

        let encoded = extra.encode().unwrap();
        let expected_bytes = vec![30, 0, 0, 0, 0, 0, 1];
        assert_eq!(encoded.as_ref(), expected_bytes.as_slice());

        let decoded = ExtraData::decode(&encoded).unwrap();
        assert_eq!(decoded.basefee_sharing_pctg, 30);
        assert_eq!(decoded.proposal_id, 1);

        // basefee_sharing_pctg = 50, proposal_id = 0
        let extra = ExtraData {
            basefee_sharing_pctg: 50,
            proposal_id: 0,
        };

        let encoded = extra.encode().unwrap();
        let expected_bytes = vec![50, 0, 0, 0, 0, 0, 0];
        assert_eq!(encoded.as_ref(), expected_bytes.as_slice());

        let decoded = ExtraData::decode(&encoded).unwrap();
        assert_eq!(decoded.basefee_sharing_pctg, 50);
        assert_eq!(decoded.proposal_id, 0);

        // larger proposal_id
        let extra = ExtraData {
            basefee_sharing_pctg: 100,
            proposal_id: 0x1234_5678_9ABC,
        };

        let encoded = extra.encode().unwrap();
        let decoded = ExtraData::decode(&encoded).unwrap();
        assert_eq!(decoded.basefee_sharing_pctg, 100);
        assert_eq!(decoded.proposal_id, 0x1234_5678_9ABC);

        // invalid basefee_sharing_pctg
        let extra = ExtraData {
            basefee_sharing_pctg: 101,
            proposal_id: 1,
        };
        assert!(extra.encode().is_err());

        // invalid proposal_id
        let extra = ExtraData {
            basefee_sharing_pctg: 100,
            proposal_id: 0x1234_5678_9ABC_DEF,
        };
        assert!(extra.encode().is_err());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = ExtraData {
            basefee_sharing_pctg: 25,
            proposal_id: 0x1234_5678_9ABC,
        };

        let encoded = original.encode().unwrap();
        assert_eq!(encoded.len(), EXTRA_DATA_LEN);

        let decoded = ExtraData::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn invalid_length() {
        let data = [0u8; 6];
        assert!(ExtraData::decode(&data).is_err());
    }

    #[test]
    fn invalid_basefee_pctg() {
        let mut data = [0u8; EXTRA_DATA_LEN];
        data[0] = 200;
        assert!(ExtraData::decode(&data).is_err());
    }

    #[test]
    fn proposal_id_too_large() {
        let extra = ExtraData {
            basefee_sharing_pctg: 10,
            proposal_id: MAX_PROPOSAL_ID + 1,
        };

        assert!(extra.encode().is_err());
    }
}
