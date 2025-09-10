mod bls;
mod commands;

use urc::bindings::{
    BLS::{G1Point, G2Point},
    IRegistry,
};

use std::str::FromStr;

use alloy::{
    network::EthereumWallet,
    primitives::{Address, FixedBytes, utils::parse_ether},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol_types::SolValue,
};
use anyhow::Error;
use bls::BLSService;
use clap::Parser;
use commands::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Register {
            rpc,
            registry,
            owner_pk,
            bls_pk,
        } => register(&rpc, &registry, &owner_pk, &bls_pk).await?,
        Commands::OptInToSlasher {
            rpc,
            registry,
            owner_pk,
            registration_root,
            slasher,
            committer,
        } => {
            opt_in_to_slasher(
                &rpc,
                &registry,
                &owner_pk,
                &registration_root,
                &slasher,
                &committer,
            )
            .await?
        }
        Commands::GenerateBlsKey => generate_bls_key(),
    }

    Ok(())
}

fn generate_bls_key() {
    let bls_service = BLSService::generate_key().expect("Failed to generate BLS key");
    println!(
        "Public key: {}",
        alloy::hex::encode(bls_service.get_public_key().to_bytes())
    );
    println!("Secret key: {}", bls_service.get_secret_key());
}

async fn register(rpc: &str, registry: &str, owner_pk: &str, bls_pk: &str) -> Result<(), Error> {
    let signer = PrivateKeySigner::from_str(owner_pk)?;
    let owner = signer.address();
    let wallet = EthereumWallet::from(signer);

    let l1_provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc.parse()?)
        .erased();
    let registry_address = Address::from_str(registry)?;
    let registry = IRegistry::new(registry_address, l1_provider.clone());

    let message = owner.abi_encode_packed();

    let bls_service = BLSService::new(bls_pk)?;

    let pk_point = bls_service.pubkey_to_g1_point();
    let pubkey = G1Point {
        x_a: pk_point[0][0],
        x_b: pk_point[0][1],
        y_a: pk_point[1][0],
        y_b: pk_point[1][1],
    };

    let signature = bls_service.sign(&message, &[0x00, 0x55, 0x52, 0x43]);
    // Sign message and convert to G2Point
    let signature_point = bls_service.signature_to_g2_point(&signature);

    let signature = G2Point {
        x_c0_a: signature_point[0][0],
        x_c0_b: signature_point[0][1],
        x_c1_a: signature_point[1][0],
        x_c1_b: signature_point[1][1],
        y_c0_a: signature_point[2][0],
        y_c0_b: signature_point[2][1],
        y_c1_a: signature_point[3][0],
        y_c1_b: signature_point[3][1],
    };

    let registrations = vec![IRegistry::SignedRegistration { pubkey, signature }];

    let tx = registry
        .register(registrations, owner)
        .value(parse_ether("1").expect("Failed to parse ether"));

    match tx.send().await {
        Ok(pending_tx) => {
            let tx_hash = pending_tx.tx_hash();
            println!("Register successfully tx_hash: {tx_hash:?}");

            let mut tx = l1_provider.get_transaction_receipt(*tx_hash).await?;

            while tx.is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                tx = l1_provider.get_transaction_receipt(*tx_hash).await?;
            }

            let receipt = tx.expect("Transaction not found");
            if receipt.logs().len() == 1 {
                let log = receipt.logs()[0].clone();
                let operator_regestred = log.log_decode::<IRegistry::OperatorRegistered>()?;
                println!(
                    "Registration root: {}",
                    operator_regestred.inner.registrationRoot
                );
            } else {
                return Err(anyhow::anyhow!("Register error: No logs"));
            }
        }
        Err(err) => {
            return Err(anyhow::anyhow!("Register error: {}", err));
        }
    }

    Ok(())
}

async fn opt_in_to_slasher(
    rpc: &str,
    registry: &str,
    owner_pk: &str,
    registration_root: &str,
    slasher: &str,
    committer: &str,
) -> Result<(), Error> {
    let signer = PrivateKeySigner::from_str(owner_pk)?;
    let wallet = EthereumWallet::from(signer);

    let l1_provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc.parse()?)
        .erased();
    let registry_address = Address::from_str(registry)?;
    let registry = IRegistry::new(registry_address, l1_provider);

    let registration_root = FixedBytes::from_str(registration_root)?;
    let slasher = Address::from_str(slasher)?;
    let committer = Address::from_str(committer)?;

    let tx = registry.optInToSlasher(registration_root, slasher, committer);

    match tx.send().await {
        Ok(pending_tx) => {
            let tx_hash = pending_tx.tx_hash();
            println!("optInToSlasher successfully: {tx_hash:?}",);
        }
        Err(err) => {
            return Err(anyhow::anyhow!("optInToSlasher error: {err}"));
        }
    }

    Ok(())
}
