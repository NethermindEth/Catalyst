use super::batch::Batch;
use crate::l1::bindings::BatchParams;
use std::collections::VecDeque;

pub type ForcedInclusionBatch = Option<BatchParams>;
pub type BatchesToSend = VecDeque<(ForcedInclusionBatch, Batch)>;
