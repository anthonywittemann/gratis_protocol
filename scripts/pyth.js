const nearAPI = require('near-api-js');
const fs = require('fs');
const path = require('path');


// Initialize NEAR connection and contract variables
let near;
let walletConnection;
let contract;
let usdtContract;

const NEAR_PRICE_ID = "0xc415de8d2eba7db216527dff4b60e8f3a5311c740dadb233e13e12547e226750"

// Initializing connection to NEAR
async function initNear() {
    const config = {
        networkId: 'default',
        nodeUrl: 'https://rpc.testnet.near.org',
        walletUrl: 'https://wallet.testnet.near.org',
        helperUrl: 'https://helper.testnet.near.org',
        contractName: 'pyth.testnet', // Replace with your contract's account ID
        usdtContractName: 'usdt.fakes.testnet',
    };

    const { keyStores } = nearAPI;
    const homedir = require("os").homedir();
    const CREDENTIALS_DIR = ".near-credentials";
    const credentialsPath = path.join(homedir, CREDENTIALS_DIR);
    const keyStore = new keyStores.UnencryptedFileSystemKeyStore(credentialsPath);

    near = await nearAPI.connect({
        deps: {
            keyStore: keyStore
        },
        ...config
    });

    const accountId = "kenobi.testnet"; // Replace with your account ID
    const account = new nearAPI.Account(near.connection, config.contractName);

    contract = new nearAPI.Contract(account, config.contractName, {
        viewMethods: ['get_price'],
        changeMethods: ['update_price_feed'],
        sender: accountId
    });

    usdtContract = new nearAPI.Contract(account, config.usdtContractName, {
        viewMethods: [
            'ft_total_supply',
            'ft_balance_of',
            'ft_metadata'
        ],
        changeMethods: [
            'ft_transfer',
            'ft_transfer_call',
            'ft_mint'
        ],
        sender: accountId
    });
}

async function fetchPriceFeed() {
    const connectionConfig = {
        networkId: "testnet",
        keyStore: new keyStores.InMemoryKeyStore(),
        nodeUrl: "https://rpc.testnet.near.org",
        walletUrl: "https://wallet.testnet.near.org",
        helperUrl: "https://helper.testnet.near.org",
        explorerUrl: "https://explorer.testnet.near.org",
    };


    // const nearConnection = await connect(connectionConfig);
    const near = await connect(connectionConfig);

    const account = await near.account("kenobi.testnet");
    const contractId = "pyth.testnet";
    const identifier = "63f341689d98a12ef60a5cff1d7f85c70a9e17bf1575f0e7c0b2512d48b1c8b3";

    const priceFeed = await account.viewFunction(contractId, "get_price", {
        identifier,
    });
    console.log("Price Feed Data: ", priceFeed);
}


async function getPrice(id) {
    console.log("ID: ", id)
    return await contract.get_price({ price_identifier: id });
}

function stringToU8Array32(str) {
    // Encode the string as UTF-8
    const encoder = new TextEncoder();
    const encoded = encoder.encode(str);

    // Create a new Uint8Array of 32 bytes
    let array = new Uint8Array(32);

    // Copy the encoded bytes to the array
    // (if the string is shorter than 32 bytes, the rest will remain zeros)
    // (if the string is longer, it will be truncated)
    array.set(encoded.slice(0, 32));

    array = Array(array)

    return array;
}

const crypto = require('crypto');

function stringTo32ByteHash(inputString) {
    return crypto.createHash('sha256').update(inputString).digest();
}


// Example usage:
(async () => {
    await initNear();
    let near_id = stringToU8Array32(NEAR_PRICE_ID);
    // const priceIdentifierBase64 = Buffer.from(priceIdentifier).toString('base64');
    const prices = await getPrice(near_id);

    contract.getPrice(near_id).then(response => {
        console.log("Function call response:", response);
    }).catch(error => {
        console.error("Error calling function:", error);
    });


    console.log(prices);
})();
