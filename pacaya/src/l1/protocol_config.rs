use super::bindings::taiko_inbox;

#[derive(Clone, Default)]
pub struct BaseFeeConfig {
    adjustment_quotient: u8,
    sharing_pctg: u8,
    gas_issuance_per_second: u32,
    min_gas_excess: u64,
    max_gas_issuance_per_block: u32,
}

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    base_fee_config: BaseFeeConfig,
    max_blocks_per_batch: u16,
    max_anchor_height_offset: u64,
    block_max_gas_limit: u32,
}

impl ProtocolConfig {
    pub fn from(pacaya_config: taiko_inbox::ITaikoInbox::Config) -> Self {
        Self {
            base_fee_config: BaseFeeConfig {
                adjustment_quotient: pacaya_config.baseFeeConfig.adjustmentQuotient,
                sharing_pctg: pacaya_config.baseFeeConfig.sharingPctg,
                gas_issuance_per_second: pacaya_config.baseFeeConfig.gasIssuancePerSecond,
                min_gas_excess: pacaya_config.baseFeeConfig.minGasExcess,
                max_gas_issuance_per_block: pacaya_config.baseFeeConfig.maxGasIssuancePerBlock,
            },
            max_blocks_per_batch: pacaya_config.maxBlocksPerBatch,
            max_anchor_height_offset: pacaya_config.maxAnchorHeightOffset,
            block_max_gas_limit: pacaya_config.blockMaxGasLimit,
        }
    }

    pub fn get_base_fee_adjustment_quotient(&self) -> u8 {
        self.base_fee_config.adjustment_quotient
    }

    pub fn get_base_fee_sharing_pctg(&self) -> u8 {
        self.base_fee_config.sharing_pctg
    }

    pub fn get_base_fee_gas_issuance_per_second(&self) -> u32 {
        self.base_fee_config.gas_issuance_per_second
    }

    pub fn get_base_fee_min_gas_excess(&self) -> u64 {
        self.base_fee_config.min_gas_excess
    }

    pub fn get_base_fee_max_gas_issuance_per_block(&self) -> u32 {
        self.base_fee_config.max_gas_issuance_per_block
    }

    pub fn get_block_max_gas_limit(&self) -> u32 {
        self.block_max_gas_limit
    }

    pub fn get_config_max_blocks_per_batch(&self) -> u16 {
        self.max_blocks_per_batch
    }

    pub fn get_config_max_anchor_height_offset(&self) -> u64 {
        self.max_anchor_height_offset
    }

    pub fn get_config_block_max_gas_limit(&self) -> u32 {
        self.block_max_gas_limit
    }

    pub fn clone_base_fee_config(&self) -> BaseFeeConfig {
        self.base_fee_config.clone()
    }

    pub fn get_protocol_config(&self) -> ProtocolConfig {
        self.clone()
    }
}
