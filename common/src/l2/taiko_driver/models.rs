use alloy::primitives::B256;
use hex::FromHex;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildPreconfBlockRequestBody {
    pub executable_data: ExecutableData,
    pub end_of_sequencing: bool,
    pub is_forced_inclusion: bool,
}

#[derive(Debug)]
pub struct BuildPreconfBlockResponse {
    pub number: u64,
    pub hash: B256,
    pub parent_hash: B256,
}

impl BuildPreconfBlockResponse {
    pub fn new_from_value(value: serde_json::Value) -> Option<Self> {
        let header = value.get("blockHeader")?;

        Some(Self {
            number: u64::from_str_radix(
                header.get("number")?.as_str()?.trim_start_matches("0x"),
                16,
            )
            .ok()?,
            hash: Self::to_b256(header.get("hash")?.as_str()?)?,
            parent_hash: Self::to_b256(header.get("parentHash")?.as_str()?)?,
        })
    }

    fn to_b256(s: &str) -> Option<B256> {
        let bytes = <[u8; 32]>::from_hex(s.trim_start_matches("0x")).ok()?;
        Some(B256::from(bytes))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableData {
    pub base_fee_per_gas: u64,
    pub block_number: u64,
    pub extra_data: String,
    pub fee_recipient: String,
    pub gas_limit: u64,
    pub parent_hash: String,
    pub timestamp: u64,
    pub transactions: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaikoStatus {
    #[serde(rename = "highestUnsafeL2PayloadBlockID")]
    pub highest_unsafe_l2_payload_block_id: u64,
    #[serde(
        rename = "endOfSequencingBlockHash",
        deserialize_with = "deserialize_end_of_sequencing_block_hash"
    )]
    pub end_of_sequencing_block_hash: B256,
}

fn deserialize_end_of_sequencing_block_hash<'de, D>(deserializer: D) -> Result<B256, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let s = s.trim_start_matches("0x");
    let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
    if bytes.len() != 32 {
        return Err(serde::de::Error::custom(
            "Invalid length for end_of_sequencing_block_hash",
        ));
    }
    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);
    Ok(B256::from(array))
}
