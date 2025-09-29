use alloy::sol;

pub mod iinbox {
    use super::*;

    sol!(

    library LibBlobs {
        // ---------------------------------------------------------------
        // Constants
        // ---------------------------------------------------------------
        uint256 internal constant FIELD_ELEMENT_BYTES = 32;
        uint256 internal constant BLOB_FIELD_ELEMENTS = 4096;
        uint256 internal constant BLOB_BYTES = BLOB_FIELD_ELEMENTS * FIELD_ELEMENT_BYTES;

        // ---------------------------------------------------------------
        // Structs
        // ---------------------------------------------------------------

        /// @notice Represents a segment of data that is stored in multiple consecutive blobs created
        /// in this transaction.
        struct BlobReference {
            /// @notice The starting index of the blob.
            uint16 blobStartIndex;
            /// @notice The number of blobs.
            uint16 numBlobs;
            /// @notice The field-element offset within the blob data.
            uint24 offset;
        }

        /// @notice Represents a frame of data that is stored in multiple blobs. Note the size is
        /// encoded as a bytes32 at the offset location.
        struct BlobSlice {
            /// @notice The blobs containing the proposal's content.
            bytes32[] blobHashes;
            /// @notice The byte offset of the proposal's content in the containing blobs.
            uint24 offset;
            /// @notice The timestamp when the frame was created.
            uint48 timestamp;
        }

        // ---------------------------------------------------------------
        // Functions
        // ---------------------------------------------------------------

        /// @dev Validates a blob locator and converts it to a blob slice.
        /// @param _blobReference The blob locator to validate.
        /// @return The blob slice.
        function validateBlobReference(BlobReference memory _blobReference)
            internal
            view
            returns (BlobSlice memory)
        {
            require(_blobReference.numBlobs > 0, NoBlobs());

            bytes32[] memory blobHashes = new bytes32[](_blobReference.numBlobs);
            for (uint256 i; i < _blobReference.numBlobs; ++i) {
                blobHashes[i] = blobhash(_blobReference.blobStartIndex + i);
                require(blobHashes[i] != 0, BlobNotFound());
            }

            return BlobSlice({
                blobHashes: blobHashes,
                offset: _blobReference.offset,
                timestamp: uint48(block.timestamp)
            });
        }

        // ---------------------------------------------------------------
        // Errors
        // ---------------------------------------------------------------

        error BlobNotFound();
        error NoBlobs();
    }


    library LibBonds {
        // ---------------------------------------------------------------
        // Enums
        // ---------------------------------------------------------------

        enum BondType {
            NONE,
            PROVABILITY,
            LIVENESS
        }

        // ---------------------------------------------------------------
        // Structs
        // ---------------------------------------------------------------

        struct BondInstruction {
            uint48 proposalId;
            BondType bondType;
            address payer;
            address payee;
        }

        // ---------------------------------------------------------------
        // Internal Functions
        // ---------------------------------------------------------------

        function aggregateBondInstruction(
            bytes32 _bondInstructionsHash,
            BondInstruction memory _bondInstruction
        )
            internal
            pure
            returns (bytes32)
        {
            return _bondInstruction.proposalId == 0 || _bondInstruction.bondType == BondType.NONE
                ? _bondInstructionsHash
                : keccak256(abi.encode(_bondInstructionsHash, _bondInstruction));
        }
    }


    interface ICheckpointStore {
        // ---------------------------------------------------------------
        // Structs
        // ---------------------------------------------------------------

        /// @notice Represents a synced checkpoint
        struct Checkpoint {
            uint48 blockNumber;
            /// @notice The block hash for the end (last) L2 block in this proposal.
            bytes32 blockHash;
            /// @notice The state root for the end (last) L2 block in this proposal.
            bytes32 stateRoot;
        }

        // ---------------------------------------------------------------
        // Events
        // ---------------------------------------------------------------

        /// @notice Emitted when a checkpoint is saved
        /// @param blockNumber The block number
        /// @param blockHash The block hash
        /// @param stateRoot The state root
        event CheckpointSaved(uint48 indexed blockNumber, bytes32 blockHash, bytes32 stateRoot);

        // ---------------------------------------------------------------
        // External Functions
        // ---------------------------------------------------------------

        /// @notice Gets a checkpoint by index
        /// @param _offset The offset of the checkpoint. Use 0 for the last checkpoint, 1 for the
        /// second last, etc.
        /// @return _ The checkpoint
        function getCheckpoint(uint48 _offset) external view returns (Checkpoint memory);

        /// @notice Gets the latest checkpoint number
        /// @return _ The latest checkpoint number
        function getLatestCheckpointBlockNumber() external view returns (uint48);

        /// @notice Gets the number of checkpoints
        /// @return _ The number of checkpoints
        function getNumberOfCheckpoints() external view returns (uint48);
    }


    interface IInbox {
        /// @notice Configuration struct for Inbox constructor parameters
        struct Config {
            /// @notice The token used for bonds
            address bondToken;
            /// @notice The proof verifier contract
            address proofVerifier;
            /// @notice The proposer checker contract
            address proposerChecker;
            /// @notice The proving window in seconds
            uint48 provingWindow;
            /// @notice The extended proving window in seconds
            uint48 extendedProvingWindow;
            /// @notice The maximum number of finalized proposals in one block
            uint256 maxFinalizationCount;
            /// @notice The finalization grace period in seconds
            uint48 finalizationGracePeriod;
            /// @notice The ring buffer size for storing proposal hashes
            uint256 ringBufferSize;
            /// @notice The percentage of basefee paid to coinbase
            uint8 basefeeSharingPctg;
            /// @notice The minimum number of forced inclusions that the proposer is forced to process
            /// if they are due
            uint256 minForcedInclusionCount;
            /// @notice The delay for forced inclusions measured in seconds
            uint64 forcedInclusionDelay;
            /// @notice The fee for forced inclusions in Gwei
            uint64 forcedInclusionFeeInGwei;
            /// @notice The maximum number of checkpoints to store in ring buffer
            uint16 maxCheckpointHistory;
        }

        /// @notice Represents a source of derivation data within a Derivation
        struct DerivationSource {
            /// @notice Whether this source is from a forced inclusion.
            bool isForcedInclusion;
            /// @notice Blobs that contain the source's manifest data.
            LibBlobs.BlobSlice blobSlice;
        }

        /// @notice Contains derivation data for a proposal that is not needed during proving.
        /// @dev This data is hashed and stored in the Proposal struct to reduce calldata size.
        struct Derivation {
            /// @notice The L1 block number when the proposal was accepted.
            uint48 originBlockNumber;
            /// @notice The hash of the origin block.
            bytes32 originBlockHash;
            /// @notice The percentage of base fee paid to coinbase.
            uint8 basefeeSharingPctg;
            /// @notice Array of derivation sources, where each can be regular or forced inclusion.
            DerivationSource[] sources;
        }

        /// @notice Represents a proposal for L2 blocks.
        struct Proposal {
            /// @notice Unique identifier for the proposal.
            uint48 id;
            /// @notice The L1 block timestamp when the proposal was accepted.
            uint48 timestamp;
            /// @notice The timestamp of the last slot where the current preconfer can propose.
            uint48 endOfSubmissionWindowTimestamp;
            /// @notice Address of the proposer.
            address proposer;
            /// @notice The current hash of coreState
            bytes32 coreStateHash;
            /// @notice Hash of the Derivation struct containing additional proposal data.
            bytes32 derivationHash;
        }

        /// @notice Represents a transition about the state transition of a proposal.
        /// @dev Prover information has been moved to TransitionMetadata for out-of-order proving
        /// support
        struct Transition {
            /// @notice The proposal's hash.
            bytes32 proposalHash;
            /// @notice The parent transition's hash, this is used to link the transition to its parent
            /// transition to
            /// finalize the corresponding proposal.
            bytes32 parentTransitionHash;
            /// @notice The end block header containing number, hash, and state root.
            ICheckpointStore.Checkpoint checkpoint;
        }

        /// @notice Metadata about the proving of a transition
        /// @dev Separated from Transition to enable out-of-order proving
        struct TransitionMetadata {
            /// @notice The designated prover for this transition.
            address designatedProver;
            /// @notice The actual prover who submitted the proof.
            address actualProver;
        }

        /// @notice Represents a record of a transition with additional metadata.
        struct TransitionRecord {
            /// @notice The span indicating how many proposals this transition record covers.
            uint8 span;
            /// @notice The bond instructions.
            LibBonds.BondInstruction[] bondInstructions;
            /// @notice The hash of the last transition in the span.
            bytes32 transitionHash;
            /// @notice The hash of the last checkpoint in the span.
            bytes32 checkpointHash;
        }

        /// @notice Represents the core state of the inbox.
        struct CoreState {
            /// @notice The next proposal ID to be assigned.
            uint48 nextProposalId;
            /// @notice The next proposal block ID to be assigned.
            uint48 nextProposalBlockId;
            /// @notice The ID of the last finalized proposal.
            uint48 lastFinalizedProposalId;
            /// @notice The hash of the last finalized transition.
            bytes32 lastFinalizedTransitionHash;
            /// @notice The hash of all bond instructions.
            bytes32 bondInstructionsHash;
        }

        /// @notice Input data for the propose function
        struct ProposeInput {
            /// @notice The deadline timestamp for transaction inclusion (0 = no deadline).
            uint48 deadline;
            /// @notice The current core state before this proposal.
            CoreState coreState;
            /// @notice Array of existing proposals for validation (1-2 elements).
            Proposal[] parentProposals;
            /// @notice Blob reference for proposal data.
            LibBlobs.BlobReference blobReference;
            /// @notice Array of transition records for finalization.
            TransitionRecord[] transitionRecords;
            /// @notice The checkpoint for finalization.
            ICheckpointStore.Checkpoint checkpoint;
            /// @notice The number of forced inclusions that the proposer wants to process.
            /// @dev This can be set to 0 if no forced inclusions are due, and there's none in the queue
            /// that he wants to include.
            uint8 numForcedInclusions;
        }

        /// @notice Input data for the prove function
        struct ProveInput {
            /// @notice Array of proposals to prove.
            Proposal[] proposals;
            /// @notice Array of transitions containing proof details.
            Transition[] transitions;
            /// @notice Array of metadata for prover information.
            /// @dev Must have same length as transitions array.
            TransitionMetadata[] metadata;
        }

        /// @notice Payload data emitted in the Proposed event
        struct ProposedEventPayload {
            /// @notice The proposal that was created.
            Proposal proposal;
            /// @notice The derivation data for the proposal.
            Derivation derivation;
            /// @notice The core state after the proposal.
            CoreState coreState;
        }

        /// @notice Payload data emitted in the Proved event
        struct ProvedEventPayload {
            /// @notice The proposal ID that was proven.
            uint48 proposalId;
            /// @notice The transition that was proven.
            Transition transition;
            /// @notice The transition record containing additional metadata.
            TransitionRecord transitionRecord;
            /// @notice The metadata containing prover information.
            TransitionMetadata metadata;
        }

        // ---------------------------------------------------------------
        // Events
        // ---------------------------------------------------------------

        /// @notice Emitted when a new proposal is proposed.
        /// @param data The encoded ProposedEventPayload
        event Proposed(bytes data);

        /// @notice Emitted when a proof is submitted
        /// @param data The encoded ProvedEventPayload
        event Proved(bytes data);

        /// @notice Emitted when bond instructions are issued
        /// @param instructions The bond instructions that need to be performed.
        event BondInstructed(LibBonds.BondInstruction[] instructions);

        // ---------------------------------------------------------------
        // External Transactional Functions
        // ---------------------------------------------------------------

        /// @notice Proposes new proposals of L2 blocks.
        /// @param _lookahead The data to post a new lookahead (currently unused).
        /// @param _data The encoded ProposeInput struct.
        function propose(bytes calldata _lookahead, bytes calldata _data) external;

        /// @notice Proves a transition about some properties of a proposal, including its state
        /// transition.
        /// @param _data The encoded ProveInput struct.
        /// @param _proof Validity proof for the transitions.
        function prove(bytes calldata _data, bytes calldata _proof) external;

        // ---------------------------------------------------------------
        // External View Functions
        // ---------------------------------------------------------------

        /// @notice Returns the proposal hash for a given proposal ID.
        /// @param _proposalId The proposal ID to look up.
        /// @return proposalHash_ The hash stored at the proposal's ring buffer slot.
        function getProposalHash(uint48 _proposalId) external view returns (bytes32 proposalHash_);

        /// @notice Returns the transition record hash for a given proposal ID and parent transition
        /// hash.
        /// @param _proposalId The proposal ID.
        /// @param _parentTransitionHash The parent transition hash.
        /// @return finalizationDeadline_ The timestamp when finalization is enforced.
        /// @return recordHash_ The hash of the transition record.
        function getTransitionRecordHash(
            uint48 _proposalId,
            bytes32 _parentTransitionHash
        )
            external
            view
            returns (uint48 finalizationDeadline_, bytes26 recordHash_);

        /// @notice Returns the configuration parameters of the Inbox contract
        /// @return config_ The configuration struct containing all immutable parameters
        function getConfig() external view returns (Config memory config_);
    }
    );
}

pub mod lib_manifest {
    use super::*;
    use alloy_rlp::{RlpDecodable, RlpEncodable};

    sol!{

        /// @notice Represents a signed Ethereum transaction
        /// @dev Follows EIP-2718 typed transaction format with EIP-1559 support
        ///
        #[derive(Debug, RlpEncodable, RlpDecodable, PartialEq)]
        struct SignedTransaction {
            uint8 txType;
            uint64 chainId;
            uint64 nonce;
            uint256 maxPriorityFeePerGas;
            uint256 maxFeePerGas;
            uint64 gasLimit;
            address to;
            uint256 value;
            bytes data;
            bytes accessList;
            uint8 v;
            bytes32 r;
            bytes32 s;
        }

        /// @notice Represents a block manifest
        #[derive(Debug, RlpEncodable, RlpDecodable, PartialEq)]
        struct BlockManifest {
            /// @notice The timestamp of the block.
            uint48 timestamp;
            /// @notice The coinbase of the block.
            address coinbase;
            /// @notice The anchor block number. This field can be zero, if so, this block will use the
            /// most recent anchor in a previous block.
            uint48 anchorBlockNumber;
            /// @notice The block's gas limit.
            uint48 gasLimit;
            /// @notice The transactions for this block.
            SignedTransaction[] transactions;
        }

        /// @notice Represents a proposal manifest
        #[derive(Debug, RlpEncodable, RlpDecodable, PartialEq)]
        struct ProposalManifest {
            bytes proverAuthBytes;
            BlockManifest[] blocks;
        }
    }
}
