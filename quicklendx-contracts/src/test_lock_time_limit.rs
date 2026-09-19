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
    env.ledger().set_timestamp(1_000);
    let contract_id = env.register(QuickLendXContract, ());
    let client = QuickLendXContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize_admin(&admin);
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
    admin: &Address,
    business: &Address,
    amount: i128,
) -> (BytesN<32>, Address) {
    let token_admin = Address::generate(env);
    let currency = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    client.add_currency(admin, &currency);
    let due_date = env.ledger().timestamp() + 10_000_000;
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
    client.verify_invoice(&invoice_id);
    (invoice_id, currency)
}

#[test]
fn test_expired_lock_rejects_actions() {
    let (env, client, admin) = setup();
    let business = create_verified_business(&env, &client, &admin);
    let (invoice_id, _) = create_test_invoice(&env, &client, &admin, &business, 100_000);

    // Freeze the invoice at timestamp 1_000
    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_some());

    // Advance time beyond the lock time limit (30 days + 1 second)
    let thirty_days_seconds = 2_592_000; // LOCK_TIME_LIMIT_SECONDS
    env.ledger().set_timestamp(1_000 + thirty_days_seconds + 1);

    // Attempt to place a bid - should fail with InvoiceLockExpired
    let investor = Address::generate(&env);
    client.submit_investor_kyc(&investor, &String::from_str(&env, "KYC data"));
    client.verify_investor(&investor, &1_000_000);
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
    let (invoice_id, _) = create_test_invoice(&env, &client, &admin, &business, 100_000);

    // Freeze the invoice at timestamp 1_000
    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_some());

    // Setup verified investor
    let investor = Address::generate(&env);
    client.submit_investor_kyc(&investor, &String::from_str(&env, "KYC data"));
    client.verify_investor(&investor, &1_000_000);

    // Keep time within the lock time limit (29 days)
    let twenty_nine_days_seconds = 2_505_600; // 29 days
    env.ledger().set_timestamp(1_000 + twenty_nine_days_seconds);

    // Attempt to place a bid - should fail with InvoiceFrozen (not expired)
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

    // At the exact boundary (30 days = 2_592_000s), lock is still fresh (InvoiceFrozen, not expired)
    let thirty_days_seconds = 2_592_000;
    env.ledger().set_timestamp(1_000 + thirty_days_seconds);
    let result_boundary = client.try_place_bid(
        &investor,
        &invoice_id,
        &10_000,
        &11_000,
        &BytesN::from_array(&env, &[1u8; 32]),
    );

    assert!(result_boundary.is_err());
    assert_eq!(
        result_boundary.unwrap_err().unwrap(),
        crate::errors::QuickLendXError::InvoiceFrozen
    );
}

#[test]
fn test_require_lock_within_time_limit_boundary_direct() {
    let (env, client, admin) = setup();
    let business = create_verified_business(&env, &client, &admin);
    let (invoice_id, _) = create_test_invoice(&env, &client, &admin, &business, 100_000);

    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);

    // Initial time (fresh)
    env.as_contract(&client.address, || {
        assert!(
            crate::storage::InvoiceStorage::require_lock_within_time_limit(&env, &invoice_id)
                .is_ok()
        );
    });

    // Exact 30 days (fresh)
    env.ledger().set_timestamp(1_000 + 2_592_000);
    env.as_contract(&client.address, || {
        assert!(
            crate::storage::InvoiceStorage::require_lock_within_time_limit(&env, &invoice_id)
                .is_ok()
        );
    });

    // 30 days + 1s (expired)
    env.ledger().set_timestamp(1_000 + 2_592_000 + 1);
    env.as_contract(&client.address, || {
        let err = crate::storage::InvoiceStorage::require_lock_within_time_limit(&env, &invoice_id);
        assert_eq!(err, Err(crate::errors::QuickLendXError::InvoiceLockExpired));
    });
}
