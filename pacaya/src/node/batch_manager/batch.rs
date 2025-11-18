use crate::shared::l2_block::L2Block;
use crate::shared::l2_tx_lists::encode_and_compress;
use alloy::primitives::Address;
use common::batch_builder::BatchLike;
use std::time::Instant;
use tracing::{debug, warn};

#[derive(Default, Clone)]
pub struct Batch {
    pub l2_blocks: Vec<L2Block>,
    pub total_bytes: u64,
    pub coinbase: Address,
    pub anchor_block_id: u64,
    pub anchor_block_timestamp_sec: u64,
}

impl Batch {
    pub fn compress(&mut self) {
        let start = Instant::now();

        let tx_vec: Vec<_> = self
            .l2_blocks
            .iter()
            .flat_map(|block| block.prebuilt_tx_list.tx_list.clone())
            .collect();

        match encode_and_compress(&tx_vec) {
            Ok(res) => match res.len().try_into() {
                Ok(len) => self.total_bytes = len,
                Err(_) => warn!("Compressed length conversion failed"),
            },
            Err(err) => warn!("Failed to compress tx list: {err}"),
        }

        let duration = start.elapsed();
        debug!("Batch compression completed in {} ms", duration.as_millis());
    }
}

impl BatchLike for Batch {
    fn l2_blocks_mut(&mut self) -> &mut Vec<L2Block> {
        &mut self.l2_blocks
    }

    fn l2_blocks(&self) -> &Vec<L2Block> {
        &self.l2_blocks
    }

    fn total_bytes_mut(&mut self) -> &mut u64 {
        &mut self.total_bytes
    }

    fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn anchor_block_id(&self) -> u64 {
        self.anchor_block_id
    }

    fn anchor_block_timestamp_sec(&self) -> u64 {
        self.anchor_block_timestamp_sec
    }

    fn compress(&mut self) {
        Batch::compress(self);
    }
}

// add test
#[cfg(test)]
mod tests {
    use crate::shared;

    use super::*;

    #[test]
    fn test_compress() {
        let mut batch = Batch {
            l2_blocks: vec![], //Vec<L2Block>,
            total_bytes: 10,
            coinbase: Address::ZERO,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
        };

        let json_data = r#"
        {
            "blockHash":"0x347bf1fbeab30fb516012c512222e229dfded991a2f1ba469f31c4273eb18921",
            "blockNumber":"0x5",
            "from":"0x0000777735367b36bc9b61c50022d9d0700db4ec",
            "gas":"0xf4240",
            "gasPrice":"0x86ff51",
            "maxFeePerGas":"0x86ff51",
            "maxPriorityFeePerGas":"0x0",
            "hash":"0xc921473ec8d6e93a9e499f4a5c7619fa9cc6ea8f24c89ad338f6c4095347af5c",
            "input":"0x48080a450000000000000000000000000000000000000000000000000000000000000146ef85e2f713b8212f4ff858962a5a5a0a1193b4033d702301cf5b68e29c7bffe6000000000000000000000000000000000000000000000000000000000001d28e0000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000004b00000000000000000000000000000000000000000000000000000000004c4b40000000000000000000000000000000000000000000000000000000004fdec7000000000000000000000000000000000000000000000000000000000023c3460000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000000",
            "nonce":"0x4",
            "to":"0x1670010000000000000000000000000000010001",
            "transactionIndex":"0x0",
            "value":"0x0",
            "type":"0x2",
            "accessList":[],
            "chainId":"0x28c59",
            "v":"0x0",
            "r":"0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            "s":"0xa8c3e2979dec89d4c055ffc1c900d33731cb43f027e427dff52a6ddf1247ec5",
            "yParity":"0x0"
        }"#;

        let tx: alloy::rpc::types::Transaction = serde_json::from_str(json_data).unwrap();
        let l2_block = L2Block {
            prebuilt_tx_list: shared::l2_tx_lists::PreBuiltTxList {
                tx_list: vec![tx.clone(), tx.clone(), tx],
                estimated_gas_used: 0,
                bytes_length: 0,
            },
            timestamp_sec: 0,
        };
        batch.l2_blocks.push(l2_block);

        batch.compress();

        assert_eq!(batch.total_bytes, 210);
    }
}
