use prometheus::{
    Counter, CounterVec, Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, Opts, Registry,
    TextEncoder,
};
use tracing::error;

pub mod server;

pub struct Metrics {
    preconfer_eth_balance: Gauge,
    preconfer_taiko_balance: Gauge,
    preconfer_l2_eth_balance: Gauge,
    blocks_preconfirmed: Counter,
    blocks_reanchored: Counter,
    batch_recovered: Counter,
    batch_proposed: Counter,
    batch_confirmed: Counter,
    batch_propose_tries: Histogram,
    batch_block_count: Histogram,
    batch_blob_size: Histogram,
    block_tx_count: Histogram,
    rpc_driver_call_duration: HistogramVec,
    rpc_driver_call: CounterVec,
    rpc_driver_call_error: CounterVec,
    skipped_l2_slots_by_low_txs_count: Counter,
    critical_errors: Counter,
    operator_whitelisted: Gauge,
    registry: Registry,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let preconfer_eth_balance = Gauge::new(
            "preconfer_eth_balance",
            "Ethereum balance of the preconfer wallet",
        )
        .expect("Failed to create preconfer_eth_balance gauge");

        let preconfer_taiko_balance = Gauge::new(
            "preconfer_taiko_balance",
            "TAIKO balance of the preconfer wallet",
        )
        .expect("Failed to create preconfer_taiko_balance gauge");

        if let Err(err) = registry.register(Box::new(preconfer_eth_balance.clone())) {
            error!("Error: Failed to register preconfer_eth_balance: {}", err);
        }

        if let Err(err) = registry.register(Box::new(preconfer_taiko_balance.clone())) {
            error!("Error: Failed to register preconfer_taiko_balance: {}", err);
        }

        let preconfer_l2_eth_balance = Gauge::new(
            "preconfer_l2_eth_balance",
            "L2 ETH balance of the preconfer wallet",
        )
        .expect("Failed to create preconfer_l2_eth_balance gauge");

        if let Err(err) = registry.register(Box::new(preconfer_l2_eth_balance.clone())) {
            error!(
                "Error: Failed to register preconfer_l2_eth_balance: {}",
                err
            );
        }

        let blocks_preconfirmed = Counter::new(
            "blocks_preconfirmed",
            "Number of blocks preconfirmed by the node",
        )
        .expect("Failed to create blocks_preconfirmed counter");

        if let Err(err) = registry.register(Box::new(blocks_preconfirmed.clone())) {
            error!("Error: Failed to register blocks_preconfirmed: {}", err);
        }

        let blocks_reanchored = Counter::new(
            "blocks_reanchored",
            "Number of blocks reanchored by the node",
        )
        .expect("Failed to create blocks_reanchored counter");

        if let Err(err) = registry.register(Box::new(blocks_reanchored.clone())) {
            error!("Error: Failed to register blocks_reanchored: {}", err);
        }

        let batch_recovered =
            Counter::new("batch_recovered", "Number of batches recovered by the node")
                .expect("Failed to create batch_recovered counter");

        if let Err(err) = registry.register(Box::new(batch_recovered.clone())) {
            error!("Error: Failed to register batch_recovered: {}", err);
        }

        let batch_proposed =
            Counter::new("batch_proposed", "Number of batches proposed by the node")
                .expect("Failed to create batch_proposed counter");

        if let Err(err) = registry.register(Box::new(batch_proposed.clone())) {
            error!("Error: Failed to register batch_proposed: {}", err);
        }

        let batch_confirmed = Counter::new("batch_confirmed", "Number of batches landed on L1")
            .expect("Failed to create batch_confirmed counter");

        if let Err(err) = registry.register(Box::new(batch_confirmed.clone())) {
            error!("Error: Failed to register batch_confirmed: {}", err);
        }

        let opts = HistogramOpts::new("batch_propose_tries", "Number of tries to propose a batch")
            .buckets(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let batch_propose_tries = match Histogram::with_opts(opts) {
            Ok(histogram) => histogram,
            Err(err) => panic!("Failed to create batch_propose_tries histogram: {err}"),
        };

        if let Err(err) = registry.register(Box::new(batch_propose_tries.clone())) {
            error!("Error: Failed to register batch_propose_tries: {}", err);
        }

        let opts =
            HistogramOpts::new("batch_block_count", "Number of blocks in a batch").buckets(vec![
                76.0, 152.0, 228.0, 304.0, 380.0, 456.0, 532.0, 608.0, 684.0, 768.0,
            ]);
        let batch_block_count = match Histogram::with_opts(opts) {
            Ok(histogram) => histogram,
            Err(err) => panic!("Failed to create batch_block_count histogram: {err}"),
        };

        if let Err(err) = registry.register(Box::new(batch_block_count.clone())) {
            error!("Error: Failed to register batch_block_count: {}", err);
        }

        let opts = HistogramOpts::new("batch_blob_size", "Size of a batch's blob in bytes")
            .buckets(vec![
                13004.0, 26008.0, 39012.0, 52016.0, 65020.0, 78024.0, 91028.0, 104032.0, 117036.0,
                130044.0,
            ]);
        let batch_blob_size = match Histogram::with_opts(opts) {
            Ok(histogram) => histogram,
            Err(err) => panic!("Failed to create batch_blob_size histogram: {err}"),
        };

        if let Err(err) = registry.register(Box::new(batch_blob_size.clone())) {
            error!("Error: Failed to register batch_blob_size: {}", err);
        }

        let opts = HistogramOpts::new("block_tx_count", "Number of transactions in each block")
            .buckets(vec![
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
                16.0, 17.0, 18.0, 19.0, 20.0, 25.0, 30.0, 40.0, 50.0, 100.0, 200.0, 500.0, 1000.0,
            ]);
        let block_tx_count = match Histogram::with_opts(opts) {
            Ok(histogram) => histogram,
            Err(err) => panic!("Failed to create block_tx_count histogram: {err}"),
        };

        if let Err(err) = registry.register(Box::new(block_tx_count.clone())) {
            error!("Error: Failed to register block_tx_count: {}", err);
        }

        let opts = HistogramOpts::new(
            "rpc_driver_call_duration_seconds",
            "Duration of RPC calls to driver in seconds",
        )
        .buckets(vec![
            0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 20.0, 30.0, 40.0,
            59.0,
        ]);

        let rpc_driver_call_duration = match HistogramVec::new(opts, &["method"]) {
            Ok(histogram) => histogram,
            Err(err) => panic!("Failed to create rpc_driver_call_duration histogram: {err}"),
        };

        if let Err(err) = registry.register(Box::new(rpc_driver_call_duration.clone())) {
            error!(
                "Error: Failed to register rpc_driver_call_duration: {}",
                err
            );
        }

        let rpc_driver_call = match CounterVec::new(
            Opts::new("rpc_driver_call_counter", "Number of RPC calls to driver"),
            &["method"],
        ) {
            Ok(counter) => counter,
            Err(err) => panic!("Failed to create rpc_driver_call_counter counter: {err}"),
        };

        if let Err(err) = registry.register(Box::new(rpc_driver_call.clone())) {
            error!("Error: Failed to register rpc_driver_call_counter: {}", err);
        }

        let rpc_driver_call_error = match CounterVec::new(
            Opts::new(
                "rpc_driver_call_error_counter",
                "Number of RPC calls to driver that failed",
            ),
            &["method"],
        ) {
            Ok(counter) => counter,
            Err(err) => panic!("Failed to create rpc_driver_call_error_counter counter: {err}"),
        };

        if let Err(err) = registry.register(Box::new(rpc_driver_call_error.clone())) {
            error!(
                "Error: Failed to register rpc_driver_call_error_counter: {}",
                err
            );
        }

        let skipped_l2_slots_by_low_txs_count = Counter::new(
            "skipped_l2_slots_by_low_txs_count",
            "Number of skipped L2 slots by low txs count",
        )
        .expect("Failed to create skipped_l2_slots_by_low_txs_count counter");

        if let Err(err) = registry.register(Box::new(skipped_l2_slots_by_low_txs_count.clone())) {
            error!(
                "Error: Failed to register skipped_l2_slots_by_low_txs_count: {}",
                err
            );
        }

        let critical_errors = Counter::new("critical_errors", "Number of critical errors")
            .expect("Failed to create critical_errors counter");

        if let Err(err) = registry.register(Box::new(critical_errors.clone())) {
            error!("Error: Failed to register critical_errors: {}", err);
        }

        let operator_whitelisted = Gauge::new(
            "operator_whitelisted",
            "Whether the operator is whitelisted (1.0 = true, 0.0 = false)",
        )
        .expect("Failed to create operator_whitelisted gauge");

        if let Err(err) = registry.register(Box::new(operator_whitelisted.clone())) {
            error!("Error: Failed to register operator_whitelisted: {}", err);
        }

        Self {
            preconfer_eth_balance,
            preconfer_taiko_balance,
            preconfer_l2_eth_balance,
            blocks_preconfirmed,
            blocks_reanchored,
            batch_recovered,
            batch_proposed,
            batch_confirmed,
            batch_propose_tries,
            batch_block_count,
            batch_blob_size,
            block_tx_count,
            rpc_driver_call_duration,
            rpc_driver_call,
            rpc_driver_call_error,
            skipped_l2_slots_by_low_txs_count,
            critical_errors,
            operator_whitelisted,
            registry,
        }
    }

    pub fn set_preconfer_eth_balance(&self, balance: alloy::primitives::U256) {
        self.preconfer_eth_balance
            .set(Metrics::u256_to_f64(balance));
    }

    pub fn set_preconfer_taiko_balance(&self, balance: alloy::primitives::U256) {
        self.preconfer_taiko_balance
            .set(Metrics::u256_to_f64(balance));
    }

    pub fn set_preconfer_l2_eth_balance(&self, balance: alloy::primitives::U256) {
        self.preconfer_l2_eth_balance
            .set(Metrics::u256_to_f64(balance));
    }

    pub fn inc_blocks_preconfirmed(&self) {
        self.blocks_preconfirmed.inc();
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn inc_by_blocks_reanchored(&self, value: u64) {
        self.blocks_reanchored.inc_by(value as f64);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn inc_by_batch_recovered(&self, value: u64) {
        self.batch_recovered.inc_by(value as f64);
    }

    pub fn inc_batch_proposed(&self) {
        self.batch_proposed.inc();
    }

    pub fn inc_batch_confirmed(&self) {
        self.batch_confirmed.inc();
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn observe_batch_propose_tries(&self, tries: u64) {
        self.batch_propose_tries.observe(tries as f64);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn observe_batch_info(&self, block_count: u64, blob_size: u64) {
        self.batch_block_count.observe(block_count as f64);
        self.batch_blob_size.observe(blob_size as f64);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn observe_block_tx_count(&self, tx_count: u64) {
        self.block_tx_count.observe(tx_count as f64);
    }

    pub fn observe_rpc_driver_call_duration(&self, method: &str, duration: f64) {
        if let Ok(metric) = self
            .rpc_driver_call_duration
            .get_metric_with_label_values(&[method])
        {
            metric.observe(duration);
        } else {
            error!(
                "Failed to observe RPC driver call duration for method: {}",
                method
            );
        }
    }

    pub fn inc_rpc_driver_call(&self, method: &str) {
        if let Ok(metric) = self.rpc_driver_call.get_metric_with_label_values(&[method]) {
            metric.inc();
        } else {
            error!(
                "Failed to increment RPC driver call counter for method: {}",
                method
            );
        }
    }

    pub fn inc_rpc_driver_call_error(&self, method: &str) {
        if let Ok(metric) = self
            .rpc_driver_call_error
            .get_metric_with_label_values(&[method])
        {
            metric.inc();
        } else {
            error!(
                "Failed to increment RPC driver call error counter for method: {}",
                method
            );
        }
    }

    pub fn inc_skipped_l2_slots_by_low_txs_count(&self) {
        self.skipped_l2_slots_by_low_txs_count.inc();
    }

    pub fn inc_critical_errors(&self) {
        tracing::debug!("Incrementing critical errors metric");
        self.critical_errors.inc();
    }

    pub fn set_operator_whitelisted(&self, whitelisted: bool) {
        self.operator_whitelisted
            .set(if whitelisted { 1.0 } else { 0.0 });
    }

    fn u256_to_f64(balance: alloy::primitives::U256) -> f64 {
        let balance_str = balance.to_string();
        let len = balance_str.len();

        // Handle very small numbers
        if len < 14 {
            return 0f64;
        }

        // Convert to f64 and divide by 10^18 to get the correct decimal places
        let value = balance_str.parse::<f64>().unwrap_or(0f64);
        value / 1_000_000_000_000_000_000.0
    }

    pub fn gather(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();

        // Handle encoding error
        if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
            error!("Failed to encode metrics: {}", e);
            return String::new();
        }

        // Handle UTF-8 conversion error
        match String::from_utf8(buffer) {
            Ok(metrics) => metrics,
            Err(e) => {
                error!("Failed to convert metrics to UTF-8: {}", e);
                String::new()
            }
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gather() {
        let metrics = Metrics::new();

        // Set some test values
        metrics.set_preconfer_eth_balance(alloy::primitives::U256::from(1000000000000000000u128));
        metrics.set_preconfer_taiko_balance(alloy::primitives::U256::from(2000000000000000000u128));
        metrics.inc_blocks_preconfirmed();
        metrics.inc_by_blocks_reanchored(1);
        metrics.inc_by_batch_recovered(1);
        metrics.inc_batch_proposed();
        metrics.inc_batch_confirmed();
        metrics.observe_batch_propose_tries(1);
        metrics.observe_batch_info(5, 1000);
        metrics.observe_block_tx_count(3);
        metrics.inc_skipped_l2_slots_by_low_txs_count();
        metrics.inc_critical_errors();

        let output = metrics.gather();
        println!("{output}");

        // Verify the output contains our metrics
        assert!(output.contains("preconfer_eth_balance 1"));
        assert!(output.contains("preconfer_taiko_balance 2"));
        assert!(output.contains("blocks_preconfirmed 1"));
        assert!(output.contains("blocks_reanchored 1"));
        assert!(output.contains("batch_recovered 1"));
        assert!(output.contains("batch_proposed 1"));
        assert!(output.contains("batch_confirmed 1"));
        assert!(output.contains("batch_propose_tries_count 1"));
        assert!(output.contains("batch_block_count_sum 5"));
        assert!(output.contains("batch_blob_size_sum 1000"));
        assert!(output.contains("block_tx_count_count 1"));
        assert!(output.contains("block_tx_count_sum 3"));
        assert!(output.contains("skipped_l2_slots_by_low_txs_count 1"));
        assert!(output.contains("critical_errors 1"));
    }

    #[test]
    fn test_u256_to_f64() {
        // Test 1 ETH (18 decimals)
        let one_eth = alloy::primitives::U256::from(1000000000000000000u128);
        assert_eq!(Metrics::u256_to_f64(one_eth), 1.0);

        // Test 0.5 ETH
        let half_eth = alloy::primitives::U256::from(500000000000000000u128);
        assert_eq!(Metrics::u256_to_f64(half_eth), 0.5);

        // Test 1234.56789 ETH
        assert_eq!(
            Metrics::u256_to_f64(alloy::primitives::U256::from(1234567890000000000000u128)),
            1234.56789
        );

        // Test 0.1 ETH
        let point_one_eth = alloy::primitives::U256::from(100000000000000000u128);
        assert_eq!(Metrics::u256_to_f64(point_one_eth), 0.1);

        // Test 0.00001 ETH
        let small_eth = alloy::primitives::U256::from(10000000000000u128);
        assert_eq!(Metrics::u256_to_f64(small_eth), 0.00001);

        // Test 1000 ETH
        let thousand_eth = alloy::primitives::U256::from(1000000000000000000000u128);
        assert_eq!(Metrics::u256_to_f64(thousand_eth), 1000.0);

        // Test very small number (should return 0)
        let tiny = alloy::primitives::U256::from(1000u128);
        assert_eq!(Metrics::u256_to_f64(tiny), 0.0);

        // Test zero
        let zero = alloy::primitives::U256::from(0u128);
        assert_eq!(Metrics::u256_to_f64(zero), 0.0);

        // Test number with more than 18 decimals
        let large = alloy::primitives::U256::from(123456789012345678901234567890u128);
        assert_eq!(Metrics::u256_to_f64(large), 123_456_789_012.345_67);
    }
}
