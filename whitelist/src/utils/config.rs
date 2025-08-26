use common::l1::config::ContractAddresses;
use common::utils::config_trait::ConfigTrait;
use tokio::sync::OnceCell;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub taiko_inbox: String,
    pub preconf_whitelist: String,
    pub preconf_router: String,
    pub taiko_wrapper: String,
    pub forced_inclusion_store: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub contract_addresses: L1ContractAddresses,
}

impl ConfigTrait for Config {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        const TAIKO_INBOX_ADDRESS: &str = "TAIKO_INBOX_ADDRESS";
        let taiko_inbox = std::env::var(TAIKO_INBOX_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No TaikoL1 contract address found in {} env var, using default",
                TAIKO_INBOX_ADDRESS
            );
            default_empty_address.clone()
        });

        const PRECONF_WHITELIST_ADDRESS: &str = "PRECONF_WHITELIST_ADDRESS";
        let preconf_whitelist = std::env::var(PRECONF_WHITELIST_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No PreconfWhitelist contract address found in {} env var, using default",
                PRECONF_WHITELIST_ADDRESS
            );
            default_empty_address.clone()
        });

        const PRECONF_ROUTER_ADDRESS: &str = "PRECONF_ROUTER_ADDRESS";
        let preconf_router = std::env::var(PRECONF_ROUTER_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No PreconfRouter contract address found in {} env var, using default",
                PRECONF_ROUTER_ADDRESS
            );
            default_empty_address.clone()
        });

        const TAIKO_WRAPPER_ADDRESS: &str = "TAIKO_WRAPPER_ADDRESS";
        let taiko_wrapper = std::env::var(TAIKO_WRAPPER_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No TaikoWrapper contract address found in {} env var, using default",
                TAIKO_WRAPPER_ADDRESS
            );
            default_empty_address.clone()
        });

        const FORCED_INCLUSION_STORE_ADDRESS: &str = "FORCED_INCLUSION_STORE_ADDRESS";
        let forced_inclusion_store =
            std::env::var(FORCED_INCLUSION_STORE_ADDRESS).unwrap_or_else(|_| {
                warn!(
                    "No ForcedInclusionStore contract address found in {} env var, using default",
                    FORCED_INCLUSION_STORE_ADDRESS
                );
                default_empty_address.clone()
            });

        Config {
            contract_addresses: L1ContractAddresses {
                taiko_inbox,
                preconf_whitelist,
                preconf_router,
                taiko_wrapper,
                forced_inclusion_store,
            },
        }
    }
}

impl TryFrom<L1ContractAddresses> for ContractAddresses {
    type Error = anyhow::Error;

    fn try_from(l1_contract_addresses: L1ContractAddresses) -> Result<Self, Self::Error> {
        let taiko_inbox = l1_contract_addresses.taiko_inbox.parse()?;
        let preconf_whitelist = l1_contract_addresses.preconf_whitelist.parse()?;
        let preconf_router = l1_contract_addresses.preconf_router.parse()?;
        let taiko_wrapper = l1_contract_addresses.taiko_wrapper.parse()?;
        let forced_inclusion_store = l1_contract_addresses.forced_inclusion_store.parse()?;

        Ok(ContractAddresses {
            taiko_inbox,
            taiko_token: OnceCell::new(),
            preconf_whitelist,
            preconf_router,
            taiko_wrapper,
            forced_inclusion_store,
        })
    }
}
