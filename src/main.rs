use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{
    env, near_bindgen, AccountId, Balance, PanicOnDefault, Promise, PublicKey, StorageUsage,
};

near_sdk::setup_alloc!();

const USDT_CONTRACT_ID: &str = "usdt.near";
const PRICE_ORACLE_CONTRACT_ID: &str = "price_oracle.near";
const MIN_COLLATERAL_RATIO: u128 = 120;
const LOWER_COLLATERAL_RATIO: u128 = 105;

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct LendingProtocol {
    pub loans: HashMap<AccountId, Loan>,
    pub allowed_accounts: HashSet<AccountId>,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct Loan {
    pub collateral: Balance,
    pub borrowed: Balance,
    pub collateral_ratio: u128,
}

impl LendingProtocol {
    pub fn new(allowed_accounts: Vec<AccountId>) -> Self {
        Self {
            loans: HashMap::new(),
            allowed_accounts: allowed_accounts.into_iter().collect(),
        }
    }

    pub fn deposit_collateral(&mut self, amount: Balance) {
        let fee = amount / 200; // 0.5% fee
        amount -= fee;

        let account_id = env::signer_account_id();
        let loan = self.loans.entry(account_id.clone()).or_insert(Loan {
            collateral: 0,
            borrowed: 0,
            collateral_ratio: if self.allowed_accounts.contains(&account_id) {
                LOWER_COLLATERAL_RATIO
            } else {
                MIN_COLLATERAL_RATIO
            },
        });

        loan.collateral += amount;
        Promise::new(account_id).transfer(amount);
    }

    pub fn borrow(&mut self, usdt_amount: Balance) {
        let account_id = env::signer_account_id();
        let loan = self.loans.get_mut(&account_id).expect("No collateral deposited");

        let usdt_value = self.get_usdt_value(loan.collateral);
        let min_usdt_value = (usdt_amount * loan.collateral_ratio) / 100;

        assert!(usdt_value >= min_usdt_value, "Insufficient collateral");

        loan.borrowed += usdt_amount;
        Promise::new(account_id).function_call(
            b"ft_transfer".to_vec(),
            format!(
                r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Borrowed USDT"}}"#,
                account_id, usdt_amount
            )
            .as_bytes()
            .to_vec(),
            0,
            50_000_000_000_000,
        );
    }

    // The "repay" method calculates the actual repayment amount and the refund amount based on the outstanding loan. If there's an overpayment, it will refund the excess amount to the user.
    pub fn repay(&mut self, usdt_amount: Balance) {
        let fee = usdt_amount / 200; // 0.5% fee
        usdt_amount -= fee;

        let account_id = env::signer_account_id();
        let loan = self.loans.get_mut(&account_id).expect("No outstanding loan");

        let (repay_amount, refund_amount) = if loan.borrowed > usdt_amount {
            (usdt_amount, 0)
        } else {
            (loan.borrowed, usdt_amount - loan.borrowed)
        };

        loan.borrowed -= repay_amount;

        if loan.borrowed == 0 {
            loan.collateral = 0;
        }

        let mut promises= vec![Promise::new(USDT_CONTRACT_ID.to_string()).function_call(
            b"ft_transfer_from".to_vec(),
            format!(
                r#"{{"sender_id": "{}", "receiver_id": "{}", "amount": "{}", "memo": "Repayment"}}"#,
                account_id, LENDING_CONTRACT_ID, repay_amount
            )
            .as_bytes()
            .to_vec(),
            0,
            50_000_000_000_000,
        )];

        if refund_amount > 0 {
            let refund_promise = Promise::new(account_id.clone()).function_call(
                b"ft_transfer".to_vec(),
                format!(
                    r#"{{"receiver_id": "{}", "amount": "{}", "memo": "Refund overpayment"}}"#,
                    account_id, refund_amount
                )
                .as_bytes()
                .to_vec(),
                0,
                50_000_000_000_000,
            );

            promises.push(refund_promise);
        }

        Promise::all(promises).return_promise();
    }
}
