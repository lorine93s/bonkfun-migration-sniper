pub mod sender;
pub mod swap;
pub mod utils;
pub mod jito;

use clap::Parser;
use eyre::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::pubkey;
use solana_sdk::signature::read_keypair_file;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
};
use std::{str::FromStr, sync::Arc};
use tokio::sync::mpsc;

use crate::sender::TxSender;

const MIGRATOR: Pubkey = pubkey!("RAYpQbFNq9i3mu6cKpTKKRwwHFDeK5AuZz8xvxUrCgw");

#[derive(Parser, Debug)]
struct Args {
    #[arg(
        long,
        default_value = "http://localhost:8899"
    )]
    rpc_url: String,

    #[arg(long, default_value = "keypair.json")]
    keypair_path: String,

    #[arg(long)]
    token_mint: String,

    #[arg(long)]
    jito_keypair_path: String,

    #[arg(long)]
    bloxroute_api_key: String,  

    #[arg(long)]
    zeroslot_api_key: String,

    #[arg(long, default_value = "1000000000")]
    amount_in_lamports: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        args.rpc_url,
        CommitmentConfig::processed(),
    ));

    let tx_sender = TxSender::new(
        read_keypair_file(&args.keypair_path).expect("Cannot read keypair"),
        rpc_client.clone(),
        Pubkey::from_str(&args.token_mint).expect("Invalid token mint"),
        read_keypair_file(&args.jito_keypair_path).expect("Cannot read jito keypair"),
        args.bloxroute_api_key,
        args.zeroslot_api_key,
        args.amount_in_lamports,
    )
    .await;
    let tx_sender_handle = tokio::spawn(tx_sender.run_loop());
    tx_sender_handle.await?;

    Ok(())
}
