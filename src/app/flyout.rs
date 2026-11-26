use super::*;
use crate::model::{ProviderKind, QuotaKind};
use chrono::{Datelike, Local};
use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, VK_BROWSER_BACK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, IsWindowVisible, KillTimer, PostMessageW, SW_HIDE, SWP_NOACTIVATE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    WHEEL_DELTA, WM_APPCOMMAND, WM_KEYDOWN, WM_XBUTTONUP, XBUTTON1,
};

const TRAY_ACTIVATION_DEBOUNCE: Duration = Duration::from_millis(300);
const FLYOUT_ACTIVATION_GUARD: Duration = Duration::from_millis(220);
const TRAY_CLOSE_COALESCE: Duration = Duration::from_millis(250);
const REFRESH_ANIMATION_DURATION: Duration = Duration::from_millis(600);
const HISTORY_WHEEL_THROTTLE: Duration = Duration::from_millis(400);
const APPCOMMAND_BROWSER_BACKWARD_ID: u32 = 1;

#[derive(Default)]
pub(super) struct HistoryWheelState {
    remainder: i32,
    last_triggered: Option<Instant>,
}

impl HistoryWheelState {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn push(&mut self, delta: i32, now: Instant) -> Option<i32> {
        if delta == 0 {
            return None;
        }
        if self.remainder != 0 && self.remainder.signum() != delta.signum() {
            self.remainder = 0;
        }
        if self
            .last_triggered
            .is_some_and(|last| now.saturating_duration_since(last) < HISTORY_WHEEL_THROTTLE)
        {
            self.remainder = 0;
            return None;
        }
        self.remainder = self.remainder.saturating_add(delta);
        if self.remainder.unsigned_abs() < WHEEL_DELTA {
            return None;
        }
        let direction = if self.remainder > 0 { -1 } else { 1 };
        self.remainder = 0;
        Some(direction)
    }

    pub(super) fn commit(&mut self, now: Instant) {
        self.last_triggered = Some(now);
    }
}

pub(super) fn pointer_coordinates(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam.0 as u32;
    let x = i32::from(packed as u16 as i16);
    let y = i32::from((packed >> 16) as u16 as i16);
    (x, y)
}

pub(super) fn standard_back_message(message: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
    match message {
        WM_KEYDOWN => wparam.0 as u16 == VK_BROWSER_BACK.0,
        WM_XBUTTONUP => ((wparam.0 >> 16) & 0xffff) as u16 == XBUTTON1,
        WM_APPCOMMAND => ((lparam.0 as u32 >> 16) & 0x0fff) == APPCOMMAND_BROWSER_BACKWARD_ID,
        _ => false,
    }
}

pub(super) fn wheel_delta(wparam: WPARAM) -> i32 {
    i32::from(((wparam.0 >> 16) as u16) as i16)
}

pub(super) fn refresh_rotation_degrees(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() % REFRESH_ANIMATION_DURATION.as_secs_f32())
        / REFRESH_ANIMATION_DURATION.as_secs_f32()
        * 360.0
}

fn cursor_rect() -> RECT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    RECT { left: point.x, top: point.y, right: point.x + 1, bottom: point.y + 1 }
}

impl AppState {
    pub(super) fn select_display_quota(&mut self, kind: QuotaKind) {
        self.settings.display_quota = kind;
        self.persist_settings();
        let _ = self.update_tray(false);
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    pub(super) fn activate_history_hit(&mut self, hit: ui::HistoryHit) {
        self.history_wheel.reset();
        if self.settings.provider != ProviderKind::Codex {
            return;
        }
        match hit {
            ui::HistoryHit::ToggleResourcePage => {
                if self.settings.provider == ProviderKind::Codex {
                    self.history_navigation.resource_page =
                        self.history_navigation.resource_page.toggle();
                }
                self.reset_panel.pressed = None;
                self.reset_panel.hovered = None;
            }
            ui::HistoryHit::Back => {
                self.history_navigation.page = ui::HistoryPage::Main;
                self.hovered_history_day = None;
            }
            ui::HistoryHit::ToggleSummaryWeek => {
                self.history_navigation.summary_week =
                    self.history_navigation.summary_week.toggle();
            }
            ui::HistoryHit::OpenHistory => {
                self.history_navigation.open_current_week(self.history_view_cache.as_ref());
                self.hovered_history_day = None;
            }
            ui::HistoryHit::PreviousMonth | ui::HistoryHit::PreviousWeek => {
                self.history_navigation.shift_visible_period(-1, Local::now().date_naive());
            }
            ui::HistoryHit::NextMonth | ui::HistoryHit::NextWeek => {
                self.history_navigation.shift_visible_period(1, Local::now().date_naive());
            }
            ui::HistoryHit::MonthTab => {
                self.history_navigation.month =
                    self.history_navigation.selected_week.with_day(1).expect("week month start");
                self.history_navigation.page = ui::HistoryPage::Month;
            }
            ui::HistoryHit::WeekTab => {
                self.history_navigation.open_selected_week();
            }
            ui::HistoryHit::Week(start) => {
                self.history_navigation.selected_week = start;
                self.history_navigation.page = ui::HistoryPage::Week;
                self.hovered_history_day = None;
            }
        }
    }

    pub(super) fn handle_history_wheel(&mut self, delta: i32) -> Option<bool> {
        if self.history_navigation.page == ui::HistoryPage::Main || !self.full_history_render {
            self.history_wheel.reset();
            return None;
        }
        let now = Instant::now();
        let Some(direction) = self.history_wheel.push(delta, now) else {
            return Some(false);
        };
        let today = self
            .history_view_cache
            .as_ref()
            .map(|view| view.today)
            .unwrap_or_else(|| Local::now().date_naive());
        let changed = self.history_navigation.shift_visible_period(direction, today);
        if changed {
            self.history_wheel.commit(now);
            self.hovered_history_day = None;
        }
        Some(changed)
    }

    pub(super) fn start_refresh_animation(&mut self) {
        self.refresh_animation_started = Some(Instant::now());
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH_ANIMATION);
            let timer = SetTimer(
                Some(self.hwnd),
                TIMER_REFRESH_ANIMATION,
                REFRESH_ANIMATION_INTERVAL_MS,
                None,
            );
            if timer == 0 {
                self.refresh_animation_started = None;
            }
        }
    }

    pub(super) fn advance_refresh_animation(&mut self) {
        if self.refresh_animation_started.is_none() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH_ANIMATION);
            }
            return;
        }
        if !self.active_slot().refreshing {
            self.stop_refresh_feedback();
            return;
        }
        if unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            unsafe {
                let _ = InvalidateRect(Some(self.flyout), None, false);
            }
        }
    }

    pub(super) fn refresh_rotation_degrees(&self) -> f32 {
        self.refresh_animation_started
            .map_or(0.0, |started| refresh_rotation_degrees(started.elapsed()))
    }

    pub(super) fn stop_refresh_feedback(&mut self) {
        self.refresh_feedback = false;
        self.refresh_animation_started = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH_ANIMATION);
        }
    }

    pub(super) fn request_toggle_flyout(&mut self) {
        let now = Instant::now();
        if self
            .flyout_hidden_for_tray_activation
            .take()
            .is_some_and(|hidden| now.duration_since(hidden) < TRAY_CLOSE_COALESCE)
        {
            // Clicking the icon transfers focus to Explorer before its tray
            // callback arrives. If that focus loss already hid the card, the
            // callback represents the same click and must not reopen it.
            self.last_tray_activation = Some(now);
            return;
        }
        if self
            .last_tray_activation
            .is_some_and(|previous| now.duration_since(previous) < TRAY_ACTIVATION_DEBOUNCE)
        {
            return;
        }
        self.last_tray_activation = Some(now);
        unsafe {
            // Showing from inside the Explorer callback lets the shell reclaim
            // activation and used to make the card flash closed. Defer it until
            // the notification callback has completely returned.
            let _ = PostMessageW(Some(self.hwnd), WM_TOGGLE_FLYOUT, WPARAM(0), LPARAM(0));
        }
    }

    pub(super) fn toggle_flyout(&mut self) {
        if unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            self.hide_flyout();
        } else {
            self.show_flyout();
        }
    }

    pub(super) fn hide_flyout(&mut self) {
        if !self.reset_panel.busy && !self.reset_panel.success {
            self.reset_panel.close();
        } else if self.reset_panel.success {
            self.reset_panel.success_interactive = false;
        }
        self.flyout_ignore_inactive_until = None;
        self.flyout_hidden_for_tray_activation = None;
        self.pressed_quota = None;
        self.pressed_history = None;
        self.hovered_history_day = None;
        self.hovered_history_values = false;
        self.history_wheel.reset();
        self.history_navigation.page = ui::HistoryPage::Main;
        self.history_navigation.selected_week =
            crate::history::monday_of(Local::now().date_naive());
        self.refresh_button_pressed = false;
        self.refresh_animation_started = None;
        if !self.active_slot().refreshing {
            self.refresh_feedback = false;
        }
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH_ANIMATION);
            let _ = ShowWindow(self.flyout, SW_HIDE);
        }
    }

    pub(super) fn handle_flyout_inactive(&mut self) {
        let guarded =
            self.flyout_ignore_inactive_until.is_some_and(|deadline| Instant::now() < deadline);
        if guarded {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
                let _ = SetTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE, 75, None);
            }
        } else {
            let over_tray_icon = self.cursor_is_over_tray_icon();
            self.hide_flyout();
            if over_tray_icon {
                self.flyout_hidden_for_tray_activation = Some(Instant::now());
            }
        }
    }

    pub(super) fn finish_flyout_activation(&mut self) {
        if !unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            self.flyout_ignore_inactive_until = None;
            return;
        }
        unsafe {
            let _ = SetForegroundWindow(self.flyout);
        }
        self.flyout_ignore_inactive_until = None;
    }

    fn cursor_is_over_tray_icon(&self) -> bool {
        let Some(rect) = self.tray_rect() else {
            return false;
        };
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    }

    pub(super) fn show_flyout(&mut self) {
        self.flyout_hidden_for_tray_activation = None;
        self.theme = ui::detect_theme(&self.settings.theme);
        ui::configure_flyout(self.flyout, self.theme);
        self.position_flyout(true);
        if self.refresh_feedback
            && self.active_slot().refreshing
            && self.refresh_animation_started.is_none()
        {
            self.start_refresh_animation();
        }
    }

    pub(super) fn sync_visible_flyout_layout(&mut self) {
        if !unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(self.flyout) }.max(96);
        let dimensions = ui::flyout_dimensions(&self.active_slot().display);
        let desired_width = ui::scale(dimensions.width, dpi);
        let desired_height = ui::scale(dimensions.height, dpi);
        let mut current = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.flyout, &mut current);
        }
        if current.right - current.left != desired_width
            || current.bottom - current.top != desired_height
        {
            self.position_flyout(false);
        }
    }

    pub(super) fn position_flyout(&mut self, show: bool) {
        let anchor = self.tray_rect().unwrap_or_else(cursor_rect);
        let point =
            POINT { x: (anchor.left + anchor.right) / 2, y: (anchor.top + anchor.bottom) / 2 };
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        let mut info =
            MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(monitor, &mut info);
        }
        let mut monitor_dpi_x = 96;
        let mut monitor_dpi_y = 96;
        let dpi = unsafe {
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut monitor_dpi_x, &mut monitor_dpi_y)
                .is_ok()
            {
                monitor_dpi_x.max(96)
            } else {
                GetDpiForWindow(self.flyout).max(96)
            }
        };
        let dimensions = ui::flyout_dimensions(&self.active_slot().display);
        let width = ui::scale(dimensions.width, dpi);
        let height = ui::scale(dimensions.height, dpi);
        let gap = ui::scale(10, dpi);
        let mut x = point.x - width / 2;
        let mut y = anchor.top - height - gap;
        if y < info.rcWork.top {
            y = anchor.bottom + gap;
        }
        x = x.clamp(info.rcWork.left, (info.rcWork.right - width).max(info.rcWork.left));
        y = y.clamp(info.rcWork.top, (info.rcWork.bottom - height).max(info.rcWork.top));
        self.flyout_ignore_inactive_until = Some(Instant::now() + FLYOUT_ACTIVATION_GUARD);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
            let flags = if show { SWP_SHOWWINDOW } else { SWP_NOACTIVATE | SWP_NOZORDER };
            let _ = SetWindowPos(self.flyout, None, x, y, width, height, flags);
            ui::apply_flyout_shape(self.flyout, dpi);
            if show {
                let _ = SetForegroundWindow(self.flyout);
            }
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;

    #[test]
    fn pointer_coordinates_sign_extend_both_axes() {
        let packed = u32::from(0xfffe_u16) | (u32::from(0xfffd_u16) << 16);
        assert_eq!(pointer_coordinates(LPARAM(packed as isize)), (-2, -3));
    }

    #[test]
    fn history_wheel_accumulates_a_notch_and_throttles_momentum() {
        let start = Instant::now();
        let mut wheel = HistoryWheelState::default();
        assert_eq!(wheel.push(60, start), None);
        assert_eq!(wheel.push(60, start), Some(-1));
        wheel.commit(start);
        assert_eq!(wheel.push(-120, start + Duration::from_millis(399)), None);
        assert_eq!(wheel.push(-120, start + Duration::from_millis(400)), Some(1));
    }

    #[test]
    fn history_wheel_discards_partial_motion_when_direction_changes() {
        let start = Instant::now();
        let mut wheel = HistoryWheelState::default();
        assert_eq!(wheel.push(80, start), None);
        assert_eq!(wheel.push(-80, start), None);
        assert_eq!(wheel.push(-40, start), Some(1));
    }

    #[test]
    fn uncommitted_boundary_motion_does_not_throttle_the_reverse_direction() {
        let start = Instant::now();
        let mut wheel = HistoryWheelState::default();
        assert_eq!(wheel.push(-120, start), Some(1));
        assert_eq!(wheel.push(120, start + Duration::from_millis(1)), Some(-1));
    }

    #[test]
    fn standard_windows_back_inputs_are_recognized() {
        assert!(standard_back_message(WM_KEYDOWN, WPARAM(VK_BROWSER_BACK.0 as usize), LPARAM(0)));
        assert!(standard_back_message(
            WM_XBUTTONUP,
            WPARAM(usize::from(XBUTTON1) << 16),
            LPARAM(0)
        ));
        assert!(standard_back_message(
            WM_APPCOMMAND,
            WPARAM(0),
            LPARAM((APPCOMMAND_BROWSER_BACKWARD_ID as isize) << 16)
        ));
        assert!(!standard_back_message(WM_KEYDOWN, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0)));
    }

    #[test]
    fn mouse_wheel_delta_is_sign_extended() {
        assert_eq!(wheel_delta(WPARAM(120_usize << 16)), 120);
        assert_eq!(wheel_delta(WPARAM((u16::MAX as usize - 119) << 16)), -120);
    }

    #[test]
    fn refresh_rotation_repeats_every_animation_cycle() {
        for (elapsed, expected) in [
            (Duration::ZERO, 0.0),
            (Duration::from_millis(300), 180.0),
            (Duration::from_millis(600), 0.0),
            (Duration::from_millis(900), 180.0),
        ] {
            assert!((refresh_rotation_degrees(elapsed) - expected).abs() < 0.001);
        }
    }
}
