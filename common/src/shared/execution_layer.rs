use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Block as RpcBlock, Filter, Log},
};
use anyhow::Error;
use tracing::debug;

pub struct ExecutionLayer {
    provider: DynProvider,
    chain_id: u64,
}

pub struct BlockInfo {
    pub timestamp: u64,
    pub hash: B256,
    pub state_root: B256,
}

impl ExecutionLayer {
    /// Creates a formatted error message with chain ID prefix
    pub fn chain_error(&self, message: &str, context: Option<&str>) -> Error {
        match context {
            Some(ctx) => Error::msg(format!(
                "[chain id: {}] {}: {}",
                self.chain_id, message, ctx
            )),
            None => Error::msg(format!("[chain id: {}] {}", self.chain_id, message)),
        }
    }

    pub async fn new(provider: DynProvider) -> Result<Self, Error> {
        debug!("Creating ExecutionLayer from provider");
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| Error::msg(format!("Failed to get chain ID: {e}")))?;

        Ok(Self { provider, chain_id })
    }

    pub async fn new_read_only(url: &str) -> Result<Self, Error> {
        debug!("Creating ExecutionLayer from URL: {}", url);
        let provider = super::alloy_tools::create_alloy_provider_without_wallet(url).await?;
        Self::new(provider).await
    }

    pub fn provider(&self) -> DynProvider {
        self.provider.clone()
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub async fn get_account_nonce(
        &self,
        account: Address,
        block: BlockNumberOrTag,
    ) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request("eth_getTransactionCount", (account, block))
            .await
            .map_err(|e| self.chain_error("Failed to get nonce", Some(&e.to_string())))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| self.chain_error("Failed to convert nonce", Some(&e.to_string())))
    }

    pub async fn get_account_balance(
        &self,
        account: Address,
    ) -> Result<alloy::primitives::U256, Error> {
        let balance = self.provider.get_balance(account).await?;
        Ok(balance)
    }

    pub async fn get_block_state_root_by_number(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| {
                self.chain_error(
                    &format!("Failed to get block by number ({number})"),
                    Some(&e.to_string()),
                )
            })?
            .ok_or_else(|| {
                self.chain_error(&format!("Failed to get block by number ({})", number), None)
            })?;
        Ok(block.header.state_root)
    }

    pub async fn get_block_info_by_number(&self, number: u64) -> Result<BlockInfo, Error> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| {
                self.chain_error(
                    &format!("Failed to get block by number ({})", number),
                    Some(&e.to_string()),
                )
            })?
            .ok_or_else(|| {
                self.chain_error(&format!("Failed to get block by number ({})", number), None)
            })?;

        Ok(BlockInfo {
            timestamp: block.header.timestamp,
            hash: block.header.hash,
            state_root: block.header.state_root,
        })
    }

    async fn get_block_timestamp_by_number_or_tag(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> Result<u64, Error> {
        let block = self
            .provider
            .get_block_by_number(block_number_or_tag)
            .await?
            .ok_or_else(|| {
                self.chain_error(
                    &format!("Failed to get block by number ({})", block_number_or_tag),
                    None,
                )
            })?;
        Ok(block.header.timestamp)
    }

    pub async fn get_block_timestamp_by_number(&self, block: u64) -> Result<u64, Error> {
        self.get_block_timestamp_by_number_or_tag(BlockNumberOrTag::Number(block))
            .await
    }

    pub async fn get_logs(&self, filter: Filter) -> Result<Vec<Log>, Error> {
        self.provider
            .get_logs(&filter)
            .await
            .map_err(|e| self.chain_error("Failed to get logs", Some(&e.to_string())))
    }

    pub async fn get_block_hash(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .get_block_header(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| self.chain_error("Failed to get block hash", Some(&e.to_string())))?;
        Ok(block.header.hash)
    }

    pub async fn get_block_header(&self, block: BlockNumberOrTag) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(block)
            .await
            .map_err(|e| self.chain_error("Failed to get block header", Some(&e.to_string())))?
            .ok_or_else(|| self.chain_error("Failed to get block header", None))
    }

    pub async fn get_latest_block_with_txs(&self) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await
            .map_err(|e| self.chain_error("Failed to get latest block", Some(&e.to_string())))?
            .ok_or_else(|| self.chain_error("Failed to get latest block", None))
    }

    pub async fn get_latest_block_id(&self) -> Result<u64, Error> {
        self.provider.get_block_number().await.map_err(|e| {
            self.chain_error("Failed to get latest block number", Some(&e.to_string()))
        })
    }

    pub async fn get_block_by_number(
        &self,
        number: u64,
        full_txs: bool,
    ) -> Result<alloy::rpc::types::Block, Error> {
        let mut block_by_number = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number));

        if full_txs {
            block_by_number = block_by_number.full();
        }

        block_by_number
            .await
            .map_err(|e| self.chain_error("Failed to get block by number", Some(&e.to_string())))?
            .ok_or_else(|| {
                self.chain_error(
                    &format!("Failed to get L2 block {}: value was None", number),
                    None,
                )
            })
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        self.provider
            .get_transaction_by_hash(hash)
            .await
            .map_err(|e| {
                self.chain_error("Failed to get L2 transaction by hash", Some(&e.to_string()))
            })?
            .ok_or_else(|| self.chain_error("Failed to get transaction: value is None", None))
    }

    pub async fn get_latest_block_number_and_timestamp(&self) -> Result<(u64, u64), Error> {
        let block = self
            .get_block_header(BlockNumberOrTag::Latest)
            .await
            .map_err(|e| {
                self.chain_error("Failed to get latest block timestamp", Some(&e.to_string()))
            })?;
        Ok((block.header.number, block.header.timestamp))
    }
}
