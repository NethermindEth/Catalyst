#![allow(dead_code)] // Remove once LookaheadBuilder is used by the node

use alloy::{
    hex,
    primitives::{Address, FixedBytes, U256},
    providers::DynProvider,
};
use anyhow::Error;
use blst::min_pk::PublicKey;
use common::{
    l1::{el_trait::ELTrait, ethereum_l1::EthereumL1},
    utils::types::Slot,
};
use std::{str::FromStr, sync::Arc};
use urc::monitor::db::DataBase as UrcDataBase;

use crate::l1::bindings::{
    BLS::G1Point,
    ILookaheadStore::{self, ILookaheadStoreInstance, LookaheadSlot},
};
use crate::l1::execution_layer::ExecutionLayer;

use super::types::Lookahead;

pub struct LookaheadBuilder {
    urc_db: UrcDataBase,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    lookahead_store_contract: ILookaheadStoreInstance<DynProvider>,
    preconf_slasher_address: Address,
}

impl LookaheadBuilder {
    pub fn new(
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        urc_db: UrcDataBase,
        lookahead_store_address: Address,
        preconf_slasher_address: Address,
    ) -> Self {
        let lookahead_store_contract = ILookaheadStore::new(
            lookahead_store_address,
            ethereum_l1.execution_layer.common().provider(),
        );

        Self {
            ethereum_l1,
            urc_db,
            lookahead_store_contract,
            preconf_slasher_address,
        }
    }

    pub async fn build(&self, epoch: u64) -> Result<Lookahead, Error> {
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

    async fn get_next_opted_in_preconfer(
        &self,
        next_preconfing_slot: Slot,
        current_lookahead_slot_index: U256,
        current_lookahead: &Lookahead,
        next_lookahead: &Lookahead,
    ) -> Result<(Option<Address>, U256), Error> {
        if current_lookahead.len() == 0 {
            // The current lookahead is empty, and thus, there is no opted in preconfer
            return Ok((None, U256::ZERO));
        }

        let next_preconf_slot_timestamp = self
            .ethereum_l1
            .slot_clock
            .start_of(next_preconfing_slot)?
            .as_secs();

        let index_usize: usize = current_lookahead_slot_index.try_into().unwrap();
        let current_lookahead_slot = &current_lookahead[index_usize];

        if next_preconf_slot_timestamp > current_lookahead_slot.timestamp.try_into().unwrap() {
            // The current lookahead slot is stale
            if current_lookahead_slot_index == current_lookahead.len() - 1 {
                // No more opted in preconfers remaining in the current lookahead
                if next_lookahead.len() != 0 {
                    // If next lookahead is not empty, the first preconfer takes over if it is active
                    return Ok((
                        self.map_operator_to_active_preconfer(
                            &next_lookahead[0].registrationRoot.to_string(),
                            next_lookahead[0].committer,
                        )
                        .await?,
                        U256::MAX,
                    ));
                } else {
                    // Else, we do not have an opted in preconfer
                    return Ok((None, U256::MAX));
                }
            } else {
                // We move to the next slot in the current lookahead
                let next_current_lookahead_slot = &current_lookahead[index_usize + 1];
                return Ok((
                    self.map_operator_to_active_preconfer(
                        &next_current_lookahead_slot.registrationRoot.to_string(),
                        next_current_lookahead_slot.committer,
                    )
                    .await?,
                    U256::from(index_usize + 1),
                ));
            }
        }

        // The current lookahead slot is still valid
        return Ok((
            Some(current_lookahead_slot.committer),
            current_lookahead_slot_index,
        ));
    }

    fn pubkey_bytes_to_g1_point(pubkey_bytes: &[u8]) -> Result<G1Point, Error> {
        let pubkey: PublicKey = PublicKey::from_bytes(pubkey_bytes)
            .map_err(|_| anyhow::anyhow!("LookaheadBuilder: pubkey parsing error"))?;
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

    async fn map_operator_to_active_preconfer(
        &self,
        registration_root: &str,
        committer: Address,
    ) -> Result<Option<Address>, Error> {
        let is_preconfer_active = self.is_operator_active(registration_root).await?;

        if is_preconfer_active {
            return Ok(Some(committer));
        } else {
            return Ok(None);
        };
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
            .unwrap_or(false);
    }

    async fn is_operator_active(&self, registration_root: &str) -> Result<bool, Error> {
        let result = self
            .urc_db
            .get_opted_in_operator(registration_root, &self.preconf_slasher_address.to_string())
            .await?;

        match result {
            None => {
                return Err(anyhow::anyhow!(
                    "LookaheadBuilder: operator with registration root {} has not opted into slasher {}",
                    registration_root,
                    self.preconf_slasher_address.to_string()
                ));
            }
            Some((_, _, unregistered_at, slashed_at, _, opted_in_at, opted_out_at)) => {
                if unregistered_at.is_some() || slashed_at.is_some() || opted_out_at >= opted_in_at
                {
                    return Ok(false);
                }
            }
        }

        return Ok(true);
    }
}
