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

/// Proof types supported by the SurgeVerifier.
/// Each variant maps to a bit flag used in `SubProof.proofBitFlag`.
/// Must match the constants in `SurgeVerifier.sol`.
///
/// Note: MOCK_ECDSA (0b00000001) is not a variant here — it is selected
/// at runtime via the `MOCK_MODE` env flag, which overrides the bit flag
/// to 1 regardless of the proof type.
#[derive(Debug, Clone, Copy)]
pub enum ProofType {
    Risc0, // 0b00000010
    Sp1,   // 0b00000100
    Zisk,  // 0b00001000
}

impl ProofType {
    pub fn proof_bit_flag(&self) -> u8 {
        match self {
            ProofType::Risc0 => 1 << 1,
            ProofType::Sp1 => 1 << 2,
            ProofType::Zisk => 1 << 3,
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

/// SurgeVerifier MOCK_ECDSA bit flag — used when `MOCK_MODE=true`.
pub const MOCK_ECDSA_BIT_FLAG: u8 = 1;

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
