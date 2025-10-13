#[allow(clippy::all)]
use crate::l1::pacaya::execution_layer::ExecutionLayer;
use anyhow::Error;
use common::{
    l1::{el_trait::ELTrait, ethereum_l1::EthereumL1},
    shared::{l2_block::L2Block, l2_tx_lists::PreBuiltTxList},
};
use std::{str::FromStr, sync::Arc};
use tokio::time::{Duration, sleep};
use tracing::info;

#[allow(clippy::all)]
pub async fn test_gas_params(
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    blocks: u32,
    anchor_height_lag: u64,
    max_bytes_size_of_batch: u64,
    mut transaction_error_receiver: tokio::sync::mpsc::Receiver<
        common::l1::transaction_error::TransactionError,
    >,
) -> Result<(), Error> {
    let timestamp_sec = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)?
        .as_secs()
        - 20;

    let tx: alloy::rpc::types::Transaction = serde_json::from_str(
        r#"{
        "blockHash": "0xdd31b8f2c9bbc36ecaadfded27d756e9c941751a7f75b0727f31f5f07a08a7fa",
        "blockNumber": "0x38f6aa",
        "from": "0xc4b0f902f4ced6dc1ad1be7ffec47ab50845955e",
        "gas": "0x7edc5",
        "gasPrice": "0x147096",
        "maxFeePerGas": "0x1470a5",
        "maxPriorityFeePerGas": "0x147085",
        "hash": "0x13490f4b6dcb0bf3d2d4f6fd08ec5da0922d4c24c871a931085bcff1184d1b51",
        "input": "0x60806040526040516104a73803806104a7833981016040819052610022916102b0565b61002d82825f610034565b50506103ca565b61003d8361005f565b5f825111806100495750805b1561005a57610058838361009e565b505b505050565b610068816100ca565b6040516001600160a01b038216907fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b905f90a250565b60606100c383836040518060600160405280602781526020016104806027913961017d565b9392505050565b6001600160a01b0381163b61013c5760405162461bcd60e51b815260206004820152602d60248201527f455243313936373a206e657720696d706c656d656e746174696f6e206973206e60448201526c1bdd08184818dbdb9d1c9858dd609a1b60648201526084015b60405180910390fd5b7f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc80546001600160a01b0319166001600160a01b0392909216919091179055565b60605f5f856001600160a01b031685604051610199919061037f565b5f60405180830381855af49150503d805f81146101d1576040519150601f19603f3d011682016040523d82523d5f602084013e6101d6565b606091505b5090925090506101e8868383876101f2565b9695505050505050565b606083156102605782515f03610259576001600160a01b0385163b6102595760405162461bcd60e51b815260206004820152601d60248201527f416464726573733a2063616c6c20746f206e6f6e2d636f6e74726163740000006044820152606401610133565b508161026a565b61026a8383610272565b949350505050565b8151156102825781518083602001fd5b8060405162461bcd60e51b81526004016101339190610395565b634e487b7160e01b5f52604160045260245ffd5b5f5f604083850312156102c1575f5ffd5b82516001600160a01b03811681146102d7575f5ffd5b60208401519092506001600160401b038111156102f2575f5ffd5b8301601f81018513610302575f5ffd5b80516001600160401b0381111561031b5761031b61029c565b604051601f8201601f19908116603f011681016001600160401b03811182821017156103495761034961029c565b604052818152828201602001871015610360575f5ffd5b8160208401602083015e5f602083830101528093505050509250929050565b5f82518060208501845e5f920191825250919050565b602081525f82518060208401528060208501604085015e5f604082850101526040601f19601f83011684010191505092915050565b60aa806103d65f395ff3fe608060405236601057600e6013565b005b600e5b601f601b6021565b6057565b565b5f60527f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc546001600160a01b031690565b905090565b365f5f375f5f365f845af43d5f5f3e8080156070573d5ff35b3d5ffdfea264697066735822122023045cdf151263894c3d51bb4ebb80a0f8c87ee631ed31876aa98f2b65ba4c0464736f6c634300081b0033416464726573733a206c6f772d6c6576656c2064656c65676174652063616c6c206661696c6564000000000000000000000000b2059ee0fde9f4f427fa3d3cc5d13d3dfd03be1e00000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000044f09a4016000000000000000000000000c4b0f902f4ced6dc1ad1be7ffec47ab50845955e000000000000000000000000c4b0f902f4ced6dc1ad1be7ffec47ab50845955e00000000000000000000000000000000000000000000000000000000",
        "nonce": "0x3",
        "to": null,
        "transactionIndex": "0x1d",
        "value": "0x0",
        "type": "0x2",
        "accessList": [],
        "chainId": "0x4268",
        "v": "0x1",
        "r": "0xcee69e0ca80103c4fc286bd5d79fca511077cdb6d008ce54dbf31124fb9e8a73",
        "s": "0x441c79fd9c663aba862884c8507bbe07e2f88a9e2a4bb81290a4ff3552cf515a",
        "yParity": "0x1"
    }"#,
    )?;

    let blocks: u64 = blocks.into();
    let tx_per_block = max_bytes_size_of_batch / 10 / blocks;
    info!("tx_per_block: {}", tx_per_block);

    let mut tx_vec = Vec::new();
    for _ in 0..tx_per_block {
        tx_vec.push(tx.clone());
    }

    let mut l2_blocks: Vec<L2Block> = Vec::new();
    for _ in 0..blocks {
        let l2_block = L2Block {
            prebuilt_tx_list: PreBuiltTxList {
                tx_list: tx_vec.clone(),
                estimated_gas_used: 0,
                bytes_length: 10,
            },
            timestamp_sec,
        };
        l2_blocks.push(l2_block);
    }

    let coinbase =
        alloy::primitives::Address::from_str("0xC4B0F902f4CEd6dC1Ad1Be7FFeC47ab50845955e")?;

    let l1_height = ethereum_l1.execution_layer.common().get_l1_height().await?;
    let anchor_block_id = l1_height - anchor_height_lag;

    info!("anchor_block_id: {}", anchor_block_id);
    info!("l1_height: {}", l1_height);

    let current_timestamp = ethereum_l1.slot_clock.get_current_slot_begin_timestamp()?;

    let _ = ethereum_l1
        .execution_layer
        .send_batch_to_l1(
            l2_blocks,
            anchor_block_id,
            coinbase,
            current_timestamp,
            None,
        )
        .await;

    while ethereum_l1
        .execution_layer
        .common()
        .is_transaction_in_progress()
        .await?
    {
        info!("Sleeping for 20 seconds...");
        sleep(Duration::from_secs(20)).await;
    }

    match transaction_error_receiver.try_recv() {
        Ok(error) => {
            panic!("Received transaction error: {error:#?}");
        }
        Err(err) => match err {
            tokio::sync::mpsc::error::TryRecvError::Empty => {
                // no errors, proceed with preconfirmation
            }
            tokio::sync::mpsc::error::TryRecvError::Disconnected => {
                panic!("Transaction error channel disconnected");
            }
        },
    }

    info!("Done");

    Ok(())
}
