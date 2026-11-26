use super::tray::quota_switching_available;
use super::*;
use crate::model::{ProviderKind, QuotaKind};
use crate::winutil::wide0;
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SW_SHOWNORMAL, SetForegroundWindow,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_CLOSE,
    WM_NULL,
};
use windows::core::{PCWSTR, w};

const CMD_REFRESH: u32 = 100;
const CMD_USAGE: u32 = 101;
const CMD_PROVIDER_CODEX: u32 = 102;
const CMD_PROVIDER_GEMINI: u32 = 103;
const CMD_DISPLAY_FIVE_HOUR: u32 = 105;
const CMD_DISPLAY_WEEKLY: u32 = 106;
const CMD_INTERVAL_1: u32 = 111;
const CMD_INTERVAL_5: u32 = 115;
const CMD_INTERVAL_15: u32 = 125;
const CMD_ALERT_OFF: u32 = 130;
const CMD_ALERT_10: u32 = 131;
const CMD_ALERT_20: u32 = 132;
const CMD_ALERT_30: u32 = 133;
const CMD_STARTUP: u32 = 140;
const CMD_UPDATE: u32 = 150;
const CMD_THEME_SYSTEM: u32 = 160;
const CMD_THEME_LIGHT: u32 = 161;
const CMD_THEME_DARK: u32 = 162;
const CMD_EXIT: u32 = 199;

const USAGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
const GEMINI_USAGE_URL: &str = "https://aistudio.google.com/usage";

struct MenuHandle(HMENU);

impl MenuHandle {
    fn into_raw(self) -> HMENU {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

impl Drop for MenuHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

pub(super) unsafe fn show_context_menu(state_ptr: *mut AppState) {
    // TrackPopupMenu runs a nested message loop, so no AppState reference may live
    // across it. Build first, then reacquire the state only after the menu closes.
    let (menu, hwnd) = {
        let Some(_state_borrow) = StateBorrow::acquire(state_ptr) else {
            return;
        };
        let state = unsafe { &*state_ptr };
        let Ok(menu) = state.build_menu() else {
            return;
        };
        (menu, state.hwnd)
    };

    let mut point = POINT::default();
    let command = unsafe {
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        )
        .0 as u32;
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        command
    };

    if let Some(_state_borrow) = StateBorrow::acquire(state_ptr) {
        unsafe { (&mut *state_ptr).handle_command(command) };
    }
}

impl AppState {
    fn build_menu(&self) -> windows::core::Result<HMENU> {
        let owned_menu = MenuHandle(unsafe { CreatePopupMenu()? });
        let menu = owned_menu.0;
        let startup_enabled = startup::is_enabled();
        unsafe {
            let allow_provider_switch = !self.reset_panel.busy;
            append_item(
                menu,
                CMD_PROVIDER_CODEX,
                "Codex",
                self.settings.provider == ProviderKind::Codex,
                allow_provider_switch,
            )?;
            append_item(
                menu,
                CMD_PROVIDER_GEMINI,
                "Gemini",
                self.settings.provider == ProviderKind::Gemini,
                allow_provider_switch,
            )?;
            separator(menu)?;
            append(menu, CMD_REFRESH, self.locale.text("Refresh now", "立即刷新"), false)?;
            let usage_label = match self.settings.provider {
                ProviderKind::Codex => self.locale.text("Open Codex usage", "打开 Codex 用量页"),
                ProviderKind::Gemini => self.locale.text("Open Gemini usage", "打开 Gemini 用量页"),
            };
            append(menu, CMD_USAGE, usage_label, false)?;
            separator(menu)?;
            if quota_switching_available(&self.active_slot().display) {
                append(
                    menu,
                    CMD_DISPLAY_FIVE_HOUR,
                    self.locale.text("Show 5-hour quota", "显示5小时额度"),
                    self.settings.display_quota == QuotaKind::FiveHour,
                )?;
                append(
                    menu,
                    CMD_DISPLAY_WEEKLY,
                    self.locale.text("Show weekly quota", "显示周额度"),
                    self.settings.display_quota == QuotaKind::Weekly,
                )?;
                separator(menu)?;
            }

            let refresh_sub = MenuHandle(CreatePopupMenu()?);
            append(
                refresh_sub.0,
                CMD_INTERVAL_1,
                self.locale.text("Refresh every 1 minute", "每 1 分钟刷新"),
                self.settings.refresh_minutes == 1,
            )?;
            append(
                refresh_sub.0,
                CMD_INTERVAL_5,
                self.locale.text("Refresh every 5 minutes", "每 5 分钟刷新"),
                self.settings.refresh_minutes == 5,
            )?;
            append(
                refresh_sub.0,
                CMD_INTERVAL_15,
                self.locale.text("Refresh every 15 minutes", "每 15 分钟刷新"),
                self.settings.refresh_minutes == 15,
            )?;
            separator(refresh_sub.0)?;
            append(
                refresh_sub.0,
                CMD_ALERT_OFF,
                self.locale.text("Low-quota alerts off", "关闭低额度提醒"),
                self.settings.alert_threshold.is_none(),
            )?;
            for (command, threshold) in [(CMD_ALERT_10, 10), (CMD_ALERT_20, 20), (CMD_ALERT_30, 30)]
            {
                let label = match (self.locale, threshold) {
                    (ui::Locale::Chinese, value) => format!("剩余低于 {value}% 时提醒"),
                    (_, value) => format!("Alert below {value}%"),
                };
                append(
                    refresh_sub.0,
                    command,
                    &label,
                    self.settings.alert_threshold == Some(threshold),
                )?;
            }
            append_submenu(menu, refresh_sub, self.locale.text("Refresh & alerts", "刷新与提醒"))?;

            let system_sub = MenuHandle(CreatePopupMenu()?);
            append(
                system_sub.0,
                CMD_THEME_SYSTEM,
                self.locale.text("Theme: system", "主题：跟随系统"),
                self.settings.theme == "system",
            )?;
            append(
                system_sub.0,
                CMD_THEME_LIGHT,
                self.locale.text("Theme: light", "主题：浅色"),
                self.settings.theme == "light",
            )?;
            append(
                system_sub.0,
                CMD_THEME_DARK,
                self.locale.text("Theme: dark", "主题：深色"),
                self.settings.theme == "dark",
            )?;
            separator(system_sub.0)?;
            append(
                system_sub.0,
                CMD_STARTUP,
                self.locale.text("Start with Windows", "开机自动启动"),
                startup_enabled,
            )?;
            append_submenu(
                menu,
                system_sub,
                self.locale.text("Appearance & system", "界面与系统"),
            )?;

            separator(menu)?;
            append(menu, CMD_UPDATE, self.locale.text("Check for updates", "检查更新"), false)?;
            append(
                menu,
                CMD_EXIT,
                self.locale.text("Exit AgentStatus", "退出 AgentStatus"),
                false,
            )?;
        }
        Ok(owned_menu.into_raw())
    }

    pub(super) fn switch_provider(&mut self, next: ProviderKind) {
        if self.settings.provider == next || self.reset_panel.busy {
            return;
        }
        if self.active_slot().display.snapshot.is_some() {
            self.active_slot_mut().last_prefetched = Some(Instant::now());
        }
        self.settings.provider = next;
        self.persist_settings();
        self.reset_panel.close();
        self.history_navigation.page = ui::HistoryPage::Main;
        self.hovered_history_day = None;

        if !self.active_slot().refreshing || self.active_slot().display.snapshot.is_some() {
            self.stop_refresh_feedback();
        }
        self.refresh_history_view_cache();
        let _ = self.update_tray(false);
        self.sync_visible_flyout_layout();
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
        let stale = self.active_slot().display.snapshot.as_ref().is_none_or(|snapshot| {
            let max_age = i64::from(self.settings.refresh_minutes).saturating_mul(60);
            Utc::now().timestamp().saturating_sub(snapshot.fetched_at) >= max_age
        });
        if stale && !self.active_slot().refreshing {
            self.start_refresh(false);
        }
    }

    unsafe fn handle_command(&mut self, command: u32) {
        unsafe {
            match command {
                0 => {}
                CMD_PROVIDER_CODEX => self.switch_provider(ProviderKind::Codex),
                CMD_PROVIDER_GEMINI => self.switch_provider(ProviderKind::Gemini),
                CMD_REFRESH => {
                    self.start_refresh(true);
                    self.prefetch_other_provider(true);
                }
                CMD_USAGE => {
                    let url = match self.settings.provider {
                        ProviderKind::Codex => USAGE_URL,
                        ProviderKind::Gemini => GEMINI_USAGE_URL,
                    };
                    self.open_url(url);
                }
                CMD_UPDATE => self.start_update_check(),
                CMD_DISPLAY_FIVE_HOUR | CMD_DISPLAY_WEEKLY => {
                    let kind = if command == CMD_DISPLAY_WEEKLY {
                        QuotaKind::Weekly
                    } else {
                        QuotaKind::FiveHour
                    };
                    self.select_display_quota(kind);
                }
                CMD_INTERVAL_1 | CMD_INTERVAL_5 | CMD_INTERVAL_15 => {
                    self.settings.refresh_minutes = match command {
                        CMD_INTERVAL_1 => 1,
                        CMD_INTERVAL_15 => 15,
                        _ => 5,
                    };
                    self.persist_settings();
                    self.reset_refresh_timer(self.settings.refresh_minutes * 60_000);
                }
                CMD_ALERT_OFF | CMD_ALERT_10 | CMD_ALERT_20 | CMD_ALERT_30 => {
                    self.settings.alert_threshold = match command {
                        CMD_ALERT_10 => Some(10),
                        CMD_ALERT_20 => Some(20),
                        CMD_ALERT_30 => Some(30),
                        _ => None,
                    };
                    self.settings.last_alert_reset = None;
                    self.settings.last_alert_reset_gemini = None;
                    self.persist_settings();
                    self.maybe_alert();
                }
                CMD_STARTUP => {
                    let result = if startup::is_enabled() {
                        startup::disable()
                    } else {
                        std::env::current_exe().and_then(|path| startup::enable(&path))
                    };
                    if let Err(error) = result {
                        self.show_balloon(
                            self.locale.text("Startup setting failed", "开机启动设置失败"),
                            &error.to_string(),
                        );
                    }
                }
                CMD_THEME_SYSTEM | CMD_THEME_LIGHT | CMD_THEME_DARK => {
                    self.settings.theme = match command {
                        CMD_THEME_LIGHT => "light",
                        CMD_THEME_DARK => "dark",
                        _ => "system",
                    }
                    .to_owned();
                    self.persist_settings();
                    self.theme = ui::detect_theme(&self.settings.theme);
                    ui::configure_flyout(self.flyout, self.theme);
                    let _ = self.update_tray(false);
                    let _ = InvalidateRect(Some(self.flyout), None, true);
                }
                CMD_EXIT => {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                _ => {}
            }
        }
    }

    fn open_url(&self, url: &str) {
        let url = wide0(url);
        let result = unsafe {
            ShellExecuteW(
                Some(self.hwnd),
                w!("open"),
                PCWSTR(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            self.show_balloon(
                self.locale.text("Could not open browser", "无法打开浏览器"),
                self.locale
                    .text("Copy the link from the project README.", "请从项目 README 复制链接。"),
            );
        }
    }
}

unsafe fn append(
    menu: HMENU,
    command: u32,
    label: &str,
    checked: bool,
) -> windows::core::Result<()> {
    unsafe { append_item(menu, command, label, checked, true) }
}

unsafe fn append_item(
    menu: HMENU,
    command: u32,
    label: &str,
    checked: bool,
    enabled: bool,
) -> windows::core::Result<()> {
    unsafe {
        let text = wide0(label);
        let mut flags = MF_STRING;
        if checked {
            flags |= MF_CHECKED;
        }
        if !enabled {
            flags |= MF_GRAYED;
        }
        AppendMenuW(menu, flags, command as usize, PCWSTR(text.as_ptr()))
    }
}

unsafe fn append_submenu(
    parent: HMENU,
    submenu: MenuHandle,
    label: &str,
) -> windows::core::Result<()> {
    unsafe {
        let text = wide0(label);
        let raw = submenu.into_raw();
        if let Err(err) = AppendMenuW(parent, MF_POPUP, raw.0 as usize, PCWSTR(text.as_ptr())) {
            let _ = DestroyMenu(raw);
            return Err(err);
        }
        Ok(())
    }
}

unsafe fn separator(menu: HMENU) -> windows::core::Result<()> {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
}
