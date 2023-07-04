cargo build --target wasm32-unknown-unknown --release

near dev-deploy ./target/wasm32-unknown-unknown/release/gratis_protocol.wasm 