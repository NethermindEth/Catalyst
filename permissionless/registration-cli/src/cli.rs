use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "registry-cli")]
#[command(about = "Call Registry contract functions")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Register {
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        registry: String,
        #[arg(long)]
        owner_pk: String,
        #[arg(long)]
        bls_pk: String,
    },
    OptInToSlasher {
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        registry: String,
        #[arg(long)]
        owner_pk: String,
        #[arg(long)]
        registration_root: String,
        #[arg(long)]
        slasher: String,
        #[arg(long)]
        committer: String,
    },
    GenerateBlsKey,
}
