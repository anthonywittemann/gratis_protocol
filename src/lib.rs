

use near_sdk::{ext_contract};

pub mod external;
use crate::external::*;

pub mod oracle;

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::borsh::maybestd::collections::{HashMap};
use near_sdk::collections::{LookupMap, UnorderedMap, UnorderedSet};
use near_sdk::{
    env, near_bindgen, AccountId, Balance, Gas, PanicOnDefault, Promise, PromiseResult, PublicKey, StorageUsage,
};

use std::str::FromStr;

const USDT_CONTRACT_ID: &str = "usdt.testnet";  // TODO: update with testnet address
const LENDING_CONTRACT_ID: &str = "gratis_protocol.testnet"; // TODO: update with testnet address
const PRICE_ORACLE_CONTRACT_ID: &str = "price_oracle.testnet";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;
const DEFAULT_GAS_AMOUNT: Gas = Gas(50_000_000_000_000);

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct LendingContract {
    pub loans: LookupMap<AccountId, Loan>,
    pub privaleged_accounts: UnorderedSet<AccountId>,
    pub general_accounts: UnorderedSet<AccountId>,
    /// The last price from the oracle
    pub last_prices: HashMap<TokenId, Price>,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct Loan {
    pub collateral: Balance,
    pub borrowed: Balance,
    pub collateral_ratio: u128,
}

#[near_bindgen]
impl LendingContract {
    /// Initializes the contract. Should only be called once
    #[init]
    pub fn new(privaleged_accounts: Vec<AccountId>) -> Self {
        assert!(env::state_read::<Self>().is_none(), "Contract is already initialized");
        assert_eq!(env::predecessor_account_id(), env::current_account_id(), "Only contract owner can call this method");
    
        Self {
            loans: LookupMap::new(),
            privaleged_accounts: privaleged_accounts.into_iter().collect(),
            general_accounts: UnorderedSet::new(),
            last_prices: HashMap::new(),
        }
    }

    fn get_usdt_price(&self, asset: String) -> Promise {
        ext_price_oracle::ext(self.oracle_id.clone())
        .with_static_gas(DEFAULT_GAS_AMOUNT)
        .get_price_data(Some(vec![asset]))
    }

    pub fn deposit_collateral(&mut self, mut amount: Balance) -> Promise {
        let fee = amount / 200; // 0.5% fee
        amount -= fee;

        assert!(amount > 0, "Deposit Amount should be greater than 0");

        let account_id = env::signer_account_id();
        let loan: &mut Loan = self.loans.entry(account_id.clone()).or_insert(Loan {
            collateral: 0,
            borrowed: 0,
            collateral_ratio: if self.privaleged_accounts.contains(&account_id) {
                LOWER_COLLATERAL_RATIO
            } else {
                MIN_COLLATERAL_RATIO
            },
        });

        loan.collateral += amount;
        Promise::new(account_id).transfer(amount)
    }

    #[private]
    pub fn borrow_callback(&mut self, 
        #[callback_result] price_promise_result: PricePromiseResult, 
        usdt_amount: Balance, 
        current_account_id: AccountId, 
        loan: &mut Loan
    ) -> Promise {
        let usdt_collateral_value: u128 = (price * loan.collateral) / 100;
        let min_usdt_value: u128 = (usdt_amount * loan.collateral_ratio) / 100;

        assert!(usdt_collateral_value >= min_usdt_value, "Insufficient collateral");

        loan.borrowed += usdt_amount;
        Promise::new(current_account_id).function_call(
            "ft_transfer".to_string(),
            format!(
                r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Borrowed USDT"}}"#,
                current_account_id, usdt_amount
            ).into_bytes(),
            0,
            DEFAULT_GAS_AMOUNT,
        )
    }

    // pub fn borrow(&mut self, usdt_amount: Balance) -> Promise {
    //     assert!(usdt_amount > 0, "Borrow Amount should be greater than 0");
    
    //     let signer_account_id: AccountId = env::signer_account_id();
    //     let loan: &mut Loan = self.loans.get_mut(&signer_account_id).expect("No collateral deposited");
    //     let asset: String = "NEAR".to_string();  // TODO rm hardcoded NEAR asset and allow for others
    //     self.get_usdt_price(asset).then(
    //         Self::ext(env::current_account_id())
    //         .with_static_gas(DEFAULT_GAS_AMOUNT)
    //         .borrow_callback()
    //         .input((
    //             usdt_amount,
    //             env::current_account_id(),
    //             loan,
    //         ))
    //         .gas(DEFAULT_GAS_AMOUNT)
    //     )
    // }

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
