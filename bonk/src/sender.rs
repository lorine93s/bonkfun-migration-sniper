use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose};
use serde_json::json;
use solana_client::{
    nonblocking::{rpc_client::RpcClient, tpu_client::TpuClient},
    rpc_client::SerializableTransaction,
    rpc_config::RpcSendTransactionConfig,
    tpu_client::TpuClientConfig,
};
use solana_program::pubkey;
use solana_quic_client::{QuicConfig, QuicConnectionManager, QuicPool};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer as _,
    transaction::{Transaction, VersionedTransaction},
};
use solana_system_interface::instruction as system_instruction;
use tokio::{
    sync::{RwLock, mpsc},
    time::interval,
};
use tracing::{error, info, instrument};

use crate::{
    jito,
    swap::{self, SwapBaseIn},
    utils,
};

pub struct TxSender {
    keypair: Keypair,
    client: Arc<RpcClient>,
    latest_blockhash: Arc<RwLock<Vec<Hash>>>,
    token_mint: Pubkey,
    jito_keypair: Keypair,
    bloxroute_api_key: String,
    zeroslot_api_key: String,
    amount_in: u64,
}

impl TxSender {
    pub async fn new(
        keypair: Keypair,
        client: Arc<RpcClient>,
        token_mint: Pubkey,
        jito_keypair: Keypair,
        bloxroute_api_key: String,
        zeroslot_api_key: String,
        amount_in: u64,
    ) -> Self {
        // Get initial blockhash
        let initial_blockhash = client
            .get_latest_blockhash()
            .await
            .expect("Failed to get initial blockhash");
        let latest_blockhash = Arc::new(RwLock::new(vec![initial_blockhash]));

        tokio::spawn(Self::blockhash_update_worker(
            client.clone(),
            latest_blockhash.clone(),
        ));

        Self {
            keypair,
            client,
            latest_blockhash,
            token_mint,
            jito_keypair,
            bloxroute_api_key,
            zeroslot_api_key,
            amount_in,
        }
    }

    pub async fn blockhash_update_worker(client: Arc<RpcClient>, blockhash: Arc<RwLock<Vec<Hash>>>) {
        let mut interval = interval(Duration::from_millis(200));
        loop {
            interval.tick().await;

            match client.get_latest_blockhash().await {
                Ok(new_blockhash) => {
                    let mut blockhashes = blockhash.write().await;
                    blockhashes.push(new_blockhash);
                    if blockhashes.len() > 4 {
                        blockhashes.remove(0);
                    }
                }
                Err(e) => {
                    error!("Failed to update blockhash: {:?}", e);
                }
            }
        }
    }

    pub async fn run_loop(mut self) {
        info!("Starting tx sender loop");

        // let tpu_client = Arc::new(TpuClient::new(
        //     "client",
        //     self.client.clone(),
        //     "wss://validator.solayer.org",
        //     TpuClientConfig::default(),
        // )
        // .await.unwrap());

        let jito_keypair = self.jito_keypair;

        let jito_client = jito::get_searcher_client_auth(
                "https://ny.mainnet.block-engine.jito.wtf",
                &Arc::new(jito_keypair),
            )
            .await
            .unwrap();

        loop {
            buy(
                self.amount_in,
                self.token_mint,
                self.keypair.insecure_clone(),
                self.latest_blockhash.clone(),
                self.client.clone(),
                // tpu_client.clone(),
                jito_client.clone(),
                &self.zeroslot_api_key,
                &self.bloxroute_api_key,
            ).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
    }
}

async fn send_transaction_via_0slot(tx: &Transaction, api_key: &str) -> eyre::Result<String> {
    // Serialize transaction to base64
    let serialized_tx = bincode::serialize(tx)?;
    let base64_encoded_transaction = general_purpose::STANDARD.encode(serialized_tx);

    // Create JSON-RPC request
    let request_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [
            base64_encoded_transaction,
            {
                "encoding": "base64",
                "skipPreflight": true,
            }
        ]
    });

    // Send the request
    let response = reqwest::Client::new()
        .post(&format!("https://ny1.0slot.trade/?api-key={}", api_key))
        .json(&request_body)
        .send()
        .await?;

    // Parse the response
    let response_json: serde_json::Value = response.json().await?;

    if let Some(result) = response_json.get("result") {
        if let Some(signature) = result.as_str() {
            Ok(signature.to_string())
        } else {
            Err(eyre::eyre!(
                "Invalid response format: result is not a string"
            ))
        }
    } else if let Some(error) = response_json.get("error") {
        Err(eyre::eyre!("0slot API error: {}", error))
    } else {
        Err(eyre::eyre!("Unexpected response format"))
    }
}


async fn send_transaction_via_bloxroute(tx: &Transaction, api_key: &str ) -> eyre::Result<String> {
    // Serialize transaction to base64
    let serialized_tx = bincode::serialize(tx)?;
    let base64_encoded_transaction = general_purpose::STANDARD.encode(serialized_tx);

    // Create JSON-RPC request
    let request_body = json!({
        "transaction": {"content": base64_encoded_transaction}, 
        "frontRunningProtection": false,
        "submitProtection": "SP_LOW",
        "useStakedRPCs": true,
        "fastBestEffort": true,
    });

    // Send the request
    let response = reqwest::Client::new()
        .post("https://ny.solana.dex.blxrbdn.com/api/v2/submit")
        .header("Authorization", api_key)
        .json(&request_body)
        .send()
        .await?;

    // Parse the response
    let response_json: serde_json::Value = response.json().await?;

    if let Some(result) = response_json.get("signature") {
        if let Some(signature) = result.as_str() {
            Ok(signature.to_string())
        } else {
            Err(eyre::eyre!(
                "Invalid response format: signature is not a string"
            ))
        }
    } else if let Some(error) = response_json.get("error") {
        Err(eyre::eyre!("0slot API error: {}", error))
    } else {
        Err(eyre::eyre!("Unexpected response format"))
    }
}

#[instrument(skip_all, fields(signature = ?token_mint))]
async fn buy(
    amount_in: u64,
    token_mint: Pubkey,
    keypair: Keypair,
    latest_blockhash: Arc<RwLock<Vec<Hash>>>,
    client: Arc<RpcClient>,
    // tpu_client: Arc<TpuClient<QuicPool, QuicConnectionManager, QuicConfig>>,
    jito_client: jito::Client,
    zeroslot_api_key: &str,
    bloxroute_api_key: &str,
) {
    // Pool reserves: 85 SOL and 206,900,000 tokens
    let reserve_sol = 85_000_000_000; // 85 SOL in lamports
    let reserve_token = 206_900_000_000_000; // 206.9M tokens with 6 decimals
    let amount_out = calculate_amount_out(amount_in, reserve_sol, reserve_token);
    let swap_base_in = SwapBaseIn::new(
        keypair.pubkey(),
        token_mint,
        amount_in,
        amount_out,
    );

    let cu_budget = 2000_000u32;
    let cu_price = 500000u64;

    let set_cu_budget_ix = Instruction {
        program_id: pubkey!("ComputeBudget111111111111111111111111111111"),
        accounts: vec![AccountMeta::new_readonly(
            pubkey!("jitodontfront11111111111111111111TrentLover"), // <= change this to something else
            false,
        )],
        data: {
            let mut data = vec![2u8]; // SetComputeUnitLimit discriminator
            data.extend_from_slice(&cu_budget.to_le_bytes()); // 300k compute units
            data
        },
    };

    let set_cu_price_ix = Instruction {
        program_id: pubkey!("ComputeBudget111111111111111111111111111111"),
        accounts: vec![],
        data: {
            let mut data = vec![3u8]; // SetComputeUnitPrice discriminator
            data.extend_from_slice(&cu_price.to_le_bytes()); // 10k micro-lamports priority fee
            data
        },
    };

    let create_ata_base_ix = create_associated_token_account_idempotent(
        &keypair.pubkey(),
        &keypair.pubkey(),
        &swap::WSOL_MINT,
        &pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    );

    let create_ata_quote_ix = create_associated_token_account_idempotent(
        &keypair.pubkey(),
        &keypair.pubkey(),
        &token_mint,
        &pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    );

    let swap_ix = swap_base_in.ix();
    let transfer_to_wsol = system_instruction::transfer(
        &keypair.pubkey(),
        &Pubkey::find_program_address(
            &[
                &keypair.pubkey().to_bytes(),
                &pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").to_bytes(),
                &swap::WSOL_MINT.to_bytes(),
            ],
            &pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
        )
        .0,
        amount_in,
    );

    let sync_native_ix = Instruction {
        program_id: pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
        accounts: vec![AccountMeta::new(
            Pubkey::find_program_address(
                &[
                    &keypair.pubkey().to_bytes(),
                    &pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").to_bytes(),
                    &swap::WSOL_MINT.to_bytes(),
                ],
                &pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
            )
            .0,
            false,
        )],
        data: vec![17], // SyncNative instruction discriminator
    };

    let ixs = [
        set_cu_budget_ix.clone(),
        set_cu_price_ix.clone(),
        create_ata_base_ix.clone(),
        create_ata_quote_ix.clone(),
        transfer_to_wsol.clone(),
        sync_native_ix.clone(),
        swap_ix.clone(),
    ];

    fn build_ixs_with_tip(ixs: &[Instruction], tip_ix: &Instruction) -> Vec<Instruction> {
        ixs.iter()
            .chain(std::slice::from_ref(tip_ix))
            .cloned()
            .collect()
    }
    let blockhashes = latest_blockhash.read().await;
    let mut futures_0slot = Vec::new();
    let mut futures_bloxroute = Vec::new();
    let mut futures_rpc = Vec::new();
    // let mut futures_tpu = Vec::new();
    let mut futures_jito = Vec::new();

    for blockhash in blockhashes.iter().rev().take(1) {
        let tx_no_tip = Transaction::new_signed_with_payer(
            &ixs,
            Some(&keypair.pubkey()),
            &[&keypair],
            *blockhash,
        );

        let tx_0slot = Transaction::new_signed_with_payer(
            &build_ixs_with_tip(
                &ixs,
                &system_instruction::transfer(
                    &keypair.pubkey(),
                    &pubkey!("FCjUJZ1qozm1e8romw216qyfQMaaWKxWsuySnumVCCNe"),
                    10_000_000,
                ),
            ),
            Some(&keypair.pubkey()),
            &[&keypair],
            *blockhash,
        );

        let tx_bloxroute = Transaction::new_signed_with_payer(
            &build_ixs_with_tip(
                &ixs,
                &system_instruction::transfer(
                    &keypair.pubkey(),
                    &pubkey!("95cfoy472fcQHaw4tPGBTKpn6ZQnfEPfBgDQx6gcRmRg"),
                    10_000_000,
                ),
            ),      
            Some(&keypair.pubkey()),
            &[&keypair],
            *blockhash,
        );

        let tx_jito = Transaction::new_signed_with_payer(
            &build_ixs_with_tip(
                &ixs,
                &system_instruction::transfer(
                    &keypair.pubkey(),
                    &pubkey!("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh"),
                    10_000_000,
                ),
            ),
            Some(&keypair.pubkey()),
            &[&keypair],
            *blockhash,
        );


        futures_0slot.push(send_via_0slot(tx_0slot.clone(), &zeroslot_api_key));
        futures_bloxroute.push(send_via_bloxroute(tx_bloxroute.clone(), &bloxroute_api_key));
        futures_rpc.push(send_via_rpc(tx_no_tip.clone(), client.clone()));
        futures_rpc.push(send_via_rpc(tx_no_tip.clone(), Arc::new(RpcClient::new_with_commitment(
            "https://acc.solayer.org".to_string(),
            CommitmentConfig::processed(),
        ))));
        // futures_tpu.push(send_via_tpu(tx_no_tip.clone(), tpu_client.clone()));
        futures_jito.push(send_via_jito(tx_jito.clone(), jito_client.clone()));
    }

    tokio::join!(
        futures::future::join_all(futures_0slot),
        futures::future::join_all(futures_bloxroute),
        futures::future::join_all(futures_rpc),
        // futures::future::join_all(futures_tpu),
        futures::future::join_all(futures_jito),
    );
}

#[instrument(skip_all, name = "0slot")]
async fn send_via_0slot(tx: Transaction, api_key: &str) {
    let start = Instant::now();
    match send_transaction_via_0slot(&tx, api_key).await {
        Ok(signature) => {
            info!(duration = ?start.elapsed(), "Tx sent: {signature}");
        }
        Err(e) => {
            error!(
                duration = ?start.elapsed(),
                "Failed to send transaction via 0slot: {:#}", e
            );
        }
    }
}

#[instrument(skip_all, name = "bloxroute")]
async fn send_via_bloxroute(tx: Transaction, api_key: &str) {
    let start = Instant::now();
    match send_transaction_via_bloxroute(&tx, api_key).await {
        Ok(signature) => {
            info!(duration = ?start.elapsed(), "Tx sent: {signature}");
        }
        Err(e) => {
            error!(
                duration = ?start.elapsed(),
                "Failed to send transaction via bloxroute: {:#}", e
            );
        }
    }
}

#[instrument(skip_all, name = "RPC")]
async fn send_via_rpc(tx: Transaction, client: Arc<RpcClient>) {
    let start = Instant::now();
    match client
        .send_transaction_with_config(
            &tx,
            RpcSendTransactionConfig {
                skip_preflight: true,
                ..Default::default()
            },
        )
        .await
    {
        Ok(sig) => {
            info!(duration = ?start.elapsed(), "Tx sent: {sig}");
        }
        Err(e) => {
            error!(
                duration = ?start.elapsed(),
                "Failed to send transaction via RPC: {:#}", e
            );
        }
    }
}

#[instrument(skip_all, name = "TPU")]
async fn send_via_tpu(
    tx: Transaction,
    tpu_client: Arc<TpuClient<QuicPool, QuicConnectionManager, QuicConfig>>,
) {
    let start = Instant::now();
    let sig = tx.get_signature();

    if tpu_client.send_transaction(&tx).await {
        info!(duration = ?start.elapsed(), "Tx sent: {sig}");
    } else {
        error!(duration = ?start.elapsed(), "Failed to send transaction");
    }
}

#[instrument(skip_all, name = "Jito")]
async fn send_via_jito(tx: Transaction, mut jito_client: jito::Client) {
    let sig = *tx.get_signature();
    let versioned_tx = VersionedTransaction::from(tx);
    let start = Instant::now();

    let r = jito::send_bundle_no_wait(&[versioned_tx], &mut jito_client).await;

    match r {
        Ok(a) => {
            info!(duration = ?start.elapsed(), "Tx sent: {sig}, Response: {a:?}");
        }
        Err(e) => {
            error!(duration = ?start.elapsed(), "Failed to send transaction via Jito: {:#}", e);
        }
    }
}

fn build_associated_token_account_instruction(
    funding_address: &Pubkey,
    wallet_address: &Pubkey,
    token_mint_address: &Pubkey,
    token_program_id: &Pubkey,
    instruction: u8,
) -> Instruction {
    let associated_account_address = Pubkey::find_program_address(
        &[
            &wallet_address.to_bytes(),
            &token_program_id.to_bytes(),
            &token_mint_address.to_bytes(),
        ],
        &pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
    )
    .0;
    // safety check, assert if not a creation instruction, which is only 0 or 1
    assert!(instruction <= 1);
    Instruction {
        program_id: pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
        accounts: vec![
            AccountMeta::new(*funding_address, true),
            AccountMeta::new(associated_account_address, false),
            AccountMeta::new_readonly(*wallet_address, false),
            AccountMeta::new_readonly(*token_mint_address, false),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(*token_program_id, false),
        ],
        data: vec![instruction],
    }
}

pub fn create_associated_token_account_idempotent(
    funding_address: &Pubkey,
    wallet_address: &Pubkey,
    token_mint_address: &Pubkey,
    token_program_id: &Pubkey,
) -> Instruction {
    build_associated_token_account_instruction(
        funding_address,
        wallet_address,
        token_mint_address,
        token_program_id,
        1, // AssociatedTokenAccountInstruction::CreateIdempotent
    )
}

pub fn calculate_amount_out(amount_in: u64, reserve_sol: u64, reserve_token: u64) -> u64 {
    const FEE_DENOMINATOR: u64 = 1_000_000;
    const TRADE_FEE_RATE: u64 = 2_500; // 0.25% fee

    // Calculate trade fee using ceiling division
    let trade_fee = (amount_in * TRADE_FEE_RATE).div_ceil(FEE_DENOMINATOR);
    let amount_in_after_fee = amount_in - trade_fee;

    // Apply constant product formula: out = (amount_in_after_fee * R_token) / (R_sol + amount_in_after_fee)
    let numerator = amount_in_after_fee as u128 * reserve_token as u128;
    let denominator = reserve_sol as u128 + amount_in_after_fee as u128;
    let mut out = (numerator / denominator) as u64;

    // Make sure we never make k smaller (off-by-one protection)
    let new_k = (reserve_sol + amount_in_after_fee) as u128 * (reserve_token - out) as u128;
    let old_k = reserve_sol as u128 * reserve_token as u128;

    if new_k < old_k {
        out -= 1;
    }

    out
}
