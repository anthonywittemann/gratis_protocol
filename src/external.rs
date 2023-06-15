use crate::*;

use near_sdk::{ext_contract};

use near_sdk::{Timestamp};

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::UnorderedMap;
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};

// pub mod util;
// use crate::util::*;

type AssetId = String;
pub type DurationSec = u32;


pub const TGAS: u64 = 1_000_000_000_000;
pub const NO_DEPOSIT: u128 = 0;
pub const XCC_SUCCESS: u64 = 1;

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AssetPrice {
    pub asset_id: AssetId,
    pub price: Price,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AssetOptionalPrice {
    pub asset_id: AssetId,
    pub price: Option<Price>,
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Clone, Copy)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[serde(crate = "near_sdk::serde")]
pub struct Price {
    // #[serde(with = "u128_dec_format")]
    pub multiplier: Balance,
    pub decimals: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PriceData {
    // #[serde(with = "u64_dec_format")]
    pub timestamp: Timestamp,
    pub recency_duration_sec: DurationSec,

    pub prices: Vec<AssetOptionalPrice>,
}


// Validator interface, for cross-contract calls
#[ext_contract(ext_price_oracle)]
trait PriceOracle {
    fn get_price_data(&self, asset_ids: Option<Vec<AssetId>>) -> PriceData;
}