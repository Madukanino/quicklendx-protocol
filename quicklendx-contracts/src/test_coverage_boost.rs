#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

use crate::init::{InitializationParams, ProtocolConfigParams};
use crate::pause::PauseReason;
use crate::types::{BusinessFreezeReason, InvoiceCategory, InvoiceStatus};
use crate::verification::BusinessVerificationStatus;
use crate::{QuickLendXContract, QuickLendXContractClient};

fn setup_initialized() -> (Env, QuickLendXContractClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let contract_id = env.register(QuickLendXContract, ());
    let client = QuickLendXContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);

    let params = InitializationParams {
        admin: admin.clone(),
        treasury: treasury.clone(),
        fee_bps: 200,
        min_invoice_amount: 10,
        max_due_date_days: 365,
        grace_period_seconds: 86_400,
        initial_currencies: Vec::new(&env),
        corridors: Vec::new(&env),
        backfill_max_batch_size: 100,
    };

    client.initialize(&params);
    client.set_admin(&admin);

    (env, client, admin, treasury)
}

#[test]
fn test_protocol_initialization_and_version() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(QuickLendXContract, ());
    let client = QuickLendXContractClient::new(&env, &contract_id);

    assert!(!client.is_initialized());

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let params = InitializationParams {
        admin: admin.clone(),
        treasury: treasury.clone(),
        fee_bps: 200,
        min_invoice_amount: 10,
        max_due_date_days: 365,
        grace_period_seconds: 86_400,
        initial_currencies: Vec::new(&env),
        corridors: Vec::new(&env),
        backfill_max_batch_size: 100,
    };

    client.initialize(&params);
    assert!(client.is_initialized());
    assert_eq!(client.get_version(), 1);

    // Second initialization with different params must fail
    let mut diff_params = params.clone();
    diff_params.fee_bps = 500;
    let second_init = client.try_initialize(&diff_params);
    assert!(second_init.is_err());
}

#[test]
fn test_protocol_config_and_preview() {
    let (env, client, admin, treasury) = setup_initialized();
    let unauthorized = Address::generate(&env);

    assert_eq!(client.get_treasury(), Some(treasury.clone()));
    assert_eq!(client.get_fee_bps(), 200);
    assert_eq!(client.get_min_invoice_amount(), 10);
    assert_eq!(client.get_max_due_date_days(), 365);
    assert_eq!(client.get_grace_period_seconds(), 86_400);
    assert_eq!(client.get_corridors().len(), 0);

    // Preview protocol config
    let preview_params = ProtocolConfigParams {
        min_invoice_amount: 50,
        max_due_date_days: 180,
        grace_period_seconds: 7_200,
        fee_bps: 200,
        backfill_max_batch_size: 50,
    };
    let diff = client.preview_protocol_config(&admin, &preview_params);
    assert!(diff.would_succeed);

    // Update config
    client.set_protocol_config(&admin, &50, &180, &7_200, &50);
    assert_eq!(client.get_min_invoice_amount(), 50);
    assert_eq!(client.get_max_due_date_days(), 180);
    assert_eq!(client.get_grace_period_seconds(), 7_200);

    // Unauthorized config update rejected
    let bad_cfg = client.try_set_protocol_config(&unauthorized, &10, &10, &10, &10);
    assert!(bad_cfg.is_err());

    // Fee config
    client.set_fee_config(&admin, &350);
    assert_eq!(client.get_fee_bps(), 350);

    let bad_fee = client.try_set_fee_config(&unauthorized, &350);
    assert!(bad_fee.is_err());

    // Treasury update
    let new_treasury = Address::generate(&env);
    client.set_treasury(&admin, &new_treasury);
    assert_eq!(client.get_treasury(), Some(new_treasury));

    let bad_treasury = client.try_set_treasury(&unauthorized, &admin);
    assert!(bad_treasury.is_err());

    // Health and operational limits
    let health = client.get_protocol_health();
    assert!(health.initialized);
    assert_eq!(health.fee_bps, 350);

    let _status = client.get_health_status();
    let _limits = client.get_operational_limits();
    let _proto_limits = client.get_protocol_limits();

    // Minimum bid and invariant checks
    let min_bid = client.update_minimum_bid(&admin, &50);
    assert_eq!(min_bid, 50);

    let ttl_report = client.extend_protocol_ttl(&admin);
    assert_eq!(ttl_report.invoices_refreshed, 0);

    let invariant_report = client.invariant_self_check(&admin);
    assert!(invariant_report.all_passed);
}

#[test]
fn test_admin_two_step_transfer_flow() {
    let (env, client, admin, _) = setup_initialized();
    let new_admin = Address::generate(&env);
    let unauthorized = Address::generate(&env);

    client.set_two_step_enabled(&admin, &true);

    // Unauthorized cannot initiate
    let bad_initiate = client.try_initiate_admin_transfer(&unauthorized, &new_admin);
    assert!(bad_initiate.is_err());

    client.initiate_admin_transfer(&admin, &new_admin);
    assert_eq!(client.get_current_admin(), Some(admin.clone()));
}

#[test]
fn test_currency_whitelist_lifecycle() {
    let (env, client, admin, _) = setup_initialized();
    let unauthorized = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let token1 = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token2 = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token3 = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    assert_eq!(client.currency_count(), 0);
    assert!(!client.is_allowed_currency(&token1));

    // Add currency
    client.add_currency(&admin, &token1);
    assert!(client.is_allowed_currency(&token1));
    assert_eq!(client.currency_count(), 1);

    // Unauthorized add rejected
    let bad_add = client.try_add_currency(&unauthorized, &token2);
    assert!(bad_add.is_err());

    // Batch add
    let mut batch = Vec::new(&env);
    batch.push_back(token2.clone());
    batch.push_back(token3.clone());
    let added = client.add_currencies_batch(&admin, &batch);
    assert_eq!(added.len(), 2);
    assert_eq!(client.currency_count(), 3);

    // Queries
    let all = client.get_whitelisted_currencies();
    assert_eq!(all.len(), 3);

    let paged = client.get_whitelisted_currencies_paged(&0, &2);
    assert_eq!(paged.items.len(), 2);
    assert_eq!(paged.total_count, 3);
    assert!(paged.has_more);

    // Batch remove
    let mut remove_batch = Vec::new(&env);
    remove_batch.push_back(token2.clone());
    client.remove_currencies_batch(&admin, &remove_batch);
    assert_eq!(client.currency_count(), 2);
    assert!(!client.is_allowed_currency(&token2));

    // Single remove
    client.remove_currency(&admin, &token3);
    assert_eq!(client.currency_count(), 1);

    // Clear all
    client.clear_currencies(&admin);
    assert_eq!(client.currency_count(), 0);
}

#[test]
fn test_pause_and_maintenance_lifecycle() {
    let (env, client, admin, _) = setup_initialized();
    let unauthorized = Address::generate(&env);

    assert!(!client.is_paused());

    // Unauthorized pause rejected
    let bad_pause = client.try_pause(&unauthorized);
    assert!(bad_pause.is_err());

    client.pause(&admin);
    assert!(client.is_paused());

    client.unpause(&admin);
    assert!(!client.is_paused());

    // Maintenance mode
    assert!(!client.is_maintenance_mode());
    client.set_maintenance_mode(&admin, &true, &String::from_str(&env, "routine"));
    assert!(client.is_maintenance_mode());
    assert!(client.get_maintenance_reason().is_some());

    client.set_maintenance_mode(&admin, &false, &String::from_str(&env, "done"));
    assert!(!client.is_maintenance_mode());

    // Incident mode
    let incident = client.enter_incident_mode(&admin, &String::from_str(&env, "security alert"));
    assert!(incident.is_paused);
    assert!(incident.is_maintenance);
    assert!(client.is_paused());
    assert_eq!(client.pause_reason(), Some(PauseReason::Incident));

    let exit = client.exit_incident_mode(&admin);
    assert!(!exit.is_paused);
    assert!(!exit.is_maintenance);
    assert!(!client.is_paused());
}

#[test]
fn test_dispute_arbiter_registry_lifecycle() {
    let (env, client, admin, _) = setup_initialized();
    let arbiter = Address::generate(&env);
    let unauthorized = Address::generate(&env);

    assert!(!client.is_arbiter(&arbiter));

    // Unauthorized cannot register
    let bad_reg = client.try_register_arbiter(&unauthorized, &arbiter);
    assert!(bad_reg.is_err());

    client.register_arbiter(&admin, &arbiter);
    assert!(client.is_arbiter(&arbiter));

    let arbiters = client.list_arbiters();
    assert_eq!(arbiters.len(), 1);

    client.unregister_arbiter(&admin, &arbiter);
    assert!(!client.is_arbiter(&arbiter));
}

#[test]
fn test_business_and_investor_kyc_lifecycle() {
    let (env, client, admin, _) = setup_initialized();

    // Business KYC
    let business = Address::generate(&env);
    client.submit_kyc_application(&business, &String::from_str(&env, "biz details"));

    let status = client.get_business_verification_status(&business);
    assert!(status.is_some());
    assert_eq!(status.unwrap().status, BusinessVerificationStatus::Pending);

    client.verify_business(&admin, &business);
    let verified_status = client.get_business_verification_status(&business).unwrap();
    assert_eq!(verified_status.status, BusinessVerificationStatus::Verified);

    let verified_businesses = client.get_verified_businesses();
    assert!(verified_businesses.contains(business.clone()));

    // Reject another business
    let business_reject = Address::generate(&env);
    client.submit_kyc_application(&business_reject, &String::from_str(&env, "biz 2"));
    client.reject_business(
        &admin,
        &business_reject,
        &String::from_str(&env, "documents missing"),
    );
    let rejected = client
        .get_business_verification_status(&business_reject)
        .unwrap();
    assert_eq!(rejected.status, BusinessVerificationStatus::Rejected);

    // Investor KYC
    let investor = Address::generate(&env);
    client.submit_investor_kyc(&investor, &String::from_str(&env, "investor details"));
    client.verify_investor(&investor, &500_000);

    let inv_rec = client.get_investor_verification(&investor);
    assert!(inv_rec.is_some());
    assert_eq!(inv_rec.unwrap().investment_limit, 375_000);

    client.set_investment_limit(&investor, &1_000_000);
    assert_eq!(
        client
            .get_investor_verification(&investor)
            .unwrap()
            .investment_limit,
        750_000
    );

    client.recompute_investor_tier(&admin, &investor);

    // Recompute investor rating
    let rated = client.investor_rating_recompute(&admin, &investor);
    assert_eq!(rated.investment_limit, 750_000);

    // Revoke investor KYC
    client.revoke_investor_kyc(&investor, &String::from_str(&env, "revoked"));
    assert_eq!(
        client.get_investor_verification(&investor).unwrap().status,
        BusinessVerificationStatus::Rejected
    );

    // Reject investor
    let investor_reject = Address::generate(&env);
    client.submit_investor_kyc(&investor_reject, &String::from_str(&env, "inv 2"));
    client.reject_investor(&investor_reject, &String::from_str(&env, "failed aml"));
}

#[test]
fn test_bid_ttl_and_grace_configuration() {
    let (_env, client, _admin, _) = setup_initialized();

    // TTL
    assert_eq!(client.get_bid_ttl_days(), 7);
    client.set_bid_ttl_days(&14);
    assert_eq!(client.get_bid_ttl_days(), 14);
    assert_eq!(client.get_bid_ttl_config().current_days, 14);

    client.reset_bid_ttl_to_default();
    assert_eq!(client.get_bid_ttl_days(), 7);

    // Grace
    assert_eq!(client.get_bid_expiry_grace_seconds(), 0);
    client.set_bid_expiry_grace_seconds(&3_600);
    assert_eq!(client.get_bid_expiry_grace_seconds(), 3_600);
    assert_eq!(client.get_bid_expiry_grace_config().current_seconds, 3_600);

    // Reset grace
    client.set_bid_expiry_grace_seconds(&0);
    assert_eq!(client.get_bid_expiry_grace_seconds(), 0);

    // Max bids per investor
    assert_eq!(client.get_max_active_bids_per_investor(), 20);
    client.set_max_active_bids_per_investor(&50);
    assert_eq!(client.get_max_active_bids_per_investor(), 50);

    client.reset_investor_bid_limit();
    assert_eq!(client.get_max_active_bids_per_investor(), 20);
}

#[test]
fn test_invoice_queries_and_lifecycle_coverage() {
    let (env, client, admin, _) = setup_initialized();

    let token_admin = Address::generate(&env);
    let currency = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    client.add_currency(&admin, &currency);

    let business = Address::generate(&env);
    client.submit_kyc_application(&business, &String::from_str(&env, "KYC data"));
    client.verify_business(&admin, &business);

    let due_date = env.ledger().timestamp() + 500_000;
    let invoice_id = client.upload_invoice(
        &business,
        &100_000,
        &currency,
        &due_date,
        &String::from_str(&env, "Consulting Services"),
        &InvoiceCategory::Services,
        &Vec::new(&env),
        &None,
        &None,
        &None,
    );

    let invoice = client.get_invoice(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Pending);

    // Verify invoice
    client.verify_invoice(&invoice_id);
    let verified_inv = client.get_invoice(&invoice_id);
    assert_eq!(verified_inv.status, InvoiceStatus::Verified);

    // Counts & Breakdown
    assert_eq!(
        client.get_invoice_count_by_status(&InvoiceStatus::Verified),
        1
    );
    assert_eq!(client.get_total_invoice_count(), 1);

    let available = client.get_available_invoices();
    assert_eq!(available.len(), 1);

    let breakdown = client.get_category_breakdown();
    assert!(!breakdown.0.is_empty());

    // Position cap
    client.set_per_investor_position_cap(&business, &invoice_id, &Some(50_000));
    assert_eq!(
        client.get_per_investor_position_cap(&invoice_id),
        Some(50_000)
    );

    // Business invoices
    let biz_invoices = client.get_business_invoices(&business);
    assert_eq!(biz_invoices.len(), 1);

    let biz_invoices2 = client.get_invoice_by_business(&business);
    assert_eq!(biz_invoices2.len(), 1);

    assert_eq!(client.get_business_default_history(&business), 0);

    // Freeze & Unfreeze
    client.freeze_invoice(&admin, &invoice_id, &BusinessFreezeReason::AdminAction);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_some());

    client.unfreeze_invoice(&admin, &invoice_id);
    assert!(client.get_invoice_freeze_info(&invoice_id).is_none());

    // Emergency withdrawal helper probes
    assert!(!client.can_exec_emergency());
    assert_eq!(client.emg_time_until_unlock(), 0);
    assert_eq!(client.emg_time_until_expire(), 0);
    assert!(client.get_pending_emergency_withdraw().is_none());
}
