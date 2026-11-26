use super::*;
use crate::icon::{create_icon, tone_for};
use crate::model::{DisplayState, ProviderKind, QuotaAvailability, QuotaKind, RefreshState};
use windows::Win32::Foundation::{GetLastError, RECT, SetLastError, WIN32_ERROR};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
#[cfg(any(codex_status_channel = "stable", codex_status_channel = "beta"))]
use windows::Win32::UI::Shell::NIF_GUID;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TrayQuotaState {
    pub(super) primary_kind: QuotaKind,
    pub(super) primary_percent: Option<u8>,
    pub(super) indicator_percent: Option<u8>,
    pub(super) refresh_state: RefreshState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LowQuotaAlert {
    pub(super) kind: QuotaKind,
    pub(super) percent: u8,
    pub(super) reset: Option<i64>,
}

pub(super) fn tray_quota_state(display: &DisplayState, preferred: QuotaKind) -> TrayQuotaState {
    let primary_kind = display.resolved_quota_kind(preferred);
    let primary_percent = display.quota_percent(primary_kind);
    let indicator_percent = match display.quota_availability() {
        QuotaAvailability::Single(_) => primary_percent,
        QuotaAvailability::Both => display.quota_percent(primary_kind.other()),
        QuotaAvailability::None => None,
    };
    TrayQuotaState {
        primary_kind,
        primary_percent,
        indicator_percent,
        refresh_state: display.refresh_state,
    }
}

pub(super) fn quota_switching_available(display: &DisplayState) -> bool {
    !matches!(display.quota_availability(), QuotaAvailability::Single(_))
}

pub(super) fn quota_alert_state(
    display: &DisplayState,
    preferred: QuotaKind,
) -> Option<LowQuotaAlert> {
    let kind = display.resolved_quota_kind(preferred);
    let window = display.quota_window(kind)?;
    Some(LowQuotaAlert { kind, percent: window.display_percent(), reset: window.resets_at })
}

pub(super) fn alert_cycle_key(alert: LowQuotaAlert) -> i64 {
    alert.reset.unwrap_or(match alert.kind {
        QuotaKind::FiveHour => i64::MIN,
        QuotaKind::Weekly => i64::MIN + 1,
    })
}

pub(super) fn copy_utf16<const N: usize>(target: &mut [u16; N], value: &str) {
    target.fill(0);
    for (destination, source) in
        target.iter_mut().take(N.saturating_sub(1)).zip(value.encode_utf16())
    {
        *destination = source;
    }
}

impl AppState {
    pub(super) fn update_tray(&mut self, force_add: bool) -> bool {
        diagnostic("tray:render");
        let dpi = unsafe { GetDpiForSystem().max(96) };
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32 };
        let quota_state = self.current_tray_quota_state();
        let display = &self.active_slot().display;
        let icon = match create_icon(
            quota_state.primary_percent,
            quota_state.indicator_percent,
            tone_for(display, quota_state.primary_percent),
            size,
            self.theme.high_contrast,
            self.theme.tray_dark,
        ) {
            Ok(icon) => icon,
            Err(error) => {
                self.tray_added = false;
                self.record_tray_failure("render", error.code().0 as u32, &error.to_string());
                return false;
            }
        };
        let mut data = self.notify_data();
        data.uFlags |= NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon.handle();
        copy_utf16(
            &mut data.szTip,
            &ui::tooltip(&self.active_slot().display, self.settings.display_quota, self.locale),
        );
        let add = force_add || !self.tray_added;
        let operation = if add { NIM_ADD } else { NIM_MODIFY };
        diagnostic(if add { "tray:add" } else { "tray:modify" });
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        let mut succeeded = unsafe { Shell_NotifyIconW(operation, &data) }.as_bool();
        if add && !succeeded {
            // An abnormal termination can leave the old icon active briefly even though this
            // channel's single-instance mutex is now unowned. Use the same tray identity for
            // deletion and addition; a GUID bound to a different path remains protected.
            diagnostic("tray:recover-after-unclean-exit");
            let old = self.notify_data();
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &old) };
            unsafe {
                SetLastError(WIN32_ERROR(0));
            }
            succeeded = unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool();
        }
        if !succeeded {
            let last_error = unsafe { GetLastError() };
            self.tray_added = false;
            diagnostic("tray:failed");
            self.record_tray_failure(
                if add { "NIM_ADD" } else { "NIM_MODIFY" },
                last_error.0,
                "Shell_NotifyIconW returned FALSE",
            );
            return false;
        }
        diagnostic("tray:ok");
        self.tray_failure_logged = false;
        self.tray_quota_state = Some(quota_state);
        self.tray_icon = Some(icon);
        if add {
            self.tray_added = true;
            let mut version = self.notify_data();
            version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            unsafe {
                SetLastError(WIN32_ERROR(0));
            }
            if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &version) }.as_bool() {
                let last_error = unsafe { GetLastError() };
                self.record_tray_failure(
                    "NIM_SETVERSION",
                    last_error.0,
                    "Shell_NotifyIconW returned FALSE",
                );
            }
            if !self.settings.onboarding_shown {
                self.show_balloon(
                    self.locale.text("AgentStatus is ready", "AgentStatus 已就绪"),
                    self.locale.text(
                        "Your selected quota is shown in the tray. Drag the icon out of the overflow area to keep it visible.",
                        "你选择的额度会直接显示在托盘图标中。可将图标从折叠区拖出，保持常显。",
                    ),
                );
                self.settings.onboarding_shown = true;
                self.persist_internal_settings();
            }
        }
        true
    }

    pub(super) fn current_tray_quota_state(&self) -> TrayQuotaState {
        tray_quota_state(&self.active_slot().display, self.settings.display_quota)
    }

    pub(super) fn notify_data(&self) -> NOTIFYICONDATAW {
        #[cfg(any(codex_status_channel = "portable", codex_status_channel = "development"))]
        {
            NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ID,
                ..Default::default()
            }
        }
        #[cfg(any(codex_status_channel = "stable", codex_status_channel = "beta"))]
        {
            NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ID,
                guidItem: TRAY_GUID,
                uFlags: NIF_GUID,
                ..Default::default()
            }
        }
    }

    pub(super) fn tray_rect(&self) -> Option<RECT> {
        #[cfg(any(codex_status_channel = "portable", codex_status_channel = "development"))]
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        #[cfg(any(codex_status_channel = "stable", codex_status_channel = "beta"))]
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            guidItem: TRAY_GUID,
        };
        unsafe { Shell_NotifyIconGetRect(&identifier).ok() }
    }

    pub(super) fn record_tray_failure(&mut self, operation: &str, win32_error: u32, detail: &str) {
        if self.tray_failure_logged {
            return;
        }
        let executable = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "<unavailable>".to_owned());
        let entry = format!(
            "{} channel={} operation={} exe={executable:?} identity={} pid={} win32_error={} detail={detail:?}",
            Utc::now().to_rfc3339(),
            CHANNEL_NAME,
            operation,
            TRAY_IDENTITY_TEXT,
            std::process::id(),
            win32_error,
        );
        let _ = self.store.append_tray_error(&entry);
        self.tray_failure_logged = true;
    }

    pub(super) fn show_balloon(&self, title: &str, body: &str) {
        if !self.tray_added {
            return;
        }
        let mut data = self.notify_data();
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        copy_utf16(&mut data.szInfoTitle, title);
        copy_utf16(&mut data.szInfo, body);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub(super) fn maybe_alert(&mut self) {
        let Some(threshold) = self.settings.alert_threshold else {
            return;
        };
        let Some(alert) =
            quota_alert_state(&self.active_slot().display, self.settings.display_quota)
        else {
            return;
        };
        let alert_key = alert_cycle_key(alert);
        let provider = self.settings.provider;
        if alert.percent >= threshold {
            // A quota window without a reset timestamp uses a stable synthetic
            // key. Clear it after recovery so a later low-quota event can alert
            // again instead of being suppressed forever.
            if alert.reset.is_none()
                && self.settings.last_alert_reset_for(provider) == Some(alert_key)
            {
                self.settings.set_last_alert_reset_for(provider, None);
                self.persist_internal_settings();
            }
            return;
        }
        if self.settings.last_alert_reset_for(provider) == Some(alert_key) {
            return;
        }
        let quota_label = match (self.locale, alert.kind) {
            (ui::Locale::Chinese, QuotaKind::FiveHour) => "5 小时剩余：",
            (ui::Locale::Chinese, QuotaKind::Weekly) => "本周剩余：",
            (_, QuotaKind::FiveHour) => "5-hour remaining:",
            (_, QuotaKind::Weekly) => "Weekly remaining:",
        };
        let title = match provider {
            ProviderKind::Codex => self.locale.text("Codex quota is running low", "Codex 额度偏低"),
            ProviderKind::Gemini => {
                self.locale.text("Gemini quota is running low", "Gemini 额度偏低")
            }
        };
        self.show_balloon(title, &format!("{quota_label} {}%", alert.percent));
        self.settings.set_last_alert_reset_for(provider, Some(alert_key));
        self.persist_internal_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, WEEK_MINUTES};

    fn display_with_quotas(weekly: Option<u8>, five_hour: Option<u8>) -> DisplayState {
        let window = |percent: u8, window_minutes| QuotaWindow {
            used_percent: f64::from(100 - percent),
            remaining_percent: f64::from(percent),
            window_minutes,
            resets_at: Some(i64::MAX),
        };
        DisplayState::live(QuotaSnapshot {
            weekly: weekly.map(|percent| window(percent, WEEK_MINUTES)),
            session: five_hour.map(|percent| window(percent, SESSION_MINUTES)),
            account: AccountSummary::default(),
            fetched_at: 0,
        })
    }

    #[test]
    fn utf16_copy_always_reserves_a_terminator() {
        let mut target = [9_u16; 4];
        copy_utf16(&mut target, "abcdef");
        assert_eq!(target[3], 0);
    }

    #[test]
    fn tray_indicator_uses_weekly_quota_when_five_hour_is_unavailable() {
        let display = display_with_quotas(Some(66), None);
        for preferred in [QuotaKind::FiveHour, QuotaKind::Weekly] {
            let state = tray_quota_state(&display, preferred);
            assert_eq!(state.primary_kind, QuotaKind::Weekly);
            assert_eq!(state.primary_percent, Some(66));
            assert_eq!(state.indicator_percent, Some(66));
        }
        assert!(!quota_switching_available(&display));
    }

    #[test]
    fn tray_indicator_uses_five_hour_quota_when_weekly_is_unavailable() {
        let display = display_with_quotas(None, Some(85));
        for preferred in [QuotaKind::FiveHour, QuotaKind::Weekly] {
            let state = tray_quota_state(&display, preferred);
            assert_eq!(state.primary_kind, QuotaKind::FiveHour);
            assert_eq!(state.primary_percent, Some(85));
            assert_eq!(state.indicator_percent, Some(85));
        }
        assert!(!quota_switching_available(&display));
    }

    #[test]
    fn tray_indicator_uses_the_other_quota_when_both_are_available() {
        let display = display_with_quotas(Some(66), Some(85));
        let five_hour = tray_quota_state(&display, QuotaKind::FiveHour);
        assert_eq!(five_hour.primary_percent, Some(85));
        assert_eq!(five_hour.indicator_percent, Some(66));

        let weekly = tray_quota_state(&display, QuotaKind::Weekly);
        assert_eq!(weekly.primary_percent, Some(66));
        assert_eq!(weekly.indicator_percent, Some(85));
        assert!(quota_switching_available(&display));
    }

    #[test]
    fn tray_has_no_quota_values_when_both_are_unavailable() {
        let display = display_with_quotas(None, None);
        let state = tray_quota_state(&display, QuotaKind::Weekly);
        assert_eq!(state.primary_kind, QuotaKind::Weekly);
        assert_eq!(state.primary_percent, None);
        assert_eq!(state.indicator_percent, None);
        assert!(quota_switching_available(&display));
    }

    #[test]
    fn low_quota_alert_follows_selection_or_the_only_available_quota() {
        let both = display_with_quotas(Some(18), Some(9));
        assert_eq!(
            quota_alert_state(&both, QuotaKind::FiveHour).map(|alert| alert.kind),
            Some(QuotaKind::FiveHour)
        );
        assert_eq!(
            quota_alert_state(&both, QuotaKind::Weekly).map(|alert| alert.kind),
            Some(QuotaKind::Weekly)
        );

        let five_only = display_with_quotas(None, Some(9));
        assert_eq!(
            quota_alert_state(&five_only, QuotaKind::Weekly).map(|alert| alert.kind),
            Some(QuotaKind::FiveHour)
        );
        let weekly_only = display_with_quotas(Some(18), None);
        assert_eq!(
            quota_alert_state(&weekly_only, QuotaKind::FiveHour).map(|alert| alert.kind),
            Some(QuotaKind::Weekly)
        );
    }

    #[test]
    fn low_quota_alert_requires_remaining_quota_to_be_strictly_below_threshold() {
        let display = display_with_quotas(Some(20), None);
        let alert = quota_alert_state(&display, QuotaKind::Weekly).unwrap();
        assert!(!(alert.percent < 20));
        assert!(alert.percent < 30);
    }

    #[test]
    fn missing_reset_alert_keys_are_stable_and_distinct_by_quota() {
        let five_hour = LowQuotaAlert { kind: QuotaKind::FiveHour, percent: 9, reset: None };
        let weekly = LowQuotaAlert { kind: QuotaKind::Weekly, percent: 9, reset: None };
        assert_eq!(alert_cycle_key(five_hour), alert_cycle_key(five_hour));
        assert_ne!(alert_cycle_key(five_hour), alert_cycle_key(weekly));
    }
}
