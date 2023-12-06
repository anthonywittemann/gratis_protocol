pub mod asset;
pub mod big_decimal;
pub mod external;
pub mod oracle;

use crate::big_decimal::BigDecimal;
use crate::external::{ext_price_oracle, PriceData};

use asset::{
    CollateralAssetBalance, ContractAsset, LoanAssetBalance, NativeAsset, OracleCanonicalValuation,
};
use external::Price;
use near_contract_standards::fungible_token::{core::ext_ft_core, receiver::FungibleTokenReceiver};
use near_sdk::store::UnorderedMap;
use near_sdk::{
    borsh::{
        self,
        maybestd::collections::{HashMap, HashSet},
        BorshDeserialize, BorshSerialize,
    },
    env,
    json_types::U128,
    log, near_bindgen, require,
    serde::{Deserialize, Serialize},
    AccountId, Balance, Gas, PanicOnDefault, Promise, PromiseOrValue,
};
use std::ops::Mul;
use std::str::FromStr;

// CONSTANTS
// const LENDING_CONTRACT_ID: &str = "gratis_protocol.testnet"; // TODO: update with testnet address
const PRICE_ORACLE_CONTRACT_ID: &str = "priceoracle.testnet";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;
pub const GAS_FOR_FT_TRANSFER: Gas = Gas(50_000_000_000_000);
pub const SAFE_GAS: Balance = 50_000_000_000_000;

#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Fraction {
    numerator: U128,
    denominator: U128,
}

impl<T: Into<U128>, U: Into<U128>> From<(T, U)> for Fraction {
    fn from((numerator, denominator): (T, U)) -> Self {
        Self {
            numerator: numerator.into(),
            denominator: denominator.into(),
        }
    }
}

impl Fraction {
    pub fn new(numerator: U128, denominator: U128) -> Self {
        Self {
            numerator,
            denominator,
        }
    }
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct LendingProtocol {
    pub loans: UnorderedMap<AccountId, Loan>,
    pub lower_collateral_accounts: HashSet<AccountId>,
    pub oracle_id: AccountId,
    pub collateral_asset: NativeAsset,
    pub loan_asset: ContractAsset,
    pub deposit_fee: Fraction,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct LendingProtocolConfiguration {
    pub oracle_id: AccountId,
    pub collateral_oracle_asset_id: String,
    pub loan_asset_id: AccountId,
    pub loan_oracle_asset_id: String,
    pub deposit_fee: Fraction,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Copy)]
#[serde(crate = "near_sdk::serde")]
pub struct Loan {
    pub collateral: CollateralAssetBalance, // NOTE: this only works with NEAR as collateral currency
    pub borrowed: LoanAssetBalance,
    pub min_collateral_ratio: u128,
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
        let excess = self.repay(&sender_id, LoanAssetBalance(amount.0));

        // close loan if requested
        if msg == "close" {
            self.close();
        }

        PromiseOrValue::Value(U128(*excess))
    }
}

fn valuation(amount: u128, price: &Price) -> OracleCanonicalValuation {
    BigDecimal::round_u128(&BigDecimal::from_balance_price(amount, price, 0)).into()
}

#[near_bindgen]
impl LendingProtocol {
    fn loan_valuation(&self, loan_amount: LoanAssetBalance) -> OracleCanonicalValuation {
        valuation(*loan_amount, &self.loan_asset.last_price.unwrap())
    }

    fn collateral_valuation(
        &self,
        collateral_amount: CollateralAssetBalance,
    ) -> OracleCanonicalValuation {
        valuation(
            *collateral_amount,
            &self.collateral_asset.last_price.unwrap(),
        )
    }

    #[init]
    #[private]
    pub fn new(
        lower_collateral_accounts: Vec<AccountId>,
        collateral_oracle_asset_id: String,
        loan_asset_id: AccountId,
        loan_oracle_asset_id: String,
        deposit_fee: Fraction,
    ) -> Self {
        require!(
            deposit_fee.numerator < deposit_fee.denominator,
            "Invalid fee"
        );

        Self {
            loans: UnorderedMap::new(b"l"),
            lower_collateral_accounts: lower_collateral_accounts.into_iter().collect(),
            oracle_id: AccountId::from_str(PRICE_ORACLE_CONTRACT_ID).unwrap(),
            collateral_asset: NativeAsset::new(collateral_oracle_asset_id),
            loan_asset: ContractAsset::new(loan_asset_id, loan_oracle_asset_id),
            deposit_fee,
        }
    }

    // Deposit collateral function allows the user to deposit the collateral to the contract. Creates a Loan if the user doesn't have a loan
    #[payable]
    pub fn deposit_collateral(&mut self) -> bool {
        let deposit = CollateralAssetBalance(env::attached_deposit());
        let fee = CollateralAssetBalance(
            deposit
                .mul(self.deposit_fee.numerator.0)
                .div_ceil(self.deposit_fee.denominator.0),
        ); // round fee up

        // TODO: track fees

        let amount = CollateralAssetBalance(
            deposit
                .checked_sub(*fee)
                // should never underflow if fee <= 100%
                .unwrap_or_else(|| env::panic_str("Underflow during fee calculation")),
        );

        // Note: this is exceedingly unlikely (really should only happen if
        // deposit is small while fee is close to 1), but definitely not
        // impossible.
        require!(
            *amount > 0,
            "Deposit amount after fee must be greater than 0",
        );

        let loan: &mut Loan = self
            .loans
            .entry(env::predecessor_account_id())
            .or_insert(Loan {
                collateral: CollateralAssetBalance(0),
                borrowed: LoanAssetBalance(0),
                min_collateral_ratio: if self
                    .lower_collateral_accounts
                    .contains(&env::predecessor_account_id())
                {
                    LOWER_COLLATERAL_RATIO
                } else {
                    MIN_COLLATERAL_RATIO
                },
            });

        loan.collateral += amount;
        true
    }

    // Remove collateral function allows the user to withdraw the collateral from the contract
    pub fn remove_collateral(&mut self, amount: U128) -> bool {
        let remove_collateral_amount = CollateralAssetBalance(amount.0);

        assert!(
            *remove_collateral_amount > 0,
            "Withdraw amount should be greater than 0"
        );

        let account_id = env::predecessor_account_id();
        let mut loan: Loan = *self
            .loans
            .get(&account_id)
            .expect("No collateral deposited");

        assert!(
            loan.collateral >= remove_collateral_amount,
            "Withdraw amount should be less than the deposited amount. Loan Collateral: {}, Amount: {}", loan.collateral, remove_collateral_amount
        );

        let collateral_value_after_withdrawal =
            self.collateral_valuation(loan.collateral - remove_collateral_amount);

        let current_loan_valuation = self.loan_valuation(loan.borrowed);

        assert!(
            collateral_value_after_withdrawal * 100
                > current_loan_valuation * loan.min_collateral_ratio,
            "Collateral ratio should be greater than 120%"
        );

        loan.collateral -= remove_collateral_amount;
        self.loans.insert(account_id.clone(), loan);

        Promise::new(account_id).transfer(*remove_collateral_amount);
        true
    }

    // Close function calculates the difference between loan and collateral and returns the difference to the user
    pub fn close(&mut self) {
        todo!()
        // let predecessor_account_id: AccountId = env::predecessor_account_id();
        // let loan = self.loans.get_mut(&predecessor_account_id).unwrap();
        // let collateral = loan.collateral;
        // let borrowed = loan.borrowed;

        // require!(*borrowed == 0, "Loan must be fully repaid before closing");

        // if *collateral > 0 {
        //     Promise::new(predecessor_account_id.clone()).transfer(*collateral);
        // }

        // // remove loans
        // self.loans.remove(&predecessor_account_id);
    }

    pub fn borrow(&mut self, amount: U128) {
        /*
           1. Calculate the collateral value
           1a. Calculate current loan value
           2. Calculate max borrowable amount
           3. Check if the max borrowable amount is greater than the requested amount
           4. If yes, then borrow the requested amount
        */
        let loan_amount = LoanAssetBalance(amount.0);

        assert!(*loan_amount > 0, "Borrow Amount should be greater than 0");

        let account_id: AccountId = env::predecessor_account_id();
        log!("predecessor_account_id: {}", account_id);

        // Get collateral price
        let price = self.collateral_asset.last_price.unwrap();

        // useless
        {
            let near_usdt_price: u128 = price.multiplier / 10000;
            log!("price: {}", price.multiplier);
            log!("near_usdt_price: {}", near_usdt_price);
        }

        let mut loan: Loan = *self
            .loans
            .get(&account_id)
            .expect("No collateral deposited");

        // get the latest price NEAR in USDT of the collateral asset

        log!("raw collateral; {}", loan.collateral);
        // Calculate collateral and borrowed value
        let collateral_value = self.collateral_valuation(loan.collateral);
        let borrowed_value = self.loan_valuation(loan.borrowed);

        log!("collateral_value: {}", collateral_value);
        log!("borrowed_value: {}", borrowed_value);
        log!("collateral_ratio: {}", loan.min_collateral_ratio);

        // get max borrowable amount
        let total_max_borrowable_value = 100u128 * collateral_value / loan.min_collateral_ratio;

        let max_additional_borrowable_valuation = if total_max_borrowable_value > borrowed_value {
            total_max_borrowable_value - borrowed_value
        } else {
            0.into()
        };

        log!(
            "max_additional_borrowable_valuation: {}",
            max_additional_borrowable_valuation
        );
        log!("loan_amount: {}", loan_amount);
        let loan_amount_valuation = self.loan_valuation(loan_amount);
        log!("loan_amount_valuation: {}", loan_amount_valuation);
        log!("current_account_id: {}", env::current_account_id());

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if loan_amount_valuation <= max_additional_borrowable_valuation {
            // borrow the requested amount
            loan.borrowed += loan_amount;
            self.loans.insert(account_id.clone(), loan);
            ext_ft_core::ext(self.loan_asset.contract_id.clone())
                .with_static_gas(Gas(5_000_000_000_000))
                .with_attached_deposit(1)
                .ft_transfer(
                    account_id.clone(),
                    loan_amount.0.into(),
                    Some("Borrowed USDT".to_string()),
                );
        } else {
            log!(
                "max_borrowable_amount: {}",
                max_additional_borrowable_valuation
            );
            log!("loan_amount: {}", loan_amount);
            // assert_eq!(false, true, "Insufficient collateral")
        }
    }

    /// Repay a loan. Returns any excess repayment, which should be refunded.
    pub(crate) fn repay(
        &mut self,
        account_id: &AccountId,
        amount: LoanAssetBalance,
    ) -> LoanAssetBalance {
        /*
          1. Calculate the collateral value
          2. Calculate current loaned value
          2. Calculate max repay amount
          3. Check if the max repay amount is greater than the requested amount
          4. If yes, then repay the requested amount
        */

        assert!(*amount > 0, "Repay amount must be greater than 0");

        let loan: &mut Loan = self
            .loans
            .get_mut(account_id)
            .expect("No collateral deposited");

        if amount > loan.borrowed {
            let excess = amount - loan.borrowed;
            loan.borrowed.0 = 0;
            excess
        } else {
            loan.borrowed -= amount;
            0.into()
        }
    }

    /* -----------------------------------------------------------------------------------
    ------------------------------------ GETTERS -----------------------------------------
    -------------------------------------------------------------------------------------- */

    pub fn get_all_loans(&self) -> HashMap<&AccountId, &Loan> {
        self.loans.iter().collect()
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
            "wrap.testnet".to_string(),
            "usdt.fakes.testnet".parse().unwrap(),
            "usdt.fakes.testnet".to_string(),
            (1, 200).into(), // 0.5%
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
        let borrow_amount = LoanAssetBalance(50);

        set_context("alice.near", collateral_amount);

        contract.deposit_collateral();
        contract.borrow(borrow_amount.0.into());

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

        contract.borrow(borrow_amount.into());

        // Need to import Stable Coin contract and do a transfer
        contract.repay(&a, 50.into());

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, 100.into());
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
        let collateral_amount = CollateralAssetBalance(10000);
        let borrow_amount = LoanAssetBalance(50);
        let fee = collateral_amount / 200;

        set_context("alice.near", *collateral_amount);

        contract.deposit_collateral();
        contract.borrow(borrow_amount.0.into());
        contract.remove_collateral(5000.into());

        let loans = contract.get_all_loans();
        for (key, value) in &loans {
            println!("Loan: {}: {}", key, value.borrowed);
        }

        let loan = contract.loans.get(&a).unwrap();
        assert_eq!(loan.borrowed, borrow_amount);
        assert_eq!(
            loan.collateral,
            collateral_amount - CollateralAssetBalance(5000) - fee
        );
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

        contract.borrow(borrow_amount.into());
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
            println!("Loan: {}: {}", key, value.borrowed);
            loan_accounts.push(key.clone());
        }

        are_vectors_equal(loan_accounts, v);
        assert_eq!(contract.loans.len(), 2);
    }
}
