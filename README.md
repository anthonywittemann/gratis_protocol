# Gratis Protocol
0% interest lending protocol on NEAR

### Getting Started
```
build.sh
dev-deploy.sh
```

cargo build --target wasm32-unknown-unknown --release

near dev-deploy ./target/wasm32-unknown-unknown/release/gratis_protocol.wasm 

export G= GRATIS_ACCOUNT
export USDT=usdt.fakes.testnet
export ACCT= YOUR_ACCOUNT


near call $G new '{"lower_collateral_accounts": ["kenobi.testnet"]}' --accountId $G

### Update Price

### Get Latest Price

near call $G get_latest_price --accountId $G

### Get all Loans

near call $G get_all_loans --accountId $G

### Deposit Collateral 
near call $G deposit_collateral '{"amount": 1000000}' --accountId $G


### Borrow 
near call $G borrow '{"usdt_amount": 1}' --accountId $G --gas 300000000000000

### Repay
near call $G repay '{"account_id": "kenobi.testnet", "usdt_amount": 50}' --accountId $ACCT --gas 300000000000000

### Get USDT Value of NEAR
near call $G get_usdt_value --accountId $G --gas 300000000000000

near call $G get_prices --accountId $G --gas 300000000000000


## Open and Close an Account
near call $G deposit_collateral '{"amount": 10}' --accountId $ACCT --deposit 10

near call $G borrow '{"usdt_amount": 500}' --accountId $ACCT --gas 300000000000000 

near call $USDT ft_transfer_call '{"receiver_id": "gratis.kenobi.testnet", "amount": "1", "memo": "Test", "msg": "close"}' --accountId $ACCT --gas 300000000000000 --depositYocto 1

near call $G close '{}' --accountId $ACCT --gas 300000000000000


