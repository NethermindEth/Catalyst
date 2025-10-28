use alloy::sol;
use alloy_rlp::{RlpDecodable, RlpEncodable};

sol! {

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
