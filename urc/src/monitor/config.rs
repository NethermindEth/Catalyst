pub struct Config {
    pub db_filename: String,
    pub l1_rpc_url: String,
    pub registry_address: String,
    pub l1_start_block: u64,
}

impl Config {
    pub fn new() -> Self {
        // Load environment variables from .env file
        let env_path = format!("{}/.env", env!("CARGO_MANIFEST_DIR"));
        dotenvy::from_path(env_path).ok();

        let db_filename = std::env::var("DB_FILENAME").unwrap_or_else(|_| {
            panic!("DB_FILENAME env var not found");
        });

        let db_filename = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), db_filename);

        let l1_rpc_url = std::env::var("L1_RPC_URL").unwrap_or_else(|_| {
            panic!("L1_RPC_URL env var not found");
        });

        let registry_address = std::env::var("REGISTRY_ADDRESS").unwrap_or_else(|_| {
            panic!("REGISTRY_ADDRESS env var not found");
        });

        let l1_start_block = std::env::var("L1_START_BLOCK")
            .unwrap_or("0".to_string())
            .parse::<u64>()
            .inspect(|&val| {
                if val == 0 {
                    panic!("L1_START_BLOCK must be a positive number");
                }
            })
            .expect("L1_START_BLOCK must be a number");

        Config {
            db_filename,
            l1_rpc_url,
            registry_address,
            l1_start_block,
        }
    }
}
