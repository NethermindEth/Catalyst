#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug, Default)]
    RealTimeInbox,
    "src/l1/abi/RealTimeInbox.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Multicall,
    "src/l1/abi/Multicall.json"
);

// Define ProposeInput and BlobReference manually since the RealTimeInbox ABI
// only exposes propose(bytes _data, ...) where _data is abi.encode(ProposeInput).
// These types are internal to the contract but needed for encoding.
sol! {
    struct BlobReference {
        uint16 blobStartIndex;
        uint16 numBlobs;
        uint24 offset;
    }

    struct ProposeInput {
        BlobReference blobReference;
        bytes32[] signalSlots;
        uint48 maxAnchorBlockNumber;
    }

    /// Input for `tentativePropose` — splits signals into existing (verified
    /// immediately) and requiredReturn (verified at finalizePropose after the
    /// L1 callback in the same multicall produces them).
    struct ProposeInputV2 {
        BlobReference blobReference;
        bytes32[] existingSignals;
        bytes32[] requiredReturnSignals;
        uint48 maxAnchorBlockNumber;
    }

    // SurgeVerifier SubProof encoding
    struct SubProof {
        uint8 proofBitFlag;
        bytes data;
    }
}

// Binding for the L2 flash loan executor's `execute` entry point. Used by the
// proposal manager to detect UserOps that target this ABI and patch their
// placeholder `returnMessage` with the simulated L1→L2 return.
//
// NOTE: this mirrors `IBridge.Message` struct layout. The field types must
// match exactly or abi_decode/abi_encode_sequence will fail.
alloy::sol! {
    #[allow(missing_docs)]
    struct FlashLoanReturnMessage {
        uint64 id;
        uint64 fee;
        uint32 gasLimit;
        address from;
        uint64 srcChainId;
        address srcOwner;
        uint64 destChainId;
        address destOwner;
        address to;
        uint256 value;
        bytes data;
    }

    #[allow(missing_docs)]
    function execute(
        uint256 amount,
        address beneficiary,
        FlashLoanReturnMessage returnMessage
    ) external;
}

/// Proof types supported by the SurgeVerifier.
/// Each variant maps to a bit flag used in `SubProof.proofBitFlag`.
#[derive(Debug, Clone, Copy)]
pub enum ProofType {
    Risc0, // 0b00000001
    Sp1,   // 0b00000010
    Zisk,  // 0b00000100
}

impl ProofType {
    pub fn proof_bit_flag(&self) -> u8 {
        match self {
            ProofType::Risc0 => 1,
            ProofType::Sp1 => 1 << 1,
            ProofType::Zisk => 1 << 2,
        }
    }

    /// Returns the proof type string expected by Raiko.
    pub fn raiko_proof_type(&self) -> &'static str {
        match self {
            ProofType::Risc0 => "risc0",
            ProofType::Sp1 => "sp1",
            ProofType::Zisk => "zisk",
        }
    }
}

impl std::str::FromStr for ProofType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "risc0" => Ok(ProofType::Risc0),
            "sp1" => Ok(ProofType::Sp1),
            "zisk" => Ok(ProofType::Zisk),
            _ => Err(anyhow::anyhow!(
                "Invalid PROOF_TYPE '{}'. Must be one of: sp1, risc0, zisk",
                s
            )),
        }
    }
}

impl std::fmt::Display for ProofType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.raiko_proof_type())
    }
}
