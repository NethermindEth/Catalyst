mod config_trait;
pub use config_trait::ConfigTrait;

use alloy::primitives::Address;
use anyhow::Error;
use std::str::FromStr;
use std::time::Duration;
use tracing::{info, warn};

use crate::blob::constants::MAX_BLOB_DATA_SIZE;

#[derive(Debug, Clone)]
pub struct Config {
    // Signer
    pub preconfer_address: Option<Address>,
    pub web3signer_l1_url: Option<String>,
    pub web3signer_l2_url: Option<String>,
    pub catalyst_node_ecdsa_private_key: Option<String>,
    // L1
    pub l1_rpc_urls: Vec<String>,
    pub l1_beacon_url: String,
    pub blob_indexer_url: Option<String>,
    pub l1_slot_duration_sec: u64,
    pub l1_slots_per_epoch: u64,
    pub preconf_heartbeat_ms: u64,
    // L2
    pub taiko_geth_rpc_url: String,
    pub taiko_geth_auth_rpc_url: String,
    pub taiko_driver_url: String,
    /// jwt secret file path for taiko-geth and taiko-driver
    pub jwt_secret_file_path: String,
    pub rpc_l2_execution_layer_timeout: Duration,
    pub rpc_driver_preconf_timeout: Duration,
    pub rpc_driver_status_timeout: Duration,
    // Taiko contracts
    pub taiko_anchor_address: Address,
    pub taiko_bridge_address: Address,
    // Batch building parameters
    pub max_bytes_size_of_batch: u64,
    pub max_blocks_per_batch: u16,
    pub max_time_shift_between_blocks_sec: u64,
    pub max_anchor_height_offset_reduction: u64,
    // Transaction parameters
    pub min_priority_fee_per_gas_wei: u64,
    pub tx_fees_increase_percentage: u64,
    pub max_attempts_to_send_tx: u64,
    pub max_attempts_to_wait_tx: u64,
    pub delay_between_tx_attempts_sec: u64,
    pub extra_gas_percentage: u64,
    // Thresholds for balances
    pub funds_monitor_interval_sec: u64,
    pub threshold_eth: u128,
    pub threshold_taiko: u128,
    // Bridging
    pub disable_bridging: bool,
    pub amount_to_bridge_from_l2_to_l1: u128,
    pub bridge_relayer_fee: u64,
    pub bridge_transaction_fee: u64,
    // Block production and throttling
    pub max_bytes_per_tx_list: u64,
    pub min_bytes_per_tx_list: u64,
    pub throttling_factor: u64,
    pub preconf_min_txs: u64,
    pub preconf_max_skipped_l2_slots: u64,
    // fork info
    pub fork_switch_transition_period_sec: u64,
    pub pacaya_timestamp_sec: u64,
    pub shasta_timestamp_sec: u64,
    pub permissionless_timestamp_sec: u64,
    // Whitelist monitor
    pub whitelist_monitor_interval_sec: u64,
}

/// Creates a formatted error message for address parsing failures.
pub fn address_parse_error(
    env_var: &str,
    error: impl std::fmt::Display,
    value: &str,
) -> anyhow::Error {
    anyhow::anyhow!(
        "Failed to parse {}: {}. Address must be exactly 42 characters (0x followed by 40 hex characters). Got: '{}' (length: {})",
        env_var,
        error,
        value,
        value.len()
    )
}

impl Config {
    pub fn read_env_variables() -> Result<Self, Error> {
        // Load environment variables from .env file
        dotenvy::dotenv().ok();

        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        const CATALYST_NODE_ECDSA_PRIVATE_KEY: &str = "CATALYST_NODE_ECDSA_PRIVATE_KEY";
        let catalyst_node_ecdsa_private_key = std::env::var(CATALYST_NODE_ECDSA_PRIVATE_KEY).ok();
        const PRECONFER_ADDRESS: &str = "PRECONFER_ADDRESS";
        let preconfer_address = std::env::var(PRECONFER_ADDRESS)
            .ok()
            .map(|s| {
                Address::from_str(&s).map_err(|e| address_parse_error(PRECONFER_ADDRESS, e, &s))
            })
            .transpose()?;
        const WEB3SIGNER_L1_URL: &str = "WEB3SIGNER_L1_URL";
        let web3signer_l1_url = std::env::var(WEB3SIGNER_L1_URL).ok();
        const WEB3SIGNER_L2_URL: &str = "WEB3SIGNER_L2_URL";
        let web3signer_l2_url = std::env::var(WEB3SIGNER_L2_URL).ok();

        if catalyst_node_ecdsa_private_key.is_none() {
            if web3signer_l1_url.is_none()
                || web3signer_l2_url.is_none()
                || preconfer_address.is_none()
            {
                return Err(anyhow::anyhow!(
                    "When {CATALYST_NODE_ECDSA_PRIVATE_KEY} is not set, {WEB3SIGNER_L1_URL}, {WEB3SIGNER_L2_URL} and {PRECONFER_ADDRESS} must be set"
                ));
            }
        } else if web3signer_l1_url.is_some()
            || web3signer_l2_url.is_some()
            || preconfer_address.is_some()
        {
            return Err(anyhow::anyhow!(
                "When {CATALYST_NODE_ECDSA_PRIVATE_KEY} is set, {WEB3SIGNER_L1_URL}, {WEB3SIGNER_L2_URL} and {PRECONFER_ADDRESS} must not be set"
            ));
        }

        let l1_beacon_url = {
            let mut url = std::env::var("L1_BEACON_URL").unwrap_or_else(|_| {
                warn!("No L1 beacon URL found in L1_BEACON_URL env var, using default",);
                "http://127.0.0.1:4000".to_string()
            });
            if !url.ends_with('/') {
                url.push('/');
            }
            url
        };

        let extra_gas_percentage = std::env::var("EXTRA_GAS_PERCENTAGE")
            .unwrap_or("100".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("EXTRA_GAS_PERCENTAGE must be a number: {}", e))?;

        let l1_slot_duration_sec = std::env::var("L1_SLOT_DURATION_SEC")
            .unwrap_or("12".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("L1_SLOT_DURATION_SEC must be a number: {}", e))
            .and_then(|val| {
                if val == 0 {
                    Err(anyhow::anyhow!(
                        "L1_SLOT_DURATION_SEC must be a positive number"
                    ))
                } else {
                    Ok(val)
                }
            })?;

        let l1_slots_per_epoch = std::env::var("L1_SLOTS_PER_EPOCH")
            .unwrap_or("32".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("L1_SLOTS_PER_EPOCH must be a number: {}", e))
            .and_then(|val| {
                if val == 0 {
                    Err(anyhow::anyhow!(
                        "L1_SLOTS_PER_EPOCH must be a positive number"
                    ))
                } else {
                    Ok(val)
                }
            })?;

        let preconf_heartbeat_ms = std::env::var("PRECONF_HEARTBEAT_MS")
            .unwrap_or("2000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PRECONF_HEARTBEAT_MS must be a number: {}", e))
            .and_then(|val| {
                if val == 0 {
                    Err(anyhow::anyhow!(
                        "PRECONF_HEARTBEAT_MS must be a positive number"
                    ))
                } else {
                    Ok(val)
                }
            })?;

        let jwt_secret_file_path = std::env::var("JWT_SECRET_FILE_PATH").unwrap_or_else(|_| {
            warn!(
                "No JWT secret file path found in {} env var, using default",
                "JWT_SECRET_FILE_PATH"
            );
            "/tmp/jwtsecret".to_string()
        });

        let rpc_driver_preconf_timeout = std::env::var("RPC_DRIVER_PRECONF_TIMEOUT_MS")
            .unwrap_or("60000".to_string())
            .parse::<u64>()
            .map_err(|e| {
                anyhow::anyhow!("RPC_DRIVER_PRECONF_TIMEOUT_MS must be a number: {}", e)
            })?;
        let rpc_driver_preconf_timeout = Duration::from_millis(rpc_driver_preconf_timeout);

        let rpc_driver_status_timeout = std::env::var("RPC_DRIVER_STATUS_TIMEOUT_MS")
            .unwrap_or("1000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("RPC_DRIVER_STATUS_TIMEOUT_MS must be a number: {}", e))?;
        let rpc_driver_status_timeout = Duration::from_millis(rpc_driver_status_timeout);

        let rpc_l2_execution_layer_timeout = std::env::var("RPC_L2_EXECUTION_LAYER_TIMEOUT_MS")
            .unwrap_or("1000".to_string())
            .parse::<u64>()
            .map_err(|e| {
                anyhow::anyhow!("RPC_L2_EXECUTION_LAYER_TIMEOUT_MS must be a number: {}", e)
            })?;
        let rpc_l2_execution_layer_timeout = Duration::from_millis(rpc_l2_execution_layer_timeout);

        const TAIKO_ANCHOR_ADDRESS: &str = "TAIKO_ANCHOR_ADDRESS";
        let taiko_anchor_address_str = std::env::var(TAIKO_ANCHOR_ADDRESS)
            .unwrap_or("0x1670010000000000000000000000000000010001".to_string());
        let taiko_anchor_address = Address::from_str(&taiko_anchor_address_str)
            .map_err(|e| address_parse_error(TAIKO_ANCHOR_ADDRESS, e, &taiko_anchor_address_str))?;

        const BRIDGE_ADDRESS: &str = "TAIKO_BRIDGE_L2_ADDRESS";
        let taiko_bridge_address_str = std::env::var(BRIDGE_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No Bridge contract address found in {} env var, using default",
                BRIDGE_ADDRESS
            );
            default_empty_address.clone()
        });
        let taiko_bridge_address = Address::from_str(&taiko_bridge_address_str)
            .map_err(|e| address_parse_error(BRIDGE_ADDRESS, e, &taiko_bridge_address_str))?;

        let blobs_per_batch = std::env::var("BLOBS_PER_BATCH")
            .unwrap_or("3".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("BLOBS_PER_BATCH must be a number: {}", e))?;

        let max_bytes_size_of_batch = u64::try_from(MAX_BLOB_DATA_SIZE)
            .map_err(|_| anyhow::anyhow!("MAX_BLOB_DATA_SIZE must be a u64 number"))?
            .checked_mul(blobs_per_batch)
            .ok_or_else(|| anyhow::anyhow!("Overflow while computing BLOBS_PER_BATCH * MAX_BLOB_DATA_SIZE. Try to reduce BLOBS_PER_BATCH"))?;

        let max_blocks_per_batch = std::env::var("MAX_BLOCKS_PER_BATCH")
            .unwrap_or("0".to_string())
            .parse::<u16>()
            .map_err(|e| anyhow::anyhow!("MAX_BLOCKS_PER_BATCH must be a number: {}", e))?;

        let max_time_shift_between_blocks_sec = std::env::var("MAX_TIME_SHIFT_BETWEEN_BLOCKS_SEC")
            .unwrap_or("255".to_string())
            .parse::<u64>()
            .map_err(|e| {
                anyhow::anyhow!("MAX_TIME_SHIFT_BETWEEN_BLOCKS_SEC must be a number: {}", e)
            })?;

        // It is the slot window in which we want to call the proposeBatch transaction
        // and avoid exceeding the MAX_ANCHOR_HEIGHT_OFFSET.
        let max_anchor_height_offset_reduction =
            std::env::var("MAX_ANCHOR_HEIGHT_OFFSET_REDUCTION_VALUE")
                .unwrap_or("10".to_string())
                .parse::<u64>()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "MAX_ANCHOR_HEIGHT_OFFSET_REDUCTION_VALUE must be a number: {}",
                        e
                    )
                })?;
        if max_anchor_height_offset_reduction < 5 {
            warn!(
                "MAX_ANCHOR_HEIGHT_OFFSET_REDUCTION_VALUE is less than 5: you have a small number of slots to call the proposeBatch transaction"
            );
        }

        let min_priority_fee_per_gas_wei = std::env::var("MIN_PRIORITY_FEE_PER_GAS_WEI")
            .unwrap_or("1000000000".to_string()) // 1 Gwei
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MIN_PRIORITY_FEE_PER_GAS_WEI must be a number: {}", e))
            .and_then(|val| {
                if val < 1000000000 {
                    Err(anyhow::anyhow!("MIN_PRIORITY_FEE_PER_GAS_WEI is less than 1 Gwei! It must be at least 1,000,000,000 wei."))
                } else {
                    Ok(val)
                }
            })?;

        let tx_fees_increase_percentage = std::env::var("TX_FEES_INCREASE_PERCENTAGE")
            .unwrap_or("0".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("TX_FEES_INCREASE_PERCENTAGE must be a number: {}", e))?;

        let max_attempts_to_send_tx = std::env::var("MAX_ATTEMPTS_TO_SEND_TX")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MAX_ATTEMPTS_TO_SEND_TX must be a number: {}", e))?;

        let max_attempts_to_wait_tx = std::env::var("MAX_ATTEMPTS_TO_WAIT_TX")
            .unwrap_or("5".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MAX_ATTEMPTS_TO_WAIT_TX must be a number: {}", e))?;

        let delay_between_tx_attempts_sec = std::env::var("DELAY_BETWEEN_TX_ATTEMPTS_SEC")
            .unwrap_or("63".to_string())
            .parse::<u64>()
            .map_err(|e| {
                anyhow::anyhow!("DELAY_BETWEEN_TX_ATTEMPTS_SEC must be a number: {}", e)
            })?;

        let funds_monitor_interval_sec = std::env::var("FUNDS_MONITOR_INTERVAL_SEC")
            .unwrap_or("60".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("FUNDS_MONITOR_INTERVAL_SEC must be a number: {}", e))?;

        // 0.5 ETH
        let threshold_eth =
            std::env::var("THRESHOLD_ETH").unwrap_or("500000000000000000".to_string());
        let threshold_eth = threshold_eth
            .parse::<u128>()
            .map_err(|e| anyhow::anyhow!("THRESHOLD_ETH must be a number: {}", e))?;

        // 1000 TAIKO
        let threshold_taiko =
            std::env::var("THRESHOLD_TAIKO").unwrap_or("10000000000000000000000".to_string());
        let threshold_taiko = threshold_taiko
            .parse::<u128>()
            .map_err(|e| anyhow::anyhow!("THRESHOLD_TAIKO must be a number: {}", e))?;

        // 1 ETH
        let amount_to_bridge_from_l2_to_l1 = std::env::var("AMOUNT_TO_BRIDGE_FROM_L2_TO_L1")
            .unwrap_or("1000000000000000000".to_string())
            .parse::<u128>()
            .map_err(|e| {
                anyhow::anyhow!("AMOUNT_TO_BRIDGE_FROM_L2_TO_L1 must be a number: {}", e)
            })?;

        let disable_bridging = std::env::var("DISABLE_BRIDGING")
            .unwrap_or("true".to_string())
            .parse::<bool>()
            .map_err(|e| anyhow::anyhow!("DISABLE_BRIDGING must be a boolean: {}", e))?;

        let max_bytes_per_tx_list = std::env::var("MAX_BYTES_PER_TX_LIST")
            .unwrap_or(MAX_BLOB_DATA_SIZE.to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MAX_BYTES_PER_TX_LIST must be a number: {}", e))?;

        // The throttling factor is used to reduce the max bytes per tx list exponentially.
        let throttling_factor = std::env::var("THROTTLING_FACTOR")
            .unwrap_or("2".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("THROTTLING_FACTOR must be a number: {}", e))?;

        let min_bytes_per_tx_list = std::env::var("MIN_BYTES_PER_TX_LIST")
            .unwrap_or("8192".to_string()) // 8KB
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MIN_BYTES_PER_TX_LIST must be a number: {}", e))?;

        let preconf_min_txs = std::env::var("PRECONF_MIN_TXS")
            .unwrap_or("3".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PRECONF_MIN_TXS must be a number: {}", e))?;

        let preconf_max_skipped_l2_slots = std::env::var("PRECONF_MAX_SKIPPED_L2_SLOTS")
            .unwrap_or("2".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PRECONF_MAX_SKIPPED_L2_SLOTS must be a number: {}", e))?;

        // 0.003 eth
        let bridge_relayer_fee = std::env::var("BRIDGE_RELAYER_FEE")
            .unwrap_or("3047459064000000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("BRIDGE_RELAYER_FEE must be a number: {}", e))?;

        // 0.001 eth
        let bridge_transaction_fee = std::env::var("BRIDGE_TRANSACTION_FEE")
            .unwrap_or("1000000000000000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("BRIDGE_TRANSACTION_FEE must be a number: {}", e))?;

        // Fork info
        let fork_switch_transition_period_sec =
            match std::env::var("FORK_SWITCH_TRANSITION_PERIOD_SEC") {
                Err(_) => 60,
                Ok(time) => time.parse::<u64>().map_err(|e| {
                    anyhow::anyhow!("FORK_SWITCH_TRANSITION_PERIOD_SEC must be a number: {}", e)
                })?,
            };
        let pacaya_timestamp_sec = std::env::var("PACAYA_TIMESTAMP_SEC")
            .unwrap_or("0".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PACAYA_TIMESTAMP_SEC must be a number: {}", e))?;
        let shasta_timestamp_sec = std::env::var("SHASTA_TIMESTAMP_SEC")
            .unwrap_or("99999999999".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("SHASTA_TIMESTAMP_SEC must be a number: {}", e))?;
        let permissionless_timestamp_sec = std::env::var("PERMISSIONLESS_TIMESTAMP_SEC")
            .unwrap_or("99999999999".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PERMISSIONLESS_TIMESTAMP_SEC must be a number: {}", e))?;

        let whitelist_monitor_interval_sec = std::env::var("WHITELIST_MONITOR_INTERVAL_SEC")
            .unwrap_or("60".to_string())
            .parse::<u64>()
            .map_err(|e| {
                anyhow::anyhow!("WHITELIST_MONITOR_INTERVAL_SEC must be a number: {}", e)
            })?;

        let config = Self {
            preconfer_address,
            taiko_geth_rpc_url: std::env::var("TAIKO_GETH_RPC_URL")
                .unwrap_or("ws://127.0.0.1:1234".to_string()),
            taiko_geth_auth_rpc_url: std::env::var("TAIKO_GETH_AUTH_RPC_URL")
                .unwrap_or("http://127.0.0.1:1235".to_string()),
            taiko_driver_url: std::env::var("TAIKO_DRIVER_URL")
                .unwrap_or("http://127.0.0.1:1236".to_string()),
            catalyst_node_ecdsa_private_key,
            l1_rpc_urls: std::env::var("L1_RPC_URLS")
                .unwrap_or("wss://127.0.0.1".to_string())
                .split(",")
                .map(|s| s.to_string())
                .collect(),
            l1_beacon_url,
            blob_indexer_url: std::env::var("BLOB_INDEXER_URL").ok(),
            web3signer_l1_url,
            web3signer_l2_url,
            l1_slot_duration_sec,
            l1_slots_per_epoch,
            preconf_heartbeat_ms,
            // contract_addresses,
            jwt_secret_file_path,
            rpc_l2_execution_layer_timeout,
            rpc_driver_preconf_timeout,
            rpc_driver_status_timeout,
            taiko_anchor_address,
            taiko_bridge_address,
            max_bytes_size_of_batch,
            max_blocks_per_batch,
            max_time_shift_between_blocks_sec,
            max_anchor_height_offset_reduction,
            min_priority_fee_per_gas_wei,
            tx_fees_increase_percentage,
            max_attempts_to_send_tx,
            max_attempts_to_wait_tx,
            delay_between_tx_attempts_sec,
            funds_monitor_interval_sec,
            threshold_eth,
            threshold_taiko,
            amount_to_bridge_from_l2_to_l1,
            disable_bridging,
            max_bytes_per_tx_list,
            throttling_factor,
            min_bytes_per_tx_list,
            extra_gas_percentage,
            preconf_min_txs,
            preconf_max_skipped_l2_slots,
            bridge_relayer_fee,
            bridge_transaction_fee,
            fork_switch_transition_period_sec,
            pacaya_timestamp_sec,
            shasta_timestamp_sec,
            permissionless_timestamp_sec,
            whitelist_monitor_interval_sec,
        };

        info!(
            r#"
Configuration:{}
Taiko geth L2 RPC URL: {},
Taiko geth auth RPC URL: {},
Taiko driver URL: {},
L1 RPC URL: {},
Consensus layer URL: {},
Blob Indexer URL: {},
Web3signer L1 URL: {},
Web3signer L2 URL: {},
L1 slot duration: {}s
L1 slots per epoch: {}
L2 slot duration (heart beat): {}
jwt secret file path: {}
rpc L2 EL timeout: {}ms
rpc driver preconf timeout: {}ms
rpc driver status timeout: {}ms
taiko anchor address: {}
taiko bridge address: {}
max bytes per tx list from taiko driver: {}
throttling factor: {}
min pending tx list size: {} bytes
max bytes size of batch: {}
max blocks per batch value: {}
max time shift between blocks: {}s
max anchor height offset reduction value: {}
min priority fee per gas: {}wei
tx fees increase percentage: {}
max attempts to send tx: {}
max attempts to wait tx: {}
delay between tx attempts: {}s
funds_monitor_interval_sec: {}s
threshold_eth: {}
threshold_taiko: {}
amount to bridge from l2 to l1: {}
disable bridging: {}
min number of transaction to create a L2 block: {}
max number of skipped L2 slots while creating a L2 block: {}
bridge relayer fee: {}wei
bridge transaction fee: {}wei
fork switch transition time: {}s
pacaya timestamp: {}s
shasta timestamp: {}s
permissionless timestamp: {}s
whitelist monitor interval: {}s
"#,
            if let Some(preconfer_address) = &config.preconfer_address {
                format!("\npreconfer address: {preconfer_address}")
            } else {
                "".to_string()
            },
            config.taiko_geth_rpc_url,
            config.taiko_geth_auth_rpc_url,
            config.taiko_driver_url,
            match config.l1_rpc_urls.split_first() {
                Some((first, rest)) => {
                    let mut urls = vec![format!("{} (main)", first)];
                    urls.extend(rest.iter().cloned());
                    urls.join(", ")
                }
                None => String::new(),
            },
            config.l1_beacon_url,
            config.blob_indexer_url.as_deref().unwrap_or("not set"),
            config.web3signer_l1_url.as_deref().unwrap_or("not set"),
            config.web3signer_l2_url.as_deref().unwrap_or("not set"),
            config.l1_slot_duration_sec,
            config.l1_slots_per_epoch,
            config.preconf_heartbeat_ms,
            config.jwt_secret_file_path,
            config.rpc_l2_execution_layer_timeout.as_millis(),
            config.rpc_driver_preconf_timeout.as_millis(),
            config.rpc_driver_status_timeout.as_millis(),
            config.taiko_anchor_address,
            config.taiko_bridge_address,
            config.max_bytes_per_tx_list,
            config.throttling_factor,
            config.min_bytes_per_tx_list,
            config.max_bytes_size_of_batch,
            config.max_blocks_per_batch,
            config.max_time_shift_between_blocks_sec,
            config.max_anchor_height_offset_reduction,
            config.min_priority_fee_per_gas_wei,
            config.tx_fees_increase_percentage,
            config.max_attempts_to_send_tx,
            config.max_attempts_to_wait_tx,
            config.delay_between_tx_attempts_sec,
            funds_monitor_interval_sec,
            threshold_eth,
            threshold_taiko,
            config.amount_to_bridge_from_l2_to_l1,
            config.disable_bridging,
            config.preconf_min_txs,
            config.preconf_max_skipped_l2_slots,
            config.bridge_relayer_fee,
            config.bridge_transaction_fee,
            config.fork_switch_transition_period_sec,
            config.pacaya_timestamp_sec,
            config.shasta_timestamp_sec,
            config.permissionless_timestamp_sec,
            config.whitelist_monitor_interval_sec,
        );

        Ok(config)
    }
}
