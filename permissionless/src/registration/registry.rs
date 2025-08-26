#![allow(unused)] // TODO: remove this once we have a used ethereum_l1 field

use crate::l1::execution_layer::ExecutionLayer;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;

pub struct Registry {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
}

impl Registry {
    pub fn new(ethereum_l1: Arc<EthereumL1<ExecutionLayer>>) -> Self {
        Self { ethereum_l1 }
    }

    async fn pull_reistriation_events(&self) {}
}
