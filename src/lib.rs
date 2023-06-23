use near_sdk::ext_contract;

pub mod big_decimal;
pub mod external;
pub mod oracle;

use crate::big_decimal::*;
use crate::external::*;

use near_sdk::borsh::maybestd::collections::{HashMap, HashSet};
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, log, near_bindgen, AccountId, Balance, Gas, PanicOnDefault, Promise, PromiseError,
    PublicKey, StorageUsage,
};

use std::str::FromStr;

const USDT_CONTRACT_ID: &str = "usdt.testnet"; // TODO: update with testnet address
const LENDING_CONTRACT_ID: &str = "gratis_protocol.testnet"; // TODO: update with testnet address
const PRICE_ORACLE_CONTRACT_ID: &str = "priceoracle.testnet";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;

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
    pub borrowed: Balance,
    pub collateral_ratio: BigDecimal,
}

#[near_bindgen]
impl LendingProtocol {
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

    fn get_usdt_value(&self, collateral: Balance) -> Promise {
        let gas: Gas = Gas(50_000_000_000_000);

        ext_price_oracle::ext(self.oracle_id.clone())
            .with_static_gas(gas)
            .get_price_data(Some(vec!["usdt.fakes.testnet".to_string()]))
            .then(Self::ext(env::current_account_id()).get_usdt_callback())
    }

    #[private]
    pub fn get_usdt_callback(
        &mut self,
        #[callback] call_result: Result<PriceData, String>,
    ) -> PriceData {
        match call_result {
            Ok(data) => {
                self.price_data = Some(data.clone());
                return data;
            }
            Err(err) => {
                log!("PromiseError occurred: {:?}", err);
                panic!("Failed to fetch price data."); // or however you want to handle this failure
            }
        }
    }

    pub fn get_latest_price(&self) -> PriceData {
        return self.price_data.clone().unwrap();
    }

    pub fn deposit_collateral(&mut self, mut amount: Balance) {
        let fee = amount / 200; // 0.5% fee
        amount -= fee;

        assert!(amount > 0, "Deposit Amount should be greater than 0");

        let account_id = env::signer_account_id();
        let loan: &mut Loan = self.loans.entry(account_id.clone()).or_insert(Loan {
            collateral: 0,
            borrowed: 0,
            collateral_ratio: if self.lower_collateral_accounts.contains(&account_id) {
                BigDecimal::from(LOWER_COLLATERAL_RATIO)
            } else {
                BigDecimal::from(MIN_COLLATERAL_RATIO)
            },
        });

        loan.collateral += amount;
        Promise::new(account_id).transfer(amount);
    }

    pub fn borrow(&mut self, usdt_amount: Balance) {
        assert!(usdt_amount > 0, "Borrow Amount should be greater than 0");

        let signer_account_id: AccountId = env::signer_account_id();

        println!("signer_account_id: {}", signer_account_id);

        let price = self.get_latest_price().prices[0].price.unwrap();

        let loan: &mut Loan = self
            .loans
            .get_mut(&signer_account_id)
            .expect("No collateral deposited");

        // get the latest price NEAR in USDT of the collateral asset

        // Calculate collateral and borrowed value
        let collateral_value: BigDecimal =
            BigDecimal::from_balance_price(loan.collateral, &price, 0);
        let borrowed_value: Balance = loan.borrowed;

        println!("collateral_value: {}", collateral_value);
        println!("borrowed_value: {}", borrowed_value);

        println!("collateral_ratio: {}", loan.collateral_ratio);

        // 0.000000000000000000014925
        // 0.000000000000000000000124375

        // get max borrowable amount
        let max_borrowable_amount = BigDecimal::from(100u128)
            * (collateral_value / loan.collateral_ratio)
            - BigDecimal::from(borrowed_value);

        println!("max_borrowable_amount: {}", max_borrowable_amount);
        println!("usdt_amount: {}", usdt_amount);

        println!("current_account_id: {}", env::current_account_id());

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        if (BigDecimal::from(usdt_amount) <= max_borrowable_amount) {
            // TODO: borrow the requested amount
            loan.borrowed += usdt_amount;
            Promise::new(env::current_account_id()).function_call(
                "ft_transfer".to_string(),
                format!(
                    r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Borrowed USDT"}}"#,
                    env::current_account_id(),
                    usdt_amount
                )
                .into_bytes(),
                0,
                Gas(50_000_000_000_000),
            );
        } else {
            assert_eq!(false, true, "Insufficient collateral");
        }
        /*
           1. Calculate the collateral value
           1a. Calculate current loan value
           2. Calculate max borrowable amount
           3. Check if the max borrowable amount is greater than the requested amount
           4. If yes, then borrow the requested amount
        */
    }

    // // The "repay" method calculates the actual repayment amount and the refund amount based on the outstanding loan. If there's an overpayment, it will refund the excess amount to the user.
    // pub fn repay(&mut self, usdt_amount: Balance) {
    //     let fee: u128 = usdt_amount / 200; // 0.5% fee
    //     usdt_amount -= fee;

    //     let account_id: AccountId = env::signer_account_id();
    //     let loan = self.loans.get_mut(&account_id).expect("No outstanding loan");

    //     let (repay_amount, refund_amount) = if loan.borrowed > usdt_amount {
    //         (usdt_amount, 0)
    //     } else {
    //         (loan.borrowed, usdt_amount - loan.borrowed)
    //     };

    //     loan.borrowed -= repay_amount;

    //     if loan.borrowed == 0 {
    //         loan.collateral = 0;
    //     }

    //     let mut promises= vec![Promise::new(AccountId::from_str(&USDT_CONTRACT_ID).unwrap()).function_call(
    //         "ft_transfer_from".to_string(),
    //         // TODO: replace LENDING_CONTRACT_ID with self.contract_id (if such functionality exists)
    //         format!(
    //             r#"{{"sender_id": "{}", "receiver_id": "{}", "amount": "{}", "memo": "Repayment"}}"#,
    //             account_id, LENDING_CONTRACT_ID.clone(), repay_amount
    //         ).into_bytes(),
    //         0,
    //         Gas(50_000_000_000_000),
    //     )];

    //     if refund_amount > 0 {
    //         let refund_promise = Promise::new(account_id.clone()).function_call(
    //             "ft_transfer".to_string(),
    //             format!(
    //                 r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Refund overpayment"}}"#,
    //                 account_id, refund_amount
    //             ).into_bytes(),
    //             0,
    //             Gas(50_000_000_000_000),
    //         );

    //         promises.push(refund_promise);
    //     }

    //     near_sdk::PromiseOrValue::when_all(promises).unwrap();
    // }
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
        let p = contract.get_usdt_value(usdt_amount);
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
}
