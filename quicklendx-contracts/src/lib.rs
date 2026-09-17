#![no_std]

// Root module declarations for the QuickLendX contract crate.
// The project is split across many files under `src/`; the public API and the
// integration tests expect these modules to be compiled and re-exported here.
pub mod admin;
pub mod analytics;
pub mod arbiter;
pub mod audit;
pub mod backup;
pub mod backup_v1;
pub mod bid;
pub mod contract;
pub mod currency;
pub mod defaults;
pub mod diagnostics;
pub mod dispute;
pub mod dispute_timeline;
pub mod emergency;
pub mod errors;
pub mod escrow;
pub mod events;
pub mod fees;
pub mod freshness;
pub mod governance;
pub mod health;
pub mod idempotency;
pub mod incident;
pub mod init;
pub mod invariants;
pub mod investment;
pub mod investment_queries;
pub mod invoice;
pub mod invoice_amount;
pub mod invoice_search;
pub mod kyc_policy;
pub mod maintenance;
pub mod monitor;
pub mod multisig;
pub mod notifications;
pub mod observability;
pub mod operational_limits;
pub mod pagination;
pub mod panic_handler;
pub mod pause;
pub mod payment_token_policy;
pub mod payments;
pub mod profits;
pub mod protocol_limits;
pub mod reentrancy;
pub mod regulatory;
pub mod resolution_policy;
pub mod settlement;
pub mod storage;
pub mod types;
pub mod upgrade;
pub mod verification;
pub mod vesting;

// Re-export the canonical public contract type and the shared domain types so
// downstream tests and integrations can use `quicklendx_contracts::*` without
// depending on internal module paths.
pub use contract::QuickLendXContract;
pub use types::*;
