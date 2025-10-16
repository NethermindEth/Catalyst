#[derive(Clone)]
pub struct BaseFeeConfig {
    pub adjustment_quotient: u8,
    pub sharing_pctg: u8,
    pub gas_issuance_per_second: u32,
    pub min_gas_excess: u64,
    pub max_gas_issuance_per_block: u32,
}

#[derive(Clone)]
pub struct ProtocolConfig {
    pub base_fee_config: BaseFeeConfig,
    pub max_blocks_per_batch: u16,
    pub max_anchor_height_offset: u64,
    pub block_max_gas_limit: u32,
}
