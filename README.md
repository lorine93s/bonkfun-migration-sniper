# BonkFun Migration Sniper

The bot predicts the Raydium CPMM accounts before the migration and starts blasting the swap transactions before migration really happens.

## Configuration

The bot accepts the following command-line arguments:

- `--rpc-url` - Solana RPC endpoint (default: http://localhost:8899)
- `--keypair-path` - Path to wallet keypair file (default: keypair.json)
- `--token-mint` - Target token mint address
- `--amount-in-lamports` - Trading amount in lamports (default: 1000000000)
- `--jito-keypair-path` - Path to Jito keypair file, remove relevant code if you don't want to use it
- `--bloxroute-api-key` - Bloxroute API key, remove relevant code if you don't want to use it
- `--zeroslot-api-key` - 0slot API key, remove relevant code if you don't want to use it

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release -- \
  --rpc-url <RPC_URL> \
  --keypair-path <KEYPAIR_PATH> \
  --token-mint <TOKEN_MINT_ADDRESS> \
  --amount-in-lamports <AMOUNT_IN_LAMPORTS> \  // 1 SOL = 10^9 lamports
  --jito-keypair-path <JITO_KEYPAIR_PATH> \
  --bloxroute-api-key <API_KEY> \
  --zeroslot-api-key <API_KEY>
```

## Credits

Built by @publicqi, @tonykebot, and @shoucccc
