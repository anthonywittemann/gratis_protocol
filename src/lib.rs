mod asset;
mod big_decimal;
mod external;
mod linked_list;
mod loan;
mod oracle;
mod util;

use crate::asset::{
    valuation, CollateralAssetBalance, ContractAsset, LoanAssetBalance, NativeAsset,
    OracleCanonicalValuation,
};
use crate::external::{ext_price_oracle, PriceData};
use crate::linked_list::LinkedList;
use crate::loan::{Loan, LoanStatus};
use crate::util::Fraction;

use near_contract_standards::fungible_token::{core::ext_ft_core, receiver::FungibleTokenReceiver};
use near_sdk::assert_one_yocto;
use near_sdk::json_types::U64;
use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    env,
    json_types::U128,
    log, near_bindgen, require,
    serde::{Deserialize, Serialize},
    store::{LookupMap, LookupSet, UnorderedMap},
    AccountId, Balance, BorshStorageKey, Gas, PanicOnDefault, Promise, PromiseOrValue,
};

use std::ops::Mul;

// CONSTANTS
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;
pub const GAS_FOR_FT_TRANSFER: Gas = Gas(50_000_000_000_000);
pub const SAFE_GAS: Balance = 50_000_000_000_000;

#[derive(BorshSerialize, BorshStorageKey)]
enum StorageKey {
    Loans,
    Lenders,
    LendingPoolWithdrawalRequests,
    LendingPoolInFlightWithdrawalRequests,
    LendingPoolWithdrawalQueue,
    LowerCollateralAccounts,
}

type UniqueId = u64;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct LendingProtocolConfiguration {
    pub oracle_contract_id: AccountId,
    pub collateral_oracle_asset_id: String,
    pub loan_asset_contract_id: AccountId,
    pub loan_oracle_asset_id: String,
    pub deposit_fee: Fraction,
    pub lower_collateral_accounts: Vec<AccountId>,
}

#[derive(BorshSerialize, BorshDeserialize, Default, Clone, Debug)]
pub struct LenderEntry {
    pub amount_in_lending_pool: LoanAssetBalance,
    pub pending_withdrawal_requests: Vec<UniqueId>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct WithdrawalRequest {
    account_id: AccountId,
    amount: LoanAssetBalance,
}

impl WithdrawalRequest {
    pub fn new(account_id: AccountId, amount: LoanAssetBalance) -> Self {
        Self { account_id, amount }
    }
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct LendingProtocol {
    pub unique_id: UniqueId,
    pub loans: UnorderedMap<AccountId, Loan>,
    pub lenders: UnorderedMap<AccountId, LenderEntry>,
    pub lending_pool_withdrawal_requests: LookupMap<UniqueId, WithdrawalRequest>,
    pub lending_pool_in_flight_withdrawal_requests: LookupSet<UniqueId>,
    pub lending_pool_withdrawal_queue: LinkedList<UniqueId>,
    pub lower_collateral_accounts: LookupSet<AccountId>,
    pub oracle_id: AccountId,
    pub collateral_asset: NativeAsset,
    pub loan_asset: ContractAsset,
    pub deposit_fee: Fraction,
    pub collateral_deposit_fee_pool: CollateralAssetBalance,
    pub liquidated_collateral_pool: CollateralAssetBalance,
}

#[near_bindgen]
impl FungibleTokenReceiver for LendingProtocol {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let token_contract_id = env::predecessor_account_id();

        if token_contract_id == self.loan_asset.contract_id {
            let amount = LoanAssetBalance(amount.0);
            match &msg[..] {
                "close" => {
                    // Repay and close
                    let excess = self.repay(&sender_id, amount);

                    self.close();

                    PromiseOrValue::Value(U128(*excess))
                }
                "lend" => {
                    self.add_funds_to_lending_pool(sender_id, amount);

                    // Add all of the funds to the lending pool
                    PromiseOrValue::Value(U128(0))
                }
                _ => {
                    // Repay by default
                    let excess = self.repay(&sender_id, amount);

                    PromiseOrValue::Value(U128(*excess))
                }
            }
        } else {
            env::panic_str("Unknown token");
        }
    }
}

#[near_bindgen]
impl LendingProtocol {
    /* -----------------------------------------------------------------------------------
    -------------------------------- PRIVATE HELPERS -------------------------------------
    ----------------------------------------------------------------------------------- */

    fn new_id(&mut self) -> UniqueId {
        let id = self.unique_id;
        self.unique_id = id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Overflow"));
        id
    }

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

    fn get_loan(&self, account_id: &AccountId) -> &Loan {
        self.loans
            .get(account_id)
            .unwrap_or_else(|| env::panic_str("No collateral deposited"))
    }

    fn get_loan_status(&self, loan: &Loan) -> LoanStatus {
        let borrowed_valuation = self.loan_valuation(loan.borrowed);
        let collateral_valuation = self.collateral_valuation(loan.collateral);

        let is_undercollateralized = borrowed_valuation
            * loan.minimum_collateral_ratio.denominator.0
            >= collateral_valuation * loan.minimum_collateral_ratio.numerator.0;

        LoanStatus {
            borrowed_amount: loan.borrowed,
            borrowed_valuation,
            collateral_amount: loan.collateral,
            collateral_valuation,
            minimum_collateral_ratio: loan.minimum_collateral_ratio,
            is_undercollateralized,
        }
    }

    fn add_funds_to_lending_pool(&mut self, lender_id: AccountId, amount: LoanAssetBalance) {
        let lender_entry = self.lenders.entry(lender_id).or_default();
        lender_entry.amount_in_lending_pool += amount;
    }

    /* -----------------------------------------------------------------------------------
    ----------------------------- PUBLIC CHANGE METHODS ----------------------------------
    ----------------------------------------------------------------------------------- */

    #[init]
    #[private]
    pub fn new(config: LendingProtocolConfiguration) -> Self {
        require!(
            config.deposit_fee.numerator < config.deposit_fee.denominator,
            "Invalid fee"
        );

        let mut lower_collateral_accounts = LookupSet::new(StorageKey::LowerCollateralAccounts);

        for account in config.lower_collateral_accounts {
            lower_collateral_accounts.insert(account);
        }

        Self {
            unique_id: 0,
            loans: UnorderedMap::new(StorageKey::Loans),
            lenders: UnorderedMap::new(StorageKey::Lenders),
            lending_pool_withdrawal_requests: LookupMap::new(
                StorageKey::LendingPoolWithdrawalRequests,
            ),
            lending_pool_withdrawal_queue: LinkedList::new(StorageKey::LendingPoolWithdrawalQueue),
            lending_pool_in_flight_withdrawal_requests: LookupSet::new(
                StorageKey::LendingPoolInFlightWithdrawalRequests,
            ),
            lower_collateral_accounts,
            oracle_id: config.oracle_contract_id,
            collateral_asset: NativeAsset::new(config.collateral_oracle_asset_id),
            loan_asset: ContractAsset::new(
                config.loan_asset_contract_id,
                config.loan_oracle_asset_id,
            ),
            deposit_fee: config.deposit_fee,
            collateral_deposit_fee_pool: CollateralAssetBalance(0),
            liquidated_collateral_pool: CollateralAssetBalance(0),
        }
    }

    #[payable]
    pub fn request_withdrawal_from_lending_pool(&mut self, amount: U128) -> U64 {
        assert_one_yocto();

        let amount = LoanAssetBalance(amount.0);

        let lender_id = env::predecessor_account_id();

        let mut lender_entry = self
            .lenders
            .get(&lender_id)
            .unwrap_or_else(|| env::panic_str("No funds in lending pool"))
            .clone();

        require!(
            lender_entry.amount_in_lending_pool >= amount,
            "Cannot withdraw more funds than deposited"
        );

        let withdrawal_request_id = self.new_id();

        self.lending_pool_withdrawal_requests.insert(
            withdrawal_request_id,
            WithdrawalRequest::new(lender_id.clone(), amount),
        );
        self.lending_pool_withdrawal_queue
            .enqueue(withdrawal_request_id);
        lender_entry
            .pending_withdrawal_requests
            .push(withdrawal_request_id);
        self.lenders.insert(lender_id.clone(), lender_entry);

        U64(withdrawal_request_id)
    }

    #[payable]
    pub fn process_next_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        assert_one_yocto();

        let (request_id, request) = loop {
            let request_id = match self.lending_pool_withdrawal_queue.dequeue() {
                Some(id) => id,
                None => return PromiseOrValue::Value(()), // Queue is empty (there may still be some in-flight requests)
            };

            if let Some(request) = self.lending_pool_withdrawal_requests.get(&request_id) {
                break (request_id, request);
            } else {
                // Does not exist. Probably should never happen.
            }
        };

        self.lending_pool_in_flight_withdrawal_requests
            .insert(request_id);

        // Don't bother with checking if we have the balance to perform this
        // transfer. If we don't have sufficient balance of the loan asset, we
        // recover in `on_withdraw_funds_from_lending_pool`.
        ext_ft_core::ext(self.loan_asset.contract_id.clone())
            .with_attached_deposit(1)
            // TODO: Gas.
            .ft_transfer(request.account_id.clone(), U128(*request.amount), None)
            .then(
                Self::ext(env::current_account_id())
                    // TODO: Gas.
                    .on_withdraw_funds_from_lending_pool(U64(request_id)),
            )
            .into()
    }

    #[private]
    pub fn on_withdraw_funds_from_lending_pool(
        &mut self,
        request_id: U64,
        #[callback_result] result: Result<(), near_sdk::PromiseError>,
    ) {
        let request_id = request_id.0;

        self.lending_pool_in_flight_withdrawal_requests
            .remove(&request_id);

        if result.is_ok() {
            self.lending_pool_withdrawal_requests.remove(&request_id);
        } else {
            // TODO: Fix exploit:
            //  1. Lender A submits a withdrawal request for 100 tokens.
            //  2. The contract currently only has 80 tokens available, so Lender A cannot successfully call `process_next_withdrawal_request` and receive their funds.
            //  3. Lender B submits a withdrawal request for 80 tokens. It is queued behind Lender A's request.
            //  4. Lender B calls `process_next_withdrawal_request` twice.
            //  5. Lender A's withdrawal request fails, but Lender B's request succeeds, and empties the pool, even though Lender A was in line before Lender B.
            // One way to resolve this is to maintain a total of "locked" (in-flight withdrawal requests) tokens *and* include a balance check before processing every withdrawal, or to simply track the balance of the pool locally, in this contract, instead of waiting for `ft_balance_of` queries to resolve...
            self.lending_pool_withdrawal_queue.prepend(request_id);
        }
    }

    /// Deposit collateral function allows the user to deposit the collateral
    /// to the contract. Creates a [`Loan`] if the user doesn't have a loan.
    #[payable]
    pub fn deposit_collateral(&mut self) -> bool {
        let deposit = CollateralAssetBalance(env::attached_deposit());
        let fee = CollateralAssetBalance(
            deposit
                .mul(self.deposit_fee.numerator.0)
                .div_ceil(self.deposit_fee.denominator.0), // round fee up
        );

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
                minimum_collateral_ratio: (
                    if self
                        .lower_collateral_accounts
                        .contains(&env::predecessor_account_id())
                    {
                        LOWER_COLLATERAL_RATIO
                    } else {
                        MIN_COLLATERAL_RATIO
                    },
                    100,
                )
                    .into(),
            });

        loan.collateral += amount;

        // Track fees
        self.collateral_deposit_fee_pool += fee;

        true
    }

    /// Remove collateral function allows the user to withdraw the collateral from the contract.
    pub fn remove_collateral(&mut self, amount: U128) -> Promise {
        let remove_collateral_amount = CollateralAssetBalance(amount.0);

        require!(
            *remove_collateral_amount > 0,
            "Withdraw amount should be greater than 0"
        );

        let account_id = env::predecessor_account_id();
        let mut loan: Loan = *self.get_loan(&account_id);

        if loan.collateral < remove_collateral_amount {
            env::panic_str(&format!(
                "Attempted to withdraw more collateral than deposited. Currently deposited: {}",
                loan.collateral,
            ));
        }

        // Remove the amount from the collateral and see if the loan is undercollateralized.
        loan.collateral -= remove_collateral_amount;
        let new_loan_status = self.get_loan_status(&loan);

        if new_loan_status.is_undercollateralized {
            // The proposed loan is undercollateralized, so reject instead of saving changes.
            env::panic_str(&format!(
                "Collateral ratio must be greater than {}%",
                loan.minimum_collateral_ratio.to_percentage(),
            ));
        }

        self.loans.insert(account_id.clone(), loan);

        Promise::new(account_id).transfer(*remove_collateral_amount)
    }

    // Close function calculates the difference between loan and collateral and returns the difference to the user
    pub fn close(&mut self) -> PromiseOrValue<()> {
        let account_id = env::predecessor_account_id();
        let loan = self.get_loan(&account_id);
        let collateral = loan.collateral;
        let borrowed = loan.borrowed;

        require!(*borrowed == 0, "Loan must be fully repaid before closing");

        // remove loans
        self.loans.remove(&account_id);

        if *collateral > 0 {
            Promise::new(account_id).transfer(*collateral).into()
        } else {
            PromiseOrValue::Value(())
        }
    }

    pub fn liquidate(&mut self, account_id: Option<AccountId>) {
        let self_liquidate = account_id.is_none();
        let account_id = account_id.unwrap_or_else(env::predecessor_account_id);

        let mut loan = *self.get_loan(&account_id);

        require!(*loan.borrowed > 0, "No loan to liquidate");

        let status = self.get_loan_status(&loan);

        require!(
            status.is_undercollateralized || self_liquidate,
            "Loan must be undercollateralized to be automatically liquidated",
        );

        let liquidated_collateral = if status.collateral_valuation >= status.borrowed_valuation {
            // Happy path: we have enough collateral to liquidate the loan.

            // Invariant: This value is guaranteed to be <= loan.collateral
            let liquidate_collateral_amount = CollateralAssetBalance(
                loan.collateral
                    .mul(*status.borrowed_valuation)
                    .div_ceil(*status.collateral_valuation),
            );

            loan.collateral -= liquidate_collateral_amount;
            liquidate_collateral_amount
        } else {
            // Loan is <100% collateralized, so even liquidating all
            // collateral will not pay back the loan.
            log!("Loan is <100% collateralized!");

            // TODO: This loan was <100% collateralized when liquidated. Do we
            // need to do some additional tracking/other stuff here?
            let amount = loan.collateral;
            loan.collateral = CollateralAssetBalance(0);
            amount
        };

        self.loans.insert(account_id, loan);
        self.liquidated_collateral_pool += liquidated_collateral;
    }

    pub fn borrow(&mut self, amount: U128) -> Promise {
        /*
           1. Calculate the collateral value
           1a. Calculate current loan value
           2. Calculate max borrowable amount
           3. Check if the max borrowable amount is greater than the requested amount
           4. If yes, then borrow the requested amount
        */
        let loan_amount = LoanAssetBalance(amount.0);

        assert!(*loan_amount > 0, "Borrow Amount should be greater than 0");

        let account_id = env::predecessor_account_id();
        log!("predecessor_account_id: {}", account_id);

        // Get collateral price
        let price = self.collateral_asset.last_price.unwrap();

        // useless
        {
            let near_usdt_price: u128 = price.multiplier / 10000;
            log!("price: {}", price.multiplier);
            log!("near_usdt_price: {}", near_usdt_price);
        }

        let mut loan = *self.get_loan(&account_id);
        let loan_status = self.get_loan_status(&loan);

        // get the latest price NEAR in USDT of the collateral asset

        log!("raw collateral; {}", loan.collateral);
        // Calculate collateral and borrowed value

        log!("collateral_value: {}", loan_status.collateral_valuation);
        log!("borrowed_value: {}", loan_status.borrowed_valuation);
        log!("collateral_ratio: {:?}", loan.minimum_collateral_ratio);

        // get max borrowable amount
        let total_max_borrowable_value = loan_status.total_max_borrowable_valuation();

        let max_additional_borrowable_valuation =
            if total_max_borrowable_value > loan_status.borrowed_valuation {
                total_max_borrowable_value - loan_status.borrowed_valuation
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

        require!(
            max_additional_borrowable_valuation >= loan_amount_valuation,
            "Insufficient collateral"
        );

        // If max borrowable amount is greater than the requested amount, then borrow the requested amount
        loan.borrowed += loan_amount;
        self.loans.insert(account_id.clone(), loan);

        ext_ft_core::ext(self.loan_asset.contract_id.clone())
            .with_static_gas(Gas(5_000_000_000_000))
            .with_attached_deposit(1)
            // TODO: should this be `ft_transfer` or `ft_transfer_call`?
            .ft_transfer(
                // TODO: handle transfer failure (lock in-flight assets)
                account_id.clone(),
                loan_amount.0.into(),
                Some("Borrowed USDT".to_string()),
            )
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

        let mut loan = *self.get_loan(account_id);

        let excess = if amount > loan.borrowed {
            loan.borrowed.0 = 0;
            amount - loan.borrowed
        } else {
            loan.borrowed -= amount;
            0.into()
        };

        self.loans.insert(account_id.clone(), loan);

        excess
    }

    /* -----------------------------------------------------------------------------------
    ------------------------------------ GETTERS -----------------------------------------
    ----------------------------------------------------------------------------------- */

    pub fn get_total_max_borrowable_valuation_for_account(&self, account_id: &AccountId) -> U128 {
        U128(
            *self
                .get_loan_status_for_account(account_id)
                .total_max_borrowable_valuation(),
        )
    }

    pub fn get_loan_status_for_account(&self, account_id: &AccountId) -> LoanStatus {
        self.get_loan_status(self.get_loan(account_id))
    }

    pub fn get_all_loans(&self) -> std::collections::HashMap<&AccountId, &Loan> {
        self.loans.iter().collect()
    }

    pub fn get_collateral_deposit_fee_pool(&self) -> U128 {
        // TODO: Fee distribution.
        U128(*self.collateral_deposit_fee_pool)
    }

    pub fn get_liquidated_collateral_pool(&self) -> U128 {
        // TODO: Sell liquidated collateral or distribute to lenders?
        U128(*self.liquidated_collateral_pool)
    }

    pub fn get_withdrawal(&self, request_id: U64) -> Option<WithdrawalRequest> {
        self.lending_pool_withdrawal_requests
            .get(&request_id.0)
            .cloned()
    }

    pub fn get_next_withdrawal_id(&self) -> Option<U64> {
        self.lending_pool_withdrawal_queue.peek().map(U64)
    }

    pub fn get_withdrawal_queue_length(&self) -> u32 {
        self.lending_pool_withdrawal_queue.len()
    }

    pub fn get_withdrawal_queue_depth(&self, request_id: Option<U64>) -> LoanAssetBalance {
        let request_id = request_id.map(|id| id.0);
        self.lending_pool_withdrawal_queue
            .iter()
            .take_while(|id| Some(*id) != request_id)
            .map(|id| {
                self.lending_pool_withdrawal_requests
                    .get(&id)
                    .unwrap()
                    .amount
            })
            .sum()
    }

    pub fn get_position_in_withdrawal_queue(&self, request_id: U64) -> Option<u32> {
        self.lending_pool_withdrawal_queue
            .iter()
            .position(|id| id == request_id.0)
            .map(|pos| pos as u32)
    }

    pub fn get_withdrawal_queue_ids(&self, limit: Option<u32>) -> Vec<U64> {
        let iter = self.lending_pool_withdrawal_queue.iter().map(U64);

        if let Some(limit) = limit {
            iter.take(limit as usize).collect()
        } else {
            iter.collect()
        }
    }

    pub fn get_withdrawal_queue(&self, limit: Option<u32>) -> Vec<WithdrawalRequest> {
        let iter = self.lending_pool_withdrawal_queue.iter().map(|id| {
            self.lending_pool_withdrawal_requests
                .get(&id)
                .unwrap()
                .clone()
        });

        if let Some(limit) = limit {
            iter.take(limit as usize).collect()
        } else {
            iter.collect()
        }
    }

    /* -----------------------------------------------------------------------------------
    -------------------------------- ORACLE FUNCTIONS ------------------------------------
    ----------------------------------------------------------------------------------- */

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
    use std::collections::HashMap;

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
        LendingProtocol::new(LendingProtocolConfiguration {
            oracle_contract_id: "priceoracle.testnet".parse().unwrap(),
            collateral_oracle_asset_id: "wrap.testnet".to_string(),
            loan_asset_contract_id: "usdt.fakes.testnet".parse().unwrap(),
            loan_oracle_asset_id: "usdt.fakes.testnet".to_string(),
            deposit_fee: (1, 200).into(),
            lower_collateral_accounts,
        })
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
    #[should_panic = "Loan must be fully repaid before closing"]
    pub fn close_loan_fail() {
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
