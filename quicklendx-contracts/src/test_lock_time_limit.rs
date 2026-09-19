//! Test for invoice lock time limit guard (issue #2103)
//!
//! This test verifies that actions on expired locks are rejected with
//! InvoiceLockExpired error, providing defense-in-depth against
//! indefinite invoice freezing.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

use crate::types::BusinessFreezeReason;
use crate::{QuickLendXContract, QuickLendXContractClient};

fn setup() -> (Env, QuickLendXContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(QuickLendXContract, ());
    let client = QuickLendXContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.set_admin(&admin);
    (env, client, admin)
}

fn create_verified_business(
    env: &Env,
    client: &QuickLendXContractClient,
    admin: &Address,
) -> Address {
    let business = Address::generate(env);
    client.submit_kyc_application(&business, &String::from_str(env, "KYC data"));
    client.verify_business(admin, &business);
    business
}

fn create_test_invoice(
    env: &Env,
    client: &QuickLendXContractClient,
    business: &Address,
    amount: i128,
) -> (BytesN<32>, Address) {
    let token_admin = Address::generate(env);
    let currency = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let due_date = env.ledger().timestamp() + 864_000;
    let invoice_id = client.upload_invoice(
        business,
        &amount,
        &currency,
        &due_date,
        &String::from_str(env, "test invoice"),
        &crate::invoice::InvoiceCategory::Services,
        &Vec::new(env),
        &None,
        &None,
        &None,
    );
    (invoice_id, currency)
}

#[test]
fn test_expired_lock_rejects_actions() {
    let (env, client, admin) = setup();
    let business = create_verified_business(&env, &client, &admin);
    let (invoice_id, _) = create_test_invoice(&env, &client, &business, 100_000);

    // Freeze the invoice
    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_some());

    // Advance time beyond the lock time limit (30 days + 1 second)
    let current_time = env.ledger().timestamp();
    let thirty_days_seconds = 2_592_000; // LOCK_TIME_LIMIT_SECONDS
    env.ledger()
        .set_timestamp(current_time + thirty_days_seconds + 1);

    // Attempt to place a bid - should fail with InvoiceLockExpired
    let investor = Address::generate(&env);
    let result = client.try_place_bid(
        &investor,
        &invoice_id,
        &10_000,
        &11_000,
        &BytesN::from_array(&env, &[0u8; 32]),
    );

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().unwrap(),
        crate::errors::QuickLendXError::InvoiceLockExpired
    );
}

#[test]
fn test_fresh_lock_allows_actions() {
    let (env, client, admin) = setup();
    let business = create_verified_business(&env, &client, &admin);
    let (invoice_id, _) = create_test_invoice(&env, &client, &business, 100_000);

    // Freeze the invoice
    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_some());

    // Keep time within the lock time limit (29 days)
    let current_time = env.ledger().timestamp();
    let twenty_nine_days_seconds = 2_505_600; // 29 days
    env.ledger()
        .set_timestamp(current_time + twenty_nine_days_seconds);

    // Attempt to place a bid - should fail with InvoiceFrozen (not expired)
    let investor = Address::generate(&env);
    let result = client.try_place_bid(
        &investor,
        &invoice_id,
        &10_000,
        &11_000,
        &BytesN::from_array(&env, &[0u8; 32]),
    );

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().unwrap(),
        crate::errors::QuickLendXError::InvoiceFrozen
    );
}
