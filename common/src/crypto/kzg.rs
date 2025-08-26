// KZG helper functions
// Taken from https://github.com/ralexstokes/ethereum-consensus/blob/main/ethereum-consensus/src/crypto/kzg.rs
use anyhow::Error;
use c_kzg::{KzgCommitment, KzgProof, KzgSettings};

pub fn blob_to_kzg_commitment<Blob: AsRef<[u8]>>(
    blob: Blob,
    kzg_settings: &KzgSettings,
) -> Result<KzgCommitment, Error> {
    let blob = c_kzg::Blob::from_bytes(blob.as_ref())?;

    Ok(kzg_settings.blob_to_kzg_commitment(&blob)?)
}

pub fn compute_blob_kzg_proof<Blob: AsRef<[u8]>>(
    blob: Blob,
    commitment: &KzgCommitment,
    kzg_settings: &KzgSettings,
) -> Result<KzgProof, Error> {
    let blob = c_kzg::Blob::from_bytes(blob.as_ref())?;
    let commitment = c_kzg::Bytes48::from_bytes(commitment.as_ref()).expect("correct size");

    Ok(kzg_settings.compute_blob_kzg_proof(&blob, &commitment)?)
}
