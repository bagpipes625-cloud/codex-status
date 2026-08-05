use crate::history::UsageHistoryView;
use chrono::{Datelike, Local, NaiveDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSummaryDay {
    Yesterday,
    Today,
}

impl UsageSummaryDay {
    pub fn toggle(self) -> Self {
        match self {
            Self::Yesterday => Self::Today,
            Self::Today => Self::Yesterday,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryPage {
    Main,
    Month,
    Cycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryNavigation {
    pub page: HistoryPage,
    pub month: NaiveDate,
    pub selected_cycle: Option<usize>,
    pub summary_day: UsageSummaryDay,
}

impl Default for HistoryNavigation {
    fn default() -> Self {
        let today = Local::now().date_naive();
        Self {
            page: HistoryPage::Main,
            month: today.with_day(1).expect("first day of current month"),
            selected_cycle: None,
            summary_day: UsageSummaryDay::Yesterday,
        }
    }
}

impl HistoryNavigation {
    pub fn open_month(&mut self, view: Option<&UsageHistoryView>) {
        if let Some(view) = view {
            self.month = view.today.with_day(1).expect("first day of current month");
            self.selected_cycle = view.current_cycle_index();
        }
        self.page = HistoryPage::Month;
    }

    pub fn open_selected_or_current_cycle(&mut self, view: &UsageHistoryView) {
        let selected = self.selected_cycle.filter(|index| *index < view.cycles.len());
        self.selected_cycle = selected.or_else(|| view.current_cycle_index());
        if self.selected_cycle.is_some() {
            self.page = HistoryPage::Cycle;
        }
    }

    pub fn shift_month(&mut self, delta: i32) {
        let month_index = self.month.year() * 12 + self.month.month0() as i32 + delta;
        let year = month_index.div_euclid(12);
        let month = month_index.rem_euclid(12) as u32 + 1;
        if let Some(value) = NaiveDate::from_ymd_opt(year, month, 1) {
            self.month = value;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHit {
    Back,
    ToggleSummaryDay,
    OpenHistory,
    PreviousMonth,
    NextMonth,
    MonthTab,
    CycleTab,
    PreviousCycle,
    NextCycle,
    Cycle(usize),
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
                if contains(x, y, 197, 59, 315, 91) {
                    Some(HistoryHit::ToggleSummaryDay)
                } else if contains(x, y, 197, 91, 315, 151) {
                    Some(HistoryHit::OpenHistory)
                } else {
                    None
                }
            } else if contains(x, y, 30, 278, 178, 303) {
                Some(HistoryHit::ToggleSummaryDay)
            } else if contains(x, y, 30, 303, 178, 335) {
                Some(HistoryHit::OpenHistory)
            } else {
                None
            }
        }
        HistoryPage::Month | HistoryPage::Cycle => {
            if contains(x, y, 10, 7, 72, 39) {
                return Some(HistoryHit::Back);
            }
            if !full_history {
                return None;
            }
            let width = if compact { 336 } else { 376 };
            if contains(x, y, 43, 43, 72, 77) {
                return Some(match navigation.page {
                    HistoryPage::Month => HistoryHit::PreviousMonth,
                    HistoryPage::Cycle => HistoryHit::PreviousCycle,
                    HistoryPage::Main => unreachable!(),
                });
            }
            if contains(x, y, width - 72, 43, width - 43, 77) {
                return Some(match navigation.page {
                    HistoryPage::Month => HistoryHit::NextMonth,
                    HistoryPage::Cycle => HistoryHit::NextCycle,
                    HistoryPage::Main => unreachable!(),
                });
            }
            let tabs_top = if compact { 249 } else { 316 };
            let tabs_left = (width - 160) / 2;
            if contains(x, y, tabs_left, tabs_top, tabs_left + 80, tabs_top + 28) {
                return Some(HistoryHit::MonthTab);
            }
            if contains(x, y, tabs_left + 80, tabs_top, tabs_left + 160, tabs_top + 28) {
                return Some(HistoryHit::CycleTab);
            }
            if navigation.page == HistoryPage::Month {
                return history
                    .and_then(|view| month_cycle_hit(navigation.month, view, compact, x, y));
            }
            None
        }
    }
}

pub fn hovered_cycle(
    navigation: &HistoryNavigation,
    history: Option<&UsageHistoryView>,
    compact: bool,
    x: i32,
    y: i32,
    dpi: u32,
    full_history: bool,
) -> Option<usize> {
    match hit_test(navigation, history, compact, x, y, dpi, full_history) {
        Some(HistoryHit::Cycle(index)) => Some(index),
        _ => None,
    }
}

fn month_cycle_hit(
    month: NaiveDate,
    history: &UsageHistoryView,
    compact: bool,
    x: i32,
    y: i32,
) -> Option<HistoryHit> {
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
    let date =
        month - chrono::Duration::days(offset) + chrono::Duration::days((row * 7 + column) as i64);
    history
        .cycles
        .iter()
        .enumerate()
        .rfind(|(_, cycle)| {
            date >= cycle.start_date && date <= cycle.observed_end_date.min(cycle.display_end_date)
        })
        .map(|(index, _)| HistoryHit::Cycle(index))
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
    use crate::history::WeeklyCycleView;

    fn active_history() -> UsageHistoryView {
        UsageHistoryView {
            days: Vec::new(),
            cycles: vec![WeeklyCycleView {
                started_at: 0,
                ended_at: None,
                scheduled_reset_at: None,
                start_date: NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
                observed_end_date: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
                display_end_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
                consumed_percent: 28.0,
                token_activity: None,
                token_estimated: false,
                quota_complete: false,
                reset_kind: None,
                active: true,
                stale: false,
            }],
            today: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
        }
    }

    #[test]
    fn month_navigation_wraps_years() {
        let mut navigation = HistoryNavigation {
            month: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            ..HistoryNavigation::default()
        };
        navigation.shift_month(-1);
        assert_eq!(navigation.month, NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        navigation.shift_month(2);
        assert_eq!(navigation.month, NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
    }

    #[test]
    fn main_summary_hits_are_separate_from_detail_hits() {
        let navigation = HistoryNavigation::default();
        assert_eq!(
            hit_test(&navigation, None, false, 60, 285, 96, true),
            Some(HistoryHit::ToggleSummaryDay)
        );
        assert_eq!(
            hit_test(&navigation, None, false, 60, 318, 96, true),
            Some(HistoryHit::OpenHistory)
        );
    }

    #[test]
    fn current_cycle_hit_area_stops_at_today() {
        let history = active_history();
        let navigation = HistoryNavigation {
            page: HistoryPage::Month,
            month: NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            selected_cycle: Some(0),
            summary_day: UsageSummaryDay::Yesterday,
        };

        // August 3 is in the active cycle; August 6 is a future date in the same display window.
        assert_eq!(
            hit_test(&navigation, Some(&history), true, 45, 130, 96, true),
            Some(HistoryHit::Cycle(0))
        );
        assert_eq!(hit_test(&navigation, Some(&history), true, 168, 130, 96, true), None);
    }

    #[test]
    fn opening_history_selects_the_active_cycle() {
        let history = active_history();
        let mut navigation = HistoryNavigation::default();
        navigation.open_month(Some(&history));
        assert_eq!(navigation.page, HistoryPage::Month);
        assert_eq!(navigation.month, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
        assert_eq!(navigation.selected_cycle, Some(0));
    }

    #[test]
    fn simplified_fallback_exposes_only_the_visible_back_action() {
        let history = active_history();
        let navigation = HistoryNavigation {
            page: HistoryPage::Month,
            month: history.today.with_day(1).unwrap(),
            selected_cycle: Some(0),
            summary_day: UsageSummaryDay::Yesterday,
        };
        assert_eq!(
            hit_test(&navigation, Some(&history), true, 30, 20, 96, false),
            Some(HistoryHit::Back)
        );
        assert_eq!(hit_test(&navigation, Some(&history), true, 45, 130, 96, false), None);
    }
}
