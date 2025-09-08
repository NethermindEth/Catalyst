use alloy::primitives::FixedBytes;
use anyhow::Error;

use blst::min_pk::{PublicKey, SecretKey, Signature};
use rand_core::{OsRng, TryRngCore};

pub struct BLSService {
    pk: PublicKey,
    sk: SecretKey,
}

impl BLSService {
    pub fn new(private_key: &str) -> Result<Self, Error> {
        let pk_bytes = alloy::hex::decode(private_key)
            .map_err(|e| anyhow::anyhow!("BLSService: failed to decode secret key: {}", e))?;
        let sk = SecretKey::from_bytes(&pk_bytes).map_err(|e| {
            anyhow::anyhow!(
                "BLSService: failed to create secret key from bytes: {:?}",
                e
            )
        })?;
        let pk = sk.sk_to_pk();

        Ok(Self { pk, sk })
    }

    pub fn generate_key() -> Result<Self, Error> {
        let mut ikm = [0u8; 64];
        OsRng.try_fill_bytes(&mut ikm)?;

        let sk = SecretKey::key_gen(ikm.as_ref(), &[])
            .map_err(|e| anyhow::anyhow!("BLSService: failed to generate secret key: {:?}", e))?;
        let pk = sk.sk_to_pk();

        Ok(Self { pk, sk })
    }

    pub fn sign(&self, message: &[u8], dst: &[u8]) -> Signature {
        self.sk.sign(message, dst, &[])
    }

    fn to_contract_layout(value: &[u8; 48]) -> [FixedBytes<32>; 2] {
        let mut buffer = [0u8; 32];
        buffer[16..32].copy_from_slice(&value[0..16]);
        let res1: FixedBytes<32> = buffer.into();
        let res2: FixedBytes<32> = value[16..48]
            .try_into()
            .expect("Failed to convert second element");
        [res1, res2]
    }

    pub fn pubkey_to_g1_point(&self) -> [[FixedBytes<32>; 2]; 2] {
        let pk = self.get_public_key().serialize();
        let x = Self::to_contract_layout(pk[0..48].try_into().expect("Failed to convert pubkey x"));
        let y =
            Self::to_contract_layout(pk[48..96].try_into().expect("Failed to convert pubkey y"));
        [x, y]
    }

    pub fn signature_to_g2_point(&self, signature: &Signature) -> [[FixedBytes<32>; 2]; 4] {
        let signature = signature.serialize();
        let x = Self::to_contract_layout(
            signature[0..48]
                .try_into()
                .expect("Failed to convert signature x"),
        );
        let x_i = Self::to_contract_layout(
            signature[48..96]
                .try_into()
                .expect("Failed to convert signature x_i"),
        );
        let y = Self::to_contract_layout(
            signature[96..144]
                .try_into()
                .expect("Failed to convert signature y"),
        );
        let y_i = Self::to_contract_layout(
            signature[144..192]
                .try_into()
                .expect("Failed to convert signature y_i"),
        );
        [x, x_i, y, y_i]
    }

    pub fn get_public_key(&self) -> PublicKey {
        self.pk
    }

    pub fn get_secret_key(&self) -> String {
        alloy::hex::encode(self.sk.to_bytes())
    }
}
