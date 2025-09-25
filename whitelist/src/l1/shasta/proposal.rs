use alloy::primitives::{FixedBytes, Uint};

use super::bindings::iinbox;

struct Proposal {}

impl Proposal {
    pub fn build() {}

    fn construct_propose_input() {
        let core_state = iinbox::IInbox::CoreState {
            nextProposalId: Uint::<48, 1>::from(0),
            nextProposalBlockId: Uint::<48, 1>::from(0),
            lastFinalizedProposalId: Uint::<48, 1>::from(0),
            lastFinalizedTransitionHash: FixedBytes::from([0u8; 32]),
            bondInstructionsHash: FixedBytes::from([0u8; 32]),
        };

        let blob_reference = iinbox::LibBlobs::BlobReference {
            blobStartIndex: 0u16,
            numBlobs: 0u16,
            offset: Uint::<24, 1>::from(0),
        };

        let checkpoint = iinbox::ICheckpointStore::Checkpoint {
            blockNumber: Uint::<48, 1>::from(0),
            blockHash: FixedBytes::from([0u8; 32]),
            stateRoot: FixedBytes::from([0u8; 32]),
        };

        let propose_input = iinbox::IInbox::ProposeInput {
            deadline: Uint::<48, 1>::from(0),
            coreState: core_state,
            parentProposals: vec![],
            blobReference: blob_reference,
            transitionRecords: vec![],
            checkpoint,
            numForcedInclusions: 0,
        };
    }

    pub fn send() {}
}
