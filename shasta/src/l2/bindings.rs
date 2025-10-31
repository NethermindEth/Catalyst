#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    BondManager,
    "src/l2/abi/BondManager.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    Bridge,
    "src/l2/abi/Bridge.json"
);
/*
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
    contract Anchor {
        struct ProverAuth {
            uint48 proposalId; // The proposal ID this auth is for
            address proposer; // The original proposer address
            uint256 provingFee; // Fee (Wei) that prover will receive
            bytes signature; // ECDSA signature from the designated prover
        }

        struct State {
            bytes32 bondInstructionsHash; // Latest known bond instructions hash
            uint48 anchorBlockNumber; // Latest L1 block number anchored to L2
            address designatedProver; // The prover designated for the current batch
            bool isLowBondProposal; // Indicates if the proposal has insufficient bonds
            uint48 endOfSubmissionWindowTimestamp; // The timestamp of the last slot where the current
                // preconfer can submit preconf-ed blocks to the L2 network.
        }

        /// @notice Proposal-level data that applies to the entire batch of blocks.
        struct ProposalParams {
            uint48 proposalId; // Unique identifier of the proposal
            address proposer; // Address of the entity that proposed this batch
            bytes proverAuth; // Encoded ProverAuth for prover designation
            bytes32 bondInstructionsHash; // Expected hash of bond instructions
            LibBonds.BondInstruction[] bondInstructions; // Bond credit instructions to process
        }

        /// @notice Block-level data specific to a single block within a proposal.
        struct BlockParams {
            uint16 blockIndex; // Current block index within the proposal (0-based)
            uint48 anchorBlockNumber; // L1 block number to anchor (0 to skip)
            bytes32 anchorBlockHash; // L1 block hash at anchorBlockNumber
            bytes32 anchorStateRoot; // L1 state root at anchorBlockNumber
        }

        address public bondManager;

        function anchorV4(
            ProposalParams calldata _proposalParams,
            BlockParams calldata _blockParams
        )
            external
            onlyValidSender
            nonReentrant;
    }
}
*/
