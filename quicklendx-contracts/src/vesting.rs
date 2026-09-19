//! Token vesting module with time-locked release schedules.
//!
//! Supports admin-created vesting schedules that lock protocol tokens or rewards
//!
//! in the contract and release them linearly over time after an optional cliff.
//! Beneficiaries can claim vested tokens as they unlock.

use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol};

use crate::admin::AdminStorage;
use crate::errors::QuickLendXError;
use crate::payments::transfer_funds;

const VESTING_COUNTER_KEY: Symbol = symbol_short!("vest_cnt");
const VESTING_KEY: Symbol = symbol_short!("vest");

/// Events emitted by the vesting module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VestingEvent {
    NewSchedule {
        id: u64,
        beneficiary: Address,
        token: Address,
        amount: i128,
        cliff: u64,
        start: u64,
        end: u64,
    },
    Released {
        id: u64,
        beneficiary: Address,
        token: Address,
        amount: i128,
    },
}

/// Vesting schedule stored on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSchedule {
    pub id: u64,
    pub token: Address,
    pub beneficiary: Address,
    pub total_amount: i128,
    pub released_amount: i128,
    pub start_time: u64,
    pub cliff_time: u64,
    pub end_time: u64,
    pub created_at: u64,
    pub created_by: Address,
}

pub struct VestingStorage;

impl VestingStorage {
    fn next_id(env: &Env) -> u64 {
        let next: u64 = env
            .storage()
            .instance()
            .get(&VESTING_COUNTER_KEY)
            .unwrap_or(0);
        let new_next = next.saturating_add(1);
        env.storage()
            .instance()
            .set(&VESTING_COUNTER_KEY, &new_next);
        new_next
    }

    fn key(id: u64) -> (Symbol, u64) {
        (VESTING_KEY, id)
    }

    pub fn store(env: &Env, schedule: &VestingSchedule) {
        env.storage()
            .persistent()
            .set(&Self::key(schedule.id), schedule);
    }

    pub fn get(env: &Env, id: u64) -> Option<VestingSchedule> {
        env.storage().persistent().get(&Self::key(id))
    }

    pub fn update(env: &Env, schedule: &VestingSchedule) {
        env.storage()
            .persistent()
            .set(&Self::key(schedule.id), schedule);
    }
}

/// Aggregated vesting summary for a single beneficiary across all their schedules.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSummary {
    /// Number of schedules belonging to this user.
    pub grant_count: u32,
    /// Sum of `total_amount` across all schedules.
    pub total_granted: i128,
    /// Sum of `released_amount` across all schedules.
    pub total_released: i128,
    /// Sum of currently releasable amounts across all schedules.
    pub total_releasable: i128,
}

pub struct Vesting;

impl Vesting {
    /// Validate vesting schedule inputs and compute the derived cliff timestamp.
    ///
    /// # Security
    /// - Rejects zero-value schedules
    /// - Prevents backdated or non-monotonic timelines
    /// - Rejects cliff configurations that eliminate the post-cliff vesting window
    fn validate_schedule_inputs(
        env: &Env,
        total_amount: i128,
        start_time: u64,
        cliff_seconds: u64,
        end_time: u64,
    ) -> Result<u64, QuickLendXError> {
        if total_amount <= 0 {
            return Err(QuickLendXError::InvalidAmount);
        }

        let now = env.ledger().timestamp();
        if start_time < now {
            return Err(QuickLendXError::InvalidTimestamp);
        }
        if end_time <= start_time {
            return Err(QuickLendXError::InvalidTimestamp);
        }

        let cliff_time = start_time
            .checked_add(cliff_seconds)
            .ok_or(QuickLendXError::InvalidTimestamp)?;
        if cliff_time >= end_time {
            return Err(QuickLendXError::InvalidTimestamp);
        }

        Ok(cliff_time)
    }

    /// Validate schedule invariants before performing vesting arithmetic.
    fn validate_schedule_state(schedule: &VestingSchedule) -> Result<(), QuickLendXError> {
        if schedule.total_amount <= 0 {
            return Err(QuickLendXError::InvalidAmount);
        }
        if schedule.released_amount < 0 || schedule.released_amount > schedule.total_amount {
            return Err(QuickLendXError::InvalidAmount);
        }
        if schedule.start_time >= schedule.end_time {
            return Err(QuickLendXError::InvalidTimestamp);
        }
        if schedule.cliff_time < schedule.start_time || schedule.cliff_time >= schedule.end_time {
            return Err(QuickLendXError::InvalidTimestamp);
        }

        Ok(())
    }

    /// Create a new vesting schedule for a beneficiary.
    ///
    /// # Arguments
    /// * `admin` - Admin address that funds the vesting
    /// * `token` - Token address to lock
    /// * `beneficiary` - Address receiving vested tokens
    /// * `total_amount` - Total amount to vest (must be > 0)
    /// * `start_time` - Unix timestamp when vesting starts
    /// * `cliff_seconds` - Seconds after start before any release
    /// * `end_time` - Unix timestamp when all tokens are vested
    ///
    /// # Security
    /// - Requires admin authorization
    /// - Transfers tokens into contract custody immediately
    pub fn create_schedule(
        env: &Env,
        admin: &Address,
        token: Address,
        beneficiary: Address,
        total_amount: i128,
        start_time: u64,
        cliff_seconds: u64,
        end_time: u64,
    ) -> Result<u64, QuickLendXError> {
        admin.require_auth();
        AdminStorage::require_admin(env, admin)?;

        let cliff_time =
            Self::validate_schedule_inputs(env, total_amount, start_time, cliff_seconds, end_time)?;

        let id = VestingStorage::next_id(env);
        let now = env.ledger().timestamp();

        let schedule = VestingSchedule {
            id,
            token: token.clone(),
            beneficiary: beneficiary.clone(),
            total_amount,
            released_amount: 0,
            start_time,
            cliff_time,
            end_time,
            created_at: now,
            created_by: admin.clone(),
        };

        // Move tokens into contract custody.
        let contract = env.current_contract_address();
        transfer_funds(env, &token, admin, &contract, total_amount)?;

        VestingStorage::store(env, &schedule);
        env.events().publish(
            (symbol_short!("vesting"), symbol_short!("created")),
            (
                id,
                beneficiary.clone(),
                token.clone(),
                total_amount,
                start_time,
                cliff_time,
                end_time,
            ),
        );

        Ok(id)
    }

    /// Return the vesting schedule, if present.
    pub fn get_schedule(env: &Env, id: u64) -> Option<VestingSchedule> {
        VestingStorage::get(env, id)
    }

    /// Return an aggregated summary of all vesting schedules for `user`.
    ///
    /// Scans every schedule from id 1 up to the current counter value and
    /// collects those whose `beneficiary` matches `user`.  Returns a zeroed
    /// `VestingSummary` when no matching schedule exists.
    pub fn get_summary_for_user(env: &Env, user: &Address) -> VestingSummary {
        let max_id: u64 = env
            .storage()
            .instance()
            .get(&VESTING_COUNTER_KEY)
            .unwrap_or(0);

        let mut grant_count: u32 = 0;
        let mut total_granted: i128 = 0;
        let mut total_released: i128 = 0;
        let mut total_releasable: i128 = 0;

        for id in 1..=max_id {
            if let Some(schedule) = VestingStorage::get(env, id) {
                if &schedule.beneficiary == user {
                    grant_count = grant_count.saturating_add(1);
                    total_granted = total_granted.saturating_add(schedule.total_amount);
                    total_released = total_released.saturating_add(schedule.released_amount);
                    if let Ok(r) = Self::releasable_amount(env, &schedule) {
                        total_releasable = total_releasable.saturating_add(r);
                    }
                }
            }
        }

        VestingSummary {
            grant_count,
            total_granted,
            total_released,
            total_releasable,
        }
    }

    /// Calculate total vested amount for a schedule at current time.
    ///
    /// # Vesting curve
    ///
    /// The curve is **linear** from `start_time` to `end_time`, gated by a cliff:
    ///
    /// ```text
    /// vested(t) = 0                                          if t < cliff_time
    ///           = total_amount                               if t >= end_time
    ///           = total_amount * (t - start_time)           otherwise
    ///                          / (end_time - start_time)
    /// ```
    ///
    /// Integer division **truncates** (rounds toward zero), so the beneficiary
    /// may receive up to 1 token less than the real-valued curve until the next
    /// second boundary.  The final release at `end_time` always delivers the
    /// exact `total_amount`, eliminating any accumulated rounding dust.
    ///
    /// # Overflow safety
    ///
    /// - `elapsed` and `duration` are computed with `saturating_sub` on `u64`,
    ///   so they are always ≥ 0 and ≤ `u64::MAX`.
    /// - The numerator `total_amount * elapsed` uses `checked_mul` on `i128`;
    ///   overflow returns `InvalidAmount` rather than wrapping.
    /// - Because `elapsed < duration` in the linear branch, the quotient is
    ///   strictly less than `total_amount`, so the result fits in `i128`.
    pub fn vested_amount(env: &Env, schedule: &VestingSchedule) -> Result<i128, QuickLendXError> {
        Self::validate_schedule_state(schedule)?;

        let now = env.ledger().timestamp();
        if now < schedule.cliff_time {
            return Ok(0);
        }
        if now <= schedule.start_time {
            return Ok(0);
        }
        if now >= schedule.end_time {
            return Ok(schedule.total_amount);
        }

        // duration > 0 is guaranteed by validate_schedule_state (end > start).
        let duration = schedule.end_time.saturating_sub(schedule.start_time);
        if duration == 0 {
            return Err(QuickLendXError::InvalidTimestamp);
        }
        // elapsed < duration because now < end_time, so the quotient < total_amount.
        let elapsed = now.saturating_sub(schedule.start_time);
        let numerator = schedule
            .total_amount
            .checked_mul(elapsed as i128)
            .ok_or(QuickLendXError::InvalidAmount)?;
        Ok(numerator / duration as i128)
    }

    /// Compute how much can be released right now.
    ///
    /// `releasable = vested_amount(now) - released_amount`
    ///
    /// This is always ≥ 0 because `released_amount` is only ever incremented
    /// by the return value of a previous `releasable_amount` call, and
    /// `vested_amount` is monotonically non-decreasing.  `checked_sub` is used
    /// as a defence-in-depth guard; a negative result would indicate state
    /// corruption and returns `InvalidAmount`.
    pub fn releasable_amount(
        env: &Env,
        schedule: &VestingSchedule,
    ) -> Result<i128, QuickLendXError> {
        let vested = Self::vested_amount(env, schedule)?;
        // Defence-in-depth: checked_sub catches any state corruption where
        // released_amount somehow exceeds vested_amount.
        let releasable = vested
            .checked_sub(schedule.released_amount)
            .ok_or(QuickLendXError::InvalidAmount)?;
        Ok(releasable)
    }

    /// Release vested tokens to the beneficiary.
    ///
    /// # Security
    /// - Requires beneficiary authorization
    /// - Enforces timelock/cliff: returns `InvalidTimestamp` if called before cliff
    /// - Prevents over-release via `released_amount` tracking
    /// - Idempotent after full release: returns `Ok(0)` when nothing remains
    pub fn release(env: &Env, beneficiary: &Address, id: u64) -> Result<i128, QuickLendXError> {
        beneficiary.require_auth();

        let mut schedule =
            VestingStorage::get(env, id).ok_or(QuickLendXError::StorageKeyNotFound)?;

        if &schedule.beneficiary != beneficiary {
            return Err(QuickLendXError::Unauthorized);
        }

        // Enforce cliff: reject early calls with a typed error so callers can distinguish
        // "too early" from "already fully released".
        let now = env.ledger().timestamp();
        if now < schedule.cliff_time {
            return Err(QuickLendXError::InvalidTimestamp);
        }

        let releasable = Self::releasable_amount(env, &schedule)?;
        if releasable <= 0 {
            // Idempotent behavior: repeated calls return 0 instead of error
            return Ok(0);
        }
        let contract = env.current_contract_address();
        transfer_funds(env, &schedule.token, &contract, beneficiary, releasable)?;

        schedule.released_amount = schedule
            .released_amount
            .checked_add(releasable)
            .ok_or(QuickLendXError::InvalidAmount)?;
        Self::validate_schedule_state(&schedule)?;
        VestingStorage::update(env, &schedule);

        env.events().publish(
            (symbol_short!("vesting"), symbol_short!("released")),
            (id, beneficiary.clone(), schedule.token.clone(), releasable),
        );
        Ok(releasable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QuickLendXContract, QuickLendXContractClient};
    use soroban_sdk::testutils::{Address as _, Ledger as _};
    use soroban_sdk::{token, Address, Env};

    struct TestContext {
        env: Env,
        client: QuickLendXContractClient<'static>,
        admin: Address,
        beneficiary: Address,
        token_id: Address,
        token_client: token::Client<'static>,
    }

    fn setup_test() -> TestContext {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let contract_id = env.register(QuickLendXContract, ());
        let client = QuickLendXContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        client.initialize_admin(&admin);

        let token_admin = Address::generate(&env);
        let token_id = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let sac = token::StellarAssetClient::new(&env, &token_id);
        let token_client = token::Client::new(&env, &token_id);

        sac.mint(&admin, &100_000_000i128);
        let exp = env.ledger().sequence() + 10_000;
        token_client.approve(&admin, &contract_id, &100_000_000i128, &exp);

        TestContext {
            env,
            client,
            admin,
            beneficiary,
            token_id,
            token_client,
        }
    }

    #[test]
    fn test_schedule_input_validation() {
        let ctx = setup_test();

        // 1. Invalid amount <= 0
        let res = ctx.client.try_create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &0i128,
            &1_000u64,
            &100u64,
            &2_000u64,
        );
        assert_eq!(res, Err(Ok(QuickLendXError::InvalidAmount)));

        // 2. Start time in the past
        let res = ctx.client.try_create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &10_000i128,
            &999u64, // now is 1_000
            &100u64,
            &2_000u64,
        );
        assert_eq!(res, Err(Ok(QuickLendXError::InvalidTimestamp)));

        // 3. End time <= Start time
        let res = ctx.client.try_create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &10_000i128,
            &1_000u64,
            &100u64,
            &1_000u64,
        );
        assert_eq!(res, Err(Ok(QuickLendXError::InvalidTimestamp)));

        // 4. Cliff time >= End time
        let res = ctx.client.try_create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &10_000i128,
            &1_000u64,
            &1_000u64, // cliff = 2_000 == end
            &2_000u64,
        );
        assert_eq!(res, Err(Ok(QuickLendXError::InvalidTimestamp)));
    }

    #[test]
    fn test_schedule_state_validation_helper() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        let mut schedule = VestingSchedule {
            id: 1,
            token,
            beneficiary,
            total_amount: 1000,
            released_amount: 0,
            start_time: 1000,
            cliff_time: 1500,
            end_time: 2000,
            created_at: 1000,
            created_by: admin,
        };

        assert!(Vesting::validate_schedule_state(&schedule).is_ok());

        // total_amount <= 0
        schedule.total_amount = 0;
        assert_eq!(
            Vesting::validate_schedule_state(&schedule),
            Err(QuickLendXError::InvalidAmount)
        );
        schedule.total_amount = 1000;

        // released_amount > total_amount
        schedule.released_amount = 1001;
        assert_eq!(
            Vesting::validate_schedule_state(&schedule),
            Err(QuickLendXError::InvalidAmount)
        );
        schedule.released_amount = 0;

        // start_time >= end_time
        schedule.start_time = 2000;
        assert_eq!(
            Vesting::validate_schedule_state(&schedule),
            Err(QuickLendXError::InvalidTimestamp)
        );
        schedule.start_time = 1000;

        // cliff_time < start_time
        schedule.cliff_time = 999;
        assert_eq!(
            Vesting::validate_schedule_state(&schedule),
            Err(QuickLendXError::InvalidTimestamp)
        );

        // cliff_time >= end_time
        schedule.cliff_time = 2000;
        assert_eq!(
            Vesting::validate_schedule_state(&schedule),
            Err(QuickLendXError::InvalidTimestamp)
        );
    }

    #[test]
    fn test_vesting_cliff_and_linear_vesting_lifecycle() {
        let ctx = setup_test();

        let total = 1_000_000i128;
        let start = 1_000u64;
        let cliff_seconds = 500u64; // cliff = 1_500
        let end = 2_000u64;

        let id = ctx.client.create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &total,
            &start,
            &cliff_seconds,
            &end,
        );

        let sched = ctx.client.get_vesting_schedule(&id).unwrap();
        assert_eq!(sched.total_amount, total);
        assert_eq!(sched.released_amount, 0);
        assert_eq!(sched.cliff_time, 1_500);

        // 1. Before cliff (t = 1_200)
        ctx.env.ledger().set_timestamp(1_200);
        assert_eq!(ctx.client.get_vesting_vested(&id), Some(0));
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(0));

        let res = ctx.client.try_release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(res, Err(Ok(QuickLendXError::InvalidTimestamp)));

        // 2. Exactly at cliff (t = 1_500, 50% through duration)
        ctx.env.ledger().set_timestamp(1_500);
        assert_eq!(ctx.client.get_vesting_vested(&id), Some(500_000));
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(500_000));

        let released = ctx.client.release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(released, 500_000);
        assert_eq!(ctx.token_client.balance(&ctx.beneficiary), 500_000);
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(0));

        // 3. Idempotent release at same timestamp
        let re_release = ctx.client.release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(re_release, 0);

        // 4. Progress past cliff (t = 1_750, 75% through duration)
        ctx.env.ledger().set_timestamp(1_750);
        assert_eq!(ctx.client.get_vesting_vested(&id), Some(750_000));
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(250_000));

        let released_2 = ctx.client.release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(released_2, 250_000);
        assert_eq!(ctx.token_client.balance(&ctx.beneficiary), 750_000);

        // 5. At end time (t = 2_000, 100%)
        ctx.env.ledger().set_timestamp(2_000);
        assert_eq!(ctx.client.get_vesting_vested(&id), Some(1_000_000));
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(250_000));

        let released_final = ctx.client.release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(released_final, 250_000);
        assert_eq!(ctx.token_client.balance(&ctx.beneficiary), 1_000_000);
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(0));

        // 6. After end time (t = 5_000)
        ctx.env.ledger().set_timestamp(5_000);
        assert_eq!(ctx.client.get_vesting_vested(&id), Some(1_000_000));
        assert_eq!(ctx.client.get_vesting_releasable(&id), Some(0));
        let after_end = ctx.client.release_vested_tokens(&ctx.beneficiary, &id);
        assert_eq!(after_end, 0);
    }

    #[test]
    fn test_unauthorized_beneficiary_release() {
        let ctx = setup_test();
        let stranger = Address::generate(&ctx.env);

        let id = ctx.client.create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &100_000i128,
            &1_000u64,
            &100u64,
            &2_000u64,
        );

        ctx.env.ledger().set_timestamp(1_500);
        let res = ctx.client.try_release_vested_tokens(&stranger, &id);
        assert_eq!(res, Err(Ok(QuickLendXError::Unauthorized)));
    }

    #[test]
    fn test_nonexistent_vesting_schedule_returns_error_or_none() {
        let ctx = setup_test();

        assert_eq!(ctx.client.get_vesting_schedule(&999), None);
        assert_eq!(ctx.client.get_vesting_vested(&999), None);
        assert_eq!(ctx.client.get_vesting_releasable(&999), None);

        let res = ctx.client.try_release_vested_tokens(&ctx.beneficiary, &999);
        assert_eq!(res, Err(Ok(QuickLendXError::StorageKeyNotFound)));
    }

    #[test]
    fn test_vesting_summary_for_user() {
        let ctx = setup_test();
        let other_user = Address::generate(&ctx.env);

        // Schedule 1 for beneficiary: 1_000_000 total
        let id1 = ctx.client.create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &1_000_000i128,
            &1_000u64,
            &200u64,
            &2_000u64,
        );

        // Schedule 2 for beneficiary: 2_000_000 total
        let _id2 = ctx.client.create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &ctx.beneficiary,
            &2_000_000i128,
            &1_000u64,
            &400u64,
            &3_000u64,
        );

        // Schedule 3 for other_user: 500_000 total
        let _id3 = ctx.client.create_vesting_schedule(
            &ctx.admin,
            &ctx.token_id,
            &other_user,
            &500_000i128,
            &1_000u64,
            &100u64,
            &2_000u64,
        );

        // Check before cliff
        let summary_empty = ctx.client.get_vesting_summary(&ctx.beneficiary);
        assert_eq!(summary_empty.grant_count, 2);
        assert_eq!(summary_empty.total_granted, 3_000_000);
        assert_eq!(summary_empty.total_released, 0);
        assert_eq!(summary_empty.total_releasable, 0);

        // Advance to t = 1_500
        ctx.env.ledger().set_timestamp(1_500);
        let summary_mid = ctx.client.get_vesting_summary(&ctx.beneficiary);
        // sched 1 (duration 1000): 500 elapsed -> 500_000 releasable
        // sched 2 (duration 2000): 500 elapsed -> 500_000 releasable
        assert_eq!(summary_mid.grant_count, 2);
        assert_eq!(summary_mid.total_granted, 3_000_000);
        assert_eq!(summary_mid.total_releasable, 1_000_000);

        // Release on sched 1
        ctx.client.release_vested_tokens(&ctx.beneficiary, &id1);

        let summary_after_rel = ctx.client.get_vesting_summary(&ctx.beneficiary);
        assert_eq!(summary_after_rel.total_released, 500_000);
        assert_eq!(summary_after_rel.total_releasable, 500_000);

        // Check user with no schedules
        let stranger = Address::generate(&ctx.env);
        let summary_stranger = ctx.client.get_vesting_summary(&stranger);
        assert_eq!(summary_stranger.grant_count, 0);
        assert_eq!(summary_stranger.total_granted, 0);
    }
}
