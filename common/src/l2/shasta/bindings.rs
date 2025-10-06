#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol! {

    library LibBonds {
        enum BondType {
            NONE,
            PROVABILITY,
            LIVENESS
        }

        struct BondInstruction {
            uint48 proposalId;
            BondType bondType;
            address payer;
            address payee;
        }
        function aggregateBondInstruction(
            bytes32 _bondInstructionsHash,
            BondInstruction memory _bondInstruction
        )
            internal
            pure
            returns (bytes32);
    }

    #[sol(rpc)]
    abstract contract ShastaAnchor {
        struct State {
            bytes32 bondInstructionsHash; // Latest known bond instructions hash
            uint48 anchorBlockNumber; // Latest L1 block number anchored to L2
            address designatedProver; // The prover designated for the current batch
            bool isLowBondProposal; // Indicates if the proposal has insufficient bonds
            uint48 endOfSubmissionWindowTimestamp; // The timestamp of the last slot where the current
                // preconfer can submit preconf-ed blocks to the L2 network.
        }

        function updateState(
            // Proposal level fields - define the overall batch
            uint48 _proposalId,
            address _proposer,
            bytes calldata _proverAuth,
            bytes32 _bondInstructionsHash,
            LibBonds.BondInstruction[] calldata _bondInstructions,
            // Block level fields - specific to this block in the proposal
            uint16 _blockIndex,
            uint48 _anchorBlockNumber,
            bytes32 _anchorBlockHash,
            bytes32 _anchorStateRoot,
            uint48 _endOfSubmissionWindowTimestamp
        )
            external
            onlyGoldenTouch
            nonReentrant
            returns (State memory previousState_, State memory newState_);
    }
}
