use crate::l1::{ethereum_l1::EthereumL1, traits::ELTrait};
use tokio::time::sleep;
use tracing::{error, info};

pub async fn synchronize_with_l1_slot_start<T: ELTrait>(ethereum_l1: &EthereumL1<T>) {
    match ethereum_l1.slot_clock.duration_to_next_slot() {
        Ok(duration) => {
            info!(
                "Sleeping for {} ms to synchronize with L1 slot start",
                duration.as_millis()
            );
            sleep(duration).await;
        }
        Err(err) => {
            error!("Failed to get duration to next slot: {}", err);
        }
    }
}
