use anyhow::Error;

pub struct Config {
    pub db_filename: String,
    pub l1_rpc_url: String,
    pub registry_address: String,
    pub l1_start_block: u64,
    pub max_l1_fork_depth: u64,
    pub index_block_batch_size: u64,
}

impl Config {
    pub fn new() -> Result<Self, Error> {
        // Load environment variables from .env file
        let env_path = format!("{}/.env", env!("CARGO_MANIFEST_DIR"));
        dotenvy::from_path(env_path).ok();

        let db_filename = std::env::var("DB_FILENAME")
            .map_err(|_| anyhow::anyhow!("DB_FILENAME env var not found"))?;

        let db_filename = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), db_filename);

        let l1_rpc_url = std::env::var("L1_RPC_URL")
            .map_err(|_| anyhow::anyhow!("L1_RPC_URL env var not found"))?;

        let registry_address = std::env::var("REGISTRY_ADDRESS")
            .map_err(|_| anyhow::anyhow!("REGISTRY_ADDRESS env var not found"))?;

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

        let index_block_batch_size = std::env::var("INDEX_BLOCK_BATCH_SIZE")
            .unwrap_or("10".to_string())
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("INDEX_BLOCK_BATCH_SIZE must be a number"))?;

        tracing::info!(
            "Startup config:\ndb_filename: {}\nl1_rpc_url: {}\nregistry_address: {}\nl1_start_block: {}\nmax_l1_fork_depth: {}\nindex_block_batch_size: {}",
            db_filename,
            l1_rpc_url,
            registry_address,
            l1_start_block,
            max_l1_fork_depth,
            index_block_batch_size
        );

        Ok(Config {
            db_filename,
            l1_rpc_url,
            registry_address,
            l1_start_block,
            max_l1_fork_depth,
            index_block_batch_size,
        })
    }
}
