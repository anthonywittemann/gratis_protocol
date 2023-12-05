pub mod big_decimal;
pub mod external;
pub mod oracle;

use crate::big_decimal::BigDecimal;
use crate::external::{ext_price_oracle, PriceData};

use external::Price;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{
    borsh::{
        self,
        maybestd::collections::{HashMap, HashSet},
        BorshDeserialize, BorshSerialize,
    },
    env,
    json_types::U128,
    log, near_bindgen,
    serde::{Deserialize, Serialize},
    AccountId, Balance, Gas, PanicOnDefault, Promise, PromiseOrValue,
};
use std::str::FromStr;

// CONSTANTS
const USDT_CONTRACT_ID: &str = "usdt.fakes.testnet"; // TODO: update with testnet address
                                                     // const LENDING_CONTRACT_ID: &str = "gratis_protocol.testnet"; // TODO: update with testnet address
const PRICE_ORACLE_CONTRACT_ID: &str = "priceoracle.testnet";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;
pub const GAS_FOR_FT_TRANSFER: Gas = Gas(50_000_000_000_000);
pub const SAFE_GAS: Balance = 50_000_000_000_000;
pub const MIN_COLLATERAL_VALUE: u128 = 100;

#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct Asset {
    pub contract_id: Option<AccountId>,
    pub oracle_asset_id: String,
    pub last_price: Option<Price>,
}

impl Asset {
    pub fn new(contract_id: Option<AccountId>, oracle_asset_id: String) -> Self {
        Self {
            contract_id,
            oracle_asset_id,
            last_price: None,
        }
    }
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, PanicOnDefault)]
#[serde(crate = "near_sdk::serde")]
pub struct LendingProtocol {
    pub loans: HashMap<AccountId, Loan>, // TODO: near-sdk::store collections
    pub lower_collateral_accounts: HashSet<AccountId>,
    pub oracle_id: AccountId,
    pub collateral_asset: Asset,
    pub loan_asset: Asset,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Copy)]
#[serde(crate = "near_sdk::serde")]
pub struct Loan {
    pub collateral: Balance, // NOTE: this only works with NEAR as collateral currency
    pub borrowed: u128,
    pub collateral_ratio: u128,
}

#[near_bindgen]
impl FungibleTokenReceiver for LendingProtocol {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        // update loan information
        self.repay(&sender_id, amount.0);

        // close loan if requested
        if msg == "close" {
            self.close();
        }

        PromiseOrValue::Value(U128(0))
    }
}

#[near_bindgen]
impl LendingProtocol {
    #[init]
    #[private]
    pub fn new(
        lower_collateral_accounts: Vec<AccountId>,
        collateral_asset_id: Option<AccountId>,
        collateral_oracle_asset_id: String,
        loan_asset_id: AccountId,
        loan_oracle_asset_id: String,
    ) -> Self {
        Self {
            loans: HashMap::new(),
            lower_collateral_accounts: lower_collateral_accounts.into_iter().collect(),
            oracle_id: AccountId::from_str(PRICE_ORACLE_CONTRACT_ID).unwrap(),
            collateral_asset: Asset::new(collateral_asset_id, collateral_oracle_asset_id),
            loan_asset: Asset::new(Some(loan_asset_id), loan_oracle_asset_id),
        }
    }

    // Deposit collateral function allows the user to deposit the collateral to the contract. Creates a Loan if the user doesn't have a loan
    #[payable]
    pub fn deposit_collateral(&mut self) -> bool {
        let deposit = env::attached_deposit();
        let fee = deposit / 200; // 0.5% fee
        let mut amount = deposit;

        // fee = fee; // FIXME: What is this supposed to do?
        amount -= fee;
        let col = deposit - fee;

        assert!(amount > 0, "Deposit Amount should be greater than 0");

        let account_id = env::predecessor_account_id();
        let loan: &mut Loan = self.loans.entry(account_id.clone()).or_insert(Loan {
            collateral: 0,
            borrowed: 0,
            collateral_ratio: if self.lower_collateral_accounts.contains(&account_id) {
                LOWER_COLLATERAL_RATIO
            } else {
                MIN_COLLATERAL_RATIO
            },
        });

        loan.collateral += col;
        true
    }

    // Remove collateral function allows the user to withdraw the collateral from the contract
    pub fn remove_collateral(&mut self, amount: Balance) -> bool {
        let account_id = env::predecessor_account_id();
        let loan: &mut Loan = self
            .loans
            .get_mut(&account_id)
            .expect("No collateral deposited");

        assert!(amount > 0, "Withdraw Amount should be greater than 0");

        assert!(
            loan.collateral >= amount,
            "Withdraw Amount should be less than the deposited amount. Loan Collateral: {}, Amount: {}", loan.collateral, amount
        );

        assert!(
            MIN_COLLATERAL_RATIO > 100 * (loan.borrowed / (loan.collateral - amount)),
            "Collateral ratio should be greater than 120%"
        );

        Promise::new(account_id.clone()).transfer(amount);
        loan.collateral -= amount;
        true
    }

    // Close function calculates the difference between loan and collateral and returns the difference to the user
    pub fn close(&mut self) {
        let predecessor_account_id: AccountId = env::predecessor_account_id();
        let loan = self.loans.get_mut(&predecessor_account_id).unwrap();
        let collateral = loan.collateral;
        let borrowed = loan.borrowed;

        let additional_collateral = collateral - borrowed;
        if additional_collateral > 0 {
            Promise::new(predecessor_account_id.clone()).transfer(additional_collateral);
        }
        // remove loans
        self.loans.remove(&predecessor_account_id);
    }

    pub fn borrow(&mut self, usdt_amount: u128) {
        /*
           1. Calculate the collateral value
           1a. Calculate current loan value
           2. Calculate max borrowable amount
           3. Check if the max borrowable amount is greater than the requested amount
           4. If yes, then borrow the requested amount
        */

        assert!(usdt_amount > 0, "Borrow Amount should be greater than 0");

        let account_id: AccountId = env::predecessor_account_id();
        log!("predecessor_account_id: {}", account_id);

        // Get NEAR Price
        let price = self.collateral_asset.last_price.unwrap();

        let near_usdt_price: u128 = price.multiplier / 10000;
        log!("price: {}", price.multiplier);
        log!("near_usdt_price: {}", near_usdt_price);

        let loan: &mut Loan = self
            .loans
            .get_mut(&account_id)
            .expect("No collateral deposited");

        // get the latest price NEAR in USDT of the collateral asset

        log!("raw collateral; {}", loan.collateral);
        // Calculate collateral and borrowed value
        // TODO convert to u128
        let collateral_value: u128 =
            BigDecimal::round_u128(&BigDecimal::from_balance_price(loan.collateral, &price, 0));

        // let collateral_value: Balance = loan.collateral * price;

        let borrowed_value: u128 = loan.borrowed;

        log!("collateral_value: {}", collateral_value);
        log!("borrowed_value: {}", borrowed_value);
        log!("collateral_ratio: {}", loan.collateral_ratio);

        // get max borrowable amount
        let total_max_borrowable_amount: u128 = 100u128 * collateral_value / loan.collateral_ratio;

        let max_borrowable_amount = total_max_borrowable_amount.saturating_sub(borrowed_value);

        log!("max_borrowable_amount: {}", max_borrowable_amount);
        log!("usdt_amount: {}", usdt_amount);
        log!("current_account_id: {}", env::current_account_id());

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if usdt_amount <= max_borrowable_amount {
            // borrow the requested amount
            let usdt_contract_account_id: AccountId =
                AccountId::from_str(USDT_CONTRACT_ID).unwrap();
            loan.borrowed += usdt_amount;
            Promise::new(usdt_contract_account_id).function_call(
                "ft_transfer".to_string(),
                format!(
                    r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Borrowed USDT"}}"#,
                    account_id.clone(),
                    usdt_amount
                )
                .into_bytes(),
                1,
                Gas(50_000_000_000_000),
            );
        } else {
            log!("max_borrowable_amount: {}", max_borrowable_amount);
            log!("usdt_amount: {}", usdt_amount);
            // assert_eq!(false, true, "Insufficient collateral")
        }
    }

    // The "repay" method calculates the actual repayment amount and the refund amount based on the outstanding loan. If there's an overpayment, it will refund the excess amount to the user.
    pub fn repay(&mut self, account_id: &AccountId, usdt_amount: u128) -> Option<Promise> {
        /*
          1. Calculate the collateral value
          2. Calculate current loaned value
          2. Calculate max repay amount
          3. Check if the max repay amount is greater than the requested amount
          4. If yes, then repay the requested amount
        */

        assert!(usdt_amount > 0, "Repay Amount should be greater than 0");

        // let predecessor_account_id: AccountId = env::predecessor_account_id();

        let price = self.collateral_asset.last_price.unwrap();

        let loan: &mut Loan = self
            .loans
            .get_mut(account_id)
            .expect("No collateral deposited");
        // get the latest price NEAR in USDT of the collateral asset

        // Calculate collateral and borrowed value
        let collateral_value: u128 =
            BigDecimal::round_u128(&BigDecimal::from_balance_price(loan.collateral, &price, 0));

        let borrowed_value: u128 = loan.borrowed;

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if usdt_amount + MIN_COLLATERAL_VALUE <= borrowed_value {
            loan.borrowed -= usdt_amount;
            // Recalculate collateral ratio
            loan.collateral_ratio = collateral_value / loan.borrowed;
            // Fix return
            None
        } else {
            // They overpaid. Protocol will return the full collateral in NEAR
            loan.borrowed = MIN_COLLATERAL_VALUE;
            loan.collateral_ratio = collateral_value / loan.borrowed;
            None
            // Some(Promise::new(predecessor_account_id.clone()).transfer(collateral_to_return))
        }
    }

    /* -----------------------------------------------------------------------------------
    ------------------------------------ GETTERS -----------------------------------------
    -------------------------------------------------------------------------------------- */

    pub fn get_all_loans(&self) -> HashMap<AccountId, Loan> {
        self.loans.clone()
    }

    pub fn get_prices(&self) -> Promise {
        let gas: Gas = Gas(50_000_000_000_000);

        ext_price_oracle::ext(self.oracle_id.clone())
            .with_static_gas(gas)
            .get_price_data(Some(vec![
                self.collateral_asset.oracle_asset_id.clone(),
                self.loan_asset.oracle_asset_id.clone(),
            ]))
            .then(Self::ext(env::current_account_id()).get_price_callback())
    }

    #[private]
    pub fn get_price_callback(&mut self, #[callback] data: PriceData) -> PriceData {
        match &data.prices[..] {
            [collateral_asset_price, loan_asset_price]
                if collateral_asset_price.asset_id == self.collateral_asset.oracle_asset_id
                    && loan_asset_price.asset_id == self.loan_asset.oracle_asset_id =>
            {
                if let Some(price) = collateral_asset_price.price {
                    self.collateral_asset.last_price.replace(price);
                }
                if let Some(price) = loan_asset_price.price {
                    self.loan_asset.last_price.replace(price);
                }
            }
            _ => env::panic_str(&format!("Invalid price data returned by oracle: {data:?}")),
        }

        // TODO: Something with the timestamp/recency data

        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use near_sdk::{test_utils::VMContextBuilder, testing_env, AccountId, ONE_NEAR};

    // Auxiliar fn: create a mock context
    fn set_context(predecessor: &str, amount: Balance) {
        let mut builder = VMContextBuilder::new();
        builder.predecessor_account_id(predecessor.parse().unwrap());
        builder.attached_deposit(amount);

        testing_env!(builder.build());
    }

    fn are_vectors_equal(vec1: Vec<AccountId>, vec2: Vec<AccountId>) {
        assert_eq!(vec1.len(), vec2.len(), "Vectors have different lengths");

        let count_occurrences = |vec: Vec<AccountId>| -> HashMap<AccountId, usize> {
            let mut map = HashMap::new();
            for item in vec {
                *map.entry(item).or_insert(0) += 1;
            }
            map
        };

        assert_eq!(
            count_occurrences(vec1.clone()),
            count_occurrences(vec2.clone()),
            "Vectors have the same length but contain different values"
        );
    }

    fn init_sane_defaults(lower_collateral_accounts: Vec<AccountId>) -> LendingProtocol {
        LendingProtocol::new(
            lower_collateral_accounts,
            None,
            "wrap.testnet".to_string(),
            "usdt.fakes.testnet".parse().unwrap(),
            "usdt.fakes.testnet".to_string(),
        )
    }

    #[test]
    pub fn initialize() {
        let a: AccountId = "alice.near".parse().unwrap();
        // let v: Vec<AccountId> = vec![a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .build());
        let contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        assert_eq!(contract.oracle_id, "priceoracle.testnet".parse().unwrap())
    }

    #[test]
    pub fn test_borrow() {
        let a: AccountId = "alice.near".parse().unwrap();
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        contract.get_price_callback(PriceData::default()); // mock oracle results
        let collateral_amount: Balance = 10000;
        let borrow_amount: Balance = 50;

        set_context("alice.near", collateral_amount);

        contract.deposit_collateral();
        contract.borrow(borrow_amount);

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, borrow_amount);
    }

    #[test]
    pub fn test_repay() {
        let a: AccountId = "alice.near".parse().unwrap();

        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        contract.get_price_callback(PriceData::default()); // mock oracle results
        let collateral_amount: Balance = 10000;
        let borrow_amount: Balance = 150;

        set_context("alice.near", collateral_amount);

        contract.deposit_collateral();

        contract.borrow(borrow_amount);

        // Need to import Stable Coin contract and do a transfer
        contract.repay(&a, 50);

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, MIN_COLLATERAL_VALUE);
    }

    #[test]
    pub fn test_remove_collateral() {
        let a: AccountId = "alice.near".parse().unwrap();

        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        contract.get_price_callback(PriceData::default()); // mock oracle results
        let collateral_amount: Balance = 10000;
        let borrow_amount: Balance = 50;
        let fee: u128 = collateral_amount / 200;

        set_context("alice.near", collateral_amount);

        contract.deposit_collateral();
        contract.borrow(borrow_amount);
        contract.remove_collateral(5000);

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, borrow_amount);
        assert_eq!(loan.collateral, collateral_amount - 5000 - fee);
    }

    #[test]
    pub fn close_loan() {
        let a: AccountId = "alice.near".parse().unwrap();
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        let collateral_amount: Balance = ONE_NEAR;
        let borrow_amount: Balance = 50;
        contract.get_price_callback(PriceData::default()); // mock oracle results

        // assert_eq!(env::account_balance(), 50);

        set_context("alice.near", collateral_amount);

        // assert_eq!(env::account_balance(), 99);

        contract.deposit_collateral();

        //assert_eq!(env::account_balance(), 100);

        contract.borrow(borrow_amount);
        contract.close();

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        assert_eq!(contract.loans.len(), 0);
        // assert_eq!(env::account_balance(), 1000 * ONE_NEAR - fee);
    }

    #[test]
    pub fn open_multiple_loans() {
        let a: AccountId = "alice.near".parse().unwrap();
        let bob: AccountId = "bob.near".parse().unwrap();
        let v: Vec<AccountId> = vec![bob.clone(), a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = init_sane_defaults(vec![a.clone()]);
        let collateral_amount: Balance = ONE_NEAR;

        set_context("alice.near", collateral_amount);
        contract.deposit_collateral();

        set_context("bob.near", collateral_amount);
        contract.deposit_collateral();

        let mut loan_accounts: Vec<AccountId> = Vec::new();
        let loans = contract.get_all_loans();
        for (key, value) in loans {
            println!("Loan: {}: {}", key.clone(), value.borrowed);
            loan_accounts.push(key);
        }

        are_vectors_equal(loan_accounts, v);
        assert_eq!(contract.loans.len(), 2);
    }
}
