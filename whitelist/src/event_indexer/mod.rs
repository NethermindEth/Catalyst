use alloy::{primitives::Address, transports::http::reqwest::Url};
use anyhow::Error;
use event_indexer::{
    indexer::{ShastaEventIndexer, ShastaEventIndexerConfig},
    interface::ShastaProposeInputReader,
};
use std::{str::FromStr, sync::Arc};
use taiko_rpc::SubscriptionSource;

pub struct EventIndexer {
    indexer: Arc<ShastaEventIndexer>,
}

impl EventIndexer {
    pub async fn new(l1_ws_rpc_url: String, inbox_contract_address: String) -> Result<Self, Error> {
        let config = ShastaEventIndexerConfig {
            l1_subscription_source: SubscriptionSource::Ws(Url::from_str(l1_ws_rpc_url.as_str())?),
            inbox_address: Address::from_str(&inbox_contract_address)?,
        };

        // Create and spawn the indexer.
        let indexer = ShastaEventIndexer::new(config).await?;
        indexer.clone().spawn();
        indexer.wait_historical_indexing_finished().await;

        // Read cached input parameters from the indexer, for submitting a `propose` transaction to
        // Shasta inbox.
        let _ = indexer.read_shasta_propose_input();

        Ok(Self { indexer })
    }
}
