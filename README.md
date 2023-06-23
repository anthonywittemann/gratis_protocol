# Gratis Protocol
0% interest lending protocol on NEAR


cargo build --target wasm32-unknown-unknown --release
near dev-deploy ./target/release/wasm32-unknown-unknown/gratis_protocol.wasm 

export G=devAcct

near call $G new '{"lowered_accounts": ["idk"]}' --accountId kenobi.testnet 

^^ Fails for me here on deserialization