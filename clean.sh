export ACCT = {YOUR_ACCOUNT_ID}
export GRATIS = "g.${ACCT}"
export USDT = "usdt.fakes.testnet"

near delete $GRATIS --masterAccount $ACCT 

cargo clean 