use anyhow::Error;

pub fn read_jwt_secret(file_path: &str) -> Result<[u8; 32], Error> {
    tracing::info!("Reading JWT secret from file: {}", file_path);
    let secret = std::fs::read_to_string(file_path)
        .map_err(|e| anyhow::anyhow!("Failed to read JWT secret from file: {}", e))?;
    let secret_bytes = hex::decode(secret.strip_prefix("0x").unwrap_or(&secret))
        .map_err(|e| anyhow::anyhow!(" Failed to decode hex string from JWT secret file: {}", e))?;
    let secret_bytes: [u8; 32] = secret_bytes.try_into().map_err(|e| {
        anyhow::anyhow!(
            "Failed to convert secret bytes to [u8; 32] from JWT secret file: {:?}",
            e
        )
    })?;
    Ok(secret_bytes)
}
