use crate::model::TokenUsageSnapshot;
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SCHEMA_VERSION: u32 = 2;
const MAX_ACCOUNTS: usize = 8;
const MAX_DAILY_RECORDS: usize = 730;

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
        usage: Option<&TokenUsageSnapshot>,
        observed_at: i64,
    ) {
        let Some(key) = account_key.filter(|value| !value.is_empty()) else {
            self.last_account_key = None;
            return;
        };
        let history = self.accounts.entry(key.to_owned()).or_default();
        if let Some(usage) = usage {
            history.record_refresh(usage, observed_at);
        }
        self.last_account_key = Some(key.to_owned());
        self.prune();
    }

    pub fn history_for(&self, account_key: Option<&str>) -> Option<&UsageHistory> {
        account_key.and_then(|key| self.accounts.get(key))
    }

    pub fn prune(&mut self) {
        if self.schema_version != SCHEMA_VERSION {
            *self = Self::default();
            return;
        }
        for history in self.accounts.values_mut() {
            history.prune();
        }
        while self.accounts.len() > MAX_ACCOUNTS {
            let protected = self.last_account_key.as_deref();
            let removal = self
                .accounts
                .iter()
                .filter(|(key, _)| Some(key.as_str()) != protected)
                .min_by_key(|(_, history)| history.last_observed_at)
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
    pub weeks: Vec<NaturalWeekView>,
    pub today: NaiveDate,
}

impl UsageHistoryView {
    pub fn day(&self, date: NaiveDate) -> Option<&DailyUsageRecord> {
        let key = date.format("%Y-%m-%d").to_string();
        self.days.iter().find(|day| day.date == key)
    }

    pub fn week(&self, start_date: NaiveDate) -> Option<&NaturalWeekView> {
        self.weeks.iter().find(|week| week.start_date == start_date)
    }

    pub fn current_week_start(&self) -> NaiveDate {
        monday_of(self.today)
    }

    pub fn previous_week_start(&self) -> NaiveDate {
        self.current_week_start() - Duration::days(7)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NaturalWeekView {
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub tokens: u64,
    pub has_data: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageRecord {
    pub date: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageHistory {
    pub days: Vec<DailyUsageRecord>,
    pub last_observed_at: i64,
}

impl UsageHistory {
    pub fn record_refresh(&mut self, usage: &TokenUsageSnapshot, observed_at: i64) {
        for bucket in &usage.daily_buckets {
            match self.days.binary_search_by(|day| day.date.cmp(&bucket.start_date)) {
                Ok(index) => self.days[index].tokens = bucket.tokens,
                Err(index) => self.days.insert(
                    index,
                    DailyUsageRecord { date: bucket.start_date.clone(), tokens: bucket.tokens },
                ),
            }
        }
        self.last_observed_at = self.last_observed_at.max(observed_at);
        self.prune();
    }

    pub fn view(&self, now: i64) -> UsageHistoryView {
        let today = local_naive_date(now).unwrap_or_else(|| Local::now().date_naive());
        let current_start = monday_of(today);
        let earliest_retained_week =
            monday_of(today - Duration::days((MAX_DAILY_RECORDS - 1) as i64));
        let first_start = self
            .days
            .first()
            .and_then(|day| parse_date(&day.date))
            .map(monday_of)
            .unwrap_or(current_start)
            .clamp(earliest_retained_week, current_start);
        let week_count = ((current_start - first_start).num_days() / 7 + 1).max(1);
        let weeks = (0..week_count)
            .map(|offset| {
                let start_date = first_start + Duration::days(offset * 7);
                let end_date = start_date + Duration::days(6);
                let mut tokens = 0_u64;
                let mut has_data = false;
                for day in &self.days {
                    let Some(date) = parse_date(&day.date) else { continue };
                    if date >= start_date && date <= end_date {
                        tokens = tokens.saturating_add(day.tokens);
                        has_data = true;
                    }
                }
                NaturalWeekView { start_date, end_date, tokens, has_data }
            })
            .collect();
        UsageHistoryView { days: self.days.clone(), weeks, today }
    }

    fn prune(&mut self) {
        self.days.sort_by(|left, right| left.date.cmp(&right.date));
        self.days.dedup_by(|left, right| {
            if left.date == right.date {
                left.tokens = right.tokens;
                true
            } else {
                false
            }
        });
        if self.days.len() > MAX_DAILY_RECORDS {
            self.days.drain(..self.days.len() - MAX_DAILY_RECORDS);
        }
    }
}

pub fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

fn local_naive_date(timestamp: i64) -> Option<NaiveDate> {
    Local.timestamp_opt(timestamp, 0).single().map(|value| value.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DailyTokenUsage;
    use chrono::NaiveDateTime;

    fn timestamp(date: &str) -> i64 {
        let value = NaiveDateTime::parse_from_str(&format!("{date} 12:00:00"), "%Y-%m-%d %H:%M:%S")
            .unwrap();
        Local.from_local_datetime(&value).single().unwrap().timestamp()
    }

    fn usage(values: &[(&str, u64)]) -> TokenUsageSnapshot {
        TokenUsageSnapshot {
            daily_buckets: values
                .iter()
                .map(|(date, tokens)| DailyTokenUsage {
                    start_date: (*date).to_owned(),
                    tokens: *tokens,
                })
                .collect(),
        }
    }

    #[test]
    fn mirrors_only_official_daily_buckets_and_overwrites_revisions() {
        let mut history = UsageHistory::default();
        history.record_refresh(&usage(&[("2026-08-03", 100), ("2026-08-04", 200)]), 1);
        history.record_refresh(&usage(&[("2026-08-04", 250)]), 2);
        assert_eq!(
            history.days,
            vec![
                DailyUsageRecord { date: "2026-08-03".to_owned(), tokens: 100 },
                DailyUsageRecord { date: "2026-08-04".to_owned(), tokens: 250 },
            ]
        );
    }

    #[test]
    fn natural_weeks_run_monday_through_sunday() {
        let mut history = UsageHistory::default();
        history.record_refresh(
            &usage(&[("2026-08-02", 10), ("2026-08-03", 20), ("2026-08-09", 30)]),
            timestamp("2026-08-05"),
        );
        let view = history.view(timestamp("2026-08-05"));
        assert_eq!(view.weeks[0].start_date, NaiveDate::from_ymd_opt(2026, 7, 27).unwrap());
        assert_eq!(view.weeks[0].tokens, 10);
        assert_eq!(view.weeks[1].start_date, NaiveDate::from_ymd_opt(2026, 8, 3).unwrap());
        assert_eq!(view.weeks[1].tokens, 50);
    }

    #[test]
    fn missing_days_are_not_invented_as_zero_buckets() {
        let mut history = UsageHistory::default();
        history.record_refresh(&usage(&[("2026-08-03", 20)]), timestamp("2026-08-05"));
        let view = history.view(timestamp("2026-08-05"));
        assert!(view.day(NaiveDate::from_ymd_opt(2026, 8, 4).unwrap()).is_none());
        assert!(view.week(view.current_week_start()).unwrap().has_data);
    }

    #[test]
    fn incompatible_schema_is_discarded() {
        let mut ledger = UsageLedger { schema_version: 1, ..UsageLedger::default() };
        ledger.accounts.insert("old".to_owned(), UsageHistory::default());
        ledger.prune();
        assert_eq!(ledger, UsageLedger::default());
    }

    #[test]
    fn account_without_usage_never_inherits_another_accounts_history() {
        let mut ledger = UsageLedger::default();
        ledger.record_refresh(Some("first"), Some(&usage(&[("2026-08-03", 20)])), 1);
        ledger.record_refresh(Some("second"), None, 2);
        assert!(ledger.history_for(Some("second")).unwrap().days.is_empty());
        assert_eq!(ledger.history_for(Some("first")).unwrap().days.len(), 1);
    }

    #[test]
    fn unidentified_accounts_are_neither_persisted_nor_reused() {
        let mut ledger = UsageLedger::default();
        ledger.record_refresh(Some("identified"), Some(&usage(&[("2026-08-03", 20)])), 1);
        ledger.record_refresh(None, Some(&usage(&[("2026-08-04", 30)])), 2);
        assert_eq!(ledger.accounts.len(), 1);
        assert!(ledger.history_for(None).is_none());
        assert!(ledger.last_account_key.is_none());
    }

    #[test]
    fn bounds_retained_official_days() {
        let mut history = UsageHistory::default();
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let values: Vec<_> = (0..MAX_DAILY_RECORDS + 2)
            .map(|offset| {
                (
                    (start + Duration::days(offset as i64)).format("%Y-%m-%d").to_string(),
                    offset as u64,
                )
            })
            .collect();
        let usage = TokenUsageSnapshot {
            daily_buckets: values
                .iter()
                .map(|(date, tokens)| DailyTokenUsage { start_date: date.clone(), tokens: *tokens })
                .collect(),
        };
        history.record_refresh(&usage, 1);
        assert_eq!(history.days.len(), MAX_DAILY_RECORDS);
    }

    #[test]
    fn view_bounds_the_week_span_even_for_an_extreme_valid_date() {
        let mut history = UsageHistory::default();
        history.days.push(DailyUsageRecord { date: "0001-01-01".to_owned(), tokens: 1 });
        let view = history.view(timestamp("2026-08-05"));
        assert!(view.weeks.len() <= MAX_DAILY_RECORDS.div_ceil(7) + 1);
    }
}
