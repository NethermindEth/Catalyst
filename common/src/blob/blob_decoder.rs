use alloy::consensus::Blob;
use anyhow::{Error, anyhow};

use super::constants::{BLOB_SIZE, ENCODING_VERSION, MAX_BLOB_DATA_SIZE, ROUNDS, VERSION_OFFSET};

pub struct BlobDecoder {
    read_offset: usize,
    data_offset: usize,
    data: [u8; MAX_BLOB_DATA_SIZE],
}

impl BlobDecoder {
    pub fn decode_blob(blob: &Blob) -> Result<Vec<u8>, Error> {
        BlobDecoder::new().decode(blob)
    }

    fn new() -> Self {
        Self {
            read_offset: 0,
            data_offset: 0,
            data: [0u8; MAX_BLOB_DATA_SIZE],
        }
    }

    // Refer to https://github.com/ethereum-optimism/optimism/blob/e6848b15f5dafb3159e993f7aa24844679a44e5b/op-service/eth/blob.go#L196
    fn decode(mut self, blob: &Blob) -> Result<Vec<u8>, Error> {
        if blob.len() != BLOB_SIZE {
            return Err(anyhow!("Invalid blob size"));
        }

        if blob[VERSION_OFFSET] != ENCODING_VERSION {
            return Err(anyhow!(
                "Invalid encoding version: expected {}, got {}",
                ENCODING_VERSION,
                blob[VERSION_OFFSET]
            ));
        }

        // ROUND 0
        // Read 3-byte big-endian length from bytes [2..=4]
        let output_len = u32::from_be_bytes([0, blob[2], blob[3], blob[4]]) as usize;
        if output_len > MAX_BLOB_DATA_SIZE {
            return Err(anyhow!(
                "Invalid data length: {} (max allowed: {})",
                output_len,
                MAX_BLOB_DATA_SIZE
            ));
        }

        self.data[0..27].copy_from_slice(&blob[5..32]);

        self.data_offset = 28;
        self.read_offset = 32;

        let mut encoded_byte = [0u8; 4];
        encoded_byte[0] = blob[0];

        for byte in encoded_byte.iter_mut().skip(1) {
            *byte = self.decode_fe(blob)?;
        }

        self.restore_control_bytes(encoded_byte);

        for _ in 1..ROUNDS {
            if self.data_offset >= output_len {
                break;
            }

            for byte in &mut encoded_byte {
                *byte = self.decode_fe(blob)?;
            }
            self.restore_control_bytes(encoded_byte);
        }

        // Ensure no extra data was decoded
        for byte in self.data.iter().skip(output_len) {
            if *byte != 0 {
                return Err(anyhow!("Extraneous data in output"));
            }
        }

        // Ensure no extra data in blob
        for byte in blob.iter().skip(self.read_offset) {
            if *byte != 0 {
                return Err(anyhow!("Extraneous data in blob past input_pos"));
            }
        }

        Ok(self.data[..output_len].to_vec())
    }

    fn decode_fe(&mut self, blob: &Blob) -> Result<u8, Error> {
        let result = blob[self.read_offset];
        if result & 0b1100_0000 != 0 {
            return Err(anyhow!("Invalid field element (overflow in high bits)"));
        }

        self.data[self.data_offset..self.data_offset + 31]
            .copy_from_slice(&blob[self.read_offset + 1..self.read_offset + 32]);

        self.data_offset += 32;
        self.read_offset += 32;

        Ok(result)
    }

    fn restore_control_bytes(&mut self, encoded_byte: [u8; 4]) {
        self.data_offset -= 1;

        let x = (encoded_byte[0] & 0b0011_1111) | ((encoded_byte[1] & 0b0011_0000) << 2);
        let y = (encoded_byte[1] & 0b0000_1111) | ((encoded_byte[3] & 0b0000_1111) << 4);
        let z = (encoded_byte[2] & 0b0011_1111) | ((encoded_byte[3] & 0b0011_0000) << 2);

        self.data[self.data_offset - 32] = z;
        self.data[self.data_offset - (32 * 2)] = y;
        self.data[self.data_offset - (32 * 3)] = x;
    }
}
