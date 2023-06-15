use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{
    env, near_bindgen, AccountId, Balance, PanicOnDefault, Promise, PublicKey, StorageUsage,
};

use std::collections::{HashMap, HashSet};

use crate::*;


#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct Oracle {
    pub loans: HashMap<AccountId, Loan>,
    pub allowed_accounts: HashSet<AccountId>,
}
