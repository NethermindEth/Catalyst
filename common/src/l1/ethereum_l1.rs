use super::{
    blob_indexer::BlobIndexer, config::EthereumL1Config, consensus_layer::ConsensusLayer,
    slot_clock::SlotClock, traits::ELTrait, transaction_error::TransactionError,
};
use anyhow::Error;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::Sender;

use crate::metrics::Metrics;

pub struct EthereumL1<T: ELTrait> {
    pub slot_clock: Arc<SlotClock>,
    pub consensus_layer: ConsensusLayer,
    pub execution_layer: Arc<T>,
    pub blob_indexer: Option<Arc<BlobIndexer>>,
}

impl<T: ELTrait> EthereumL1<T> {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: EthereumL1Config,
        specific_config: T::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        tracing::info!("Creating EthereumL1 instance");
        let consensus_layer = ConsensusLayer::new(
            &config.consensus_rpc_url,
            Duration::from_millis(config.preconf_heartbeat_ms / 2),
        )?;

        let blob_indexer = if let Some(blob_indexer_url) = &config.blob_indexer_url {
            tracing::info!("Blob Indexer configured at {}", blob_indexer_url);
            Some(Arc::new(BlobIndexer::new(
                blob_indexer_url,
                Duration::from_millis(config.preconf_heartbeat_ms / 2),
            )?))
        } else {
            tracing::info!("No Blob Indexer URL provided; Blob Indexer will not be used");
            None
        };

        let genesis_time = consensus_layer.get_genesis_time().await?;
        let slot_clock = Arc::new(SlotClock::new(
            0u64,
            genesis_time,
            config.slot_duration_sec,
            config.slots_per_epoch,
            config.preconf_heartbeat_ms,
        ));

        let execution_layer =
            T::new(config, specific_config, transaction_error_channel, metrics).await?;

        Ok(Self {
            slot_clock,
            consensus_layer,
            execution_layer: Arc::new(execution_layer),
            blob_indexer,
        })
    }
}
