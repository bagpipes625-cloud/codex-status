//! Single source of truth for flyout logical coordinates (96 DPI DIPs).

use crate::model::{DisplayState, QuotaAvailability, QuotaKind};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

pub const CARD_WIDTH: i32 = 376;
pub const CARD_HEIGHT: i32 = 352;
pub const COMPACT_CARD_WIDTH: i32 = 336;
pub const COMPACT_CARD_HEIGHT: i32 = 284;

pub const FLYOUT_CORNER_RADIUS: i32 = 8;

pub const HEADER_ACCENT_TOP: i32 = 15;
pub const HEADER_ACCENT_BOTTOM: i32 = 31;
pub const HEADER_TEXT_TOP: i32 = 8;
pub const HEADER_TEXT_BOTTOM: i32 = 38;
pub const HEADER_VERSION_TOP: i32 = HEADER_TEXT_TOP + 1;
pub const HEADER_VERSION_BOTTOM: i32 = HEADER_TEXT_BOTTOM + 1;

pub const REFRESH_BUTTON_RIGHT: i32 = 18;
pub const REFRESH_BUTTON_RADIUS: i32 = 12;
pub const REFRESH_BUTTON_GAP: i32 = 4;
pub const REFRESH_HIT_RADIUS: i32 = 14;
pub const REFRESH_ARC_START_DEGREES: f32 = 10.0;
pub const REFRESH_ARC_SWEEP_DEGREES: f32 = 305.0;

pub const QUOTA_CARD_TOP: i32 = 40;
pub const QUOTA_CARD_BOTTOM: i32 = 268;
pub const QUOTA_ARC_CENTER_Y: i32 = 149;

pub const STACKED_METRICS_CARD: LogicalRect = LogicalRect::new(192, 40, 320, 268);
pub const STACKED_DIVIDER_Y: i32 = 153;
pub const STACKED_DIVIDER_LEFT: i32 = 208;
pub const STACKED_DIVIDER_RIGHT: i32 = 304;
pub const COMPACT_CLAUDE_FIVE_HOUR: LogicalRect = LogicalRect::new(208, 74, 304, 126);
pub const COMPACT_CLAUDE_WEEKLY: LogicalRect = LogicalRect::new(208, 186, 304, 238);
pub const COMPACT_USAGE_HEADING: LogicalRect = LogicalRect::new(197, 64, 315, 88);
pub const COMPACT_USAGE_TOGGLE_HIT: LogicalRect = LogicalRect::new(197, 59, 315, 86);
pub const COMPACT_HISTORY_OPEN_HIT: LogicalRect = LogicalRect::new(197, 86, 315, 151);
pub const COMPACT_RESOURCE_HEADING: LogicalRect = LogicalRect::new(197, 173, 315, 197);
pub const COMPACT_RESOURCE_TOGGLE_HIT: LogicalRect = LogicalRect::new(197, 173, 315, 198);
pub const COMPACT_RESET_OPEN_HIT: LogicalRect = LogicalRect::new(197, 198, 315, 262);

pub const BOTTOM_METRICS_CARD: LogicalRect = LogicalRect::new(16, 276, 360, 336);
pub const BOTTOM_DIVIDER_X: i32 = 188;
pub const FULL_CLAUDE_FIVE_HOUR: LogicalRect = LogicalRect::new(42, 280, 162, 332);
pub const FULL_CLAUDE_WEEKLY: LogicalRect = LogicalRect::new(214, 280, 334, 332);
pub const FULL_USAGE_HEADING: LogicalRect = LogicalRect::new(30, 278, 175, 303);
pub const FULL_USAGE_TOGGLE_HIT: LogicalRect = LogicalRect::new(30, 278, 178, 298);
pub const FULL_HISTORY_OPEN_HIT: LogicalRect = LogicalRect::new(30, 298, 178, 335);
pub const FULL_RESOURCE_HEADING: LogicalRect = LogicalRect::new(203, 278, 345, 303);
pub const FULL_RESOURCE_TOGGLE_HIT: LogicalRect = LogicalRect::new(197, 278, 350, 303);
pub const FULL_RESET_OPEN_HIT: LogicalRect = LogicalRect::new(203, 298, 350, 335);

pub const HISTORY_BACK_HIT: LogicalRect = LogicalRect::new(10, 7, 72, 39);
pub const HISTORY_NAV_TOP: i32 = 43;
pub const HISTORY_NAV_BOTTOM: i32 = 77;
pub const HISTORY_PREV_HIT: LogicalRect =
    LogicalRect::new(43, HISTORY_NAV_TOP, 72, HISTORY_NAV_BOTTOM);
pub const HISTORY_TABS_WIDTH: i32 = 160;
pub const HISTORY_TABS_HALF_WIDTH: i32 = 80;
pub const HISTORY_TABS_HEIGHT: i32 = 28;

pub const RESET_BACK_HIT: LogicalRect = LogicalRect::new(14, 8, 110, 38);
pub const RESET_LIST_TOP: f32 = 76.0;
pub const RESET_ROW_HEIGHT: f32 = 60.0;
pub const RESET_LIST_BOTTOM_MARGIN: f32 = 16.0;
pub const RESET_USE_BUTTON_TOP_OFFSET: f32 = 9.0;
pub const RESET_USE_BUTTON_BOTTOM_OFFSET: f32 = 43.0;
pub const RESET_CONFIRM_HEIGHT: f32 = 166.0;
pub const RESET_SUCCESS_HEIGHT: f32 = 126.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl LogicalRect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub const fn contains_i32(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn contains_f32(self, x: f32, y: f32) -> bool {
        x >= self.left as f32
            && x <= self.right as f32
            && y >= self.top as f32
            && y <= self.bottom as f32
    }

    pub fn to_gdi(self, dpi: u32) -> RECT {
        RECT {
            left: scale(self.left, dpi),
            top: scale(self.top, dpi),
            right: scale(self.right, dpi),
            bottom: scale(self.bottom, dpi),
        }
    }

    pub const fn to_d2d(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.left as f32,
            top: self.top as f32,
            right: self.right as f32,
            bottom: self.bottom as f32,
        }
    }

    pub fn inset_d2d(self, inset: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.left as f32 + inset,
            top: self.top as f32 + inset,
            right: self.right as f32 - inset,
            bottom: self.bottom as f32 - inset,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlyoutDimensions {
    pub width: i32,
    pub height: i32,
}

pub const fn flyout_size(compact: bool) -> FlyoutDimensions {
    if compact {
        FlyoutDimensions { width: COMPACT_CARD_WIDTH, height: COMPACT_CARD_HEIGHT }
    } else {
        FlyoutDimensions { width: CARD_WIDTH, height: CARD_HEIGHT }
    }
}

pub fn flyout_dimensions(state: &DisplayState) -> FlyoutDimensions {
    flyout_size(matches!(state.quota_availability(), QuotaAvailability::Single(_)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaPanelSlot {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaPanelGeometry {
    pub left: i32,
    pub right: i32,
    pub center_x: i32,
}

pub const fn quota_panel_geometry(slot: QuotaPanelSlot) -> QuotaPanelGeometry {
    match slot {
        QuotaPanelSlot::Left => QuotaPanelGeometry { left: 16, right: 184, center_x: 100 },
        QuotaPanelSlot::Right => QuotaPanelGeometry { left: 192, right: 360, center_x: 276 },
    }
}

pub const fn quota_card_logical_rect(slot: QuotaPanelSlot) -> LogicalRect {
    let geometry = quota_panel_geometry(slot);
    LogicalRect::new(geometry.left, QUOTA_CARD_TOP, geometry.right, QUOTA_CARD_BOTTOM)
}

pub fn quota_card_rect(slot: QuotaPanelSlot, dpi: u32) -> RECT {
    quota_card_logical_rect(slot).to_gdi(dpi)
}

pub const fn refresh_button_center(width: i32) -> (i32, i32) {
    (
        width - REFRESH_BUTTON_RIGHT - REFRESH_BUTTON_RADIUS,
        (HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM) / 2,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLayout {
    pub width: i32,
    pub panel_bottom: i32,
    pub next_hit: LogicalRect,
    pub month_grid: LogicalRect,
    pub tabs_top: i32,
    pub tabs_left: i32,
    pub week_tab: LogicalRect,
    pub month_tab: LogicalRect,
}

pub const fn history_layout(compact: bool) -> HistoryLayout {
    let width = flyout_size(compact).width;
    let panel_bottom = if compact { 242 } else { 309 };
    let next_hit = LogicalRect::new(width - 72, HISTORY_NAV_TOP, width - 43, HISTORY_NAV_BOTTOM);
    let month_grid = LogicalRect::new(
        24,
        if compact { 96 } else { 101 },
        width - 24,
        if compact { 235 } else { 302 },
    );
    let tabs_top = if compact { 249 } else { 316 };
    let tabs_left = (width - HISTORY_TABS_WIDTH) / 2;
    let week_tab = LogicalRect::new(
        tabs_left,
        tabs_top,
        tabs_left + HISTORY_TABS_HALF_WIDTH,
        tabs_top + HISTORY_TABS_HEIGHT,
    );
    let month_tab = LogicalRect::new(
        tabs_left + HISTORY_TABS_HALF_WIDTH,
        tabs_top,
        tabs_left + HISTORY_TABS_WIDTH,
        tabs_top + HISTORY_TABS_HEIGHT,
    );
    HistoryLayout {
        width,
        panel_bottom,
        next_hit,
        month_grid,
        tabs_top,
        tabs_left,
        week_tab,
        month_tab,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResetPanelLayout {
    pub width: f32,
    pub height: f32,
    pub open_hit: LogicalRect,
    pub retry_hit: D2D_RECT_F,
    pub use_column_left: f32,
    pub use_column_right: f32,
    pub list_bottom: f32,
    pub confirm_dialog: D2D_RECT_F,
    pub confirm_cancel: D2D_RECT_F,
    pub confirm_ok: D2D_RECT_F,
    pub success_dialog: D2D_RECT_F,
    pub success_dismiss: D2D_RECT_F,
}

pub fn reset_panel_layout(compact: bool) -> ResetPanelLayout {
    let dims = flyout_size(compact);
    let (w, h) = (dims.width as f32, dims.height as f32);
    let open_hit = if compact { COMPACT_RESET_OPEN_HIT } else { FULL_RESET_OPEN_HIT };
    let confirm_top = (h - RESET_CONFIRM_HEIGHT) / 2.0;
    let success_top = (h - RESET_SUCCESS_HEIGHT) / 2.0;
    ResetPanelLayout {
        width: w,
        height: h,
        open_hit,
        retry_hit: D2D_RECT_F { left: w - 150.0, top: 46.0, right: w - 28.0, bottom: 74.0 },
        use_column_left: w - 111.0,
        use_column_right: w - 32.0,
        list_bottom: h - RESET_LIST_BOTTOM_MARGIN,
        confirm_dialog: D2D_RECT_F {
            left: 24.0,
            top: confirm_top,
            right: w - 24.0,
            bottom: confirm_top + RESET_CONFIRM_HEIGHT,
        },
        confirm_cancel: D2D_RECT_F {
            left: 40.0,
            top: confirm_top + 119.0,
            right: w / 2.0 - 5.0,
            bottom: confirm_top + 151.0,
        },
        confirm_ok: D2D_RECT_F {
            left: w / 2.0 + 5.0,
            top: confirm_top + 119.0,
            right: w - 40.0,
            bottom: confirm_top + 151.0,
        },
        success_dialog: D2D_RECT_F {
            left: 34.0,
            top: success_top,
            right: w - 34.0,
            bottom: success_top + RESET_SUCCESS_HEIGHT,
        },
        success_dismiss: D2D_RECT_F {
            left: 60.0,
            top: success_top + 76.0,
            right: w - 60.0,
            bottom: success_top + 108.0,
        },
    }
}

pub fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

pub fn refresh_button_hit_test(state: &DisplayState, x: i32, y: i32, dpi: u32) -> bool {
    let dimensions = flyout_dimensions(state);
    let (cx, cy) = refresh_button_center(dimensions.width);
    let center_x = scale(cx, dpi);
    let center_y = scale(cy, dpi);
    let radius = scale(REFRESH_HIT_RADIUS, dpi);
    x >= center_x - radius
        && x < center_x + radius
        && y >= center_y - radius
        && y < center_y + radius
}

pub fn quota_card_hit_test(state: &DisplayState, x: i32, y: i32, dpi: u32) -> Option<QuotaKind> {
    if matches!(state.quota_availability(), QuotaAvailability::Single(_)) {
        return None;
    }
    [(QuotaKind::FiveHour, QuotaPanelSlot::Left), (QuotaKind::Weekly, QuotaPanelSlot::Right)]
        .into_iter()
        .find_map(|(kind, slot)| {
            let rect = quota_card_rect(slot, dpi);
            (x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom).then_some(kind)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, WEEK_MINUTES};

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
    fn header_text_row_is_centered_on_the_status_accent() {
        assert_eq!(HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM, HEADER_ACCENT_TOP + HEADER_ACCENT_BOTTOM);
        assert_eq!(HEADER_VERSION_TOP, HEADER_TEXT_TOP + 1);
        assert_eq!(HEADER_VERSION_BOTTOM, HEADER_TEXT_BOTTOM + 1);
    }

    #[test]
    fn refresh_hit_target_tracks_compact_and_full_widths() {
        for state in [display(true, false), display(true, true)] {
            let (center_x, center_y) = refresh_button_center(flyout_dimensions(&state).width);
            assert!(refresh_button_hit_test(&state, center_x, center_y, 96));
            assert!(refresh_button_hit_test(&state, center_x - REFRESH_HIT_RADIUS, center_y, 96));
            assert!(!refresh_button_hit_test(&state, center_x + REFRESH_HIT_RADIUS, center_y, 96));
        }
    }

    #[test]
    fn quota_hit_targets_match_both_visible_cards_at_100_percent_dpi() {
        let state = display(true, true);
        assert_eq!(quota_card_hit_test(&state, 16, 40, 96), Some(QuotaKind::FiveHour));
        assert_eq!(quota_card_hit_test(&state, 183, 267, 96), Some(QuotaKind::FiveHour));
        assert_eq!(quota_card_hit_test(&state, 192, 40, 96), Some(QuotaKind::Weekly));
        assert_eq!(quota_card_hit_test(&state, 359, 267, 96), Some(QuotaKind::Weekly));
        assert_eq!(quota_card_hit_test(&state, 184, 100, 96), None);
        assert_eq!(quota_card_hit_test(&state, 191, 100, 96), None);
        assert_eq!(quota_card_hit_test(&state, 16, 268, 96), None);
    }

    #[test]
    fn single_quota_layout_is_compact_and_has_no_switch_hit_target() {
        for state in [display(true, false), display(false, true)] {
            assert_eq!(
                flyout_dimensions(&state),
                FlyoutDimensions { width: COMPACT_CARD_WIDTH, height: COMPACT_CARD_HEIGHT }
            );
            assert_eq!(quota_card_hit_test(&state, 100, 149, 96), None);
        }
        assert_eq!(
            flyout_dimensions(&display(true, true)),
            FlyoutDimensions { width: CARD_WIDTH, height: CARD_HEIGHT }
        );
    }
}
