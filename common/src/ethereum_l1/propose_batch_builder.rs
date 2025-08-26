use super::forced_inclusion_info::ForcedInclusionInfo;
use super::{l1_contracts_bindings::*, tools, transaction_error::TransactionError};
use alloy::{
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, Bytes, FixedBytes},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::SolValue,
};
use alloy_json_rpc::RpcError;
use anyhow::{Error, anyhow};
use tracing::warn;

struct FeesPerGas {
    base_fee_per_gas: u128,
    base_fee_per_blob_gas: u128,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
}

pub struct ProposeBatchBuilder {
    provider_ws: DynProvider,
    extra_gas_percentage: u64,
}

impl ProposeBatchBuilder {
    pub fn new(provider_ws: DynProvider, extra_gas_percentage: u64) -> Self {
        Self {
            provider_ws,
            extra_gas_percentage,
        }
    }

    /// Builds a proposeBatch transaction, choosing between eip1559 and eip4844 based on gas cost.
    ///
    /// # Arguments
    ///
    /// * `from`: The address of the proposer.
    /// * `to`: The address of the Taiko L1 contract.
    /// * `tx_list`: The list of preconfirmed L2 transactions.
    /// * `blocks`: The list of block params.
    /// * `last_anchor_origin_height`: The last anchor origin height.
    /// * `last_block_timestamp`: The last block timestamp.
    ///
    /// # Returns
    ///
    /// A `TransactionRequest` representing the proposeBatch transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_batch_tx(
        &self,
        from: Address,
        to: Address,
        tx_list: Vec<u8>,
        blocks: Vec<BlockParams>,
        last_anchor_origin_height: u64,
        last_block_timestamp: u64,
        coinbase: Address,
        forced_inclusion: Option<BatchParams>,
    ) -> Result<TransactionRequest, Error> {
        // Build eip4844 transaction
        let tx_blob = self
            .build_propose_batch_blob(
                from,
                to,
                &tx_list,
                blocks.clone(),
                last_anchor_origin_height,
                last_block_timestamp,
                coinbase,
                &forced_inclusion,
            )
            .await?;
        let tx_blob_gas = match self.provider_ws.estimate_gas(tx_blob.clone()).await {
            Ok(gas) => gas,
            Err(e) => {
                warn!(
                    "Build proposeBatch: Failed to estimate gas for blob transaction: {}",
                    e
                );
                match e {
                    RpcError::ErrorResp(err) => {
                        return Err(anyhow!(
                            tools::convert_error_payload(&err.to_string())
                                .unwrap_or(TransactionError::EstimationFailed)
                        ));
                    }
                    _ => return Ok(tx_blob),
                }
            }
        };
        let tx_blob_gas = tx_blob_gas + tx_blob_gas * self.extra_gas_percentage / 100;

        // Get fees from the network
        let fees_per_gas = match self.get_fees_per_gas().await {
            Ok(fees_per_gas) => fees_per_gas,
            Err(e) => {
                warn!("Build proposeBatch: Failed to get fees per gas: {}", e);
                // In case of error return eip4844 transaction
                return Ok(tx_blob);
            }
        };

        // Get blob count
        let blob_count = tx_blob
            .sidecar
            .as_ref()
            .map_or(0, |sidecar| sidecar.blobs.len() as u64);

        // Calculate the cost of the eip4844 transaction
        let eip4844_cost = self
            .get_eip4844_cost(&fees_per_gas, blob_count, tx_blob_gas)
            .await;

        // Update gas params for eip4844 transaction
        let tx_blob = self.update_eip4844(tx_blob, &fees_per_gas, tx_blob_gas);

        // Build eip1559 transaction
        let tx_calldata = self
            .build_propose_batch_calldata(
                from,
                to,
                tx_list,
                blocks.clone(),
                last_anchor_origin_height,
                last_block_timestamp,
                coinbase,
                &forced_inclusion,
            )
            .await?;
        let tx_calldata_gas = match self.provider_ws.estimate_gas(tx_calldata.clone()).await {
            Ok(gas) => gas,
            Err(e) => {
                warn!(
                    "Build proposeBatch: Failed to estimate gas for calldata transaction: {}",
                    e
                );
                match e {
                    RpcError::ErrorResp(err) => {
                        return Err(anyhow!(
                            tools::convert_error_payload(&err.to_string())
                                .unwrap_or(TransactionError::EstimationFailed)
                        ));
                    }
                    _ => return Ok(tx_blob), // In case of error return eip4844 transaction
                }
            }
        };
        let tx_calldata_gas = tx_calldata_gas + tx_calldata_gas * self.extra_gas_percentage / 100;

        tracing::debug!(
            "Build proposeBatch: eip1559 gas: {} eip4844 gas: {}",
            tx_calldata_gas,
            tx_blob_gas
        );

        // If no gas estimate, return error
        if tx_calldata_gas == 0 && tx_blob_gas == 0 {
            return Err(anyhow::anyhow!(
                "Build proposeBatch: Failed to estimate gas for both transaction types"
            ));
        }

        // Calculate the cost of the transaction
        let eip1559_cost = self.get_eip1559_cost(&fees_per_gas, tx_calldata_gas).await;

        tracing::debug!(
            "Build proposeBatch: eip1559_cost: {} eip4844_cost: {}",
            eip1559_cost,
            eip4844_cost
        );

        // If eip4844 cost is less than eip1559 cost, use eip4844
        if eip4844_cost < eip1559_cost {
            Ok(tx_blob)
        } else {
            Ok(self.update_eip1559(tx_calldata, &fees_per_gas, tx_calldata_gas))
        }
    }

    fn update_eip1559(
        &self,
        tx: TransactionRequest,
        fees_per_gas: &FeesPerGas,
        gas_limit: u64,
    ) -> TransactionRequest {
        tx.with_gas_limit(gas_limit)
            .with_max_fee_per_gas(fees_per_gas.max_fee_per_gas)
            .with_max_priority_fee_per_gas(fees_per_gas.max_priority_fee_per_gas)
    }

    fn update_eip4844(
        &self,
        tx: TransactionRequest,
        fees_per_gas: &FeesPerGas,
        gas_limit: u64,
    ) -> TransactionRequest {
        tx.with_gas_limit(gas_limit)
            .with_max_fee_per_gas(fees_per_gas.max_fee_per_gas)
            .with_max_priority_fee_per_gas(fees_per_gas.max_priority_fee_per_gas)
            .with_max_fee_per_blob_gas(fees_per_gas.base_fee_per_blob_gas)
    }

    async fn get_eip1559_cost(&self, fees_per_gas: &FeesPerGas, gas_used: u64) -> u128 {
        (fees_per_gas.base_fee_per_gas + fees_per_gas.max_priority_fee_per_gas)
            * u128::from(gas_used)
    }

    async fn get_eip4844_cost(
        &self,
        fees_per_gas: &FeesPerGas,
        blob_count: u64,
        gas_used: u64,
    ) -> u128 {
        let blob_gas_used = alloy::eips::eip4844::DATA_GAS_PER_BLOB * blob_count;
        let execution_gas_cost = u128::from(gas_used)
            * (fees_per_gas.base_fee_per_gas + fees_per_gas.max_priority_fee_per_gas);
        let blob_gas_cost = u128::from(blob_gas_used) * fees_per_gas.base_fee_per_blob_gas;
        execution_gas_cost + blob_gas_cost
    }

    async fn get_fees_per_gas(&self) -> Result<FeesPerGas, Error> {
        // Get base fee per gas
        let fee_history = self
            .provider_ws
            .get_fee_history(2, alloy::eips::BlockNumberOrTag::Latest, &[])
            .await?;

        let base_fee_per_gas = fee_history
            .base_fee_per_gas
            .last()
            .copied()
            .ok_or_else(|| anyhow::Error::msg("Failed to get base_fee_per_gas from fee history"))?;

        let base_fee_per_blob_gas = fee_history
            .base_fee_per_blob_gas
            .last()
            .copied()
            .ok_or_else(|| {
                anyhow::Error::msg("Failed to get base_fee_per_blob_gas from fee history")
            })?;

        let eip1559_estimation = self.provider_ws.estimate_eip1559_fees().await?;

        tracing::info!(
            ">max_fee_per_gas: {} base fee + priority fee: {}",
            eip1559_estimation.max_fee_per_gas,
            base_fee_per_gas + eip1559_estimation.max_priority_fee_per_gas
        );

        Ok(FeesPerGas {
            base_fee_per_gas,
            base_fee_per_blob_gas,
            max_fee_per_gas: eip1559_estimation.max_fee_per_gas,
            max_priority_fee_per_gas: eip1559_estimation.max_priority_fee_per_gas,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_propose_batch_calldata(
        &self,
        from: Address,
        to: Address,
        tx_list: Vec<u8>,
        blocks: Vec<BlockParams>,
        last_anchor_origin_height: u64,
        last_block_timestamp: u64,
        coinbase: Address,
        forced_inclusion: &Option<BatchParams>,
    ) -> Result<TransactionRequest, Error> {
        let tx_list_len = u32::try_from(tx_list.len())?;
        let tx_list = Bytes::from(tx_list);

        let bytes_x = if let Some(forced_inclusion) = forced_inclusion {
            Bytes::from(BatchParams::abi_encode(forced_inclusion))
        } else {
            Bytes::new()
        };

        let batch_params = BatchParams {
            proposer: from,
            coinbase,
            parentMetaHash: FixedBytes::from(&[0u8; 32]),
            anchorBlockId: last_anchor_origin_height,
            lastBlockTimestamp: last_block_timestamp,
            revertIfNotFirstProposal: false,
            blobParams: BlobParams {
                blobHashes: vec![],
                firstBlobIndex: 0,
                numBlobs: 0,
                byteOffset: 0,
                byteSize: tx_list_len,
                createdIn: 0,
            },
            blocks,
        };

        let encoded_batch_params = Bytes::from(BatchParams::abi_encode(&batch_params));

        let propose_batch_wrapper = ProposeBatchWrapper {
            bytesX: bytes_x,
            bytesY: encoded_batch_params,
        };

        let encoded_propose_batch_wrapper = Bytes::from(ProposeBatchWrapper::abi_encode_sequence(
            &propose_batch_wrapper,
        ));

        let tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_call(&PreconfRouter::proposeBatchCall {
                _params: encoded_propose_batch_wrapper,
                _txList: tx_list,
            });

        Ok(tx)
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_propose_batch_blob(
        &self,
        from: Address,
        to: Address,
        tx_list: &[u8],
        blocks: Vec<BlockParams>,
        last_anchor_origin_height: u64,
        last_block_timestamp: u64,
        coinbase: Address,
        forced_inclusion: &Option<BatchParams>,
    ) -> Result<TransactionRequest, Error> {
        let tx_list_len = u32::try_from(tx_list.len())?;

        let bytes_x = if let Some(forced_inclusion) = forced_inclusion {
            Bytes::from(BatchParams::abi_encode(forced_inclusion))
        } else {
            Bytes::new()
        };

        // Build sidecar
        let sidecar = crate::blob::build_blob_sidecar(tx_list)?;
        let num_blobs = u8::try_from(sidecar.blobs.len())?;

        let batch_params = BatchParams {
            proposer: from,
            coinbase,
            parentMetaHash: FixedBytes::from(&[0u8; 32]),
            anchorBlockId: last_anchor_origin_height,
            lastBlockTimestamp: last_block_timestamp,
            revertIfNotFirstProposal: false,
            blobParams: BlobParams {
                blobHashes: vec![],
                firstBlobIndex: 0,
                numBlobs: num_blobs,
                byteOffset: 0,
                byteSize: tx_list_len,
                createdIn: 0,
            },
            blocks,
        };

        let encoded_batch_params = Bytes::from(BatchParams::abi_encode(&batch_params));

        let propose_batch_wrapper = ProposeBatchWrapper {
            bytesX: bytes_x,
            bytesY: encoded_batch_params,
        };

        let encoded_propose_batch_wrapper = Bytes::from(ProposeBatchWrapper::abi_encode_sequence(
            &propose_batch_wrapper,
        ));

        let tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_blob_sidecar(sidecar)
            .with_call(&PreconfRouter::proposeBatchCall {
                _params: encoded_propose_batch_wrapper,
                _txList: Bytes::new(),
            });

        Ok(tx)
    }

    pub fn build_forced_inclusion_batch(
        proposer: Address,
        coinbase: Address,
        last_anchor_origin_height: u64,
        last_l2_block_timestamp: u64,
        info: &ForcedInclusionInfo,
    ) -> BatchParams {
        BatchParams {
            proposer,
            coinbase,
            parentMetaHash: FixedBytes::from(&[0u8; 32]),
            anchorBlockId: last_anchor_origin_height,
            lastBlockTimestamp: last_l2_block_timestamp,
            revertIfNotFirstProposal: false,
            blobParams: BlobParams {
                blobHashes: vec![info.blob_hash],
                firstBlobIndex: 0,
                numBlobs: 0,
                byteOffset: info.blob_byte_offset,
                byteSize: info.blob_byte_size,
                createdIn: info.created_in,
            },
            blocks: vec![BlockParams {
                numTransactions: 4096, // TaikoWrapper.MIN_TXS_PER_FORCED_INCLUSION
                timeShift: 0,
                signalSlots: vec![],
            }],
        }
    }
}
