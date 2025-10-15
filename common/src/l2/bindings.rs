use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    contract TaikoAnchor {

        /// @notice The last synced L1 block height.
        uint64 public lastSyncedBlock;

        /// @notice The last synced L1 block height.
        uint64 public lastCheckpoint;

        /// @dev Struct that represents L2 basefee configurations
        struct BaseFeeConfig {
            // This is the base fee change denominator per 12 second window.
            uint8 adjustmentQuotient;
            uint8 sharingPctg;
            uint32 gasIssuancePerSecond;
            uint64 minGasExcess;
            uint32 maxGasIssuancePerBlock;
        }

        /// @notice Anchors the latest L1 block details to L2 for cross-layer
        /// message verification.
        function anchorV3(
            uint64 _anchorBlockId,
            bytes32 _anchorStateRoot,
            uint32 _parentGasUsed,
            BaseFeeConfig calldata _baseFeeConfig,
            bytes32[] calldata _signalSlots
        ) external;

        /// @notice Calculates the base fee and gas excess using EIP-1559 configuration for the given
        /// parameters.
        function getBasefeeV2(
            uint32 _parentGasUsed,
            uint64 _blockTimestamp,
            BaseFeeConfig calldata _baseFeeConfig
        )
            public
            view
            returns (uint256 basefee_, uint64 newGasTarget_, uint64 newGasExcess_);
    }
);

sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    contract Bridge {
        function getMessageMinGasLimit(uint256 dataLength) public pure returns (uint32);

        struct Message {
            // Message ID whose value is automatically assigned.
            uint64 id;
            // The max processing fee for the relayer. This fee has 3 parts:
            // - the fee for message calldata.
            // - the minimal fee reserve for general processing, excluding function call.
            // - the invocation fee for the function call.
            // Any unpaid fee will be refunded to the destOwner on the destination chain.
            // Note that fee must be 0 if gasLimit is 0, or large enough to make the invocation fee
            // non-zero.
            uint64 fee;
            // gasLimit that the processMessage call must have.
            uint32 gasLimit;
            // The address, EOA or contract, that interacts with this bridge.
            // The value is automatically assigned.
            address from;
            // Source chain ID whose value is automatically assigned.
            uint64 srcChainId;
            // The owner of the message on the source chain.
            address srcOwner;
            // Destination chain ID where the `to` address lives.
            uint64 destChainId;
            // The owner of the message on the destination chain.
            address destOwner;
            // The destination address on the destination chain.
            address to;
            // value to invoke on the destination chain.
            uint256 value;
            // callData to invoke on the destination chain.
            bytes data;
        }

        function sendMessage(Message calldata _message) external payable returns (bytes32 msgHash_, Message memory message_);
    }
}
