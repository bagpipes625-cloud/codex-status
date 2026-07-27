use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WEEK_MINUTES: u64 = 7 * 24 * 60;
pub const SESSION_MINUTES: u64 = 5 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_minutes: u64,
    pub resets_at: Option<i64>,
}

impl QuotaWindow {
    pub fn display_percent(&self) -> u8 {
        self.remaining_percent.round().clamp(0.0, 100.0) as u8
    }

    pub fn is_cache_valid(&self, now: i64, fetched_at: i64) -> bool {
        match self.resets_at {
            Some(reset) => reset > now,
            None => now.saturating_sub(fetched_at) < 15 * 60,
        }
    }

    fn usage_projection(&self, observed_at: i64, now: i64) -> Option<UsageProjection> {
        if !self.used_percent.is_finite() {
            return None;
        }
        let used_percent = self.used_percent.clamp(0.0, 100.0);
        if used_percent >= 100.0 {
            return Some(UsageProjection::Exhausted);
        }

        let reset = self.resets_at?;
        if reset <= observed_at || reset <= now {
            return None;
        }
        let window_seconds = i64::try_from(self.window_minutes.checked_mul(60)?).ok()?;
        let cycle_started_at = reset.checked_sub(window_seconds)?;
        let elapsed_at_observation = observed_at.checked_sub(cycle_started_at)?;
        if elapsed_at_observation <= 0 {
            return None;
        }

        if used_percent <= f64::EPSILON {
            return Some(UsageProjection::Ample);
        }

        let remaining_percent = 100.0 - used_percent;
        let seconds_after_observation =
            elapsed_at_observation as f64 * remaining_percent / used_percent;
        if !seconds_after_observation.is_finite() {
            return None;
        }
        let projected_depletion_at = observed_at as f64 + seconds_after_observation;
        if projected_depletion_at >= reset as f64 {
            return Some(UsageProjection::Ample);
        }

        let seconds = (projected_depletion_at - now as f64).ceil().max(0.0) as i64;
        Some(UsageProjection::DepletesIn { seconds })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageProjection {
    Ample,
    Exhausted,
    DepletesIn { seconds: i64 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub plan_type: Option<String>,
    pub reset_credits: Option<u64>,
    pub reset_credit_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub weekly: Option<QuotaWindow>,
    pub session: Option<QuotaWindow>,
    pub account: AccountSummary,
    pub fetched_at: i64,
}

impl QuotaSnapshot {
    pub fn is_cache_valid(&self, now: i64) -> bool {
        self.weekly.as_ref().is_some_and(|window| window.is_cache_valid(now, self.fetched_at))
    }

    pub fn weekly_usage_projection(&self, now: i64) -> Option<UsageProjection> {
        self.weekly.as_ref()?.usage_projection(self.fetched_at, now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshState {
    Loading,
    Live,
    Cached,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct DisplayState {
    pub snapshot: Option<QuotaSnapshot>,
    pub refresh_state: RefreshState,
    pub error: Option<String>,
}

impl DisplayState {
    pub fn loading(snapshot: Option<QuotaSnapshot>) -> Self {
        let refresh_state =
            if snapshot.is_some() { RefreshState::Cached } else { RefreshState::Loading };
        Self { snapshot, refresh_state, error: None }
    }

    pub fn weekly_percent(&self) -> Option<u8> {
        self.snapshot.as_ref()?.weekly.as_ref().map(QuotaWindow::display_percent)
    }

    pub fn live(snapshot: QuotaSnapshot) -> Self {
        Self { snapshot: Some(snapshot), refresh_state: RefreshState::Live, error: None }
    }

    pub fn after_error(snapshot: Option<QuotaSnapshot>, error: String, now: i64) -> Self {
        let snapshot = snapshot.filter(|value| value.is_cache_valid(now));
        Self {
            refresh_state: if snapshot.is_some() {
                RefreshState::Cached
            } else {
                RefreshState::Unavailable
            },
            snapshot,
            error: Some(error),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Codex did not return a rate-limit bucket")]
    MissingBucket,
    #[error("Codex returned malformed rate-limit data: {0}")]
    InvalidData(String),
}

pub fn parse_snapshot(
    account_result: &Value,
    rate_result: &Value,
    fetched_at: i64,
) -> Result<QuotaSnapshot, ParseError> {
    let bucket = select_codex_bucket(rate_result).ok_or(ParseError::MissingBucket)?;
    let mut windows = Vec::with_capacity(2);
    for field in ["primary", "secondary"] {
        if let Some(raw) = bucket.get(field).filter(|value| !value.is_null()) {
            if let Some(window) = parse_window(raw)? {
                windows.push(window);
            }
        }
    }

    let weekly_index =
        windows.iter().position(|window| window.window_minutes == WEEK_MINUTES).or_else(|| {
            windows
                .iter()
                .position(|window| ((6 * 24 * 60)..=(8 * 24 * 60)).contains(&window.window_minutes))
        });
    let weekly = weekly_index.map(|index| windows[index].clone());

    let session = windows
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != weekly_index)
        .find(|(_, window)| window.window_minutes == SESSION_MINUTES)
        .or_else(|| {
            windows
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != weekly_index)
                .find(|(_, window)| ((4 * 60)..=(6 * 60)).contains(&window.window_minutes))
        })
        .map(|(_, window)| window.clone());

    let plan_type = bucket
        .get("planType")
        .and_then(Value::as_str)
        .or_else(|| account_result.pointer("/account/planType").and_then(Value::as_str))
        .map(str::to_owned);
    let reset_credits =
        rate_result.pointer("/rateLimitResetCredits/availableCount").and_then(Value::as_u64);
    let reset_credit_expires_at = rate_result
        .pointer("/rateLimitResetCredits/credits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|credit| {
            credit
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("available"))
        })
        .filter_map(|credit| credit.get("expiresAt").and_then(Value::as_i64))
        .filter(|expires_at| *expires_at > 0)
        .min();

    Ok(QuotaSnapshot {
        weekly,
        session,
        account: AccountSummary { plan_type, reset_credits, reset_credit_expires_at },
        fetched_at,
    })
}

fn select_codex_bucket(rate_result: &Value) -> Option<&Value> {
    if let Some(map) = rate_result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        if let Some(bucket) = map.get("codex") {
            return Some(bucket);
        }
        if let Some(bucket) = map.values().find(|bucket| {
            bucket.get("limitId").and_then(Value::as_str).is_some_and(|id| id == "codex")
        }) {
            return Some(bucket);
        }
    }
    rate_result.get("rateLimits").filter(|value| value.is_object())
}

fn parse_window(value: &Value) -> Result<Option<QuotaWindow>, ParseError> {
    let Some(used_percent) = value.get("usedPercent").and_then(Value::as_f64) else {
        return Ok(None);
    };
    if !used_percent.is_finite() {
        return Err(ParseError::InvalidData("usedPercent is not finite".to_owned()));
    }
    let Some(window_minutes) = value.get("windowDurationMins").and_then(Value::as_u64) else {
        return Ok(None);
    };
    let remaining_percent = (100.0 - used_percent).clamp(0.0, 100.0);
    let resets_at = value.get("resetsAt").and_then(Value::as_i64).filter(|value| *value > 0);
    Ok(Some(QuotaWindow { used_percent, remaining_percent, window_minutes, resets_at }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Value {
        json!({"account": {"type": "chatgpt", "planType": "plus"}})
    }

    #[test]
    fn parses_weekly_only_bucket() {
        let rate = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 12.4, "windowDurationMins": 10080, "resetsAt": 2_000_000_000},
                "secondary": null
            }
        });
        let snapshot = parse_snapshot(&account(), &rate, 1_900_000_000).unwrap();
        assert_eq!(snapshot.weekly.unwrap().display_percent(), 88);
        assert!(snapshot.session.is_none());
        assert_eq!(snapshot.account.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn finds_weekly_when_primary_and_secondary_are_swapped() {
        let rate = json!({
            "rateLimitsByLimitId": {
                "codex_other": {"limitId": "codex_other", "primary": {"usedPercent": 99, "windowDurationMins": 10080}},
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 40, "windowDurationMins": 300},
                    "secondary": {"usedPercent": 25, "windowDurationMins": 10080}
                }
            },
            "rateLimitResetCredits": {"availableCount": 2}
        });
        let snapshot = parse_snapshot(&account(), &rate, 100).unwrap();
        assert_eq!(snapshot.weekly.unwrap().display_percent(), 75);
        assert_eq!(snapshot.session.unwrap().display_percent(), 60);
        assert_eq!(snapshot.account.reset_credits, Some(2));
        assert_eq!(snapshot.account.reset_credit_expires_at, None);
    }

    #[test]
    fn selects_the_earliest_available_reset_credit_expiration() {
        let rate = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 25, "windowDurationMins": 10080}
            },
            "rateLimitResetCredits": {
                "availableCount": 2,
                "credits": [
                    {"status": "used", "expiresAt": 100},
                    {"status": "available", "expiresAt": 300},
                    {"status": "AVAILABLE", "expiresAt": 200},
                    {"status": "available", "expiresAt": -1},
                    {"status": "expired", "expiresAt": 50}
                ]
            }
        });
        let snapshot = parse_snapshot(&account(), &rate, 100).unwrap();
        assert_eq!(snapshot.account.reset_credits, Some(2));
        assert_eq!(snapshot.account.reset_credit_expires_at, Some(200));
    }

    #[test]
    fn keeps_old_cached_account_summaries_compatible() {
        let summary: AccountSummary =
            serde_json::from_value(json!({"planType": "pro", "resetCredits": 2})).unwrap();
        assert_eq!(summary.reset_credits, Some(2));
        assert_eq!(summary.reset_credit_expires_at, None);
    }

    #[test]
    fn prefers_quota_plan_when_it_differs_from_the_account_token() {
        let rate = json!({
            "rateLimits": {
                "limitId": "codex",
                "planType": "prolite",
                "primary": {"usedPercent": 12, "windowDurationMins": 10080}
            }
        });
        let snapshot = parse_snapshot(&account(), &rate, 100).unwrap();
        assert_eq!(snapshot.account.plan_type.as_deref(), Some("prolite"));
    }

    #[test]
    fn does_not_mislabel_short_window_as_weekly() {
        let rate = json!({"rateLimits": {
            "primary": {"usedPercent": 5, "windowDurationMins": 60},
            "secondary": {"usedPercent": 10, "windowDurationMins": 300}
        }});
        let snapshot = parse_snapshot(&account(), &rate, 100).unwrap();
        assert!(snapshot.weekly.is_none());
        assert_eq!(snapshot.session.unwrap().display_percent(), 90);
    }

    #[test]
    fn clamps_out_of_range_usage() {
        let high =
            json!({"rateLimits": {"primary": {"usedPercent": 140, "windowDurationMins": 10080}}});
        let low =
            json!({"rateLimits": {"primary": {"usedPercent": -10, "windowDurationMins": 10080}}});
        assert_eq!(
            parse_snapshot(&account(), &high, 0).unwrap().weekly.unwrap().display_percent(),
            0
        );
        assert_eq!(
            parse_snapshot(&account(), &low, 0).unwrap().weekly.unwrap().display_percent(),
            100
        );
    }

    #[test]
    fn invalidates_cached_snapshot_after_reset() {
        let window = QuotaWindow {
            used_percent: 20.0,
            remaining_percent: 80.0,
            window_minutes: WEEK_MINUTES,
            resets_at: Some(500),
        };
        assert!(window.is_cache_valid(499, 100));
        assert!(!window.is_cache_valid(500, 100));
    }

    #[test]
    fn offline_state_keeps_only_unexpired_cache() {
        let snapshot = QuotaSnapshot {
            weekly: Some(QuotaWindow {
                used_percent: 20.0,
                remaining_percent: 80.0,
                window_minutes: WEEK_MINUTES,
                resets_at: Some(500),
            }),
            session: None,
            account: AccountSummary::default(),
            fetched_at: 100,
        };
        let cached = DisplayState::after_error(Some(snapshot.clone()), "offline".to_owned(), 499);
        assert_eq!(cached.refresh_state, RefreshState::Cached);
        let expired = DisplayState::after_error(Some(snapshot), "offline".to_owned(), 500);
        assert_eq!(expired.refresh_state, RefreshState::Unavailable);
        assert!(expired.snapshot.is_none());
    }

    #[test]
    fn tolerates_missing_window_fields_without_inventing_quota() {
        let rate = json!({"rateLimits": {
            "primary": {"windowDurationMins": 10080},
            "secondary": {"usedPercent": 10}
        }});
        let snapshot = parse_snapshot(&account(), &rate, 100).unwrap();
        assert!(snapshot.weekly.is_none());
        assert!(snapshot.session.is_none());
    }

    fn weekly_snapshot(
        used_percent: f64,
        fetched_at: i64,
        resets_at: Option<i64>,
    ) -> QuotaSnapshot {
        QuotaSnapshot {
            weekly: Some(QuotaWindow {
                used_percent,
                remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
                window_minutes: WEEK_MINUTES,
                resets_at,
            }),
            session: None,
            account: AccountSummary::default(),
            fetched_at,
        }
    }

    #[test]
    fn projects_ample_usage_when_average_rate_outlasts_reset() {
        let reset = WEEK_MINUTES as i64 * 60;
        let fetched_at = 24 * 60 * 60;
        let snapshot = weekly_snapshot(10.0, fetched_at, Some(reset));
        assert_eq!(snapshot.weekly_usage_projection(fetched_at), Some(UsageProjection::Ample));
    }

    #[test]
    fn projects_depletion_from_observed_cycle_rate() {
        let reset = WEEK_MINUTES as i64 * 60;
        let fetched_at = 24 * 60 * 60;
        let snapshot = weekly_snapshot(50.0, fetched_at, Some(reset));
        assert_eq!(
            snapshot.weekly_usage_projection(fetched_at),
            Some(UsageProjection::DepletesIn { seconds: 24 * 60 * 60 })
        );
        assert_eq!(
            snapshot.weekly_usage_projection(fetched_at + 60 * 60),
            Some(UsageProjection::DepletesIn { seconds: 23 * 60 * 60 })
        );
    }

    #[test]
    fn treats_zero_usage_as_ample_and_full_usage_as_exhausted() {
        let reset = WEEK_MINUTES as i64 * 60;
        let fetched_at = 24 * 60 * 60;
        assert_eq!(
            weekly_snapshot(0.0, fetched_at, Some(reset)).weekly_usage_projection(fetched_at),
            Some(UsageProjection::Ample)
        );
        assert_eq!(
            weekly_snapshot(100.0, fetched_at, Some(reset)).weekly_usage_projection(fetched_at),
            Some(UsageProjection::Exhausted)
        );
    }

    #[test]
    fn reports_exhausted_even_without_a_reset_timestamp() {
        assert_eq!(
            weekly_snapshot(100.0, 100, None).weekly_usage_projection(100),
            Some(UsageProjection::Exhausted)
        );
    }

    #[test]
    fn omits_projection_without_a_trustworthy_active_cycle() {
        let reset = WEEK_MINUTES as i64 * 60;
        assert_eq!(weekly_snapshot(10.0, 100, None).weekly_usage_projection(100), None);
        assert_eq!(weekly_snapshot(10.0, 0, Some(reset)).weekly_usage_projection(0), None);
        assert_eq!(weekly_snapshot(10.0, reset, Some(reset)).weekly_usage_projection(reset), None);
    }
}
