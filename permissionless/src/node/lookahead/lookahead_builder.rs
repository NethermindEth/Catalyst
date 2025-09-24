#![allow(dead_code)] // Remove once LookaheadBuilder is used by the node

use alloy::{
    hex,
    primitives::{Address, Bytes, FixedBytes, U256},
    providers::DynProvider,
};
use anyhow::{Error, anyhow};
use blst::min_pk::PublicKey;
use common::{
    l1::{el_trait::ELTrait, ethereum_l1::EthereumL1},
    utils::types::Epoch,
};
use std::{str::FromStr, sync::Arc};
use tracing::info;
use urc::monitor::db::DataBase as UrcDataBase;

use crate::l1::bindings::{
    BLS::G1Point,
    ILookaheadStore::{
        self, ILookaheadStoreInstance, LookaheadData, LookaheadSlot, ProposerContext,
    },
};
use crate::l1::execution_layer::ExecutionLayer;

use super::types::Lookahead;

struct Context {
    epoch: Epoch,
    slot_index: U256,
    current_lookahead: Lookahead,
    next_lookahead: Lookahead,
}

pub struct LookaheadBuilder {
    urc_db: UrcDataBase,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    lookahead_store_contract: ILookaheadStoreInstance<DynProvider>,
    preconf_slasher_address: Address,
    context: Context,
}

impl LookaheadBuilder {
    pub async fn new(
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        urc_db: UrcDataBase,
        lookahead_store_address: Address,
        preconf_slasher_address: Address,
    ) -> Result<Self, Error> {
        let lookahead_store_contract = ILookaheadStore::new(
            lookahead_store_address,
            ethereum_l1.execution_layer.common().provider(),
        );

        let mut builder = Self {
            ethereum_l1,
            urc_db,
            lookahead_store_contract,
            preconf_slasher_address,
            context: Context {
                epoch: 0,
                slot_index: U256::ZERO,
                current_lookahead: vec![],
                next_lookahead: vec![],
            },
        };

        let current_epoch = builder.ethereum_l1.slot_clock.get_current_epoch()?;
        builder.context.current_lookahead = builder.build(current_epoch).await?;
        builder.context.next_lookahead = builder.build(current_epoch + 1).await?;
        builder.context.epoch = current_epoch;

        Ok(builder)
    }

    async fn next_preconfer(&mut self) -> Result<ProposerContext, Error> {
        let current_slot = self.ethereum_l1.slot_clock.get_current_slot()?;
        let next_slot_timestamp = U256::from(
            self.ethereum_l1
                .slot_clock
                .start_of(current_slot + 1)?
                .as_secs(),
        );
        let current_epoch = self.ethereum_l1.slot_clock.get_current_epoch()?;
        let current_epoch_timestamp = U256::from(
            self.ethereum_l1
                .slot_clock
                .get_epoch_begin_timestamp(current_epoch)?
                - self.ethereum_l1.slot_clock.get_slot_duration().as_secs(),
        );

        // Update the lookaheads if required
        if current_epoch > self.context.epoch {
            if current_epoch == self.context.epoch + 1 {
                self.context.current_lookahead = self.context.next_lookahead.clone();
            } else {
                self.context.current_lookahead = self.build(current_epoch).await?;
            }
            self.context.next_lookahead = self.build(current_epoch + 1).await?;
            self.context.slot_index = U256::ZERO;
        }

        // Update `context.slot_index` depending upon which lookahead slot covers the current
        // preconfing period
        if !self.context.current_lookahead.is_empty() {
            let mut slot_index: usize = self.context.slot_index.try_into()?;
            let lookahead_slot_timestamp = self.context.current_lookahead[slot_index].timestamp;

            if next_slot_timestamp > lookahead_slot_timestamp {
                if slot_index == self.context.current_lookahead.len() - 1 {
                    // We have reached the end of the current lookahead
                    self.context.slot_index = U256::MAX;
                } else {
                    // We are still in the current lookahead
                    let mut prev_lookahead_slot_timestamp = if slot_index == 0 {
                        current_epoch_timestamp
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

                    self.context.slot_index = U256::from(slot_index);
                }
            }
        }

        let lookahead_data = LookaheadData {
            slotIndex: self.context.slot_index,
            registrationRoot: FixedBytes::from([0_u8; 32]),
            currLookahead: self.context.current_lookahead.clone(),
            nextLookahead: self.context.next_lookahead.clone(),
            commitmentSignature: Bytes::new(),
        };

        let proposer_context = self
            .lookahead_store_contract
            .getProposerContext(lookahead_data, current_epoch_timestamp)
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

    async fn build(&self, epoch: u64) -> Result<Lookahead, Error> {
        let mut lookahead_slots: Lookahead = Vec::with_capacity(32);

        let epoch_timestamp = self
            .ethereum_l1
            .slot_clock
            .get_epoch_begin_timestamp(epoch)?;

        // Fetch all validator pubkeys for `epoch`
        let validators = self
            .ethereum_l1
            .consensus_layer
            .get_validators_for_epoch(epoch)
            .await?;

        for (index, validator) in validators.iter().enumerate() {
            let pubkey_bytes = hex::decode(&validator)?; // Compressed bytes
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
                    let slot_duration = self.ethereum_l1.slot_clock.get_slot_duration().as_secs();
                    let slot_timestamp = epoch_timestamp + ((index as u64) * slot_duration);

                    lookahead_slots.push(LookaheadSlot {
                        committer: Address::from_str(&committer).unwrap(),
                        timestamp: U256::from(slot_timestamp),
                        registrationRoot: FixedBytes::<32>::from_str(&registration_root).unwrap(),
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
        return self
            .lookahead_store_contract
            .isLookaheadOperatorValid(
                U256::from(epoch_timestamp),
                FixedBytes::<32>::from_str(registration_root).unwrap(),
            )
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
