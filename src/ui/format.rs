//! Localization, text formatting, and quota/usage summary helpers.

use super::history_view::{ResourcePage, UsageSummaryWeek};
use crate::history::UsageHistoryView;
use crate::model::{
    AccountSummary, CreditBalance, DisplayState, ProviderKind, QuotaKind, QuotaWindow, RefreshState,
};
use chrono::{DateTime, Datelike, Local};
use windows::Win32::Globalization::GetUserDefaultLocaleName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    English,
    Chinese,
}

impl Locale {
    pub fn detect(setting: &str) -> Self {
        match setting {
            "en" => Self::English,
            "zh-CN" => Self::Chinese,
            _ => {
                let mut name = [0_u16; 85];
                let length = unsafe { GetUserDefaultLocaleName(&mut name) };
                let locale = if length > 0 {
                    String::from_utf16_lossy(&name[..length.saturating_sub(1) as usize])
                } else {
                    String::new()
                };
                if locale.to_ascii_lowercase().starts_with("zh") {
                    Self::Chinese
                } else {
                    Self::English
                }
            }
        }
    }

    pub fn text(self, english: &'static str, chinese: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Chinese => chinese,
        }
    }
}

pub(super) struct ClaudeWindowSummary {
    pub percent: Option<u8>,
    pub reset_hint: String,
}

pub(super) struct ClaudeSummaryRows {
    pub five_hour: ClaudeWindowSummary,
    pub weekly: ClaudeWindowSummary,
}

pub(super) struct AccountMetrics {
    pub credits: String,
    pub credit_detail: Option<String>,
    pub page: ResourcePage,
}

pub(super) struct WeeklyUsageMetrics {
    pub tokens: String,
    pub range: String,
    pub selection: UsageSummaryWeek,
}

pub(super) fn header_title(_provider: ProviderKind) -> &'static str {
    "AgentStatus"
}

pub(super) const fn ui_font_face(_locale: Locale) -> &'static str {
    "Microsoft YaHei UI"
}

pub(super) fn version_text() -> String {
    format!(" - v{}", env!("CARGO_PKG_VERSION"))
}

pub fn tooltip(state: &DisplayState, preferred: QuotaKind, locale: Locale) -> String {
    let status = match state.refresh_state {
        RefreshState::Loading => locale.text("refreshing", "刷新中"),
        RefreshState::Live => locale.text("live", "实时"),
        RefreshState::Cached => locale.text("cached", "缓存"),
        RefreshState::Unavailable => locale.text("unavailable", "不可用"),
    };
    let kind = state.resolved_quota_kind(preferred);
    match state.quota_percent(kind) {
        Some(percent) => {
            format!("AgentStatus · {} {}% · {status}", quota_remaining_label(kind, locale), percent)
        }
        None => format!("AgentStatus · {status}"),
    }
}

pub(super) fn updated_text(state: &DisplayState, locale: Locale, refresh_feedback: bool) -> String {
    if refresh_feedback || state.refresh_state == RefreshState::Loading {
        return locale.text("Refreshing…", "刷新中…").to_owned();
    }
    let time = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| DateTime::from_timestamp(snapshot.fetched_at, 0))
        .map(|time| time.with_timezone(&Local).format("%H:%M").to_string());
    let prefix = match state.refresh_state {
        RefreshState::Cached => locale.text("Cached", "缓存"),
        RefreshState::Unavailable => locale.text("Unavailable", "不可用"),
        _ => locale.text("Updated", "更新"),
    };
    time.map_or_else(|| prefix.to_owned(), |time| format!("{prefix} {time}"))
}

fn claude_reset_hint(window: &QuotaWindow, now: i64, locale: Locale) -> String {
    if window.is_unused(now) {
        return locale.text("Unused", "未使用").to_owned();
    }
    let Some(reset) = window.resets_at else {
        return "--".to_owned();
    };
    let seconds = reset.saturating_sub(now).max(0);
    let countdown = reset_countdown(seconds, locale);
    let short_date = chrono::DateTime::from_timestamp(reset, 0).map(|utc| {
        let dt = utc.with_timezone(&Local);
        if window.window_minutes == crate::model::SESSION_MINUTES {
            dt.format("%H:%M").to_string()
        } else {
            match locale {
                Locale::Chinese => format!("{}月{}日", dt.month(), dt.day()),
                Locale::English => dt.format("%b %-d").to_string(),
            }
        }
    });
    match short_date {
        Some(date) => format!("{countdown} · {date}"),
        None => countdown,
    }
}

pub(super) fn claude_summary_rows(state: &DisplayState, locale: Locale) -> ClaudeSummaryRows {
    let now = chrono::Utc::now().timestamp();
    let summarize = |window: Option<&QuotaWindow>, fetched_at: i64| -> ClaudeWindowSummary {
        match window.filter(|w| w.is_cache_valid(now, fetched_at)) {
            Some(w) => ClaudeWindowSummary {
                percent: Some(w.display_percent()),
                reset_hint: claude_reset_hint(w, now, locale),
            },
            None => ClaudeWindowSummary { percent: None, reset_hint: "--".to_owned() },
        }
    };
    let Some(snapshot) = state.snapshot.as_ref() else {
        return ClaudeSummaryRows {
            five_hour: ClaudeWindowSummary { percent: None, reset_hint: "--".to_owned() },
            weekly: ClaudeWindowSummary { percent: None, reset_hint: "--".to_owned() },
        };
    };
    let Some(claude) = snapshot.account.claude_quotas.as_ref() else {
        return ClaudeSummaryRows {
            five_hour: ClaudeWindowSummary { percent: None, reset_hint: "--".to_owned() },
            weekly: ClaudeWindowSummary { percent: None, reset_hint: "--".to_owned() },
        };
    };
    ClaudeSummaryRows {
        five_hour: summarize(claude.five_hour.as_ref(), snapshot.fetched_at),
        weekly: summarize(claude.weekly.as_ref(), snapshot.fetched_at),
    }
}

pub(super) fn weekly_usage_metrics(
    history: Option<&UsageHistoryView>,
    selection: UsageSummaryWeek,
    locale: Locale,
) -> WeeklyUsageMetrics {
    let Some(history) = history else {
        return WeeklyUsageMetrics { tokens: "--".to_owned(), range: "--".to_owned(), selection };
    };
    let start = match selection {
        UsageSummaryWeek::Previous => history.previous_week_start(),
        UsageSummaryWeek::Current => history.current_week_start(),
    };
    let week = history.week(start);
    let range = match locale {
        Locale::Chinese => format!(
            "{}月{}日-{}月{}日",
            start.month(),
            start.day(),
            (start + chrono::Duration::days(6)).month(),
            (start + chrono::Duration::days(6)).day()
        ),
        Locale::English => format!(
            "{}-{}",
            start.format("%b %-d"),
            (start + chrono::Duration::days(6)).format("%b %-d")
        ),
    };
    WeeklyUsageMetrics {
        tokens: week
            .filter(|week| week.has_data)
            .map(|week| format_tokens(week.tokens, locale))
            .unwrap_or_else(|| "--".to_owned()),
        range,
        selection,
    }
}

pub(super) fn format_tokens(value: u64, locale: Locale) -> String {
    let (divisor, suffix) = match locale {
        Locale::Chinese if value >= 100_000_000 => (100_000_000.0, "亿"),
        Locale::Chinese if value >= 10_000 => (10_000.0, "万"),
        Locale::Chinese => return value.to_string(),
        Locale::English if value >= 1_000_000_000 => (1_000_000_000.0, "B"),
        Locale::English if value >= 1_000_000 => (1_000_000.0, "M"),
        Locale::English if value >= 1_000 => (1_000.0, "K"),
        Locale::English => return value.to_string(),
    };
    let scaled = value as f64 / divisor;
    if scaled >= 100.0 || (scaled - scaled.round()).abs() < 0.05 {
        format!("{scaled:.0}{suffix}")
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

pub(super) fn account_metrics(
    state: &DisplayState,
    locale: Locale,
    page: ResourcePage,
) -> AccountMetrics {
    let snapshot = state.snapshot.as_ref();
    if page == ResourcePage::Points {
        return AccountMetrics {
            credits: snapshot
                .and_then(|s| s.account.credit_balance.as_ref())
                .map(format_credit_balance)
                .unwrap_or_else(|| "--".into()),
            credit_detail: Some(locale.text("For extra usage", "用于额外用量").into()),
            page,
        };
    }
    let credits = snapshot
        .and_then(|snapshot| snapshot.account.reset_credits)
        .map(|credits| format!("{credits} {}", locale.text("resets", "次")))
        .unwrap_or_else(|| "--".to_owned());
    let credit_detail =
        snapshot.and_then(|snapshot| reset_credit_detail(&snapshot.account, locale));
    AccountMetrics { credits, credit_detail, page }
}

pub(super) fn resource_label(page: ResourcePage, locale: Locale) -> &'static str {
    match page {
        ResourcePage::Resets => locale.text("Reset credits", "重置机会"),
        ResourcePage::Points => locale.text("Credits", "额度点数"),
    }
}

fn format_credit_balance(balance: &CreditBalance) -> String {
    let CreditBalance::Amount(raw) = balance else {
        return "∞".into();
    };
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole == "0" && !fraction.is_empty() && fraction.starts_with("00") {
        return "<0.01".into();
    }
    let mut output = String::with_capacity(raw.len() + raw.len() / 3);
    for (index, digit) in whole.bytes().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(char::from(digit));
    }
    let fraction = fraction.get(..fraction.len().min(2)).unwrap_or_default().trim_end_matches('0');
    if !fraction.is_empty() {
        output.push('.');
        output.push_str(fraction);
    }
    output
}

pub(super) fn theoretical_remaining_percent(window: &QuotaWindow, now: i64) -> Option<u8> {
    let reset = window.resets_at?;
    let total_seconds = i64::try_from(window.window_minutes.checked_mul(60)?).ok()?;
    if total_seconds == 0 {
        return None;
    }
    let remaining = reset.saturating_sub(now).clamp(0, total_seconds);
    let total_seconds = u128::try_from(total_seconds).ok()?;
    let remaining = u128::try_from(remaining).ok()?;
    ((remaining * 100 + total_seconds / 2) / total_seconds).try_into().ok()
}

pub(super) fn reset_details(window: &QuotaWindow, locale: Locale) -> (String, String) {
    reset_details_at(window, Local::now().timestamp(), locale)
}

fn reset_details_at(window: &QuotaWindow, now: i64, locale: Locale) -> (String, String) {
    if window.is_unused(now) {
        return (
            locale.text("Unused", "未使用").to_owned(),
            locale.text("Starts on use", "使用后计时").to_owned(),
        );
    }
    let Some(reset) = window.resets_at else {
        return (
            locale.text("Unavailable", "暂无").to_owned(),
            locale.text("Reset time", "重置时间").to_owned(),
        );
    };
    let seconds = reset.saturating_sub(now).max(0);
    let countdown = reset_countdown(seconds, locale);
    let local_time = DateTime::from_timestamp(reset, 0)
        .map(|time| {
            let time = time.with_timezone(&Local);
            if locale == Locale::Chinese {
                time.format("%-m月%-d日 %H:%M").to_string()
            } else {
                time.format("%m/%d %H:%M").to_string()
            }
        })
        .unwrap_or_else(|| "--".to_owned());
    (countdown, local_time)
}

fn reset_countdown(seconds: i64, locale: Locale) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let total_minutes = seconds / 60;
    let minutes = total_minutes % 60;
    if locale == Locale::Chinese {
        if seconds >= 86_400 {
            format!("{days}天{hours}小时")
        } else if seconds >= 3_600 {
            format!("{hours}小时{minutes}分")
        } else if seconds >= 60 {
            format!("{total_minutes}分钟")
        } else {
            "不足1分钟".to_owned()
        }
    } else if seconds >= 86_400 {
        format!("{days}d {hours}h")
    } else if seconds >= 3_600 {
        format!("{hours}h {minutes}m")
    } else if seconds >= 60 {
        format!("{total_minutes}m")
    } else {
        "Less than 1m".to_owned()
    }
}

fn reset_credit_detail(account: &AccountSummary, locale: Locale) -> Option<String> {
    match account.reset_credits {
        Some(0) => Some(locale.text("No reset credits available", "暂无可用重置券").to_owned()),
        Some(_) => account.reset_credit_expires_at.and_then(|expires_at| {
            DateTime::from_timestamp(expires_at, 0).map(|time| {
                let time = time.with_timezone(&Local);
                if locale == Locale::Chinese {
                    time.format("%-m月%-d日 %H:%M").to_string()
                } else {
                    time.format("%m/%d %H:%M").to_string()
                }
            })
        }),
        None => None,
    }
}

fn quota_remaining_label(kind: QuotaKind, locale: Locale) -> &'static str {
    match kind {
        QuotaKind::FiveHour => locale.text("5-hour remaining", "5小时剩余"),
        QuotaKind::Weekly => locale.text("Weekly remaining", "本周剩余"),
    }
}

pub(super) fn quota_label(kind: QuotaKind, locale: Locale) -> &'static str {
    match kind {
        QuotaKind::FiveHour => locale.text("5-hour quota", "5小时额度"),
        QuotaKind::Weekly => locale.text("Weekly quota", "本周额度"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::DailyUsageRecord;
    use crate::model::{
        AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, SecondaryQuotas, WEEK_MINUTES,
    };
    use chrono::{NaiveDate, TimeZone};

    fn quota(remaining_percent: f64, window_minutes: u64) -> QuotaWindow {
        QuotaWindow {
            used_percent: 100.0 - remaining_percent,
            remaining_percent,
            window_minutes,
            resets_at: Some(i64::MAX),
        }
    }

    fn display(session: bool, weekly: bool) -> DisplayState {
        DisplayState::live(QuotaSnapshot {
            weekly: weekly.then(|| quota(66.0, WEEK_MINUTES)),
            session: session.then(|| quota(85.0, SESSION_MINUTES)),
            account: AccountSummary::default(),
            fetched_at: 0,
        })
    }

    #[test]
    fn points_display_is_bounded_and_never_invents_an_expiry() {
        assert_eq!(format_credit_balance(&CreditBalance::Amount("1200.259".into())), "1,200.25");
        assert_eq!(format_credit_balance(&CreditBalance::Amount("0.0001".into())), "<0.01");
        assert_eq!(format_credit_balance(&CreditBalance::Amount("0".into())), "0");
        assert_eq!(format_credit_balance(&CreditBalance::Unlimited), "∞");
        let metrics =
            account_metrics(&DisplayState::loading(None), Locale::Chinese, ResourcePage::Points);
        assert_eq!(metrics.credits, "--");
        assert_eq!(metrics.credit_detail.as_deref(), Some("用于额外用量"));
    }

    #[test]
    fn localizes_tooltip_status() {
        let state =
            DisplayState { snapshot: None, refresh_state: RefreshState::Unavailable, error: None };
        assert!(tooltip(&state, QuotaKind::FiveHour, Locale::English).contains("unavailable"));
        assert!(tooltip(&state, QuotaKind::FiveHour, Locale::Chinese).contains("不可用"));
    }

    #[test]
    fn version_label_uses_the_running_package_version() {
        assert_eq!(version_text(), format!(" - v{}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn refresh_feedback_replaces_the_timestamp_without_changing_display_state() {
        let state = display(true, true);
        assert_eq!(updated_text(&state, Locale::Chinese, true), "刷新中…");
        assert_eq!(state.refresh_state, RefreshState::Live);
    }

    #[test]
    fn theoretical_remaining_tracks_time_left_in_the_window() {
        let window = QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes: 300,
            resets_at: Some(18_000),
        };
        assert_eq!(theoretical_remaining_percent(&window, 0), Some(100));
        assert_eq!(theoretical_remaining_percent(&window, 9_000), Some(50));
        assert_eq!(theoretical_remaining_percent(&window, 18_000), Some(0));
        assert_eq!(theoretical_remaining_percent(&window, 20_000), Some(0));
    }

    #[test]
    fn theoretical_remaining_handles_the_largest_convertible_window() {
        let window = QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes: i64::MAX as u64 / 60,
            resets_at: Some(i64::MAX),
        };
        assert_eq!(theoretical_remaining_percent(&window, 0), Some(100));
    }

    #[test]
    fn reset_countdown_never_goes_negative() {
        let window = QuotaWindow {
            used_percent: 20.0,
            remaining_percent: 80.0,
            window_minutes: 10_080,
            resets_at: Some(1),
        };
        let (countdown, _) = reset_details_at(&window, 100, Locale::English);
        assert!(!countdown.contains('-'));
    }

    #[test]
    fn formats_reset_countdown_at_each_requested_boundary() {
        assert_eq!(reset_countdown(2 * 86_400 + 3 * 3_600, Locale::Chinese), "2天3小时");
        assert_eq!(reset_countdown(23 * 3_600 + 7 * 60, Locale::Chinese), "23小时7分");
        assert_eq!(reset_countdown(59 * 60, Locale::Chinese), "59分钟");
        assert_eq!(reset_countdown(59, Locale::Chinese), "不足1分钟");
    }

    #[test]
    fn formats_localized_reset_timestamp_without_leading_zeroes_in_chinese() {
        let reset = Local.with_ymd_and_hms(2026, 8, 2, 11, 0, 0).single().unwrap().timestamp();
        let now = reset - 3_600;
        let window = QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes: 10_080,
            resets_at: Some(reset),
        };

        assert_eq!(reset_details_at(&window, now, Locale::Chinese).1, "8月2日 11:00");
        assert_eq!(reset_details_at(&window, now, Locale::English).1, "08/02 11:00");
        assert_eq!(
            reset_details_at(&window, reset, Locale::Chinese),
            ("未使用".to_owned(), "使用后计时".to_owned())
        );
    }

    #[test]
    fn formats_the_nearest_reset_credit_expiration_to_minutes() {
        let expires_at = Local.with_ymd_and_hms(2026, 8, 1, 3, 8, 34).single().unwrap().timestamp();
        let account = AccountSummary {
            reset_credits: Some(2),
            reset_credit_expires_at: Some(expires_at),
            ..AccountSummary::default()
        };

        assert_eq!(reset_credit_detail(&account, Locale::Chinese).as_deref(), Some("8月1日 03:08"));
        assert_eq!(reset_credit_detail(&account, Locale::English).as_deref(), Some("08/01 03:08"));
    }

    #[test]
    fn labels_the_no_reset_credit_state_without_an_expiration() {
        let account = AccountSummary { reset_credits: Some(0), ..AccountSummary::default() };
        assert_eq!(
            reset_credit_detail(&account, Locale::Chinese).as_deref(),
            Some("暂无可用重置券")
        );
        assert_eq!(
            reset_credit_detail(&account, Locale::English).as_deref(),
            Some("No reset credits available")
        );
    }

    #[test]
    fn uses_microsoft_yahei_ui_in_every_locale() {
        assert_eq!(ui_font_face(Locale::Chinese), ui_font_face(Locale::English));
        assert_eq!(ui_font_face(Locale::Chinese), "Microsoft YaHei UI");
    }

    #[test]
    fn weekly_summary_uses_natural_weeks_and_compact_units() {
        let view = UsageHistoryView {
            days: vec![
                DailyUsageRecord { date: "2026-07-27".to_owned(), tokens: 120_000_000 },
                DailyUsageRecord { date: "2026-08-03".to_owned(), tokens: 25_000 },
            ],
            weeks: vec![
                crate::history::NaturalWeekView {
                    start_date: NaiveDate::from_ymd_opt(2026, 7, 27).unwrap(),
                    end_date: NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
                    tokens: 120_000_000,
                    has_data: true,
                },
                crate::history::NaturalWeekView {
                    start_date: NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(),
                    end_date: NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(),
                    tokens: 25_000,
                    has_data: true,
                },
            ],
            today: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
        };

        let previous =
            weekly_usage_metrics(Some(&view), UsageSummaryWeek::Previous, Locale::Chinese);
        assert_eq!(previous.tokens, "1.2亿");
        let current = weekly_usage_metrics(Some(&view), UsageSummaryWeek::Current, Locale::Chinese);
        assert_eq!(current.tokens, "2.5万");
    }

    #[test]
    fn weekly_summary_distinguishes_missing_from_zero() {
        let view = UsageHistoryView {
            days: vec![DailyUsageRecord { date: "2026-08-05".to_owned(), tokens: 0 }],
            weeks: vec![crate::history::NaturalWeekView {
                start_date: NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(),
                end_date: NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(),
                tokens: 0,
                has_data: true,
            }],
            today: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
        };
        let missing =
            weekly_usage_metrics(Some(&view), UsageSummaryWeek::Previous, Locale::Chinese);
        assert_eq!(missing.tokens, "--");
        let zero = weekly_usage_metrics(Some(&view), UsageSummaryWeek::Current, Locale::Chinese);
        assert_eq!(zero.tokens, "0");
        let unavailable = weekly_usage_metrics(None, UsageSummaryWeek::Previous, Locale::Chinese);
        assert_eq!(unavailable.tokens, "--");
    }

    #[test]
    fn token_labels_keep_one_decimal_only_below_three_integer_digits() {
        assert_eq!(format_tokens(340_000_000, Locale::Chinese), "3.4亿");
        assert_eq!(format_tokens(1_230_000_000, Locale::Chinese), "12.3亿");
        assert_eq!(format_tokens(78_983_000, Locale::Chinese), "7898万");
        assert_eq!(format_tokens(120_000_000, Locale::Chinese), "1.2亿");
    }

    #[test]
    fn claude_summary_rows_displays_unused_for_five_hour_and_weekly_windows() {
        let now = chrono::Utc::now().timestamp();
        for resets_at in [None, Some(0)] {
            let state = DisplayState::live(QuotaSnapshot {
                weekly: Some(quota(80.0, WEEK_MINUTES)),
                session: Some(quota(90.0, SESSION_MINUTES)),
                account: AccountSummary {
                    claude_quotas: Some(SecondaryQuotas {
                        five_hour: Some(QuotaWindow {
                            used_percent: 0.0,
                            remaining_percent: 100.0,
                            window_minutes: SESSION_MINUTES,
                            resets_at,
                        }),
                        weekly: Some(QuotaWindow {
                            used_percent: 0.0,
                            remaining_percent: 100.0,
                            window_minutes: WEEK_MINUTES,
                            resets_at,
                        }),
                    }),
                    ..AccountSummary::default()
                },
                fetched_at: now,
            });

            let zh = claude_summary_rows(&state, Locale::Chinese);
            assert_eq!(zh.five_hour.percent, Some(100));
            assert_eq!(zh.five_hour.reset_hint, "未使用");
            assert_eq!(zh.weekly.percent, Some(100));
            assert_eq!(zh.weekly.reset_hint, "未使用");

            let en = claude_summary_rows(&state, Locale::English);
            assert_eq!(en.five_hour.percent, Some(100));
            assert_eq!(en.five_hour.reset_hint, "Unused");
            assert_eq!(en.weekly.percent, Some(100));
            assert_eq!(en.weekly.reset_hint, "Unused");
        }
    }
}
