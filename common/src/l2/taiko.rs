use super::{
    bindings::TaikoAnchor::BaseFeeConfig,
    config::TaikoConfig,
    execution_layer::L2ExecutionLayer,
    operation_type::OperationType,
    preconf_blocks::{self, BuildPreconfBlockResponse},
};
use crate::{
    l1::{el_trait::ELTrait, ethereum_l1::EthereumL1},
    metrics::Metrics,
    shared::{
        l2_block::L2Block,
        l2_slot_info::L2SlotInfo,
        l2_tx_lists::{self, PreBuiltTxList},
    },
    utils::rpc_client::{HttpRPCClient, JSONRPCClient},
};
use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
};
use anyhow::Error;
use serde_json::Value;
use std::{
    cmp::{max, min},
    sync::Arc,
    time::Duration,
};
use tracing::{debug, trace};

pub struct Taiko<ELE: ELTrait> {
    l2_execution_layer: L2ExecutionLayer,
    taiko_geth_auth_rpc: JSONRPCClient,
    driver_preconf_rpc: HttpRPCClient,
    driver_status_rpc: HttpRPCClient,
    ethereum_l1: Arc<EthereumL1<ELE>>,
    metrics: Arc<Metrics>,
    config: TaikoConfig,
}

impl<ELE: ELTrait> Taiko<ELE> {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ethereum_l1: Arc<EthereumL1<ELE>>,
        metrics: Arc<Metrics>,
        taiko_config: TaikoConfig,
    ) -> Result<Self, Error> {
        Ok(Self {
            l2_execution_layer: L2ExecutionLayer::new(taiko_config.clone())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?,
            taiko_geth_auth_rpc: JSONRPCClient::new_with_timeout_and_jwt(
                &taiko_config.taiko_geth_auth_url,
                taiko_config.rpc_l2_execution_layer_timeout,
                &taiko_config.jwt_secret_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to create JSONRPCClient for taiko geth auth: {}", e)
            })?,
            driver_preconf_rpc: HttpRPCClient::new_with_jwt(
                &taiko_config.driver_url,
                taiko_config.rpc_driver_preconf_timeout,
                &taiko_config.jwt_secret_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to create HttpRPCClient for driver preconf: {}", e)
            })?,
            driver_status_rpc: HttpRPCClient::new_with_jwt(
                &taiko_config.driver_url,
                taiko_config.rpc_driver_status_timeout,
                &taiko_config.jwt_secret_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to create HttpRPCClient for driver status: {}", e)
            })?,
            ethereum_l1,
            metrics,
            config: taiko_config,
        })
    }

    pub async fn get_pending_l2_tx_list_from_taiko_geth(
        &self,
        base_fee: u64,
        batches_ready_to_send: u64,
    ) -> Result<Option<PreBuiltTxList>, Error> {
        let max_bytes_per_tx_list = calculate_max_bytes_per_tx_list(
            self.config.max_bytes_per_tx_list,
            self.config.throttling_factor,
            batches_ready_to_send,
            self.config.min_bytes_per_tx_list,
        );
        let params = vec![
            Value::String(format!(
                "0x{}",
                hex::encode(
                    self.ethereum_l1
                        .execution_layer
                        .common()
                        .get_preconfer_address()
                )
            )), // beneficiary address
            Value::from(base_fee), // baseFee
            Value::Number(
                self.ethereum_l1
                    .execution_layer
                    .common()
                    .get_block_max_gas_limit()
                    .into(),
            ), // blockMaxGasLimit
            Value::Number(max_bytes_per_tx_list.into()), // maxBytesPerTxList (128KB by default)
            Value::Array(vec![]),  // locals (empty array)
            Value::Number(1.into()), // maxTransactionsLists
            Value::Number(0.into()), // minTip
        ];

        let result = self
            .taiko_geth_auth_rpc
            .call_method("taikoAuth_txPoolContentWithMinTip", params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 tx lists: {}", e))?;
        if result != Value::Null {
            let mut tx_lists = l2_tx_lists::decompose_pending_lists_json_from_geth(result)
                .map_err(|e| anyhow::anyhow!("Failed to decompose L2 tx lists: {}", e))?;
            // ignoring rest of tx lists, only one list per L2 block is processed
            Ok(Some(tx_lists.remove(0)))
        } else {
            Ok(None)
        }
    }

    pub async fn get_balance(&self, address: Address) -> Result<alloy::primitives::U256, Error> {
        self.l2_execution_layer.get_balance(address).await
    }

    pub async fn get_latest_l2_block_id(&self) -> Result<u64, Error> {
        self.l2_execution_layer.get_latest_l2_block_id().await
    }

    pub async fn get_l2_block_by_number(
        &self,
        number: u64,
        full_txs: bool,
    ) -> Result<alloy::rpc::types::Block, Error> {
        self.l2_execution_layer
            .get_l2_block_by_number(number, full_txs)
            .await
    }

    pub async fn fetch_l2_blocks_until_latest(
        &self,
        start_block: u64,
        full_txs: bool,
    ) -> Result<Vec<alloy::rpc::types::Block>, Error> {
        let start_time = std::time::Instant::now();
        let end_block = self.get_latest_l2_block_id().await?;
        let mut blocks = Vec::with_capacity(usize::try_from(end_block - start_block + 1)?);
        for block_number in start_block..=end_block {
            let block = self.get_l2_block_by_number(block_number, full_txs).await?;
            blocks.push(block);
        }
        debug!(
            "Fetched L2 blocks from {} to {} in {} ms",
            start_block,
            end_block,
            start_time.elapsed().as_millis()
        );
        Ok(blocks)
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        self.l2_execution_layer.get_transaction_by_hash(hash).await
    }

    pub async fn get_l2_block_id_hash_and_gas_used(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<(u64, B256, u64), Error> {
        let block = self.l2_execution_layer.get_l2_block_header(block).await?;

        Ok((
            block.header.number(),
            block.header.hash,
            block.header.gas_used(),
        ))
    }

    pub async fn get_l2_block_hash(&self, number: u64) -> Result<B256, Error> {
        self.l2_execution_layer.get_l2_block_hash(number).await
    }

    pub async fn get_l2_slot_info(&self) -> Result<L2SlotInfo, Error> {
        self.get_l2_slot_info_by_parent_block(BlockNumberOrTag::Latest)
            .await
    }

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        self.l2_execution_layer
            .get_forced_inclusion_form_l1origin(block_id)
            .await
    }

    pub async fn get_l2_slot_info_by_parent_block(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<L2SlotInfo, Error> {
        let l2_slot_timestamp = self.ethereum_l1.slot_clock.get_l2_slot_begin_timestamp()?;
        let (parent_id, parent_hash, parent_gas_used) =
            self.get_l2_block_id_hash_and_gas_used(block).await?;

        // Safe conversion with overflow check
        let parent_gas_used_u32 = u32::try_from(parent_gas_used).map_err(|_| {
            anyhow::anyhow!("parent_gas_used {} exceeds u32 max value", parent_gas_used)
        })?;

        let base_fee_config = self.get_base_fee_config();

        let base_fee = self
            .get_base_fee(
                parent_hash,
                parent_gas_used_u32,
                base_fee_config,
                l2_slot_timestamp,
            )
            .await?;

        trace!(
            timestamp = %l2_slot_timestamp,
            parent_hash = %parent_hash,
            parent_gas_used = %parent_gas_used_u32,
            base_fee = %base_fee,
            "L2 slot info"
        );

        Ok(L2SlotInfo::new(
            base_fee,
            l2_slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_used_u32,
        ))
    }

    pub async fn advance_head_to_new_l2_block(
        &self,
        l2_block: L2Block,
        anchor_origin_height: u64,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        is_forced_inclusion: bool,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        tracing::debug!(
            "Submitting new L2 block to the Taiko driver with {} txs",
            l2_block.prebuilt_tx_list.tx_list.len()
        );

        let anchor_block_state_root = self
            .ethereum_l1
            .execution_layer
            .common()
            .get_block_state_root_by_number(anchor_origin_height)
            .await?;

        let base_fee_config = self.get_base_fee_config();
        let sharing_pctg = base_fee_config.sharingPctg;

        let anchor_tx = self
            .l2_execution_layer
            .construct_anchor_tx(
                l2_slot_info,
                anchor_origin_height,
                anchor_block_state_root,
                base_fee_config.clone(),
            )
            .await?;
        let tx_list = std::iter::once(anchor_tx)
            .chain(l2_block.prebuilt_tx_list.tx_list.into_iter())
            .collect::<Vec<_>>();

        let tx_list_bytes = l2_tx_lists::encode_and_compress(&tx_list)?;
        let extra_data = vec![sharing_pctg];

        let executable_data = preconf_blocks::ExecutableData {
            base_fee_per_gas: l2_slot_info.base_fee(),
            block_number: l2_slot_info.parent_id() + 1,
            extra_data: format!("0x{:0>64}", hex::encode(extra_data)),
            fee_recipient: format!(
                "0x{}",
                hex::encode(
                    self.ethereum_l1
                        .execution_layer
                        .common()
                        .get_preconfer_address()
                )
            ),
            gas_limit: 241_000_000u64,
            parent_hash: format!("0x{}", hex::encode(l2_slot_info.parent_hash())),
            timestamp: l2_block.timestamp_sec,
            transactions: format!("0x{}", hex::encode(tx_list_bytes)),
        };

        let request_body = preconf_blocks::BuildPreconfBlockRequestBody {
            executable_data,
            end_of_sequencing,
            is_forced_inclusion,
        };

        const API_ENDPOINT: &str = "preconfBlocks";

        let response = self
            .call_driver(
                &self.driver_preconf_rpc,
                http::Method::POST,
                API_ENDPOINT,
                &request_body,
                operation_type,
            )
            .await?;

        trace!("Response from preconfBlocks: {:?}", response);

        let preconfirmed_block = BuildPreconfBlockResponse::new_from_value(response);

        if preconfirmed_block.is_none() {
            tracing::error!("Block was preconfirmed, but failed to decode response from driver.");
        }

        self.metrics.inc_blocks_preconfirmed();

        Ok(preconfirmed_block)
    }

    pub async fn get_status(&self) -> Result<preconf_blocks::TaikoStatus, Error> {
        trace!("Get status form taiko driver");

        const API_ENDPOINT: &str = "status";
        let request_body = serde_json::json!({});

        let response = self
            .call_driver(
                &self.driver_status_rpc,
                http::Method::GET,
                API_ENDPOINT,
                &request_body,
                OperationType::Status,
            )
            .await?;

        trace!("Response from taiko status: {:?}", response);

        let status: preconf_blocks::TaikoStatus = serde_json::from_value(response)?;

        Ok(status)
    }

    async fn call_driver<T>(
        &self,
        client: &HttpRPCClient,
        method: http::Method,
        endpoint: &str,
        payload: &T,
        operation_type: OperationType,
    ) -> Result<Value, Error>
    where
        T: serde::Serialize,
    {
        let heartbeat_ms = self.ethereum_l1.slot_clock.get_preconf_heartbeat_ms();
        let max_duration = Duration::from_millis(heartbeat_ms / 2); // half of the heartbeat duration, leave time for other operations

        let metric_label = operation_type.to_string();
        self.metrics.inc_rpc_driver_call(&metric_label);
        let start_time = std::time::Instant::now();

        match client
            .retry_request_with_timeout(method, endpoint, payload, max_duration)
            .await
        {
            Ok(response) => {
                self.metrics.observe_rpc_driver_call_duration(
                    &metric_label,
                    start_time.elapsed().as_secs_f64(),
                );
                Ok(response)
            }
            Err(e) => {
                self.metrics.inc_rpc_driver_call_error(&metric_label);
                let metric_label_error = format!("{metric_label}-error");
                self.metrics.observe_rpc_driver_call_duration(
                    &metric_label_error,
                    start_time.elapsed().as_secs_f64(),
                );
                Err(e)
            }
        }
    }

    fn get_base_fee_config(&self) -> BaseFeeConfig {
        let config = self
            .ethereum_l1
            .execution_layer
            .common()
            .get_protocol_config();
        BaseFeeConfig {
            adjustmentQuotient: config.base_fee_config.adjustment_quotient,
            sharingPctg: config.base_fee_config.sharing_pctg,
            gasIssuancePerSecond: config.base_fee_config.gas_issuance_per_second,
            minGasExcess: config.base_fee_config.min_gas_excess,
            maxGasIssuancePerBlock: config.base_fee_config.max_gas_issuance_per_block,
        }
    }

    pub async fn get_base_fee(
        &self,
        parent_hash: B256,
        parent_gas_used: u32,
        base_fee_config: BaseFeeConfig,
        l2_slot_timestamp: u64,
    ) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_base_fee(
                parent_hash,
                parent_gas_used,
                base_fee_config,
                l2_slot_timestamp,
            )
            .await
    }

    pub async fn get_last_synced_anchor_block_id_from_taiko_anchor(&self) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_last_synced_anchor_block_id_from_taiko_anchor()
            .await
    }

    pub async fn get_last_synced_anchor_block_id_from_geth(&self) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_last_synced_anchor_block_id_from_geth()
            .await
    }

    pub async fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        self.l2_execution_layer
            .transfer_eth_from_l2_to_l1(
                amount,
                self.ethereum_l1.execution_layer.common().chain_id(),
                self.ethereum_l1
                    .execution_layer
                    .common()
                    .get_preconfer_alloy_address(),
                bridge_relayer_fee,
            )
            .await
    }
}

pub trait PreconfDriver {
    fn get_status(
        &self,
    ) -> impl std::future::Future<Output = Result<preconf_blocks::TaikoStatus, Error>> + Send;
}

impl<ELE: ELTrait> PreconfDriver for Taiko<ELE> {
    async fn get_status(&self) -> Result<preconf_blocks::TaikoStatus, Error> {
        Taiko::get_status(self).await
    }
}

pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
    L2ExecutionLayer::decode_anchor_id_from_tx_data(data)
}

/// Calculate the max bytes per tx list based on the number of batches ready to send.
/// The max bytes per tx list is reduced exponentially by given factor.
fn calculate_max_bytes_per_tx_list(
    max_bytes_per_tx_list: u64,
    throttling_factor: u64,
    batches_ready_to_send: u64,
    min_bytes_per_tx_list: u64,
) -> u64 {
    let mut size = max_bytes_per_tx_list;
    for _ in 0..batches_ready_to_send {
        size = size.saturating_sub(size / throttling_factor);
    }
    size = min(max_bytes_per_tx_list, max(size, min_bytes_per_tx_list));
    if batches_ready_to_send > 0 {
        debug!("Reducing max bytes per tx list to {}", size);
    }
    size
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_calculate_max_bytes_per_tx_list() {
        let max_bytes = 1000; // 128KB
        let throttling_factor = 10;
        let min_value = 100;

        // Test with no throttling (attempt = 0)
        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 0, min_value),
            max_bytes
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 1, min_value),
            900
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 2, min_value),
            810
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 3, min_value),
            729
        );

        // Test with throttling factor greater than max_bytes
        assert_eq!(calculate_max_bytes_per_tx_list(100, 200, 1, min_value), 100);

        // Test with zero max_bytes
        assert_eq!(
            calculate_max_bytes_per_tx_list(0, throttling_factor, 1, min_value),
            0
        );

        // Test with min_value
        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 500, min_value),
            min_value
        );
    }
}

// #[cfg(test)]
// mod test {
//     use super::*;
//     use crate::utils::rpc_server::test::RpcServer;
//     use std::net::SocketAddr;

//     #[tokio::test]
//     async fn test_get_pending_l2_tx_lists() {
//         let (mut rpc_server, taiko) = setup_rpc_server_and_taiko(3030).await;
//         let json = taiko
//             .get_pending_l2_tx_lists_from_taiko_geth()
//             .await
//             .unwrap();

//         assert_eq!(json.len(), 1);
//         assert_eq!(json[0].tx_list.len(), 2);
//         rpc_server.stop().await;
//     }

//     #[tokio::test]
//     async fn test_advance_head_to_new_l2_block() {
//         let (mut rpc_server, taiko) = setup_rpc_server_and_taiko(3040).await;
//         let value = serde_json::json!({
//             "TxLists": [
//                 [
//                     {
//                         "type": "0x0",
//                         "chainId": "0x28c61",
//                         "nonce": "0x1",
//                         "to": "0xbfadd5365bb2890ad832038837115e60b71f7cbb",
//                         "gas": "0x267ac",
//                         "gasPrice": "0x5e76e0800",
//                         "maxPriorityFeePerGas": null,
//                         "maxFeePerGas": null,
//                         "value": "0x0",
//                         "input": "0x40d097c30000000000000000000000004cea2c7d358e313f5d0287c475f9ae943fe1a913",
//                         "v": "0x518e6",
//                         "r": "0xb22da5cdc4c091ec85d2dda9054aa497088e55bd9f0335f39864ae1c598dd35",
//                         "s": "0x6eee1bcfe6a1855e89dd23d40942c90a036f273159b4c4fd217d58169493f055",
//                         "hash": "0x7c76b9906579e54df54fe77ad1706c47aca706b3eb5cfd8a30ccc3c5a19e8ecd"
//                     }
//                 ]
//             ]
//         });

//         let response = taiko.advance_head_to_new_l2_blocks(value).await.unwrap();
//         assert_eq!(
//             response["result"],
//             "Request received and processed successfully"
//         );
//         rpc_server.stop().await;
//     }

// async fn setup_rpc_server_and_taiko(port: u16) -> (RpcServer, Taiko) {
//     // Start the RPC server
//     let mut rpc_server = RpcServer::new();
//     let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
//     rpc_server.start_test_responses(addr).await.unwrap();

//     let taiko = Taiko::new(
//         &format!("ws://127.0.0.1:{}", port + 1),
//         &format!("http://127.0.0.1:{}", port),
//         &format!("http://127.0.0.1:{}", port + 2), // driver_url
//         1,
//         Duration::from_secs(10),
//         &[
//             0xa6, 0xea, 0x92, 0x58, 0xca, 0x91, 0x2c, 0x59, 0x3b, 0x3e, 0x36, 0xee, 0x36, 0xc1,
//             0x7f, 0xe9, 0x74, 0x47, 0xf9, 0x20, 0xf5, 0xb3, 0x6a, 0x90, 0x74, 0x4d, 0x79, 0xd4,
//             0xf2, 0xd6, 0xae, 0x62,
//         ],
//         PRECONFER_ADDRESS_ZERO,

//         "0x1670010000000000000000000000000000010001".to_string(),
//     )
//     .await
//     .unwrap();
//     (rpc_server, taiko)
// }
// }
