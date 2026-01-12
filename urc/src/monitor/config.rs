use std::path::PathBuf;
use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::Error;
use common::config::address_parse_error;

#[derive(Debug, Clone)]
pub enum DatabaseConfig {
    Sqlite { path: PathBuf },
    MySql { url: String },
}

impl DatabaseConfig {
    fn description(&self) -> String {
        match self {
            Self::Sqlite { path } => format!("sqlite://{}", path.display()),
            Self::MySql { url } => url.clone(),
        }
    }
}

pub struct Config {
    pub database: DatabaseConfig,
    pub l1_rpc_url: String,
    pub registry_address: Address,
    pub l1_start_block: u64,
    pub max_l1_fork_depth: u64,
    pub index_block_batch_size: u64,
}

impl Config {
    pub fn new() -> Result<Self, Error> {
        // Load environment variables from .env file
        let env_path = format!("{}/.env", env!("CARGO_MANIFEST_DIR"));
        dotenvy::from_path(env_path).ok();

        let mysql_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("MYSQL_URL"))
            .ok();
        let db_filename = std::env::var("DB_FILENAME").ok();

        let database = match (mysql_url, db_filename) {
            (Some(_url), Some(_)) => {
                return Err(anyhow::anyhow!(
                    "Both DATABASE_URL (or MYSQL_URL) and DB_FILENAME are set; only one should be provided"
                ));
            }
            (Some(url), None) => {
                if !url.starts_with("mysql://") {
                    return Err(anyhow::anyhow!(
                        "DATABASE_URL must start with mysql:// when provided"
                    ));
                }
                DatabaseConfig::MySql { url }
            }
            (None, Some(filename)) => {
                let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                path.push(filename);
                DatabaseConfig::Sqlite { path }
            }
            (None, None) => {
                return Err(anyhow::anyhow!(
                    "Provide either DB_FILENAME for SQLite or DATABASE_URL / MYSQL_URL for MySQL"
                ));
            }
        };

        let l1_rpc_url = std::env::var("L1_RPC_URL")
            .map_err(|_| anyhow::anyhow!("L1_RPC_URL env var not found"))?;

        const REGISTRY_ADDRESS: &str = "REGISTRY_ADDRESS";
        let registry_address_str = std::env::var(REGISTRY_ADDRESS)
            .map_err(|_| anyhow::anyhow!("{} env var not found", REGISTRY_ADDRESS))?;
        let registry_address = Address::from_str(&registry_address_str)
            .map_err(|e| address_parse_error(REGISTRY_ADDRESS, e, &registry_address_str))?;

        let l1_start_block = std::env::var("L1_START_BLOCK")
            .unwrap_or("1".to_string())
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("L1_START_BLOCK must be a number"))
            .and_then(|val| {
                if val == 0 {
                    return Err(anyhow::anyhow!("L1_START_BLOCK must be a positive number"));
                }
                Ok(val)
            })?;

        let max_l1_fork_depth = std::env::var("MAX_L1_FORK_DEPTH")
            .unwrap_or("2".to_string())
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("MAX_L1_FORK_DEPTH must be a number"))?;

        // How many blocks to index at once when we are not yet fully synced
        let index_block_batch_size = std::env::var("INDEX_BLOCK_BATCH_SIZE")
            .unwrap_or("1".to_string())
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("INDEX_BLOCK_BATCH_SIZE must be a number"))
            .and_then(|val| {
                if val == 0 {
                    return Err(anyhow::anyhow!(
                        "INDEX_BLOCK_BATCH_SIZE must be a positive number"
                    ));
                }
                Ok(val)
            })?;

        tracing::info!(
            "Startup config:\ndatabase: {}\nl1_rpc_url: {}\nregistry_address: {}\nl1_start_block: {}\nmax_l1_fork_depth: {}\nindex_block_batch_size: {}",
            database.description(),
            l1_rpc_url,
            registry_address,
            l1_start_block,
            max_l1_fork_depth,
            index_block_batch_size
        );

        Ok(Config {
            database,
            l1_rpc_url,
            registry_address,
            l1_start_block,
            max_l1_fork_depth,
            index_block_batch_size,
        })
    }
}
