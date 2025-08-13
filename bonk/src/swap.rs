use solana_program::pubkey;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::utils;

pub const CP_SWAP_PROGRAM_ID: Pubkey = pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const AUTHORITY: Pubkey = pubkey!("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL");
pub const ATA_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub const AMM_CONFIG: Pubkey = pubkey!("D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2");

#[derive(Debug)]
pub struct SwapBaseIn {
    pub payer: Pubkey,
    pub authority: Pubkey,
    pub amm_config: Pubkey,
    pub pool_state: Pubkey,
    pub input_token_account: Pubkey,
    pub output_token_account: Pubkey,
    pub input_vault: Pubkey,
    pub output_vault: Pubkey,
    pub input_token_program: Pubkey,
    pub output_token_program: Pubkey,
    pub input_token_mint: Pubkey,
    pub output_token_mint: Pubkey,
    pub observation_state: Pubkey,

    // swap args
    amount_in: u64,
    min_amount_out: u64,
}

impl SwapBaseIn {
    pub fn new(
        payer: Pubkey,
        mint: Pubkey,
        amount_in: u64,
        min_amount_out: u64,
    ) -> Self {
        let pool_state = Pubkey::find_program_address(
            &[
                b"pool",
                &AMM_CONFIG.to_bytes(),
                &WSOL_MINT.to_bytes(),
                &mint.to_bytes(),
            ],
            &CP_SWAP_PROGRAM_ID,
        )
        .0;
        Self {
            payer,
            authority: AUTHORITY,
            amm_config: AMM_CONFIG,
            pool_state,
            input_token_account: utils::find_ata(WSOL_MINT, payer),
            output_token_account: utils::find_ata(mint, payer),
            input_vault: Pubkey::find_program_address(
                &[b"pool_vault", &pool_state.to_bytes(), &WSOL_MINT.to_bytes()],
                &CP_SWAP_PROGRAM_ID,
            )
            .0,
            output_vault: Pubkey::find_program_address(
                &[b"pool_vault", &pool_state.to_bytes(), &mint.to_bytes()],
                &CP_SWAP_PROGRAM_ID,
            )
            .0,
            input_token_program: TOKEN_PROGRAM_ID,
            output_token_program: TOKEN_PROGRAM_ID,
            input_token_mint: WSOL_MINT,
            output_token_mint: mint,
            observation_state: Pubkey::find_program_address(
                &[b"observation", &pool_state.to_bytes()],
                &CP_SWAP_PROGRAM_ID,
            )
            .0,

            amount_in,
            min_amount_out: min_amount_out * 90 / 100,
        }
    }

    pub fn ix(&self) -> Instruction {
        let mut data = vec![0x8fu8, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde];
        // append amount_in
        data.extend_from_slice(&self.amount_in.to_le_bytes());
        // append min_amount_out
        data.extend_from_slice(&self.min_amount_out.to_le_bytes());

        Instruction {
            program_id: CP_SWAP_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer, true),
                AccountMeta::new_readonly(self.authority, false),
                AccountMeta::new_readonly(self.amm_config, false),
                AccountMeta::new(self.pool_state, false),
                AccountMeta::new(self.input_token_account, false),
                AccountMeta::new(self.output_token_account, false),
                AccountMeta::new(self.input_vault, false),
                AccountMeta::new(self.output_vault, false),
                AccountMeta::new_readonly(self.input_token_program, false),
                AccountMeta::new_readonly(self.output_token_program, false),
                AccountMeta::new_readonly(self.input_token_mint, false),
                AccountMeta::new_readonly(self.output_token_mint, false),
                AccountMeta::new(self.observation_state, false),
            ],
            data,
        }
    }
}
