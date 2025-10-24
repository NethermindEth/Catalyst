use std::time::Duration;

pub struct TaikoDriverConfig {
    pub driver_url: String,
    pub rpc_driver_preconf_timeout: Duration,
    pub rpc_driver_status_timeout: Duration,
    pub jwt_secret_bytes: [u8; 32],
    pub call_timeout: Duration,
}
