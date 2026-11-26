//! Usage metering and quotas for hosted Botropolis.
//!
//! Usage is metered per tenant from the tenant's usage log, in
//! calendar-month periods (UTC). Quotas are per plan. The free plan
//! ships with defaults that the owner can change: 1,000 requests
//! and 1,000,000 tokens per month, overridable with
//! `BOTROPOLIS_FREE_REQUESTS_PER_MONTH` and
//! `BOTROPOLIS_FREE_TOKENS_PER_MONTH`. These are starting defaults,
//! not a pricing decision: billing is a later phase.
//!
//! Quotas gate new runs, never reads: exceeding a quota returns a
//! 402-style usage error on /ask and /team, while /analytics and
//! /teams/usage keep answering.

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// One plan's monthly limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quota {
    pub requests_per_month: u64,
    pub tokens_per_month: u64,
}

/// The free plan's defaults. Change them with the environment
/// variables above; the numbers here are starting defaults.
pub const FREE_REQUESTS_PER_MONTH: u64 = 1_000;
pub const FREE_TOKENS_PER_MONTH: u64 = 1_000_000;

/// The quota for a plan name. Only the free plan exists so far;
/// any other plan name falls back to the free quota.
pub fn quota_for_plan(_plan: &str) -> Quota {
    Quota {
        requests_per_month: env_u64(
            "BOTROPOLIS_FREE_REQUESTS_PER_MONTH",
            FREE_REQUESTS_PER_MONTH,
        ),
        tokens_per_month: env_u64("BOTROPOLIS_FREE_TOKENS_PER_MONTH", FREE_TOKENS_PER_MONTH),
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// A team's quota standing for the current period.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QuotaStatus {
    pub plan: String,
    pub period: String,
    pub requests_used: u64,
    pub requests_limit: u64,
    pub requests_remaining: u64,
    pub tokens_used: u64,
    pub tokens_limit: u64,
    pub tokens_remaining: u64,
    pub exceeded: bool,
}

/// Days from civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Civil date from days since the epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Start of the current UTC calendar month, as unix seconds.
pub fn period_start_secs(now: f64) -> f64 {
    let days = (now as i64).div_euclid(86400);
    let (year, month, _) = civil_from_days(days);
    (days_from_civil(year, month, 1) * 86400) as f64
}

/// Label for the current UTC calendar month, `YYYY-MM`.
pub fn current_period(now: f64) -> String {
    let days = (now as i64).div_euclid(86400);
    let (year, month, _) = civil_from_days(days);
    format!("{year:04}-{month:02}")
}

/// A tenant's quota standing for the current period, from its
/// usage log.
pub fn quota_status(tenant_id: &str, plan: &str) -> QuotaStatus {
    let quota = quota_for_plan(plan);
    let now = now_secs();
    let start = period_start_secs(now);
    let (requests_used, tokens_used, _) = crate::usage::period_totals(tenant_id, start);
    QuotaStatus {
        plan: plan.to_string(),
        period: current_period(now),
        requests_used,
        requests_limit: quota.requests_per_month,
        requests_remaining: quota.requests_per_month.saturating_sub(requests_used),
        tokens_used,
        tokens_limit: quota.tokens_per_month,
        tokens_remaining: quota.tokens_per_month.saturating_sub(tokens_used),
        exceeded: requests_used >= quota.requests_per_month
            || tokens_used >= quota.tokens_per_month,
    }
}

/// The quota error message when a team has run out for the period.
pub fn quota_error(status: &QuotaStatus) -> String {
    if status.requests_used >= status.requests_limit {
        format!(
            "monthly request quota reached ({} of {} requests used in {})",
            status.requests_used, status.requests_limit, status.period
        )
    } else {
        format!(
            "monthly token quota reached ({} of {} tokens used in {})",
            status.tokens_used, status.tokens_limit, status.period
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_boundaries_are_utc_midnights() {
        // 2026-11-25 22:00 UTC sits inside November 2026.
        assert_eq!(period_start_secs(1_795_644_000.0), 1_793_491_200.0);
        assert_eq!(current_period(1_795_644_000.0), "2026-11");
        // Exactly on a boundary, the new month starts.
        assert_eq!(period_start_secs(1_796_083_200.0), 1_796_083_200.0);
        assert_eq!(current_period(1_796_083_200.0), "2026-12");
        assert_eq!(current_period(1_798_761_600.0), "2027-01");
    }

    #[test]
    fn free_quota_defaults_and_env_override() {
        let _guard = crate::test_support::lock_env();
        std::env::remove_var("BOTROPOLIS_FREE_REQUESTS_PER_MONTH");
        std::env::remove_var("BOTROPOLIS_FREE_TOKENS_PER_MONTH");
        let quota = quota_for_plan("free");
        assert_eq!(quota.requests_per_month, FREE_REQUESTS_PER_MONTH);
        assert_eq!(quota.tokens_per_month, FREE_TOKENS_PER_MONTH);
        assert_eq!(quota_for_plan("something-new"), quota);
        std::env::set_var("BOTROPOLIS_FREE_REQUESTS_PER_MONTH", "5");
        assert_eq!(quota_for_plan("free").requests_per_month, 5);
        std::env::remove_var("BOTROPOLIS_FREE_REQUESTS_PER_MONTH");
    }
}
