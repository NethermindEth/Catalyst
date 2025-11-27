use crate::l2::bindings::BondManager;

use crate::node::proposal_manager::proposal::BondInstructionData;
use alloy::{
    consensus::Transaction as AnchorTransaction,
    consensus::{SignableTransaction, TxEnvelope, transaction::Recovered},
    primitives::{Address, B256, Bytes},
    providers::{DynProvider, Provider},
    rpc::types::Transaction,
    signers::Signature,
};
use anyhow::Error;
use common::shared::{
    alloy_tools, execution_layer::ExecutionLayer as ExecutionLayerCommon,
    l2_slot_info_v2::L2SlotInfoV2,
};
use common::{
    crypto::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY},
    l1::traits::PreconferBondProvider,
};
use pacaya::l2::config::TaikoConfig;
use taiko_bindings::anchor::Anchor;
use tracing::{debug, info, warn};

use serde_json::Value;
pub struct L2ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    shasta_anchor: Anchor::AnchorInstance<DynProvider>,
    bond_manager: Address,
    chain_id: u64,
    pub config: TaikoConfig,
}

impl L2ExecutionLayer {
    pub async fn new(taiko_config: TaikoConfig) -> Result<Self, Error> {
        let provider =
            alloy_tools::create_alloy_provider_without_wallet(&taiko_config.taiko_geth_url).await?;

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get chain ID: {}", e))?;
        info!("L2 Chain ID: {}", chain_id);

        let shasta_anchor = Anchor::new(taiko_config.taiko_anchor_address, provider.clone());

        let common = ExecutionLayerCommon::new(provider.clone()).await?;

        let bond_manager = shasta_anchor.bondManager().call().await.map_err(|e| {
            anyhow::anyhow!("Failed to get BondManager address from TaikoAnchor: {e}")
        })?;

        info!("Bond manager address: {}", bond_manager);

        Ok(Self {
            common,
            provider,
            shasta_anchor,
            bond_manager,
            chain_id,
            config: taiko_config,
        })
    }

    pub fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }

    pub async fn construct_anchor_tx(
        &self,
        proposal_id: u64,
        l2_slot_info: &L2SlotInfoV2,
        anchor_block_params: Anchor::BlockParams,
        bond_instructions: BondInstructionData,
    ) -> Result<Transaction, Error> {
        debug!(
            "Constructing anchor transaction for block number: {}",
            l2_slot_info.parent_id() + 1
        );
        let nonce = self
            .provider
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id((*l2_slot_info.parent_hash()).into())
            .await
            .map_err(|e| {
                self.common
                    .chain_error("Failed to get transaction count", Some(&e.to_string()))
            })?;

        let call_builder = self
            .shasta_anchor
            .anchorV4(
                Anchor::ProposalParams {
                    proposalId: proposal_id.try_into()?,
                    proposer: self.config.signer.get_address(),
                    proverAuth: Bytes::new(), // no prover designation for now
                    bondInstructionsHash: bond_instructions.hash(),
                    bondInstructions: bond_instructions.instructions_mut(),
                },
                anchor_block_params,
            )
            .gas(1_000_000) // value expected by Taiko
            .max_fee_per_gas(u128::from(l2_slot_info.base_fee())) // value expected by Taiko
            .max_priority_fee_per_gas(0) // value expected by Taiko
            .nonce(nonce)
            .chain_id(self.chain_id);

        let typed_tx = call_builder
            .into_transaction_request()
            .build_typed_tx()
            .map_err(|_| anyhow::anyhow!("AnchorTX: Failed to build typed transaction"))?;

        let tx_eip1559 = typed_tx
            .eip1559()
            .ok_or_else(|| anyhow::anyhow!("AnchorTX: Failed to extract EIP-1559 transaction"))?;

        let signature = self.sign_hash_deterministic(tx_eip1559.signature_hash())?;
        let sig_tx = tx_eip1559.clone().into_signed(signature);

        let tx_envelope = TxEnvelope::from(sig_tx);

        debug!("AnchorTX transaction hash: {}", tx_envelope.tx_hash());

        let tx = Transaction {
            inner: Recovered::new_unchecked(tx_envelope, GOLDEN_TOUCH_ADDRESS),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            effective_gas_price: None,
        };
        Ok(tx)
    }

    fn sign_hash_deterministic(&self, hash: B256) -> Result<Signature, Error> {
        common::crypto::fixed_k_signer::sign_hash_deterministic(GOLDEN_TOUCH_PRIVATE_KEY, hash)
    }

    async fn get_preconfer_deposited_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let contract = BondManager::new(self.bond_manager, &self.provider);
        let bonds = contract
            .bond(self.config.signer.get_address())
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get bonds balance: {e}")))?;
        Ok(bonds.balance)
    }

    pub async fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        dest_chain_id: u64,
        preconfer_address: Address,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        info!(
            "Transfer ETH from L2 to L1: srcChainId: {}, dstChainId: {}",
            self.chain_id, dest_chain_id
        );

        let provider =
            alloy_tools::construct_alloy_provider(&self.config.signer, &self.config.taiko_geth_url)
                .await?;

        self.transfer_eth_from_l2_to_l1_with_provider(
            provider,
            amount,
            dest_chain_id,
            preconfer_address,
            bridge_relayer_fee,
        )
        .await?;

        Ok(())
    }

    async fn transfer_eth_from_l2_to_l1_with_provider(
        &self,
        _provider: DynProvider,
        _amount: u128,
        _dest_chain_id: u64,
        _preconfer_address: Address,
        _bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        // TODO: implement the actual transfer logic
        warn!("Implement bridge transfer logic here");
        Ok(())
    }

    pub async fn get_last_synced_proposal_id_from_geth(&self) -> Result<u64, Error> {
        self.get_latest_anchor_transaction_input()
            .await
            .map_err(|e| anyhow::anyhow!("get_last_synced_proposal_id_from_geth: {e}"))
            .and_then(|input| Self::decode_proposal_id_from_tx_data(&input))
    }

    pub fn decode_proposal_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        let tx_data =
            <Anchor::anchorV4Call as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode proposal id from tx data: {}", e))?;
        Ok(tx_data._proposalParams.proposalId.to::<u64>())
    }

    async fn get_latest_anchor_transaction_input(&self) -> Result<Vec<u8>, Error> {
        let block = self.common.get_latest_block_with_txs().await?;
        let anchor_tx = match block.transactions.as_transactions() {
            Some(txs) => txs.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "get_latest_anchor_transaction_input: Cannot get anchor transaction from block {}",
                    block.number()
                )
            })?,
            None => {
                return Err(anyhow::anyhow!(
                    "No transactions in L2 block {}",
                    block.number()
                ));
            }
        };

        Ok(anchor_tx.input().to_vec())
    }

    pub async fn get_last_synced_anchor_block_id_from_geth(&self) -> Result<u64, Error> {
        self.get_latest_anchor_transaction_input()
            .await
            .map_err(|e| anyhow::anyhow!("get_last_synced_anchor_block_id_from_geth: {e}"))
            .and_then(|input| Self::decode_anchor_id_from_tx_data(&input))
    }

    pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        let tx_data =
            <Anchor::anchorV4Call as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode anchor id from tx data: {}", e))?;
        Ok(tx_data._blockParams.anchorBlockNumber.to::<u64>())
    }

    pub fn get_anchor_tx_data(data: &[u8]) -> Result<Anchor::anchorV4Call, Error> {
        let tx_data =
            <Anchor::anchorV4Call as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode anchor tx data: {}", e))?;
        Ok(tx_data)
    }

    pub async fn get_head_l1_origin(&self) -> Result<u64, Error> {
        let response = self
            .provider
            .raw_request::<_, Value>(std::borrow::Cow::Borrowed("taiko_headL1Origin"), ())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch taiko_headL1Origin: {}", e))?;

        let hex_str = response
            .get("blockID")
            .or_else(|| response.get("blockId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("Missing or invalid  block id in taiko_headL1Origin response, allowed keys are: blockID, blockId")
            })?;

        u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|e| anyhow::anyhow!("Failed to parse 'blockID' as u64: {}", e))
    }

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        self.provider
            .raw_request::<_, Value>(
                std::borrow::Cow::Borrowed("taiko_l1OriginByID"),
                vec![Value::String(block_id.to_string())],
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get forced inclusion: {}", e))?
            .get("isForcedInclusion")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse isForcedInclusion"))
    }

    pub async fn get_last_synced_block_params_from_geth(
        &self,
    ) -> Result<Anchor::BlockParams, Error> {
        self.get_latest_anchor_transaction_input()
            .await
            .map_err(|e| anyhow::anyhow!("get_last_synced_proposal_id_from_geth: {e}"))
            .and_then(|input| Self::decode_block_params_from_tx_data(&input))
    }

    pub fn decode_block_params_from_tx_data(data: &[u8]) -> Result<Anchor::BlockParams, Error> {
        let tx_data =
            <Anchor::anchorV4Call as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode proposal id from tx data: {}", e))?;
        Ok(tx_data._blockParams)
    }
}

impl PreconferBondProvider for L2ExecutionLayer {
    async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        // Check TAIKO TOKEN balance
        let bond_balance = self
            .get_preconfer_deposited_bonds()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch bond balance: {e}")))?;

        Ok(bond_balance)
    }
}
