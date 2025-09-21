#![allow(dead_code)] // Remove once LookaheadBuilder is used by the node

use alloy::{
    hex,
    primitives::{Address, FixedBytes, U256},
    providers::DynProvider,
};
use anyhow::Error;
use blst::min_pk::PublicKey;
use common::l1::{el_trait::ELTrait, ethereum_l1::EthereumL1};
use std::{str::FromStr, sync::Arc};
use urc::monitor::db::DataBase as UrcDataBase;

use crate::l1::bindings::{
    BLS::G1Point,
    ILookaheadStore::{self, ILookaheadStoreInstance, LookaheadSlot},
};
use crate::l1::execution_layer::ExecutionLayer;

pub struct LookaheadBuilder {
    urc_db: UrcDataBase,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    lookahead_store_contract: ILookaheadStoreInstance<DynProvider>,
    lookahead_slasher_address: Address,
}

impl LookaheadBuilder {
    pub fn new(
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        urc_db: UrcDataBase,
        lookahead_store_address: Address,
        lookahead_slasher_address: Address,
    ) -> Self {
        let lookahead_store_contract = ILookaheadStore::new(
            lookahead_store_address,
            ethereum_l1.execution_layer.common().provider(),
        );

        Self {
            ethereum_l1,
            urc_db,
            lookahead_store_contract,
            lookahead_slasher_address,
        }
    }

    pub async fn build(&self, epoch: u64) -> Result<Vec<LookaheadSlot>, Error> {
        let mut lookahead_slots: Vec<LookaheadSlot> = Vec::with_capacity(32);

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
                    self.lookahead_slasher_address.to_string().as_str(),
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
}
