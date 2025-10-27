use crate::l2::taiko_driver::models::TaikoStatus;
use anyhow::Error;

pub trait StatusProvider {
    fn get_status(&self) -> impl std::future::Future<Output = Result<TaikoStatus, Error>> + Send;
}
