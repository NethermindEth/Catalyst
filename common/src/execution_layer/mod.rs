use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Block as RpcBlock, Log},
};
use anyhow::Error;

pub struct ExecutionLayer {
    provider: DynProvider,
    chain_id: u64,
}

impl ExecutionLayer {
    pub async fn new(provider: DynProvider) -> Result<Self, Error> {
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| Error::msg(format!("Failed to get chain ID: {e}")))?;

        Ok(Self { provider, chain_id })
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
            .map_err(|e| Error::msg(format!("Failed to get nonce: {e}")))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| Error::msg(format!("Failed to convert nonce: {e}")))
    }

    pub async fn get_chain_height(&self) -> Result<u64, Error> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| Error::msg(format!("Failed to get L1 height: {e}")))
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
            .map_err(|e| Error::msg(format!("Failed to get block by number ({number}): {e}")))?
            .ok_or(anyhow::anyhow!("Failed to get block by number ({number})"))?;
        Ok(block.header.state_root)
    }

    async fn get_block_timestamp_by_number_or_tag(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> Result<u64, Error> {
        let block = self
            .provider
            .get_block_by_number(block_number_or_tag)
            .await?
            .ok_or(anyhow::anyhow!(
                "Failed to get block by number ({})",
                block_number_or_tag
            ))?;
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
            .map_err(|e| Error::msg(format!("Failed to get logs: {e}")))
    }

    pub async fn get_block_hash(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .get_block_header(BlockNumberOrTag::Number(number))
            .await?;
        Ok(block.header.hash)
    }

    pub async fn get_block_header(&self, block: BlockNumberOrTag) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(block)
            .await
            .map_err(|e| anyhow::anyhow!("[chain_id: {}]  Failed to get  block header: {}", self.chain_id, e))?
            .ok_or(anyhow::anyhow!("[chain_id: {}] Failed to get block header", self.chain_id))
    }

    pub async fn get_latest_block_with_txs(&self) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await
            .map_err(|e| anyhow::anyhow!("[chain_id: {}]  Failed to get latest block: {}", self.chain_id, e))?
            .ok_or(anyhow::anyhow!("[chain_id: {}]  Failed to get latest block", self.chain_id))
    }

    pub async fn get_latest_block_id(&self) -> Result<u64, Error> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("[chain_id: {}] Failed to get latest block number: {}",self.chain_id, e))
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
            .map_err(|e| anyhow::anyhow!("[chain_id: {}]  Failed to get block by number: {}", self.chain_id, e))?
            .ok_or(anyhow::anyhow!(
                "[chain_id: {}]  Failed to get L2 block {}: value was None",
                self.chain_id,
                number
            ))
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        self.provider
            .get_transaction_by_hash(hash)
            .await
            .map_err(|e| anyhow::anyhow!("[chain_id: {}] Failed to get L2 transaction by hash: {}", self.chain_id, e))?
            .ok_or(anyhow::anyhow!(
                "[chain_id: {}] Failed to get transaction: value is None",
                self.chain_id
            ))
    }
}
