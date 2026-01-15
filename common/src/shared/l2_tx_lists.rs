use alloy::{
    consensus::TxEnvelope,
    consensus::transaction::{Recovered, SignerRecoverable},
    rpc::types::Transaction,
};

use alloy_rlp::Decodable;
use anyhow::Error;
use flate2::{
    Compression,
    write::{ZlibDecoder, ZlibEncoder},
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::io::Write;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct RPCReplyL2TxLists {
    #[serde(deserialize_with = "deserialize_tx_lists_bytes")]
    pub tx_list_bytes: Vec<Vec<u8>>,
    #[serde(deserialize_with = "deserialize_parent_meta_hash")]
    pub parent_meta_hash: [u8; 32],
    #[serde(rename = "ParentBlockID")]
    pub parent_block_id: u64,
}

fn deserialize_tx_lists_bytes<'de, D>(deserializer: D) -> Result<Vec<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    let vec: Vec<String> = Deserialize::deserialize(deserializer)?;
    let result = vec
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect::<Vec<Vec<u8>>>();
    Ok(result)
}

fn deserialize_parent_meta_hash<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let s = s.trim_start_matches("0x");
    let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
    if bytes.len() != 32 {
        return Err(serde::de::Error::custom(
            "Invalid length for parent_meta_hash",
        ));
    }
    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);
    Ok(array)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PreBuiltTxList {
    #[serde(
        rename = "TxList",
        alias = "txList",
        deserialize_with = "deserialize_tx_list"
    )]
    pub tx_list: Vec<Transaction>,

    #[serde(rename = "EstimatedGasUsed", alias = "estimatedGasUsed")]
    pub estimated_gas_used: u64,

    #[serde(rename = "BytesLength", alias = "bytesLength")]
    pub bytes_length: u64,
}

impl PreBuiltTxList {
    pub fn empty() -> Self {
        PreBuiltTxList {
            tx_list: Vec::new(),
            estimated_gas_used: 0,
            bytes_length: 0,
        }
    }
}

pub fn uncompress_and_decode(data: &[u8]) -> Result<Vec<Transaction>, Error> {
    // First decompress using zlib
    let mut decoder = ZlibDecoder::new(Vec::new());
    decoder.write_all(data)?;
    let decompressed_data = decoder.finish()?;

    // Decode into inner transactions
    let tx_list: Vec<TxEnvelope> = Decodable::decode(&mut decompressed_data.as_slice())
        .map_err(|e| anyhow::anyhow!("Failed to decode RLP: {}", e))?;

    // Convert to transactions
    convert_tx_envelopes_to_transactions(tx_list)
}

pub fn convert_tx_envelopes_to_transactions(
    tx_envelopes: impl IntoIterator<Item = TxEnvelope>,
) -> Result<Vec<Transaction>, Error> {
    tx_envelopes
        .into_iter()
        .map(|tx| {
            let signer = tx
                .recover_signer()
                .map_err(|e| anyhow::anyhow!("Failed to recover signer: {}", e))?;
            Ok(Transaction {
                inner: Recovered::new_unchecked(tx, signer),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                effective_gas_price: None,
            })
        })
        .collect()
}

pub fn rlp_encode(tx_list: &[Transaction]) -> Vec<u8> {
    let mut buffer = Vec::<u8>::new();
    alloy_rlp::encode_iter(tx_list.iter().map(|tx| tx.inner.clone()), &mut buffer);
    buffer
}

// RLP encode and zlib compress
pub fn encode_and_compress(tx_list: &[Transaction]) -> Result<Vec<u8>, Error> {
    // First RLP encode the transactions
    let buffer = rlp_encode(tx_list);

    // Then compress using zlib
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&buffer)
        .map_err(|e| anyhow::anyhow!("PreBuiltTxList::encode: Failed to compress: {}", e))?;
    encoder
        .finish()
        .map_err(|e| anyhow::anyhow!("PreBuiltTxList::encode: Failed to finish: {}", e))
}

fn deserialize_tx_list<'de, D>(deserializer: D) -> Result<Vec<Transaction>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let transactions = value
        .as_array()
        .ok_or_else(|| serde::de::Error::custom("Expected array"))?
        .iter()
        .map(|tx| {
            let tx_envelope = serde_json::from_value::<alloy::consensus::TxEnvelope>(tx.clone())
                .map_err(|e| {
                    serde::de::Error::custom(format!("Failed to parse transaction: {e}"))
                })?;
            let signer = tx_envelope
                .recover_signer()
                .map_err(|e| serde::de::Error::custom(format!("Failed to recover signer: {e}")))?;
            Ok(Transaction {
                inner: Recovered::new_unchecked(tx_envelope, signer),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                effective_gas_price: None,
            })
        })
        .collect::<Result<Vec<Transaction>, D::Error>>()?;
    Ok(transactions)
}

pub fn decompose_pending_lists_json_from_geth(json: Value) -> Result<Vec<PreBuiltTxList>, Error> {
    let rpc_reply: Vec<PreBuiltTxList> = serde_json::from_value(json)?;
    Ok(rpc_reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_pending_tx_lists() {
        let pending_tx_lists = serde_json::from_str::<Vec<PreBuiltTxList>>(include_str!(
            "../utils/tx_lists_test_response_from_geth.json"
        ))
        .unwrap();

        println!("{pending_tx_lists:?}");

        assert_eq!(pending_tx_lists.len(), 1);
        assert_eq!(pending_tx_lists[0].tx_list.len(), 2);
        let tx_legacy = pending_tx_lists[0].tx_list[0].inner.as_legacy().unwrap();
        assert_eq!(tx_legacy.tx().chain_id, Some(167000));
        assert_eq!(
            pending_tx_lists[0].tx_list[1].inner.signer(),
            "0xe25583099ba105d9ec0a67f5ae86d90e50036425"
                .parse::<alloy::primitives::Address>()
                .unwrap()
        );
        assert_eq!(pending_tx_lists[0].estimated_gas_used, 42000);
        assert_eq!(pending_tx_lists[0].bytes_length, 203);
    }
}
