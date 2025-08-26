pub const BLOB_SIZE: usize = 4096 * 32; // byte size of a blob. 4096 field elements * 32 bytes
pub const DATA_LENGTH_SIZE: usize = 4; // number of bytes to store the data length
pub const DATA_WRITTEN_PER_ROUND: usize = 4 * 31 + 3; // number of bytes written per encode/decode round
pub const ROUNDS: usize = 1024; // number of encode/decode rounds
pub const MAX_BLOB_DATA_SIZE: usize = DATA_WRITTEN_PER_ROUND * ROUNDS - DATA_LENGTH_SIZE; // maximum number of bytes that can be encoded in the blob
pub const ENCODING_VERSION: u8 = 0;
pub const VERSION_OFFSET: usize = 1; // offset of the version byte in the blob encoding
