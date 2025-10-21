#![allow(dead_code)] // Remove once LookaheadBuilder is used by the node

use crate::monitor::db::DataBase as UrcDataBase;
use alloy::{
    hex,
    primitives::{Address, Bytes, FixedBytes, U256},
    providers::DynProvider,
};
use anyhow::{Error, anyhow};
use blst::min_pk::PublicKey;
use common::{
    l1::{consensus_layer::ConsensusLayer, slot_clock::SlotClock},
    utils::types::{Epoch, Slot},
};
use std::{str::FromStr, sync::Arc};
use tracing::info;

use crate::bindings::{
    BLS::G1Point,
    ILookaheadStore::{
        self, ILookaheadStoreInstance, LookaheadData, LookaheadSlot, ProposerContext,
    },
};

use super::types::Lookahead;

// Contains data that would be required to build the `LookaheadData` struct expected by the
// `getProposerContext(..)` view when fetching the next preconfer, or the `propose(..)` function
// when proposing a new batch.
#[derive(Clone)]
pub struct Context {
    // Last slot at which the context was updated
    context_updated_at_slot: Slot,
    // Last epoch when the lookaheads were updated
    lookahead_updated_at_slot: Epoch,
    current_lookahead: Lookahead,
    // Position of the lookahead of slot of the next preconfer
    current_lookahead_slot_index: U256,
    next_lookahead: Lookahead,
}

pub struct LookaheadBuilder {
    urc_db: UrcDataBase,
    slot_clock: Arc<SlotClock>,
    consensus_layer: Arc<ConsensusLayer>,
    lookahead_store_contract: ILookaheadStoreInstance<DynProvider>,
    preconf_slasher_address: Address,
    context: Context,
}

impl LookaheadBuilder {
    pub async fn new(
        provider: DynProvider,
        slot_clock: Arc<common::l1::slot_clock::SlotClock>,
        consensus_layer: Arc<common::l1::consensus_layer::ConsensusLayer>,
        urc_db: UrcDataBase,
        lookahead_store_address: Address,
        preconf_slasher_address: Address,
    ) -> Result<Self, Error> {
        let lookahead_store_contract = ILookaheadStore::new(lookahead_store_address, provider);

        let mut builder = Self {
            urc_db,
            slot_clock,
            consensus_layer,
            lookahead_store_contract,
            preconf_slasher_address,
            context: Context {
                context_updated_at_slot: 0,
                lookahead_updated_at_slot: 0,
                current_lookahead: vec![],
                current_lookahead_slot_index: U256::ZERO,
                next_lookahead: vec![],
            },
        };

        let current_epoch = builder.slot_clock.get_current_epoch()?;
        builder.context.current_lookahead = builder.build(current_epoch).await?;
        builder.context.next_lookahead = builder.build(current_epoch + 1).await?;
        builder.context.lookahead_updated_at_slot = current_epoch;

        Ok(builder)
    }

    pub async fn get_next_preconfer(&mut self) -> Result<ProposerContext, Error> {
        let (next_slot, lookahead_data) = self.get_lookahead_data().await?;

        // The epoch timestamp expected by the `getProposerContext(..)` function in the contract.
        // When we are at the boundary of an epoch, this is the starting timestamp of the next epoch.
        // Otherwise, it is the starting timestamp of the current epoch.
        let epoch = self.slot_clock.get_epoch_from_slot(next_slot);
        let epoch_timestamp = U256::from(self.slot_clock.get_epoch_begin_timestamp(epoch)?);

        let proposer_context = self
            .lookahead_store_contract
            .getProposerContext(lookahead_data, epoch_timestamp)
            .call()
            .await
            .map_err(|err| {
                anyhow!(
                    "Call to `LookaheadStore.getProposerContext failed: {}`",
                    err
                )
            })?;

        Ok(proposer_context)
    }

    pub async fn get_lookahead_data(&mut self) -> Result<(Slot, LookaheadData), Error> {
        let next_slot = self.update_context().await?;
        let lookahead_data = LookaheadData {
            slotIndex: self.context.current_lookahead_slot_index,
            registrationRoot: FixedBytes::from([0_u8; 32]),
            currLookahead: self.context.current_lookahead.clone(),
            nextLookahead: self.context.next_lookahead.clone(),
            commitmentSignature: Bytes::new(),
        };

        Ok((next_slot, lookahead_data))
    }

    async fn update_context(&mut self) -> Result<Slot, Error> {
        let current_epoch = self.slot_clock.get_current_epoch()?;
        let next_slot = self.slot_clock.get_current_slot()? + 1;

        if self.context.context_updated_at_slot == next_slot {
            return Ok(next_slot);
        } else {
            self.context.context_updated_at_slot = next_slot;
        }

        // Update the lookaheads if we have moved into a new epoch
        if current_epoch > self.context.lookahead_updated_at_slot {
            if current_epoch == self.context.lookahead_updated_at_slot + 1 {
                self.context.current_lookahead = std::mem::take(&mut self.context.next_lookahead);
            } else {
                self.context.current_lookahead = self.build(current_epoch).await?;
            }
            self.context.next_lookahead = self.build(current_epoch + 1).await?;
            self.context.current_lookahead_slot_index = U256::ZERO;
            self.context.lookahead_updated_at_slot = current_epoch;
        }

        if self.slot_clock.get_epoch_from_slot(next_slot) == current_epoch + 1 {
            // If we are the boundary of the current epoch, adjust the context to use
            // the preconfer of the first slot of the next epoch
            self.context.current_lookahead = std::mem::take(&mut self.context.next_lookahead);
            self.context.current_lookahead_slot_index = U256::ZERO;
        } else if !self.context.current_lookahead.is_empty() {
            // Use the next preconfer from the current epoch
            let mut slot_index: usize = self.context.current_lookahead_slot_index.try_into()?;
            let lookahead_slot_timestamp = self.context.current_lookahead[slot_index].timestamp;
            let next_slot_timestamp = U256::from(self.slot_clock.start_of(next_slot)?.as_secs());

            // If the timestamp range of the last used lookahead slot no longer covers the next
            // slot, we update `current_lookahead_slot_index`
            if next_slot_timestamp > lookahead_slot_timestamp {
                if slot_index == self.context.current_lookahead.len() - 1 {
                    self.context.current_lookahead_slot_index = U256::MAX;
                } else {
                    let mut prev_lookahead_slot_timestamp = if slot_index == 0 {
                        let current_epoch_timestamp =
                            U256::from(self.slot_clock.get_epoch_begin_timestamp(current_epoch)?);
                        let slot_duration =
                            U256::from(self.slot_clock.get_slot_duration().as_secs());
                        current_epoch_timestamp - slot_duration
                    } else {
                        lookahead_slot_timestamp
                    };
                    let mut next_lookahead_slot_timestamp =
                        self.context.current_lookahead[slot_index + 1].timestamp;

                    loop {
                        if next_slot_timestamp > prev_lookahead_slot_timestamp
                            && next_slot_timestamp <= next_lookahead_slot_timestamp
                        {
                            break;
                        } else {
                            prev_lookahead_slot_timestamp =
                                self.context.current_lookahead[slot_index].timestamp;
                            next_lookahead_slot_timestamp =
                                self.context.current_lookahead[slot_index + 1].timestamp;

                            slot_index += 1;
                        }
                    }

                    self.context.current_lookahead_slot_index = U256::from(slot_index);
                }
            }
        }

        Ok(next_slot)
    }

    async fn build(&self, epoch: u64) -> Result<Lookahead, Error> {
        let mut lookahead_slots: Lookahead = Vec::with_capacity(32);

        let epoch_timestamp = self.slot_clock.get_epoch_begin_timestamp(epoch)?;

        // Fetch all validator pubkeys for `epoch`
        let validators = self.consensus_layer.get_validators_for_epoch(epoch).await?;
        for (index, validator) in validators.iter().enumerate() {
            let pubkey_bytes = hex::decode(validator)?; // Compressed bytes
            let pubkey_g1 = Self::pubkey_bytes_to_g1_point(&pubkey_bytes)?;

            // Fetch all operators that have registered the validator
            let operators = self
                .urc_db
                .get_operators_by_pubkey(
                    self.preconf_slasher_address.to_string().as_str(),
                    (
                        pubkey_g1.x_a.to_string(),
                        pubkey_g1.x_b.to_string(),
                        pubkey_g1.y_a.to_string(),
                        pubkey_g1.y_b.to_string(),
                    ),
                )
                .await?;

            for operator in operators {
                let (registration_root, validator_leaf_index, committer) = operator;

                if self
                    .is_operator_valid(epoch_timestamp, &registration_root)
                    .await
                {
                    let slot_duration = self.slot_clock.get_slot_duration().as_secs();
                    let slot_timestamp = epoch_timestamp + ((index as u64) * slot_duration);

                    lookahead_slots.push(LookaheadSlot {
                        committer: Address::from_str(&committer)?,
                        timestamp: U256::from(slot_timestamp),
                        registrationRoot: FixedBytes::<32>::from_str(&registration_root)?,
                        validatorLeafIndex: U256::from(validator_leaf_index),
                    });

                    // We only include one valid operator that has registered the validator
                    break;
                }
            }
        }

        Ok(lookahead_slots)
    }

    async fn is_operator_valid(&self, epoch_timestamp: u64, registration_root: &str) -> bool {
        let registration_root = match FixedBytes::<32>::from_str(registration_root) {
            Ok(root) => root,
            Err(_) => {
                info!("is_operator_valid: registration_root parsing error");
                return false;
            }
        };
        return self
            .lookahead_store_contract
            .isLookaheadOperatorValid(U256::from(epoch_timestamp), registration_root)
            .call()
            .await
            .unwrap_or_else(|_| {
                info!("Call to `LookaheadStore.isLookaheadOperatorValid failed.`");
                false
            });
    }

    fn pubkey_bytes_to_g1_point(pubkey_bytes: &[u8]) -> Result<G1Point, Error> {
        let pubkey: PublicKey = PublicKey::from_bytes(pubkey_bytes)
            .map_err(|_| anyhow!("LookaheadBuilder: pubkey parsing error"))?;
        let serialized_bytes = pubkey.serialize(); // Uncompressed bytes

        Ok(G1Point {
            x_a: {
                let mut x_a = [0u8; 32];
                x_a[16..32].copy_from_slice(&serialized_bytes[0..16]);
                FixedBytes::from(x_a)
            },
            x_b: FixedBytes::from_slice(&serialized_bytes[16..48]),
            y_a: {
                let mut y_a = [0u8; 32];
                y_a[16..32].copy_from_slice(&serialized_bytes[48..64]);
                FixedBytes::from(y_a)
            },
            y_b: FixedBytes::from_slice(&serialized_bytes[64..96]),
        })
    }
}
