pub mod big_decimal;
pub mod external;
pub mod oracle;

use crate::big_decimal::*;
use crate::external::*;
use near_sdk::ext_contract;

use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_contract_standards::fungible_token::resolver::FungibleTokenResolver;
use near_sdk::borsh::maybestd::collections::{HashMap, HashSet};
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::env::current_account_id;
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, log, near_bindgen, AccountId, Balance, Gas, PanicOnDefault, Promise};
use near_sdk::{is_promise_success, serde_json, PromiseOrValue};
use std::str::FromStr;
// CONSTANTS
const USDT_CONTRACT_ID: &str = "usdt.fakes.testnet"; // TODO: update with testnet address
const LENDING_CONTRACT_ID: &str = "gratis_protocol.testnet"; // TODO: update with testnet address
const PRICE_ORACLE_CONTRACT_ID: &str = "priceoracle.testnet";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;
pub const ONE_NEAR: Balance = 1_000_000_000_000_000_000_000_000;
pub const GAS_FOR_FT_TRANSFER: Gas = Gas(50_000_000_000_000);
pub const SAFE_GAS: Balance = 50_000_000_000_000;

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, PanicOnDefault)]
#[serde(crate = "near_sdk::serde")]
pub struct LendingProtocol {
    pub loans: HashMap<AccountId, Loan>,
    pub lower_collateral_accounts: HashSet<AccountId>,
    pub oracle_id: AccountId,
    pub price_data: Option<PriceData>,
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
        // Empty message is used for stable coin depositing.

        let token_id = env::predecessor_account_id();

        // Update Borrowed Balance
        let mut loan: &mut Loan = self
            .loans
            .get_mut(&sender_id)
            .expect("No collateral deposited");

        // Handle transfer case when it is more than the borrowed amount
        assert!(amount.le(&loan.borrowed.into()));

        // let collateral_value: Balance = loan.collateral * price;

        loan.borrowed = loan.borrowed - amount.0;

        // TODO: Handle case to close the loan
        if msg == "close" && loan.borrowed == 0 {
            log!("Close the Loan");
            let collateral = loan.collateral;
            log!("Collateral: {}", collateral);
            log!("Send back: {}", collateral - SAFE_GAS);
            // gratis.transfer(collateral);
            Promise::new(sender_id.clone()).transfer(collateral - SAFE_GAS);
            loan.collateral = 0;
            self.loans.remove(&sender_id);
        }

        PromiseOrValue::Value(U128(0))
    }
}

#[near_bindgen]
impl FungibleTokenResolver for LendingProtocol {
    #[private]
    fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
    ) -> U128 {
        let sender_id: AccountId = sender_id.into();
        // IDK what to do here
        return U128(0);
    }
}

#[near_bindgen]
impl LendingProtocol {
    #[init]
    pub fn new(lower_collateral_accounts: Vec<AccountId>) -> Self {
        assert!(
            env::state_read::<Self>().is_none(),
            "Contract is already initialized"
        );
        assert_eq!(
            env::predecessor_account_id(),
            env::current_account_id(),
            "Only contract owner can call this method"
        );

        Self {
            loans: HashMap::new(),
            lower_collateral_accounts: lower_collateral_accounts.into_iter().collect(),
            oracle_id: AccountId::from_str(&PRICE_ORACLE_CONTRACT_ID).unwrap(),
            price_data: Some(PriceData::default()),
        }
    }

    pub fn get_all_loans(&self) -> HashMap<AccountId, Loan> {
        return self.loans.clone();
    }

    pub fn get_prices(&self) -> Promise {
        let gas: Gas = Gas(50_000_000_000_000);

        ext_price_oracle::ext(self.oracle_id.clone())
            .with_static_gas(gas)
            .get_price_data(Some(vec![
                "wrap.testnet".to_string(),
                "usdt.fakes.testnet".to_string(),
            ]))
            .then(Self::ext(env::current_account_id()).get_price_callback())
    }

    #[private]
    pub fn get_price_callback(&mut self, #[callback] data: PriceData) -> PriceData {
        self.price_data = Some(data.clone());
        data
    }

    pub fn get_latest_price(&self) -> PriceData {
        return self.price_data.clone().unwrap();
    }

    #[payable]
    pub fn deposit_collateral(&mut self, mut amount: Balance) -> bool {
        let deposit = env::attached_deposit();
        let mut fee = amount / 200; // 0.5% fee
        amount = amount * ONE_NEAR;
        assert!(
            deposit == amount,
            "Attached deposit is not equal to the amount"
        );
        fee = fee * ONE_NEAR;
        amount -= fee;

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

        loan.collateral += deposit;
        true
        // Promise::new(account_id).transfer(amount)
    }

    #[payable]
    pub fn borrow(&mut self, usdt_amount: u128) {
        /*S
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
        let price = self.get_latest_price().prices[0].price.unwrap();

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

        let borrowed_value: Balance = loan.borrowed;

        log!("collateral_value: {}", collateral_value);
        log!("borrowed_value: {}", borrowed_value);
        log!("collateral_ratio: {}", loan.collateral_ratio);

        // get max borrowable amount
        let total_max_borrowable_amount: u128 = 100u128 * collateral_value / loan.collateral_ratio;

        let max_borrowable_amount = total_max_borrowable_amount
            .checked_sub(borrowed_value)
            .unwrap_or(0);

        log!("max_borrowable_amount: {}", max_borrowable_amount);
        log!("usdt_amount: {}", usdt_amount);
        log!("current_account_id: {}", env::current_account_id());

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if usdt_amount <= max_borrowable_amount {
            // borrow the requested amount
            let usdt_contract_account_id: AccountId =
                AccountId::from_str(USDT_CONTRACT_ID.clone()).unwrap();
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

    #[payable]
    pub fn call_ft_transfer(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
    ) -> Promise {
        let usdt_contract_id: AccountId = AccountId::from_str(&USDT_CONTRACT_ID).unwrap();
        ext_usdt::ext(usdt_contract_id.clone())
            .with_static_gas(GAS_FOR_FT_TRANSFER)
            .with_attached_deposit(1)
            .ft_transfer(receiver_id, amount, memo)
            .then(Self::ext(env::current_account_id()).on_ft_transfer())
    }

    pub fn close(&mut self, collateral: Balance, sender_id: AccountId) {
        Promise::new(sender_id.clone()).transfer(collateral * ONE_NEAR);
    }

    #[private]
    pub fn on_ft_transfer(&self) {
        // Here you can handle the result of `ft_transfer_call`.
        // `success` indicates whether the transfer was successful.
        if "idk" != "" {
            log!("Transfer success!");
        } else {
            log!("Transfer failed!");
        }
    }

    // DOESN"T WORK
    // May need to call USDT contract directly from the frontend instead of using this method
    // Will use on_ft_transfer method to handle the result of ft_transfer_call to determine internal transfer stuff
    #[payable]
    pub fn transfer_to_self(&mut self, amount: U128) {
        let usdt_contract_id: AccountId = AccountId::from_str(&USDT_CONTRACT_ID).unwrap();

        ext_usdt::ext(usdt_contract_id.clone())
            .with_static_gas(GAS_FOR_FT_TRANSFER)
            .with_attached_deposit(1)
            .ft_transfer_call(
                env::current_account_id(), // receiver_id is the smart contract itself
                amount,
                None, // optional message
                "MSG".to_string(),
            );
    }

    // The "repay" method calculates the actual repayment amount and the refund amount based on the outstanding loan. If there's an overpayment, it will refund the excess amount to the user.
    pub fn repay(&mut self, usdt_amount: Balance) -> Promise {
        /*
          1. Calculate the collateral value
          2. Calculate current loaned value
          2. Calculate max repay amount
          3. Check if the max repay amount is greater than the requested amount
          4. If yes, then repay the requested amount
        */

        assert!(usdt_amount > 0, "Repay Amount should be greater than 0");

        let predecessor_account_id: AccountId = env::predecessor_account_id();

        let price = self.get_latest_price().prices[0].price.unwrap();

        let loan: &mut Loan = self
            .loans
            .get_mut(&predecessor_account_id)
            .expect("No collateral deposited");
        // get the latest price NEAR in USDT of the collateral asset

        // Calculate collateral and borrowed value
        // TODO convert to u128
        let collateral_value: u128 =
            BigDecimal::round_u128(&BigDecimal::from_balance_price(loan.collateral, &price, 0));
        let borrowed_value: Balance = loan.borrowed;

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if usdt_amount <= borrowed_value {
            loan.borrowed -= usdt_amount;
            // return collateral repaid value in NEAR
            // TODO calculate amount of collateral to return
            let collateral_to_return: u128 = loan.collateral.clone();
            Promise::new(predecessor_account_id.clone()).transfer(collateral_to_return)
        } else {
            // They overpaid. Protocol will return the full collateral in NEAR
            loan.borrowed = 0u128;
            let collateral_to_return: u128 = loan.collateral.clone();
            Promise::new(predecessor_account_id.clone()).transfer(collateral_to_return)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use near_sdk::{
        borsh::{self, BorshSerialize},
        near_bindgen,
        test_utils::VMContextBuilder,
        testing_env, AccountId, BorshStorageKey, Promise, PromiseOrValue, StorageUsage,
    };

    #[test]
    pub fn initialize() {
        let a: AccountId = "alice.near".parse().unwrap();
        // let v: Vec<AccountId> = vec![a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .build());
        let contract: LendingProtocol = LendingProtocol::new(vec![a.clone()]);
        assert_eq!(contract.oracle_id, "priceoracle.testnet".parse().unwrap())
    }

    #[test]
    pub fn test_get_usdt() {
        let a: AccountId = "alice.near".parse().unwrap();
        let v: Vec<AccountId> = vec![a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .build());

        let contract: LendingProtocol = LendingProtocol::new(vec![a.clone()]);
        let usdt_amount: Balance = 100;
        let p = contract.get_prices();
        // let result = contract.get_usdt_callback(); // Replace with actual callback method
        // println!("{:?}", result.prices.first().unwrap().price);

        // assert_eq!(result.prices.into(), 100);
        // let otherp: Promise = Promise::new(a.clone());
        // println!("{:?}", contract.price_data.unwrap().timestamp.to_string());
        // println!("{:?}", p.timestamp);
        // assert_eq!(p, otherp);

        // assert_eq!(contract.oracle_id, "price_oracle.testnet".parse().unwrap())
    }

    #[test]
    pub fn test_borrow() {
        let a: AccountId = "alice.near".parse().unwrap();
        let bob: AccountId = "bob.near".parse().unwrap();
        let v: Vec<AccountId> = vec![a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = LendingProtocol::new(vec![a.clone()]);
        let usdt_amount: Balance = 10000;
        let borrow_amount: Balance = 50;

        contract.deposit_collateral(usdt_amount);
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
        let bob: AccountId = "bob.near".parse().unwrap();
        let v: Vec<AccountId> = vec![a.clone()];
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(a.clone())
            .signer_account_id(a.clone())
            .build());

        let mut contract: LendingProtocol = LendingProtocol::new(vec![a.clone()]);
        let usdt_amount: Balance = 10000;
        let borrow_amount: Balance = 50;

        contract.deposit_collateral(usdt_amount);
        contract.borrow(borrow_amount);
        contract.repay(borrow_amount);

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, 0);
    }
}
