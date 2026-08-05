use crate::model::{QuotaSnapshot, TokenUsageSnapshot};
use chrono::{Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SCHEMA_VERSION: u32 = 1;
const NATURAL_RESET_TOLERANCE_SECS: i64 = 5 * 60;
const CONTINUOUS_SAMPLE_GAP_SECS: i64 = 20 * 60;
const PERCENT_EPSILON: f64 = 0.000_001;
const UNIDENTIFIED_ACCOUNT: &str = "unidentified";
const MAX_ACCOUNTS: usize = 8;
const MAX_DAILY_RECORDS: usize = 730;
const MAX_COMPLETED_CYCLES: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageLedger {
    pub schema_version: u32,
    pub accounts: BTreeMap<String, UsageHistory>,
    pub last_account_key: Option<String>,
}

impl Default for UsageLedger {
    fn default() -> Self {
        Self { schema_version: SCHEMA_VERSION, accounts: BTreeMap::new(), last_account_key: None }
    }
}

impl UsageLedger {
    pub fn record_refresh(
        &mut self,
        account_key: Option<&str>,
        quota: &QuotaSnapshot,
        usage: Option<&TokenUsageSnapshot>,
    ) {
        let key = account_key.filter(|value| !value.is_empty()).unwrap_or(UNIDENTIFIED_ACCOUNT);
        self.accounts.entry(key.to_owned()).or_default().record_refresh(quota, usage);
        self.last_account_key = Some(key.to_owned());
        self.prune();
    }

    pub fn history_for(&self, account_key: Option<&str>) -> Option<&UsageHistory> {
        account_key
            .and_then(|key| self.accounts.get(key))
            .or_else(|| {
                account_key
                    .is_none()
                    .then(|| {
                        self.last_account_key.as_deref().and_then(|key| self.accounts.get(key))
                    })
                    .flatten()
            })
            .or_else(|| (self.accounts.len() == 1).then(|| self.accounts.values().next()).flatten())
    }

    pub fn prune(&mut self) {
        for history in self.accounts.values_mut() {
            history.prune();
        }
        while self.accounts.len() > MAX_ACCOUNTS {
            let protected = self.last_account_key.as_deref();
            let removal = self
                .accounts
                .iter()
                .filter(|(key, _)| Some(key.as_str()) != protected)
                .min_by_key(|(_, history)| history.last_observed_at())
                .map(|(key, _)| key.clone());
            let Some(removal) = removal else { break };
            self.accounts.remove(&removal);
        }
        if self.last_account_key.as_ref().is_some_and(|key| !self.accounts.contains_key(key)) {
            self.last_account_key = None;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageHistoryView {
    pub days: Vec<DailyUsageRecord>,
    pub cycles: Vec<WeeklyCycleView>,
    pub today: NaiveDate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeeklyCycleView {
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub scheduled_reset_at: Option<i64>,
    pub start_date: NaiveDate,
    pub observed_end_date: NaiveDate,
    pub display_end_date: NaiveDate,
    pub consumed_percent: f64,
    pub token_activity: Option<u64>,
    pub token_estimated: bool,
    pub quota_complete: bool,
    pub reset_kind: Option<ResetKind>,
    pub active: bool,
    pub stale: bool,
}

impl UsageHistoryView {
    pub fn day(&self, date: NaiveDate) -> Option<&DailyUsageRecord> {
        let key = date.format("%Y-%m-%d").to_string();
        self.days.iter().find(|day| day.date == key)
    }

    pub fn current_cycle_index(&self) -> Option<usize> {
        self.cycles
            .iter()
            .rposition(|cycle| cycle.active)
            .or_else(|| self.cycles.len().checked_sub(1))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResetKind {
    Natural,
    NonNatural,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageRecord {
    pub date: String,
    pub weekly_consumed_percent: f64,
    pub tokens: Option<u64>,
    pub quota_complete: bool,
    pub token_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompletedWeeklyCycle {
    pub started_at: i64,
    pub ended_at: i64,
    pub scheduled_reset_at: Option<i64>,
    pub consumed_percent: f64,
    pub token_activity: Option<u64>,
    pub token_estimated: bool,
    pub quota_complete: bool,
    pub reset_kind: ResetKind,
    #[serde(default)]
    pub started_local_date: Option<String>,
    #[serde(default)]
    pub ended_local_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ActiveWeeklyCycle {
    started_at: i64,
    scheduled_reset_at: Option<i64>,
    max_used_percent: f64,
    token_activity: Option<u64>,
    token_estimated: bool,
    #[serde(default)]
    quota_complete: bool,
    #[serde(default)]
    started_local_date: Option<String>,
    #[serde(default)]
    last_observed_local_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct WeeklyObservation {
    observed_at: i64,
    local_date: String,
    started_at: i64,
    scheduled_reset_at: Option<i64>,
    used_percent: f64,
    lifetime_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TokenObservation {
    observed_at: i64,
    local_date: String,
    lifetime_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageHistory {
    pub days: Vec<DailyUsageRecord>,
    pub completed_weekly_cycles: Vec<CompletedWeeklyCycle>,
    active_weekly_cycle: Option<ActiveWeeklyCycle>,
    last_weekly_observation: Option<WeeklyObservation>,
    last_token_observation: Option<TokenObservation>,
}

impl UsageHistory {
    pub fn view(&self, now: i64) -> UsageHistoryView {
        #[derive(Clone)]
        struct RawCycle {
            started_at: i64,
            ended_at: Option<i64>,
            scheduled_reset_at: Option<i64>,
            consumed_percent: f64,
            token_activity: Option<u64>,
            token_estimated: bool,
            quota_complete: bool,
            reset_kind: Option<ResetKind>,
            active: bool,
            started_local_date: Option<String>,
            ended_local_date: Option<String>,
        }

        let today = local_naive_date(now).unwrap_or_else(|| Local::now().date_naive());
        let mut raw: Vec<_> = self
            .completed_weekly_cycles
            .iter()
            .map(|cycle| RawCycle {
                started_at: cycle.started_at,
                ended_at: Some(cycle.ended_at),
                scheduled_reset_at: cycle.scheduled_reset_at,
                consumed_percent: cycle.consumed_percent,
                token_activity: cycle.token_activity,
                token_estimated: cycle.token_estimated,
                quota_complete: cycle.quota_complete,
                reset_kind: Some(cycle.reset_kind),
                active: false,
                started_local_date: cycle.started_local_date.clone(),
                ended_local_date: cycle.ended_local_date.clone(),
            })
            .collect();
        if let Some(active) = &self.active_weekly_cycle {
            raw.push(RawCycle {
                started_at: active.started_at,
                ended_at: None,
                scheduled_reset_at: active.scheduled_reset_at,
                consumed_percent: active.max_used_percent,
                token_activity: active.token_activity,
                token_estimated: active.token_estimated,
                quota_complete: active.quota_complete,
                reset_kind: None,
                active: true,
                started_local_date: active.started_local_date.clone(),
                ended_local_date: active.last_observed_local_date.clone(),
            });
        }
        raw.sort_by_key(|cycle| cycle.started_at);

        let cycles = raw
            .iter()
            .enumerate()
            .filter_map(|(index, cycle)| {
                let start_date = cycle
                    .started_local_date
                    .as_deref()
                    .and_then(parse_local_date)
                    .or_else(|| local_naive_date(cycle.started_at))?;
                let observed_end_date = (if cycle.active {
                    cycle
                        .ended_local_date
                        .as_deref()
                        .and_then(parse_local_date)
                        .or_else(|| {
                            self.last_weekly_observation
                                .as_ref()
                                .and_then(|observation| parse_local_date(&observation.local_date))
                        })
                        .unwrap_or(start_date)
                        .max(start_date)
                } else {
                    cycle
                        .ended_local_date
                        .as_deref()
                        .and_then(parse_local_date)
                        .or_else(|| local_naive_date(cycle.ended_at?))?
                        .max(start_date)
                })
                .min(start_date + Duration::days(6));
                let next_start = raw.get(index + 1).and_then(|next| {
                    next.started_local_date
                        .as_deref()
                        .and_then(parse_local_date)
                        .or_else(|| local_naive_date(next.started_at))
                });
                let display_end_date =
                    if cycle.active || cycle.reset_kind == Some(ResetKind::Natural) {
                        start_date + Duration::days(6)
                    } else if let Some(next_start) = next_start.filter(|date| *date > start_date) {
                        (next_start - Duration::days(1)).max(start_date)
                    } else {
                        observed_end_date
                    };
                Some(WeeklyCycleView {
                    started_at: cycle.started_at,
                    ended_at: cycle.ended_at,
                    scheduled_reset_at: cycle.scheduled_reset_at,
                    start_date,
                    observed_end_date,
                    display_end_date,
                    consumed_percent: cycle.consumed_percent,
                    token_activity: cycle.token_activity,
                    token_estimated: cycle.token_estimated,
                    quota_complete: cycle.quota_complete
                        && !(cycle.active
                            && cycle.scheduled_reset_at.is_some_and(|reset| now >= reset)),
                    reset_kind: cycle.reset_kind,
                    active: cycle.active,
                    stale: cycle.active
                        && cycle.scheduled_reset_at.is_some_and(|reset| now >= reset),
                })
            })
            .collect();
        UsageHistoryView { days: self.days.clone(), cycles, today }
    }

    fn prune(&mut self) {
        if self.days.len() > MAX_DAILY_RECORDS {
            self.days.drain(..self.days.len() - MAX_DAILY_RECORDS);
        }
        if self.completed_weekly_cycles.len() > MAX_COMPLETED_CYCLES {
            self.completed_weekly_cycles
                .drain(..self.completed_weekly_cycles.len() - MAX_COMPLETED_CYCLES);
        }
    }

    fn last_observed_at(&self) -> i64 {
        self.last_weekly_observation
            .as_ref()
            .map(|observation| observation.observed_at)
            .or_else(|| self.completed_weekly_cycles.last().map(|cycle| cycle.ended_at))
            .unwrap_or(i64::MIN)
    }

    pub fn record_refresh(&mut self, quota: &QuotaSnapshot, usage: Option<&TokenUsageSnapshot>) {
        let observed_at = quota.fetched_at;
        let local_date = local_date(observed_at);
        self.record_at(quota, usage, observed_at, local_date);
    }

    fn record_at(
        &mut self,
        quota: &QuotaSnapshot,
        usage: Option<&TokenUsageSnapshot>,
        observed_at: i64,
        local_date: String,
    ) {
        self.record_tokens(usage, observed_at, &local_date);
        let Some(weekly) = quota.weekly.as_ref() else {
            return;
        };

        let started_at = weekly
            .resets_at
            .map(|reset| reset.saturating_sub((weekly.window_minutes as i64).saturating_mul(60)))
            .unwrap_or(observed_at);
        let lifetime_tokens = usage.and_then(|value| value.lifetime_tokens);
        let observation = WeeklyObservation {
            observed_at,
            local_date: local_date.clone(),
            started_at,
            scheduled_reset_at: weekly.resets_at,
            used_percent: weekly.used_percent,
            lifetime_tokens,
        };

        if self.active_weekly_cycle.is_none() {
            let began_today = local_date_for_timestamp(started_at).as_deref() == Some(&local_date);
            if began_today {
                let day = self.day_mut(&local_date);
                day.weekly_consumed_percent += weekly.used_percent;
                day.quota_complete = true;
            } else {
                self.day_mut(&local_date).quota_complete = false;
            }
            self.active_weekly_cycle = Some(ActiveWeeklyCycle {
                started_at,
                scheduled_reset_at: weekly.resets_at,
                max_used_percent: weekly.used_percent,
                token_activity: lifetime_tokens.map(|_| 0),
                token_estimated: true,
                quota_complete: began_today,
                started_local_date: local_date_for_timestamp(started_at),
                last_observed_local_date: Some(local_date),
            });
            self.last_weekly_observation = Some(observation);
            return;
        }

        let previous = self.last_weekly_observation.clone();
        let reset_detected = self
            .active_weekly_cycle
            .as_ref()
            .is_some_and(|active| cycle_changed(active, &observation));
        if reset_detected {
            let (initial_tokens, boundary) = self.finish_cycle(&observation, previous.as_ref());
            self.start_cycle(observation, initial_tokens, boundary);
        } else {
            self.continue_cycle(&observation, previous.as_ref());
            self.last_weekly_observation = Some(observation);
        }
    }

    fn record_tokens(
        &mut self,
        usage: Option<&TokenUsageSnapshot>,
        observed_at: i64,
        local_date: &str,
    ) {
        let Some(usage) = usage else {
            return;
        };
        let mut has_current_official_bucket = false;
        for bucket in &usage.daily_buckets {
            let day = self.day_mut(&bucket.start_date);
            day.tokens = Some(bucket.tokens);
            day.token_complete = true;
            has_current_official_bucket |= bucket.start_date == local_date;
        }

        if let Some(lifetime_tokens) = usage.lifetime_tokens {
            if !has_current_official_bucket {
                match self.last_token_observation.as_ref() {
                    Some(previous) if lifetime_tokens >= previous.lifetime_tokens => {
                        let delta = lifetime_tokens - previous.lifetime_tokens;
                        let day = self.day_mut(local_date);
                        day.tokens = Some(day.tokens.unwrap_or(0).saturating_add(delta));
                        day.token_complete = false;
                    }
                    Some(_) => {
                        self.day_mut(local_date).token_complete = false;
                    }
                    None => {
                        self.day_mut(local_date).token_complete = false;
                    }
                }
            }
            self.last_token_observation = Some(TokenObservation {
                observed_at,
                local_date: local_date.to_owned(),
                lifetime_tokens,
            });
        }
    }

    fn continue_cycle(
        &mut self,
        observation: &WeeklyObservation,
        previous: Option<&WeeklyObservation>,
    ) {
        if let Some(previous) = previous {
            if observation.observed_at.saturating_sub(previous.observed_at)
                > CONTINUOUS_SAMPLE_GAP_SECS
            {
                self.active_weekly_cycle.as_mut().expect("active cycle").quota_complete = false;
            }
            if previous.local_date != observation.local_date {
                // A cross-midnight delta cannot be assigned exactly without a
                // sample at midnight, even when the surrounding samples are close.
                self.day_mut(&previous.local_date).quota_complete = false;
                self.day_mut(&observation.local_date).quota_complete = false;
            }
        }

        let active = self.active_weekly_cycle.as_mut().expect("active cycle");
        if let Some(reset) = observation.scheduled_reset_at {
            active.scheduled_reset_at = Some(reset);
            active.started_at = observation.started_at;
            if active.started_local_date.is_none() {
                active.started_local_date = local_date_for_timestamp(observation.started_at);
            }
        }
        active.last_observed_local_date = Some(observation.local_date.clone());
        if observation.used_percent > active.max_used_percent + PERCENT_EPSILON {
            let delta = observation.used_percent - active.max_used_percent;
            active.max_used_percent = observation.used_percent;
            self.day_mut(&observation.local_date).weekly_consumed_percent += delta;
        }
        if let (Some(previous), Some(current)) =
            (previous.and_then(|value| value.lifetime_tokens), observation.lifetime_tokens)
        {
            if current >= previous {
                let active = self.active_weekly_cycle.as_mut().expect("active cycle");
                active.token_activity =
                    Some(active.token_activity.unwrap_or(0).saturating_add(current - previous));
            } else {
                self.active_weekly_cycle.as_mut().expect("active cycle").token_estimated = true;
            }
        }
    }

    fn finish_cycle(
        &mut self,
        next: &WeeklyObservation,
        previous: Option<&WeeklyObservation>,
    ) -> (Option<u64>, i64) {
        let mut active = self.active_weekly_cycle.take().expect("active cycle");
        let boundary = if next.started_at > active.started_at + NATURAL_RESET_TOLERANCE_SECS {
            next.started_at.min(next.observed_at)
        } else {
            next.observed_at
        };
        let mut initial_tokens = None;
        if let Some((old_tokens, new_tokens)) = split_token_delta(previous, next, boundary) {
            active.token_activity = Some(active.token_activity.unwrap_or(0) + old_tokens);
            active.token_estimated = true;
            initial_tokens = Some(new_tokens);
        }
        let reset_kind = match active.scheduled_reset_at {
            Some(scheduled)
                if boundary >= scheduled.saturating_sub(NATURAL_RESET_TOLERANCE_SECS) =>
            {
                ResetKind::Natural
            }
            _ => ResetKind::NonNatural,
        };
        let quota_complete = active.quota_complete
            && previous.is_some_and(|value| {
                next.observed_at.saturating_sub(value.observed_at) <= CONTINUOUS_SAMPLE_GAP_SECS
            });
        if !quota_complete {
            if let Some(previous) = previous {
                self.day_mut(&previous.local_date).quota_complete = false;
            }
            self.day_mut(&next.local_date).quota_complete = false;
        } else if let Some(previous) = previous.filter(|value| value.local_date != next.local_date)
        {
            self.day_mut(&previous.local_date).quota_complete = false;
            self.day_mut(&next.local_date).quota_complete = false;
        }
        self.completed_weekly_cycles.push(CompletedWeeklyCycle {
            started_at: active.started_at,
            ended_at: boundary,
            scheduled_reset_at: active.scheduled_reset_at,
            consumed_percent: active.max_used_percent,
            token_activity: active.token_activity,
            token_estimated: active.token_estimated,
            quota_complete,
            reset_kind,
            started_local_date: active.started_local_date,
            ended_local_date: local_date_for_timestamp(boundary),
        });
        (initial_tokens, boundary)
    }

    fn start_cycle(
        &mut self,
        observation: WeeklyObservation,
        initial_tokens: Option<u64>,
        detected_boundary: i64,
    ) {
        let boundary_was_observed =
            observation.started_at + NATURAL_RESET_TOLERANCE_SECS < detected_boundary;
        let started_at =
            if boundary_was_observed { detected_boundary } else { observation.started_at };
        self.day_mut(&observation.local_date).weekly_consumed_percent += observation.used_percent;
        self.active_weekly_cycle = Some(ActiveWeeklyCycle {
            started_at,
            scheduled_reset_at: observation.scheduled_reset_at,
            max_used_percent: observation.used_percent,
            token_activity: initial_tokens.or_else(|| observation.lifetime_tokens.map(|_| 0)),
            token_estimated: true,
            quota_complete: !boundary_was_observed
                && local_date_for_timestamp(started_at).as_deref()
                    == Some(observation.local_date.as_str()),
            started_local_date: local_date_for_timestamp(started_at),
            last_observed_local_date: Some(observation.local_date.clone()),
        });
        self.last_weekly_observation = Some(observation);
    }

    fn day_mut(&mut self, date: &str) -> &mut DailyUsageRecord {
        if let Some(index) = self.days.iter().position(|day| day.date == date) {
            return &mut self.days[index];
        }
        let index = self
            .days
            .binary_search_by(|day| day.date.as_str().cmp(date))
            .unwrap_or_else(|index| index);
        self.days.insert(
            index,
            DailyUsageRecord {
                date: date.to_owned(),
                weekly_consumed_percent: 0.0,
                tokens: None,
                quota_complete: false,
                token_complete: false,
            },
        );
        &mut self.days[index]
    }
}

fn cycle_changed(active: &ActiveWeeklyCycle, observation: &WeeklyObservation) -> bool {
    let usage_dropped = observation.used_percent + PERCENT_EPSILON < active.max_used_percent;
    let window_moved = match (active.scheduled_reset_at, observation.scheduled_reset_at) {
        (Some(previous), Some(current)) => {
            current.abs_diff(previous) > NATURAL_RESET_TOLERANCE_SECS as u64
        }
        (None, _) | (_, None) => false,
    };
    usage_dropped || window_moved
}

fn split_token_delta(
    previous: Option<&WeeklyObservation>,
    current: &WeeklyObservation,
    boundary: i64,
) -> Option<(u64, u64)> {
    let previous = previous?;
    let before = previous.lifetime_tokens?;
    let after = current.lifetime_tokens?;
    if after < before {
        return None;
    }
    let delta = after - before;
    let elapsed = current.observed_at.saturating_sub(previous.observed_at);
    if elapsed <= 0 {
        return Some((0, delta));
    }
    let old_elapsed = boundary.saturating_sub(previous.observed_at).clamp(0, elapsed);
    let old = ((delta as u128 * old_elapsed as u128) / elapsed as u128) as u64;
    Some((old, delta - old))
}

fn local_date(timestamp: i64) -> String {
    local_date_for_timestamp(timestamp)
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string())
}

fn local_date_for_timestamp(timestamp: i64) -> Option<String> {
    Local.timestamp_opt(timestamp, 0).single().map(|value| value.format("%Y-%m-%d").to_string())
}

fn local_naive_date(timestamp: i64) -> Option<NaiveDate> {
    Local.timestamp_opt(timestamp, 0).single().map(|value| value.date_naive())
}

fn parse_local_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountSummary, DailyTokenUsage, QuotaWindow, WEEK_MINUTES};

    fn snapshot(used: f64, reset: i64, fetched: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            weekly: Some(QuotaWindow {
                used_percent: used,
                remaining_percent: 100.0 - used,
                window_minutes: WEEK_MINUTES,
                resets_at: Some(reset),
            }),
            session: None,
            account: AccountSummary::default(),
            fetched_at: fetched,
        }
    }

    fn usage(lifetime: u64, date: &str, daily: u64) -> TokenUsageSnapshot {
        TokenUsageSnapshot {
            lifetime_tokens: Some(lifetime),
            daily_buckets: vec![DailyTokenUsage { start_date: date.to_owned(), tokens: daily }],
        }
    }

    fn record(
        history: &mut UsageHistory,
        quota: &QuotaSnapshot,
        usage: Option<&TokenUsageSnapshot>,
        date: &str,
    ) {
        history.record_at(quota, usage, quota.fetched_at, date.to_owned());
    }

    #[test]
    fn ledger_keeps_accounts_isolated() {
        let quota = snapshot(10.0, 2_000_000, 1_000_000);
        let mut ledger = UsageLedger::default();
        ledger.record_refresh(Some("account-a"), &quota, None);
        ledger.record_refresh(Some("account-b"), &quota, None);

        assert_eq!(ledger.accounts.len(), 2);
        assert!(ledger.accounts.contains_key("account-a"));
        assert!(ledger.accounts.contains_key("account-b"));
    }

    #[test]
    fn accumulates_weekly_percentage_and_uses_official_daily_tokens() {
        let mut history = UsageHistory::default();
        let reset = 1_000_000;
        let first = snapshot(20.0, reset, 900_000);
        let second = snapshot(27.0, reset, 900_300);
        record(&mut history, &first, Some(&usage(1_000, "2026-08-05", 400)), "2026-08-05");
        record(&mut history, &second, Some(&usage(1_300, "2026-08-05", 700)), "2026-08-05");

        let day = &history.days[0];
        assert_eq!(day.weekly_consumed_percent, 7.0);
        assert_eq!(day.tokens, Some(700));
        assert!(day.token_complete);
        assert!(!day.quota_complete);
    }

    #[test]
    fn daily_consumption_can_exceed_one_hundred_after_early_reset() {
        let mut history = UsageHistory::default();
        let old_reset = 2_000_000;
        let old_start = old_reset - WEEK_MINUTES as i64 * 60;
        let first = snapshot(0.0, old_reset, old_start + 60);
        let before_reset = snapshot(80.0, old_reset, old_start + 86_000);
        let new_reset = old_start + 86_400 + WEEK_MINUTES as i64 * 60;
        let second = snapshot(35.0, new_reset, old_start + 86_700);
        record(&mut history, &first, None, "2026-08-05");
        record(&mut history, &before_reset, None, "2026-08-05");
        record(&mut history, &second, None, "2026-08-05");

        assert_eq!(history.days[0].weekly_consumed_percent, 115.0);
        assert_eq!(history.completed_weekly_cycles[0].reset_kind, ResetKind::NonNatural);
    }

    #[test]
    fn classifies_reset_at_the_scheduled_boundary_as_natural() {
        let mut history = UsageHistory::default();
        let old_reset = 2_000_000;
        let first = snapshot(55.0, old_reset, old_reset - 60);
        let second = snapshot(4.0, old_reset + WEEK_MINUTES as i64 * 60, old_reset + 60);
        record(&mut history, &first, None, "2026-08-05");
        record(&mut history, &second, None, "2026-08-05");

        assert_eq!(history.completed_weekly_cycles[0].reset_kind, ResetKind::Natural);
        assert_eq!(history.completed_weekly_cycles[0].consumed_percent, 55.0);
        assert!(!history.completed_weekly_cycles[0].quota_complete);
    }

    #[test]
    fn a_drop_with_an_unchanged_deadline_is_an_observed_early_reset() {
        let mut history = UsageHistory::default();
        let reset = 2_000_000;
        let first = snapshot(60.0, reset, 1_500_000);
        let second = snapshot(5.0, reset, 1_500_300);
        record(&mut history, &first, None, "2026-08-05");
        record(&mut history, &second, None, "2026-08-05");

        let completed = &history.completed_weekly_cycles[0];
        assert_eq!(completed.ended_at, second.fetched_at);
        assert_eq!(completed.reset_kind, ResetKind::NonNatural);
        assert_eq!(history.active_weekly_cycle.as_ref().unwrap().started_at, second.fetched_at);
        assert!(!history.active_weekly_cycle.as_ref().unwrap().quota_complete);
    }

    #[test]
    fn a_small_server_deadline_correction_does_not_create_a_false_cycle() {
        let mut history = UsageHistory::default();
        let first = snapshot(20.0, 2_000_000, 1_500_000);
        let second = snapshot(21.0, 2_000_120, 1_500_300);
        record(&mut history, &first, None, "2026-08-05");
        record(&mut history, &second, None, "2026-08-05");

        assert!(history.completed_weekly_cycles.is_empty());
        assert_eq!(history.active_weekly_cycle.as_ref().unwrap().max_used_percent, 21.0);
        assert_eq!(
            history.active_weekly_cycle.as_ref().unwrap().scheduled_reset_at,
            Some(2_000_120)
        );
    }

    #[test]
    fn ignores_five_hour_only_snapshots_for_quota_history() {
        let mut value = snapshot(10.0, 2_000_000, 1_000_000);
        value.session = value.weekly.take();
        let mut history = UsageHistory::default();
        record(&mut history, &value, None, "2026-08-05");

        assert!(history.days.is_empty());
        assert!(history.completed_weekly_cycles.is_empty());
    }

    #[test]
    fn falls_back_to_lifetime_token_deltas_without_daily_buckets() {
        let mut history = UsageHistory::default();
        let first_usage =
            TokenUsageSnapshot { lifetime_tokens: Some(1_000), daily_buckets: Vec::new() };
        let second_usage =
            TokenUsageSnapshot { lifetime_tokens: Some(1_250), daily_buckets: Vec::new() };
        let quota = snapshot(20.0, 2_000_000, 1_000_000);
        record(&mut history, &quota, Some(&first_usage), "2026-08-05");
        record(&mut history, &quota, Some(&second_usage), "2026-08-05");

        assert_eq!(history.days[0].tokens, Some(250));
        assert!(!history.days[0].token_complete);
    }

    #[test]
    fn imports_every_official_daily_token_bucket() {
        let mut history = UsageHistory::default();
        let usage = TokenUsageSnapshot {
            lifetime_tokens: Some(3_000),
            daily_buckets: vec![
                DailyTokenUsage { start_date: "2026-08-04".to_owned(), tokens: 1_000 },
                DailyTokenUsage { start_date: "2026-08-05".to_owned(), tokens: 2_000 },
                DailyTokenUsage { start_date: "2026-08-06".to_owned(), tokens: 0 },
            ],
        };
        let quota = snapshot(20.0, 2_000_000, 1_000_000);
        record(&mut history, &quota, Some(&usage), "2026-08-05");

        assert_eq!(history.days.len(), 3);
        assert_eq!(history.days[0].tokens, Some(1_000));
        assert_eq!(history.days[1].tokens, Some(2_000));
        assert_eq!(history.days[2].tokens, Some(0));
        assert!(history.days.iter().all(|day| day.token_complete));
        assert!(history.days.iter().all(|day| !day.quota_complete));
    }

    #[test]
    fn cross_midnight_sampling_keeps_daily_attribution_estimated() {
        let mut history = UsageHistory::default();
        let reset = 2_000_000;
        let before_midnight = snapshot(20.0, reset, 1_000_000);
        let after_midnight = snapshot(22.0, reset, 1_000_300);
        record(&mut history, &before_midnight, None, "2026-08-05");
        record(&mut history, &after_midnight, None, "2026-08-06");

        let current = history.days.iter().find(|day| day.date == "2026-08-06").unwrap();
        assert_eq!(current.weekly_consumed_percent, 2.0);
        assert!(!current.quota_complete);
        assert!(!history.days.iter().find(|day| day.date == "2026-08-05").unwrap().quota_complete);
    }

    #[test]
    fn cross_midnight_observed_reset_keeps_both_days_estimated() {
        let start = local_timestamp(2026, 8, 1, 8);
        let reset = local_timestamp(2026, 8, 8, 8);
        let before = local_timestamp(2026, 8, 1, 23) + 55 * 60;
        let after = before + 10 * 60;
        let mut history = UsageHistory {
            days: vec![DailyUsageRecord {
                date: "2026-08-01".to_owned(),
                weekly_consumed_percent: 60.0,
                tokens: None,
                quota_complete: true,
                token_complete: false,
            }],
            active_weekly_cycle: Some(ActiveWeeklyCycle {
                started_at: start,
                scheduled_reset_at: Some(reset),
                max_used_percent: 60.0,
                token_activity: None,
                token_estimated: true,
                quota_complete: true,
                started_local_date: Some("2026-08-01".to_owned()),
                last_observed_local_date: Some("2026-08-01".to_owned()),
            }),
            last_weekly_observation: Some(WeeklyObservation {
                observed_at: before,
                local_date: "2026-08-01".to_owned(),
                started_at: start,
                scheduled_reset_at: Some(reset),
                used_percent: 60.0,
                lifetime_tokens: None,
            }),
            ..UsageHistory::default()
        };
        let next = snapshot(5.0, reset, after);
        record(&mut history, &next, None, "2026-08-02");

        assert!(history.days.iter().all(|day| !day.quota_complete));
    }

    fn local_timestamp(year: i32, month: u32, day: u32, hour: u32) -> i64 {
        Local.with_ymd_and_hms(year, month, day, hour, 0, 0).single().unwrap().timestamp()
    }

    #[test]
    fn natural_cycle_view_always_uses_the_seven_day_window() {
        let start = local_timestamp(2026, 7, 1, 8);
        let delayed_observation = local_timestamp(2026, 7, 10, 8);
        let next_start = local_timestamp(2026, 7, 11, 8);
        let history = UsageHistory {
            completed_weekly_cycles: vec![
                CompletedWeeklyCycle {
                    started_at: start,
                    ended_at: delayed_observation,
                    scheduled_reset_at: Some(start + WEEK_MINUTES as i64 * 60),
                    consumed_percent: 55.0,
                    token_activity: Some(3_100_000_000),
                    token_estimated: false,
                    quota_complete: true,
                    reset_kind: ResetKind::Natural,
                    started_local_date: Some("2026-07-01".to_owned()),
                    ended_local_date: Some("2026-07-10".to_owned()),
                },
                CompletedWeeklyCycle {
                    started_at: next_start,
                    ended_at: local_timestamp(2026, 7, 14, 8),
                    scheduled_reset_at: None,
                    consumed_percent: 5.0,
                    token_activity: Some(500_000_000),
                    token_estimated: true,
                    quota_complete: false,
                    reset_kind: ResetKind::NonNatural,
                    started_local_date: Some("2026-07-11".to_owned()),
                    ended_local_date: Some("2026-07-14".to_owned()),
                },
            ],
            ..UsageHistory::default()
        };

        let view = history.view(local_timestamp(2026, 8, 5, 12));
        assert_eq!(view.cycles[0].start_date, NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
        assert_eq!(view.cycles[0].display_end_date, NaiveDate::from_ymd_opt(2026, 7, 7).unwrap());
        assert_eq!(view.cycles[1].display_end_date, NaiveDate::from_ymd_opt(2026, 7, 14).unwrap());
    }

    #[test]
    fn active_cycle_view_exposes_today_but_reserves_seven_display_dates() {
        let start = local_timestamp(2026, 8, 1, 8);
        let history = UsageHistory {
            active_weekly_cycle: Some(ActiveWeeklyCycle {
                started_at: start,
                scheduled_reset_at: Some(start + WEEK_MINUTES as i64 * 60),
                max_used_percent: 28.0,
                token_activity: Some(900_000_000),
                token_estimated: false,
                quota_complete: true,
                started_local_date: Some("2026-08-01".to_owned()),
                last_observed_local_date: Some("2026-08-05".to_owned()),
            }),
            ..UsageHistory::default()
        };

        let view = history.view(local_timestamp(2026, 8, 5, 12));
        let cycle = &view.cycles[0];
        assert!(cycle.active);
        assert_eq!(cycle.observed_end_date, NaiveDate::from_ymd_opt(2026, 8, 5).unwrap());
        assert_eq!(cycle.display_end_date, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
        assert_eq!(view.current_cycle_index(), Some(0));
    }

    #[test]
    fn stale_active_cycle_does_not_expand_through_offline_days() {
        let start = local_timestamp(2026, 7, 1, 8);
        let history = UsageHistory {
            active_weekly_cycle: Some(ActiveWeeklyCycle {
                started_at: start,
                scheduled_reset_at: Some(start + WEEK_MINUTES as i64 * 60),
                max_used_percent: 40.0,
                token_activity: None,
                token_estimated: true,
                quota_complete: false,
                started_local_date: Some("2026-07-01".to_owned()),
                last_observed_local_date: Some("2026-07-02".to_owned()),
            }),
            ..UsageHistory::default()
        };

        let view = history.view(local_timestamp(2026, 8, 5, 12));
        assert!(view.cycles[0].stale);
        assert!(!view.cycles[0].quota_complete);
        assert_eq!(view.cycles[0].observed_end_date, NaiveDate::from_ymd_opt(2026, 7, 2).unwrap());
        assert_eq!(view.cycles[0].display_end_date, NaiveDate::from_ymd_opt(2026, 7, 7).unwrap());
    }

    #[test]
    fn a_sampling_gap_permanently_marks_the_cycle_incomplete() {
        let start = local_timestamp(2026, 8, 1, 8);
        let reset = start + WEEK_MINUTES as i64 * 60;
        let first = snapshot(0.0, reset, start + 60);
        let after_gap = snapshot(50.0, reset, start + 3_600);
        let after_reset = snapshot(1.0, reset + WEEK_MINUTES as i64 * 60, reset + 60);
        let mut history = UsageHistory::default();
        record(&mut history, &first, None, "2026-08-01");
        record(&mut history, &after_gap, None, "2026-08-01");
        record(&mut history, &after_reset, None, "2026-08-08");

        assert!(!history.completed_weekly_cycles[0].quota_complete);
    }

    #[test]
    fn ledger_prunes_old_records_and_inactive_accounts() {
        let mut ledger = UsageLedger::default();
        for account in 0..MAX_ACCOUNTS + 2 {
            let key = format!("account-{account}");
            let history = ledger.accounts.entry(key.clone()).or_default();
            history.days = (0..MAX_DAILY_RECORDS + 2)
                .map(|day| DailyUsageRecord {
                    date: format!("2024-01-{day:04}"),
                    weekly_consumed_percent: 0.0,
                    tokens: None,
                    quota_complete: false,
                    token_complete: false,
                })
                .collect();
            ledger.last_account_key = Some(key);
        }

        ledger.prune();
        assert_eq!(ledger.accounts.len(), MAX_ACCOUNTS);
        assert!(ledger.accounts.contains_key(ledger.last_account_key.as_deref().unwrap()));
        assert!(ledger.accounts.values().all(|history| history.days.len() == MAX_DAILY_RECORDS));
    }
}
