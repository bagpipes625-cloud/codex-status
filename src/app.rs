use crate::app_server::AppServerClient;
use crate::icon::{OwnedIcon, create_icon, tone_for};
use crate::model::{DisplayState, QuotaSnapshot, RefreshState};
use crate::settings::{AppStore, Settings};
use crate::{startup, ui};
use chrono::Utc;
use std::cell::Cell;
use std::mem::size_of;
use std::ptr;
use std::thread;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    POINT, RECT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HBRUSH, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Shell::{
    NIF_GUID, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
    NIIF_RESPECT_QUIET_TIME, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect,
    Shell_NotifyIconW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetMessageW, HMENU, IDC_ARROW, IsWindowVisible, KillTimer, LoadCursorW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SM_CXSMICON, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WA_INACTIVE,
    WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_NULL, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_OVERLAPPED, WS_POPUP,
};
use windows::core::{GUID, PCWSTR, w};

const MAIN_CLASS: PCWSTR = w!("CodexStatus.MainWindow.v1");
const FLYOUT_CLASS: PCWSTR = w!("CodexStatus.FlyoutWindow.v1");
const MUTEX_NAME: PCWSTR = w!("Local\\CodexStatus.4B7D5A91-45A5-4B78-A095-A9B43A2A4F7D");
const TRAY_GUID: GUID = GUID::from_u128(0x7a89d848_0611_4cb4_98c9_88ca9b59ff84);
const TRAY_ID: u32 = 1;

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH_COMPLETE: u32 = WM_APP + 2;
const WM_SHOW_EXISTING: u32 = WM_APP + 3;

const TIMER_REFRESH: usize = 1;
const TIMER_STARTUP: usize = 2;
const TIMER_CARD: usize = 3;

const CMD_REFRESH: u32 = 100;
const CMD_USAGE: u32 = 101;
const CMD_INTERVAL_1: u32 = 111;
const CMD_INTERVAL_5: u32 = 115;
const CMD_INTERVAL_15: u32 = 125;
const CMD_ALERT_OFF: u32 = 130;
const CMD_ALERT_10: u32 = 131;
const CMD_ALERT_20: u32 = 132;
const CMD_ALERT_30: u32 = 133;
const CMD_STARTUP: u32 = 140;
const CMD_RELEASES: u32 = 150;
const CMD_EXIT: u32 = 199;

const USAGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
const RELEASES_URL: &str = "https://github.com/mmm1h/codex-status/releases";

thread_local! {
    static STATE: Cell<*mut AppState> = const { Cell::new(ptr::null_mut()) };
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("CodexStatus only supports the --background option")]
    InvalidArguments,
    #[error("Windows could not initialize CodexStatus: {0}")]
    Windows(#[from] windows::core::Error),
}

struct InstanceHandle(HANDLE);

impl Drop for InstanceHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct RefreshOutcome {
    result: Result<QuotaSnapshot, String>,
}

struct AppState {
    hwnd: HWND,
    flyout: HWND,
    taskbar_created: u32,
    store: AppStore,
    settings: Settings,
    locale: ui::Locale,
    theme: ui::Theme,
    display: DisplayState,
    client: AppServerClient,
    tray_icon: Option<OwnedIcon>,
    tray_added: bool,
    refreshing: bool,
    refresh_pending: bool,
    failures: u8,
}

pub fn run() -> Result<(), AppError> {
    diagnostic("run:enter");
    let background = parse_arguments()?;
    diagnostic("run:arguments");
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    let mutex = unsafe { InstanceHandle(CreateMutexW(None, false, MUTEX_NAME)?) };
    let mutex_was_existing = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
    diagnostic("run:mutex");
    if mutex_was_existing {
        if let Ok(existing) = unsafe { FindWindowW(MAIN_CLASS, PCWSTR::null()) } {
            unsafe {
                let _ = PostMessageW(Some(existing), WM_SHOW_EXISTING, WPARAM(0), LPARAM(0));
            }
        }
        return Ok(());
    }

    let instance = unsafe { HINSTANCE(GetModuleHandleW(None)?.0) };
    register_classes(instance)?;
    diagnostic("run:classes");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            MAIN_CLASS,
            w!("CodexStatus"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?
    };
    let flyout = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0),
            FLYOUT_CLASS,
            w!("CodexStatus"),
            WS_POPUP,
            0,
            0,
            ui::CARD_WIDTH,
            ui::CARD_HEIGHT,
            Some(hwnd),
            None,
            Some(instance),
            None,
        )?
    };
    diagnostic("run:windows");

    let store = AppStore::discover();
    let settings = store.load_settings();
    let locale = ui::Locale::detect(&settings.locale);
    let now = Utc::now().timestamp();
    let cached = store.load_snapshot().filter(|snapshot| snapshot.is_cache_valid(now));
    let display = DisplayState::loading(cached);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    let state = Box::new(AppState {
        hwnd,
        flyout,
        taskbar_created,
        store,
        settings,
        locale,
        theme: ui::detect_theme(),
        display,
        client: AppServerClient::new(),
        tray_icon: None,
        tray_added: false,
        refreshing: false,
        refresh_pending: false,
        failures: 0,
    });
    let raw = Box::into_raw(state);
    STATE.with(|slot| slot.set(raw));
    diagnostic("run:state");

    let initialization = unsafe {
        let state = &mut *raw;
        ui::configure_flyout(state.flyout, state.theme);
        diagnostic("run:dwm");
        state.update_tray(true)
    };
    diagnostic("run:tray-returned");
    if let Err(error) = initialization {
        STATE.with(|slot| slot.set(ptr::null_mut()));
        unsafe {
            drop(Box::from_raw(raw));
        }
        return Err(error.into());
    }

    unsafe {
        let state = &mut *raw;
        state.reset_refresh_timer(state.settings.refresh_minutes.saturating_mul(60_000));
        if background {
            let _ = SetTimer(Some(hwnd), TIMER_STARTUP, 30_000, None);
        } else {
            state.start_refresh(false);
        }
    }
    diagnostic("run:message-loop");

    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    STATE.with(|slot| slot.set(ptr::null_mut()));
    unsafe {
        drop(Box::from_raw(raw));
    }
    drop(mutex);
    Ok(())
}

fn parse_arguments() -> Result<bool, AppError> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [] => Ok(false),
        [argument] if argument == "--background" => Ok(true),
        _ => Err(AppError::InvalidArguments),
    }
}

fn register_classes(instance: HINSTANCE) -> windows::core::Result<()> {
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let main = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            hInstance: instance,
            lpszClassName: MAIN_CLASS,
            lpfnWndProc: Some(main_window_proc),
            hCursor: cursor,
            ..Default::default()
        };
        if RegisterClassExW(&main) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let flyout = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
            hInstance: instance,
            lpszClassName: FLYOUT_CLASS,
            lpfnWndProc: Some(flyout_window_proc),
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        if RegisterClassExW(&flyout) == 0 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

unsafe extern "system" fn main_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = STATE.with(Cell::get);
        if !state_ptr.is_null() {
            let state = &mut *state_ptr;
            if message == state.taskbar_created {
                state.tray_added = false;
                let _ = state.update_tray(true);
                return LRESULT(0);
            }
            match message {
                WM_TRAY => {
                    let event = lparam.0 as u32 & 0xffff;
                    match event {
                        WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_SELECT => state.toggle_flyout(),
                        WM_RBUTTONUP | WM_CONTEXTMENU => state.show_menu(),
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_REFRESH_COMPLETE => {
                    if lparam.0 != 0 {
                        let outcome = *Box::from_raw(lparam.0 as *mut RefreshOutcome);
                        state.finish_refresh(outcome);
                    }
                    return LRESULT(0);
                }
                WM_SHOW_EXISTING => {
                    state.show_flyout();
                    return LRESULT(0);
                }
                WM_TIMER => {
                    match wparam.0 {
                        TIMER_REFRESH => state.start_refresh(false),
                        TIMER_STARTUP => {
                            let _ = KillTimer(Some(hwnd), TIMER_STARTUP);
                            state.start_refresh(false);
                        }
                        TIMER_CARD => {
                            state.expire_cache_if_needed();
                            let _ = InvalidateRect(Some(state.flyout), None, false);
                        }
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                    state.theme = ui::detect_theme();
                    ui::configure_flyout(state.flyout, state.theme);
                    let _ = state.update_tray(false);
                    let _ = InvalidateRect(Some(state.flyout), None, true);
                    return LRESULT(0);
                }
                WM_QUERYENDSESSION => return LRESULT(1),
                WM_ENDSESSION if wparam.0 != 0 => {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

unsafe extern "system" fn flyout_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = STATE.with(Cell::get);
        match message {
            WM_PAINT if !state_ptr.is_null() => {
                let state = &*state_ptr;
                ui::paint_card(hwnd, &state.display, state.locale, state.theme);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_ACTIVATE if (wparam.0 as u32 & 0xffff) == WA_INACTIVE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let _ = KillTimer(Some(state.hwnd), TIMER_CARD);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return LRESULT(0);
            }
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

impl AppState {
    fn start_refresh(&mut self, force: bool) {
        diagnostic("refresh:start");
        if self.refreshing {
            self.refresh_pending |= force;
            return;
        }
        self.refreshing = true;
        self.display.error = None;
        if self.display.snapshot.is_none() {
            self.display.refresh_state = RefreshState::Loading;
        }
        let _ = self.update_tray(false);
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }

        let hwnd_value = self.hwnd.0 as isize;
        let client = self.client.clone();
        let spawn_result = thread::Builder::new()
            .name("codex-status-refresh".to_owned())
            .stack_size(512 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
                diagnostic("refresh:worker");
                let outcome =
                    RefreshOutcome { result: client.fetch().map_err(|error| error.to_string()) };
                diagnostic(if outcome.result.is_ok() {
                    "refresh:success"
                } else {
                    "refresh:error"
                });
                let raw = Box::into_raw(Box::new(outcome));
                let posted = unsafe {
                    PostMessageW(Some(hwnd), WM_REFRESH_COMPLETE, WPARAM(0), LPARAM(raw as isize))
                };
                if posted.is_err() {
                    unsafe {
                        drop(Box::from_raw(raw));
                    }
                }
            });
        if let Err(error) = spawn_result {
            self.finish_refresh(RefreshOutcome {
                result: Err(format!("Could not start refresh: {error}")),
            });
        }
    }

    fn finish_refresh(&mut self, outcome: RefreshOutcome) {
        self.refreshing = false;
        match outcome.result {
            Ok(snapshot) => {
                self.failures = 0;
                let _ = self.store.save_snapshot(&snapshot);
                self.display = DisplayState::live(snapshot);
                self.reset_refresh_timer(self.settings.refresh_minutes.saturating_mul(60_000));
                self.maybe_alert();
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                let now = Utc::now().timestamp();
                let snapshot = self.display.snapshot.take();
                self.display =
                    DisplayState::after_error(snapshot, friendly_error(&error, self.locale), now);
                let backoff = match self.failures {
                    1 => 60_000,
                    2 => 5 * 60_000,
                    _ => 15 * 60_000,
                };
                self.reset_refresh_timer(backoff);
            }
        }
        let _ = self.update_tray(false);
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
        if self.refresh_pending {
            self.refresh_pending = false;
            self.start_refresh(true);
        }
    }

    fn expire_cache_if_needed(&mut self) {
        if self.display.refresh_state == RefreshState::Live {
            return;
        }
        let now = Utc::now().timestamp();
        if self.display.snapshot.as_ref().is_some_and(|value| !value.is_cache_valid(now)) {
            self.display.snapshot = None;
            self.display.refresh_state = RefreshState::Unavailable;
            let _ = self.update_tray(false);
        }
    }

    fn reset_refresh_timer(&self, milliseconds: u32) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
            let _ = SetTimer(Some(self.hwnd), TIMER_REFRESH, milliseconds.max(1_000), None);
        }
    }

    fn update_tray(&mut self, force_add: bool) -> windows::core::Result<()> {
        diagnostic("tray:render");
        let dpi = unsafe { GetDpiForSystem().max(96) };
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32 };
        let icon = create_icon(
            self.display.weekly_percent(),
            tone_for(&self.display),
            size,
            self.theme.high_contrast,
        )?;
        let mut data = self.notify_data();
        data.uFlags = NIF_GUID | NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon.handle();
        copy_utf16(&mut data.szTip, &ui::tooltip(&self.display, self.locale));
        let add = force_add || !self.tray_added;
        let operation = if add { NIM_ADD } else { NIM_MODIFY };
        diagnostic(if add { "tray:add" } else { "tray:modify" });
        if !unsafe { Shell_NotifyIconW(operation, &data) }.as_bool() {
            diagnostic("tray:failed");
            return Err(windows::core::Error::from_thread());
        }
        diagnostic("tray:ok");
        self.tray_icon = Some(icon);
        if add {
            self.tray_added = true;
            let mut version = self.notify_data();
            version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &version) };
            if !self.settings.onboarding_shown {
                self.show_balloon(
                    self.locale.text("CodexStatus is ready", "CodexStatus 已就绪"),
                    self.locale.text(
                        "Your weekly quota is shown in the tray. Drag the icon out of the overflow area to keep it visible.",
                        "周剩余额度会直接显示在托盘图标中。可将图标从折叠区拖出，保持常显。",
                    ),
                );
                self.settings.onboarding_shown = true;
                let _ = self.store.save_settings(&self.settings);
            }
        }
        Ok(())
    }

    fn notify_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            guidItem: TRAY_GUID,
            uFlags: NIF_GUID,
            ..Default::default()
        }
    }

    fn show_balloon(&self, title: &str, body: &str) {
        if !self.tray_added {
            return;
        }
        let mut data = self.notify_data();
        data.uFlags = NIF_GUID | NIF_INFO;
        data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        copy_utf16(&mut data.szInfoTitle, title);
        copy_utf16(&mut data.szInfo, body);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn maybe_alert(&mut self) {
        let Some(threshold) = self.settings.alert_threshold else {
            return;
        };
        let Some(snapshot) = self.display.snapshot.as_ref() else {
            return;
        };
        let Some(weekly) = snapshot.weekly.as_ref() else {
            return;
        };
        if weekly.display_percent() > threshold {
            return;
        }
        let reset = weekly.resets_at.unwrap_or(0);
        if self.settings.last_alert_reset == Some(reset) {
            return;
        }
        self.show_balloon(
            self.locale.text("Codex quota is running low", "Codex 周额度偏低"),
            &format!(
                "{} {}%",
                self.locale.text("Weekly remaining:", "本周剩余："),
                weekly.display_percent()
            ),
        );
        self.settings.last_alert_reset = Some(reset);
        let _ = self.store.save_settings(&self.settings);
    }

    fn toggle_flyout(&mut self) {
        if unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            unsafe {
                let _ = ShowWindow(self.flyout, SW_HIDE);
                let _ = KillTimer(Some(self.hwnd), TIMER_CARD);
            }
        } else {
            self.show_flyout();
        }
    }

    fn show_flyout(&mut self) {
        self.theme = ui::detect_theme();
        ui::configure_flyout(self.flyout, self.theme);
        let anchor = self.tray_rect().unwrap_or_else(cursor_rect);
        let point =
            POINT { x: (anchor.left + anchor.right) / 2, y: (anchor.top + anchor.bottom) / 2 };
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        let mut info =
            MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(monitor, &mut info);
        }
        let dpi = unsafe { GetDpiForWindow(self.flyout).max(GetDpiForSystem()).max(96) };
        let width = ui::scale(ui::CARD_WIDTH, dpi);
        let height = ui::scale(ui::CARD_HEIGHT, dpi);
        let gap = ui::scale(10, dpi);
        let mut x = point.x - width / 2;
        let mut y = anchor.top - height - gap;
        if y < info.rcWork.top {
            y = anchor.bottom + gap;
        }
        x = x.clamp(info.rcWork.left, (info.rcWork.right - width).max(info.rcWork.left));
        y = y.clamp(info.rcWork.top, (info.rcWork.bottom - height).max(info.rcWork.top));
        unsafe {
            let _ = SetWindowPos(self.flyout, None, x, y, width, height, SWP_SHOWWINDOW);
            let _ = ShowWindow(self.flyout, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.flyout);
            let _ = SetTimer(Some(self.hwnd), TIMER_CARD, 30_000, None);
            let _ = InvalidateRect(Some(self.flyout), None, true);
        }
    }

    fn tray_rect(&self) -> Option<RECT> {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            guidItem: TRAY_GUID,
        };
        unsafe { Shell_NotifyIconGetRect(&identifier).ok() }
    }

    fn show_menu(&mut self) {
        let Ok(menu) = self.build_menu() else {
            return;
        };
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            )
            .0 as u32;
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            self.handle_command(command);
        }
    }

    fn build_menu(&self) -> windows::core::Result<HMENU> {
        let menu = unsafe { CreatePopupMenu()? };
        let startup_enabled = startup::is_enabled();
        unsafe {
            append(menu, CMD_REFRESH, self.locale.text("Refresh now", "立即刷新"), false)?;
            append(
                menu,
                CMD_USAGE,
                self.locale.text("Open Codex usage", "打开 Codex 用量页"),
                false,
            )?;
            separator(menu)?;
            append(
                menu,
                CMD_INTERVAL_1,
                self.locale.text("Refresh every 1 minute", "每 1 分钟刷新"),
                self.settings.refresh_minutes == 1,
            )?;
            append(
                menu,
                CMD_INTERVAL_5,
                self.locale.text("Refresh every 5 minutes", "每 5 分钟刷新"),
                self.settings.refresh_minutes == 5,
            )?;
            append(
                menu,
                CMD_INTERVAL_15,
                self.locale.text("Refresh every 15 minutes", "每 15 分钟刷新"),
                self.settings.refresh_minutes == 15,
            )?;
            separator(menu)?;
            append(
                menu,
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
                append(menu, command, &label, self.settings.alert_threshold == Some(threshold))?;
            }
            separator(menu)?;
            append(
                menu,
                CMD_STARTUP,
                self.locale.text("Start with Windows", "开机自动启动"),
                startup_enabled,
            )?;
            append(menu, CMD_RELEASES, self.locale.text("Open releases", "查看新版本"), false)?;
            separator(menu)?;
            append(
                menu,
                CMD_EXIT,
                self.locale.text("Exit CodexStatus", "退出 CodexStatus"),
                false,
            )?;
        }
        Ok(menu)
    }

    unsafe fn handle_command(&mut self, command: u32) {
        unsafe {
            match command {
                0 => {}
                CMD_REFRESH => self.start_refresh(true),
                CMD_USAGE => self.open_url(USAGE_URL),
                CMD_RELEASES => self.open_url(RELEASES_URL),
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
                CMD_EXIT => {
                    let _ = DestroyWindow(self.hwnd);
                }
                _ => {}
            }
        }
    }

    fn persist_settings(&self) {
        let _ = self.store.save_settings(&self.settings);
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

impl Drop for AppState {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
            let _ = KillTimer(Some(self.hwnd), TIMER_STARTUP);
            let _ = KillTimer(Some(self.hwnd), TIMER_CARD);
            if self.tray_added {
                let data = self.notify_data();
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
        }
    }
}

unsafe fn append(
    menu: HMENU,
    command: u32,
    label: &str,
    checked: bool,
) -> windows::core::Result<()> {
    unsafe {
        let text = wide0(label);
        let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
        AppendMenuW(menu, flags, command as usize, PCWSTR(text.as_ptr()))
    }
}

unsafe fn separator(menu: HMENU) -> windows::core::Result<()> {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
}

fn cursor_rect() -> RECT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    RECT { left: point.x, top: point.y, right: point.x + 1, bottom: point.y + 1 }
}

fn copy_utf16<const N: usize>(target: &mut [u16; N], value: &str) {
    target.fill(0);
    for (destination, source) in
        target.iter_mut().take(N.saturating_sub(1)).zip(value.encode_utf16())
    {
        *destination = source;
    }
}

fn wide0(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn friendly_error(error: &str, locale: ui::Locale) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not installed") || lower.contains("not available on path") {
        return locale
            .text("Codex is not installed or not on PATH", "未找到 Codex，请先安装并加入 PATH")
            .to_owned();
    }
    if lower.contains("not logged") || lower.contains("login") || lower.contains("unauthorized") {
        return locale
            .text("Sign in to Codex, then refresh", "请先登录 Codex，然后刷新")
            .to_owned();
    }
    if lower.contains("within 8 seconds") || lower.contains("timed out") {
        return locale.text("Codex did not respond in time", "Codex 响应超时").to_owned();
    }
    error.chars().take(180).collect()
}

#[cfg(feature = "diagnostics")]
fn diagnostic(stage: &str) {
    use std::io::Write;
    eprintln!("{stage}");
    let path = std::env::temp_dir().join("CodexStatus-diagnostic.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {stage}", Utc::now().to_rfc3339());
    }
}

#[cfg(not(feature = "diagnostics"))]
fn diagnostic(_stage: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_options() {
        assert_eq!(
            friendly_error("Codex is not installed", ui::Locale::Chinese),
            "未找到 Codex，请先安装并加入 PATH"
        );
    }

    #[test]
    fn utf16_copy_always_reserves_a_terminator() {
        let mut target = [9_u16; 4];
        copy_utf16(&mut target, "abcdef");
        assert_eq!(target[3], 0);
    }
}
