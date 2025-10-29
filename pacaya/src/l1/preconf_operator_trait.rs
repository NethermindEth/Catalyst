use anyhow::Error;

pub trait PreconfOperator {
    fn is_operator_for_current_epoch(&self) -> impl Future<Output = Result<bool, Error>> + Send;
    fn is_operator_for_next_epoch(&self) -> impl Future<Output = Result<bool, Error>> + Send;
    fn is_preconf_router_specified_in_taiko_wrapper(
        &self,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
    fn get_l2_height_from_taiko_inbox(&self) -> impl Future<Output = Result<u64, Error>> + Send;
    fn get_handover_window_slots(&self) -> impl Future<Output = Result<u64, Error>> + Send;
}
