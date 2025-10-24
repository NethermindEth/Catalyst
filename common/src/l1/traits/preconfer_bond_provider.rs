use alloy::primitives::U256;
use anyhow::Error;

pub trait PreconferBondProvider {
    // bond balance
    fn get_preconfer_total_bonds(&self) -> impl Future<Output = Result<U256, Error>> + Send;
}
