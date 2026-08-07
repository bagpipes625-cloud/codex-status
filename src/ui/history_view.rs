use crate::history::{UsageHistoryView, monday_of};
use chrono::{Datelike, Duration, Local, NaiveDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSummaryWeek {
    Previous,
    Current,
}

impl UsageSummaryWeek {
    pub fn toggle(self) -> Self {
        match self {
            Self::Previous => Self::Current,
            Self::Current => Self::Previous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryPage {
    Main,
    Month,
    Week,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryNavigation {
    pub page: HistoryPage,
    pub month: NaiveDate,
    pub selected_week: NaiveDate,
    pub summary_week: UsageSummaryWeek,
}

impl Default for HistoryNavigation {
    fn default() -> Self {
        let today = Local::now().date_naive();
        Self {
            page: HistoryPage::Main,
            month: today.with_day(1).expect("first day of current month"),
            selected_week: monday_of(today),
            summary_week: UsageSummaryWeek::Current,
        }
    }
}

impl HistoryNavigation {
    pub fn open_current_week(&mut self, view: Option<&UsageHistoryView>) {
        if let Some(view) = view {
            self.month = view.today.with_day(1).expect("first day of current month");
            self.selected_week = view.current_week_start();
        }
        self.page = HistoryPage::Week;
    }

    pub fn open_selected_week(&mut self) {
        self.page = HistoryPage::Week;
    }

    pub fn shift_month(&mut self, delta: i32) {
        let month_index = self.month.year() * 12 + self.month.month0() as i32 + delta;
        let year = month_index.div_euclid(12);
        let month = month_index.rem_euclid(12) as u32 + 1;
        if let Some(value) = NaiveDate::from_ymd_opt(year, month, 1) {
            self.month = value;
        }
    }

    pub fn shift_week(&mut self, delta: i64) {
        self.selected_week += Duration::days(delta.saturating_mul(7));
    }

    pub fn shift_visible_period(&mut self, delta: i32, today: NaiveDate) -> bool {
        if delta == 0 {
            return false;
        }
        match self.page {
            HistoryPage::Month => {
                let latest = today.with_day(1).expect("first day of current month");
                if delta > 0 && self.month >= latest {
                    return false;
                }
                let previous = self.month;
                self.shift_month(delta.signum());
                self.month != previous
            }
            HistoryPage::Week => {
                let latest = monday_of(today);
                if delta > 0 && self.selected_week >= latest {
                    return false;
                }
                let previous = self.selected_week;
                self.shift_week(i64::from(delta.signum()));
                self.selected_week != previous
            }
            HistoryPage::Main => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHit {
    Back,
    ToggleSummaryWeek,
    OpenHistory,
    PreviousMonth,
    NextMonth,
    MonthTab,
    WeekTab,
    PreviousWeek,
    NextWeek,
    Week(NaiveDate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoveredHistoryDay {
    pub date: NaiveDate,
    pub row: usize,
    pub column: usize,
}

pub fn hit_test(
    navigation: &HistoryNavigation,
    history: Option<&UsageHistoryView>,
    compact: bool,
    x: i32,
    y: i32,
    dpi: u32,
    full_history: bool,
) -> Option<HistoryHit> {
    let x = logical(x, dpi);
    let y = logical(y, dpi);
    match navigation.page {
        HistoryPage::Main => {
            if compact {
                if contains(x, y, 197, 59, 315, 86) {
                    Some(HistoryHit::ToggleSummaryWeek)
                } else if contains(x, y, 197, 86, 315, 151) {
                    Some(HistoryHit::OpenHistory)
                } else {
                    None
                }
            } else if contains(x, y, 30, 278, 178, 298) {
                Some(HistoryHit::ToggleSummaryWeek)
            } else if contains(x, y, 30, 298, 178, 335) {
                Some(HistoryHit::OpenHistory)
            } else {
                None
            }
        }
        HistoryPage::Month | HistoryPage::Week => {
            if contains(x, y, 10, 7, 72, 39) {
                return Some(HistoryHit::Back);
            }
            if !full_history {
                return None;
            }
            let width = if compact { 336 } else { 376 };
            if contains(x, y, 43, 43, 72, 77) {
                return Some(if navigation.page == HistoryPage::Month {
                    HistoryHit::PreviousMonth
                } else {
                    HistoryHit::PreviousWeek
                });
            }
            if contains(x, y, width - 72, 43, width - 43, 77) {
                let today =
                    history.map(|view| view.today).unwrap_or_else(|| Local::now().date_naive());
                let at_latest = match navigation.page {
                    HistoryPage::Month => {
                        navigation.month >= today.with_day(1).expect("first day of current month")
                    }
                    HistoryPage::Week => navigation.selected_week >= monday_of(today),
                    HistoryPage::Main => false,
                };
                if at_latest {
                    return None;
                }
                return Some(if navigation.page == HistoryPage::Month {
                    HistoryHit::NextMonth
                } else {
                    HistoryHit::NextWeek
                });
            }
            let tabs_top = if compact { 249 } else { 316 };
            let tabs_left = (width - 160) / 2;
            if contains(x, y, tabs_left, tabs_top, tabs_left + 80, tabs_top + 28) {
                return Some(HistoryHit::WeekTab);
            }
            if contains(x, y, tabs_left + 80, tabs_top, tabs_left + 160, tabs_top + 28) {
                return Some(HistoryHit::MonthTab);
            }
            if navigation.page == HistoryPage::Month {
                let today =
                    history.map(|view| view.today).unwrap_or_else(|| Local::now().date_naive());
                return month_date_at(navigation.month, compact, x, y)
                    .filter(|date| *date <= today)
                    .map(|date| HistoryHit::Week(monday_of(date)));
            }
            None
        }
    }
}

pub fn hovered_day(
    navigation: &HistoryNavigation,
    history: Option<&UsageHistoryView>,
    compact: bool,
    x: i32,
    y: i32,
    dpi: u32,
    full_history: bool,
) -> Option<HoveredHistoryDay> {
    if navigation.page != HistoryPage::Month || !full_history {
        return None;
    }
    let date = month_date_at(navigation.month, compact, logical(x, dpi), logical(y, dpi))?;
    history?.day(date)?;
    let offset = navigation.month.weekday().num_days_from_monday() as i64;
    let first = navigation.month - Duration::days(offset);
    let index = (date - first).num_days() as usize;
    Some(HoveredHistoryDay { date, row: index / 7, column: index % 7 })
}

fn month_date_at(month: NaiveDate, compact: bool, x: i32, y: i32) -> Option<NaiveDate> {
    let grid_left = 24;
    let grid_right = if compact { 312 } else { 352 };
    let grid_top = if compact { 96 } else { 101 };
    let grid_bottom = if compact { 235 } else { 302 };
    if !contains(x, y, grid_left, grid_top, grid_right, grid_bottom) {
        return None;
    }
    let column = ((x - grid_left) * 7 / (grid_right - grid_left)).clamp(0, 6);
    let row = ((y - grid_top) * 6 / (grid_bottom - grid_top)).clamp(0, 5);
    let offset = month.weekday().num_days_from_monday() as i64;
    Some(month - Duration::days(offset) + Duration::days((row * 7 + column) as i64))
}

fn logical(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * 96) / i64::from(dpi.max(96))) as i32
}

fn contains(x: i32, y: i32, left: i32, top: i32, right: i32, bottom: i32) -> bool {
    x >= left && x < right && y >= top && y < bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_and_week_navigation_cross_boundaries() {
        let mut navigation = HistoryNavigation {
            month: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            selected_week: NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(),
            ..HistoryNavigation::default()
        };
        navigation.shift_month(-1);
        assert_eq!(navigation.month, NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        navigation.shift_week(-1);
        assert_eq!(navigation.selected_week, NaiveDate::from_ymd_opt(2025, 12, 29).unwrap());
    }

    #[test]
    fn calendar_click_selects_the_containing_monday() {
        let navigation = HistoryNavigation {
            page: HistoryPage::Month,
            month: NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            ..HistoryNavigation::default()
        };
        assert_eq!(
            hit_test(&navigation, None, true, 168, 130, 96, true),
            Some(HistoryHit::Week(NaiveDate::from_ymd_opt(2026, 8, 3).unwrap()))
        );
    }

    #[test]
    fn fallback_exposes_only_back_navigation() {
        let navigation = HistoryNavigation { page: HistoryPage::Month, ..Default::default() };
        assert_eq!(hit_test(&navigation, None, true, 30, 20, 96, false), Some(HistoryHit::Back));
        assert_eq!(hit_test(&navigation, None, true, 45, 130, 96, false), None);
    }

    #[test]
    fn current_week_cannot_navigate_into_the_future() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let view = UsageHistoryView { days: Vec::new(), weeks: Vec::new(), today };
        let navigation = HistoryNavigation {
            page: HistoryPage::Week,
            selected_week: monday_of(today),
            ..HistoryNavigation::default()
        };
        assert_eq!(hit_test(&navigation, Some(&view), true, 280, 60, 96, true), None);
    }

    #[test]
    fn visible_period_shift_moves_one_page_and_stops_at_today() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 7).unwrap();
        let mut navigation = HistoryNavigation {
            page: HistoryPage::Week,
            selected_week: monday_of(today),
            ..Default::default()
        };
        assert!(!navigation.shift_visible_period(1, today));
        assert!(navigation.shift_visible_period(-4, today));
        assert_eq!(navigation.selected_week, NaiveDate::from_ymd_opt(2026, 7, 27).unwrap());
        assert!(navigation.shift_visible_period(8, today));
        assert_eq!(navigation.selected_week, monday_of(today));

        navigation.page = HistoryPage::Month;
        navigation.month = today.with_day(1).unwrap();
        assert!(!navigation.shift_visible_period(1, today));
        assert!(navigation.shift_visible_period(-1, today));
        assert_eq!(navigation.month, NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
    }

    #[test]
    fn opening_history_defaults_to_the_current_natural_week() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let view = UsageHistoryView { days: Vec::new(), weeks: Vec::new(), today };
        let mut navigation = HistoryNavigation::default();
        navigation.open_current_week(Some(&view));
        assert_eq!(navigation.page, HistoryPage::Week);
        assert_eq!(navigation.selected_week, NaiveDate::from_ymd_opt(2026, 8, 3).unwrap());
    }

    #[test]
    fn summary_heading_toggles_but_token_value_opens_week_view() {
        let navigation = HistoryNavigation::default();
        assert_eq!(
            hit_test(&navigation, None, true, 250, 72, 96, true),
            Some(HistoryHit::ToggleSummaryWeek)
        );
        assert_eq!(
            hit_test(&navigation, None, true, 250, 100, 96, true),
            Some(HistoryHit::OpenHistory)
        );
        assert_eq!(
            hit_test(&navigation, None, false, 100, 286, 96, true),
            Some(HistoryHit::ToggleSummaryWeek)
        );
        assert_eq!(
            hit_test(&navigation, None, false, 100, 310, 96, true),
            Some(HistoryHit::OpenHistory)
        );
    }

    #[test]
    fn history_navigation_rejects_future_months_weeks_and_days_without_data() {
        let today = Local::now().date_naive();
        let mut week = HistoryNavigation {
            page: HistoryPage::Week,
            selected_week: monday_of(today),
            ..HistoryNavigation::default()
        };
        assert_eq!(hit_test(&week, None, true, 280, 60, 96, true), None);

        week.page = HistoryPage::Month;
        week.month = today.with_day(1).unwrap();
        assert_eq!(hit_test(&week, None, true, 280, 60, 96, true), None);

        let next_month = if today.month() == 12 {
            NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1).unwrap()
        };
        week.month = next_month;
        assert!(hit_test(&week, None, true, 168, 130, 96, true).is_none());
    }

    #[test]
    fn history_tabs_map_week_left_and_month_right() {
        let navigation = HistoryNavigation { page: HistoryPage::Week, ..Default::default() };
        assert_eq!(
            hit_test(&navigation, None, true, 110, 260, 96, true),
            Some(HistoryHit::WeekTab)
        );
        assert_eq!(
            hit_test(&navigation, None, true, 220, 260, 96, true),
            Some(HistoryHit::MonthTab)
        );
    }
}
