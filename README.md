# Gratis Protocol
0% interest lending protocol on NEAR


cargo build --target wasm32-unknown-unknown --release

near dev-deploy ./target/wasm32-unknown-unknown/release/gratis_protocol.wasm 

export G=devAcct

near call $G new '{"lower_collateral_accounts": ["idk"]}' --accountId $G

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
near call $G repay '{"usdt_amount": 50}' --accountId $G --gas 300000000000000

### Get USDT Value of NEAR
near call $G get_usdt_value --accountId $G --gas 300000000000000

near call $G get_prices --accountId $G --gas 300000000000000



near call $G deposit_collateral '{"amount": 100}' --accountId kenobi.testnet

near call $G borrow '{"usdt_amount": 80}' --accountId kenobi.testnet --gas 300000000000000


near call $G deposit_collateral '{"amount": 99}' --accountId tomohiro.testnet

near call $G borrow '{"usdt_amount": 30}' --accountId tomohiro.testnet --gas 300000000000000

near call $G repay '{"usdt_amount": 50}' --accountId tomohiro.testnet --gas 300000000000000
