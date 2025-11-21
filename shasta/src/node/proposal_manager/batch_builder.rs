use std::{collections::VecDeque, sync::Arc};

use super::proposal::Proposals;
use crate::{
    l1::execution_layer::ExecutionLayer,
    metrics::Metrics,
    node::proposal_manager::proposal::{BondInstructionData, Proposal},
    shared::{l2_block::L2Block, l2_tx_lists::PreBuiltTxList},
};
use alloy::primitives::Address;
use anyhow::Error;
use common::{
    batch_builder::{BatchBuilderConfig, BatchBuilderCore, BatchLike},
    l1::{ethereum_l1::EthereumL1, slot_clock::SlotClock, transaction_error::TransactionError},
    shared::anchor_block_info::AnchorBlockInfo,
};
use tracing::{debug, trace};

pub struct BatchBuilder {
    core: BatchBuilderCore<Proposal, ()>,
}

impl BatchBuilder {
    pub fn new(
        config: BatchBuilderConfig,
        slot_clock: Arc<SlotClock>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            core: BatchBuilderCore::new(None, config, slot_clock, metrics),
        }
    }

    pub fn get_config(&self) -> &BatchBuilderConfig {
        &self.core.config
    }

    pub fn can_consume_l2_block(&mut self, l2_block: &L2Block) -> bool {
        self.core.can_consume_l2_block(l2_block)
    }

    pub fn current_proposal_is_empty(&self) -> bool {
        self.core
            .current_batch
            .as_ref()
            .is_none_or(|b| b.l2_blocks().is_empty())
    }

    pub fn create_new_batch(
        &mut self,
        id: u64,
        anchor_block: AnchorBlockInfo,
        bond_instructions: BondInstructionData,
    ) {
        self.core.finalize_current_batch();

        self.core.current_batch = Some(Proposal {
            id,
            l2_blocks: vec![],
            total_bytes: 0,
            coinbase: self.core.config.default_coinbase,
            anchor_block_id: anchor_block.id(),
            anchor_block_timestamp_sec: anchor_block.timestamp_sec(),
            anchor_block_hash: anchor_block.hash(),
            anchor_state_root: anchor_block.state_root(),
            bond_instructions,
            num_forced_inclusion: 0,
        });
    }

    pub fn remove_current_batch(&mut self) {
        self.core.current_batch = None;
    }

    pub fn add_l2_block_and_get_current_proposal(
        &mut self,
        l2_block: L2Block,
    ) -> Result<&Proposal, Error> {
        self.core.add_l2_block(l2_block)?;
        self.core
            .current_batch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No current proposal after adding L2 block"))
    }

    pub fn remove_last_l2_block(&mut self) {
        self.core.remove_last_l2_block();
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn recover_from(
        &mut self,
        proposal_id: u64,
        anchor_info: AnchorBlockInfo,
        coinbase: Address,
        bond_instructions: BondInstructionData,
        tx_list: Vec<alloy::rpc::types::Transaction>,
        l2_block_timestamp_sec: u64,
        is_forced_inclusion: bool,
    ) -> Result<(), Error> {
        // We have a new proposal when proposal ID differs
        // Otherwise we continue with the current proposal
        if !self.is_same_proposal_id(proposal_id) {
            self.core.finalize_current_batch();
            debug!(
                "Creating new proposal during recovery: proposal_id {}, anchor_block_id {} coinbase {}",
                proposal_id,
                anchor_info.id(),
                coinbase
            );
            self.core.current_batch = Some(Proposal {
                id: proposal_id,
                total_bytes: 0,
                l2_blocks: vec![],
                coinbase,
                anchor_block_id: anchor_info.id(),
                anchor_block_timestamp_sec: anchor_info.timestamp_sec(),
                anchor_block_hash: anchor_info.hash(),
                anchor_state_root: anchor_info.state_root(),
                bond_instructions,
                num_forced_inclusion: 0,
            });
        }

        if is_forced_inclusion {
            if let Some(batch) = self.core.current_batch.as_ref()
                && !batch.l2_blocks.is_empty()
            {
                return Err(anyhow::anyhow!(
                    "recover_from: Cannot add forced inclusion L2 block to non-empty proposal"
                ));
            }

            self.inc_forced_inclusion()?;
        } else {
            if let Some(batch) = self.core.current_batch.as_mut()
                && batch.anchor_block_id < anchor_info.id()
            {
                batch.anchor_block_id = anchor_info.id();
                batch.anchor_block_timestamp_sec = anchor_info.timestamp_sec();
                batch.anchor_block_hash = anchor_info.hash();
                batch.anchor_state_root = anchor_info.state_root();
            }

            let bytes_length =
                crate::shared::l2_tx_lists::encode_and_compress(&tx_list)?.len() as u64;
            let l2_block = L2Block::new_from(
                crate::shared::l2_tx_lists::PreBuiltTxList {
                    tx_list,
                    estimated_gas_used: 0,
                    bytes_length,
                },
                l2_block_timestamp_sec,
            );

            // TODO we add block to the current proposal.
            // But we should verify that it fit N blob data size
            // Otherwise we should do a reorg
            // TODO align on blob count with all teams
            self.add_l2_block_and_get_current_proposal(l2_block)?;
        }
        Ok(())
    }

    fn is_same_proposal_id(&self, proposal_id: u64) -> bool {
        // Note: proposal.id is not part of BatchLike trait, so we need to access it directly
        // Since Proposal has a public id field, we can access it
        self.core
            .current_batch
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "batch_builder::is_empty: current_proposal is none: {}, proposals_to_send len: {}",
            self.core.current_batch.is_none(),
            self.core.batches_to_send.len()
        );
        self.core.is_empty()
    }

    pub async fn try_submit_oldest_batch(
        &mut self,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        if self.core.current_batch.is_some()
            && (!submit_only_full_batches
                || !self.core.config.is_within_block_limit(
                    u16::try_from(
                        self.core
                            .current_batch
                            .as_ref()
                            .map(|b| b.l2_blocks().len())
                            .unwrap_or(0),
                    )? + 1,
                ))
        {
            self.core.finalize_current_batch();
        }

        if let Some(batch) = self.core.batches_to_send.front().map(|(_, batch)| batch) {
            if ethereum_l1
                .execution_layer
                .transaction_monitor
                .is_transaction_in_progress()
                .await?
            {
                debug!(
                    proposals_to_send = %self.core.batches_to_send.len(),
                    current_proposal = %self.core.current_batch.is_some(),
                    "Cannot submit batch, transaction is in progress.",
                );
                return Err(anyhow::anyhow!(
                    "Cannot submit batch, transaction is in progress."
                ));
            }

            debug!(
                anchor_block_id = %batch.anchor_block_id(),
                coinbase = %batch.coinbase,
                l2_blocks_len = %batch.l2_blocks().len(),
                total_bytes = %batch.total_bytes(),
                proposals_to_send = %self.core.batches_to_send.len(),
                current_proposal = %self.core.current_batch.is_some(),
                "Submitting batch"
            );

            if let Err(err) = ethereum_l1
                .execution_layer
                // TODO send a Proosal to function
                .send_batch_to_l1(
                    batch.l2_blocks().clone(),
                    batch.anchor_block_id(),
                    batch.coinbase,
                    batch.num_forced_inclusion,
                )
                .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>()
                    && !matches!(transaction_error, TransactionError::EstimationTooEarly)
                {
                    debug!("BatchBuilder: Transaction error, removing all batches");
                    self.core.batches_to_send.clear();
                }
                return Err(err);
            }

            self.core.batches_to_send.pop_front();
        }

        Ok(())
    }

    // TODO do we have that check in SC?
    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        self.core.is_time_shift_expired(current_l2_slot_timestamp)
    }
    // TODO do we have that check in SC?
    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        self.core
            .is_time_shift_between_blocks_expiring(current_l2_slot_timestamp)
    }
    // TODO do we have that check in SC?
    fn is_the_last_l1_slot_to_add_an_empty_l2_block(
        &self,
        current_l2_slot_timestamp: u64,
        last_block_timestamp: u64,
    ) -> bool {
        self.core.is_the_last_l1_slot_to_add_an_empty_l2_block(
            current_l2_slot_timestamp,
            last_block_timestamp,
        )
    }

    pub fn is_greater_than_max_anchor_height_offset(&self) -> Result<bool, Error> {
        self.core.is_greater_than_max_anchor_height_offset()
    }

    fn is_empty_block_required(&self, preconfirmation_timestamp: u64) -> bool {
        self.core.is_empty_block_required(preconfirmation_timestamp)
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            core: self.core.clone_without_batches(),
        }
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.core.get_number_of_batches()
    }

    pub fn get_number_of_batches_ready_to_send(&self) -> u64 {
        self.core.batches_to_send.len() as u64
    }

    pub fn take_proposals_to_send(&mut self) -> VecDeque<Proposal> {
        std::mem::take(&mut self.core.batches_to_send)
            .into_iter()
            .map(|(_, batch)| batch)
            .collect()
    }

    /// Alias for `take_proposals_to_send` for compatibility
    pub fn take_batches_to_send(&mut self) -> VecDeque<Proposal> {
        self.take_proposals_to_send()
    }

    pub fn prepend_batches(&mut self, batches: Proposals) {
        let mut new_batches: VecDeque<(Option<()>, Proposal)> =
            batches.into_iter().map(|batch| (None, batch)).collect();
        new_batches.append(&mut self.core.batches_to_send);
        self.core.batches_to_send = new_batches;
    }

    pub fn get_current_proposal_id(&self) -> Option<u64> {
        self.core.current_batch.as_ref().map(|b| b.id)
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        // TODO handle forced inclusion
        self.core.finalize_current_batch();
        Ok(())
    }

    pub fn finalize_current_batch(&mut self) {
        self.core.finalize_current_batch();
    }

    pub fn try_creating_l2_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> Option<L2Block> {
        self.core
            .try_creating_l2_block(pending_tx_list, l2_slot_timestamp, end_of_sequencing)
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        let proposal = self.core.current_batch();
        proposal.is_some_and(|p| p.num_forced_inclusion > 0)
    }

    pub fn inc_forced_inclusion(&mut self) -> Result<(), Error> {
        if let Some(proposal) = self.core.current_batch.as_mut() {
            proposal.num_forced_inclusion += 1;
        } else {
            return Err(anyhow::anyhow!(
                "No current batch to add forced inclusion to"
            ));
        }
        Ok(())
    }

    pub fn get_current_proposal(&self) -> Option<&Proposal> {
        self.core.current_batch.as_ref()
    }
}
