
export ACCT = {YOUR_ACCOUNT_ID}
export GRATIS = "g.${ACCT}"
export USDT = "usdt.fakes.testnet"

# Deploy
cargo build --target wasm32-unknown-unknown --release
near create-account $GRATIS --masterAccount $ACCT --initialBalance 10
near deploy --accountId $GRATIS --wasmFile ./target/wasm32-unknown-unknown/release/gratis_protocol.wasm 


# Register and provide USDT token
near call $USDT register_account '{"account_id": "'$GRATIS'"}' --accountId $ACCT --amount 0.125 --gas 300000000000000
near call $USDT ft_transfer '{"receiver_id": "'$GRATIS'", "amount": "10"}' --accountId $ACCT --amount 0.125 --gas 300000000000000