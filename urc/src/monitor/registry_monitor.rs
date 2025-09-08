use std::str::FromStr;

use alloy::{
    consensus::Transaction,
    primitives::Address,
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::Filter,
    sol_types::{SolCall, SolEvent},
};
use anyhow::Error;

use crate::monitor::{config::Config, db::DataBase};

use crate::bindings::IRegistry;

use tokio::time::{Duration, sleep};

pub struct RegistryMonitor {
    indexed_block: u64,
    db: DataBase,
    l1_provider: DynProvider,
    registry_address: Address,
}

impl RegistryMonitor {
    pub async fn new(config: Config) -> Result<Self, Error> {
        let db = DataBase::new(&config.db_filename).await?;
        let indexed_block = db.get_indexed_block().await.max(config.l1_start_block);
        let l1_provider = ProviderBuilder::new()
            .connect_http(config.l1_rpc_url.parse()?)
            .erased();

        let registry_address = Address::from_str(config.registry_address.as_str())?;

        Ok(Self {
            indexed_block,
            db,
            l1_provider,
            registry_address,
        })
    }

    pub async fn run_indexing_loop(&mut self) {
        tracing::info!("Starting indexing loop");
        loop {
            let current_block = self
                .l1_provider
                .get_block_number()
                .await
                .expect("Could not get block number");
            let block_to_index = self.indexed_block + 1;

            if current_block >= block_to_index {
                if let Err(e) = self.index_register(block_to_index).await {
                    panic!("Failed to index OperatorRegistered events: {e}");
                }

                if let Err(e) = self.index_opt_in(block_to_index).await {
                    panic!("Failed to index OperatorOptedIn events: {e}")
                }

                self.indexed_block = block_to_index;

                if let Err(e) = self.db.update_status(self.indexed_block).await {
                    panic!("Failed to update status: {e}");
                }
            }

            if self.indexed_block == current_block {
                tracing::debug!("Sleeping for 12 seconds");
                tracing::debug!(
                    "Current block: {}, indexed block: {}",
                    current_block,
                    self.indexed_block
                );
                sleep(Duration::from_secs(12)).await;
            } else {
                tracing::debug!("Sleeping for 2 seconds");
                tracing::debug!(
                    "Current block: {}, indexed block: {}",
                    current_block,
                    self.indexed_block
                );
                sleep(Duration::from_secs(2)).await;
            }
        }
    }

    pub async fn index_register(&self, block_to_index: u64) -> Result<(), Error> {
        let batch_proposed = IRegistry::OperatorRegistered::SIGNATURE_HASH;
        let filter = Filter::new()
            .address(self.registry_address)
            .event_signature(batch_proposed)
            .from_block(block_to_index)
            .to_block(block_to_index);
        let logs = self.l1_provider.get_logs(&filter).await?;

        for log in logs {
            // Add operator
            let operator_regestred = log.log_decode::<IRegistry::OperatorRegistered>()?;
            let registration_root = operator_regestred.inner.registrationRoot.to_string();
            let owner = operator_regestred.inner.owner.to_string();
            let block_number = log
                .block_number
                .unwrap_or_else(|| panic!("Block number not found"));
            let block = self
                .l1_provider
                .get_block(block_number.into())
                .await?
                .expect("Block not found");
            let registered_at = block.header.inner.timestamp;

            tracing::info!(
                "Insert operator\nregistration_root: {}\nowner: {}\nregistered_at: {}",
                registration_root,
                owner,
                registered_at
            );
            self.db
                .insert_operator(&registration_root, owner, registered_at)
                .await?;

            // Add signed_registration
            let tx = match self
                .l1_provider
                .get_transaction_by_hash(
                    log.transaction_hash.expect("Transaction receipt not found"),
                )
                .await?
            {
                Some(tx) => tx,
                None => panic!(
                    "Transaction receipt not found for {:?}",
                    log.transaction_hash
                ),
            };

            let register_call = IRegistry::registerCall::abi_decode(tx.input())?;
            for (idx, registration) in register_call.registrations.into_iter().enumerate() {
                let pubkey = registration.pubkey;
                tracing::info!(
                    "Insert signed_registration\nregistration_root: {}\nidx: {}\npubkey_x_a: {}\npubkey_x_b: {}\npubkey_y_a: {}\npubkey_y_b: {}",
                    registration_root,
                    idx,
                    pubkey.x_a,
                    pubkey.x_b,
                    pubkey.y_a,
                    pubkey.y_b
                );
                self.db
                    .insert_signed_registrations(
                        &registration_root,
                        idx,
                        pubkey.x_a.to_string(),
                        pubkey.x_b.to_string(),
                        pubkey.y_a.to_string(),
                        pubkey.y_b.to_string(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn index_opt_in(&self, block_to_index: u64) -> Result<(), Error> {
        let batch_proposed = IRegistry::OperatorOptedIn::SIGNATURE_HASH;
        let filter = Filter::new()
            .address(self.registry_address)
            .event_signature(batch_proposed)
            .from_block(block_to_index)
            .to_block(block_to_index);
        let logs = self.l1_provider.get_logs(&filter).await?;

        for log in logs {
            let operator_opt_in = log.log_decode::<IRegistry::OperatorOptedIn>()?;
            let registration_root = operator_opt_in.inner.registrationRoot.to_string();
            let slasher = operator_opt_in.inner.slasher.to_string();
            let committer = operator_opt_in.inner.committer.to_string();
            let block_number = log
                .block_number
                .unwrap_or_else(|| panic!("Block number not found"));

            let block = self
                .l1_provider
                .get_block(block_number.into())
                .await?
                .expect("Block not found");

            let opt_in_at = block.header.inner.timestamp;

            tracing::info!(
                "Find OperatorOptedIn Event:\nregistration root {}\ncommitter {}\nslasher {}\nopt_in_at {}",
                registration_root,
                committer,
                slasher,
                opt_in_at,
            );

            self.db
                .on_operator_opt_in(&registration_root, slasher, committer, opt_in_at)
                .await?;
        }

        Ok(())
    }
}
