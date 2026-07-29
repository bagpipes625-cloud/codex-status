use crate::app_server::AppServerClient;
use crate::icon::{OwnedIcon, create_icon, tone_for};
use crate::model::{DisplayState, QuotaKind, QuotaSnapshot, RefreshState};
use crate::settings::{AppStore, Settings};
use crate::{startup, ui, updater};
use chrono::Utc;
use std::cell::Cell;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};
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
use windows::Win32::UI::Input::Ime::ImmDisableIME;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
#[cfg(not(codex_status_channel = "portable"))]
use windows::Win32::UI::Shell::NIF_GUID;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetMessageW, HMENU, IDC_ARROW, IsWindowVisible, KillTimer, LoadCursorW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SM_CXSMICON, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WA_INACTIVE,
    WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NULL, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONUP,
    WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_OVERLAPPED,
    WS_POPUP,
};
#[cfg(not(codex_status_channel = "portable"))]
use windows::core::GUID;
use windows::core::{PCWSTR, w};

const CHANNEL_NAME: &str = env!("CODEX_STATUS_CHANNEL");
const TRAY_ID: u32 = 1;

#[cfg(codex_status_channel = "stable")]
const MAIN_CLASS: PCWSTR = w!("CodexStatus.MainWindow.v1");
#[cfg(codex_status_channel = "stable")]
const FLYOUT_CLASS: PCWSTR = w!("CodexStatus.FlyoutWindow.v1");
#[cfg(codex_status_channel = "stable")]
const MUTEX_NAME: PCWSTR = w!("Local\\CodexStatus.4B7D5A91-45A5-4B78-A095-A9B43A2A4F7D");
#[cfg(codex_status_channel = "stable")]
const TRAY_GUID: GUID = GUID::from_u128(0xeab4363d_13a7_45eb_aa46_1b6d4e278d53);
#[cfg(codex_status_channel = "stable")]
const TRAY_IDENTITY_TEXT: &str = "GUID eab4363d-13a7-45eb-aa46-1b6d4e278d53";

#[cfg(codex_status_channel = "beta")]
const MAIN_CLASS: PCWSTR = w!("CodexStatus.Beta.MainWindow.v1");
#[cfg(codex_status_channel = "beta")]
const FLYOUT_CLASS: PCWSTR = w!("CodexStatus.Beta.FlyoutWindow.v1");
#[cfg(codex_status_channel = "beta")]
const MUTEX_NAME: PCWSTR = w!("Local\\CodexStatus.Beta.C379EB04-3507-4D7C-8911-1ADC3AE077A6");
#[cfg(codex_status_channel = "beta")]
const TRAY_GUID: GUID = GUID::from_u128(0xc379eb04_3507_4d7c_8911_1adc3ae077a6);
#[cfg(codex_status_channel = "beta")]
const TRAY_IDENTITY_TEXT: &str = "GUID c379eb04-3507-4d7c-8911-1adc3ae077a6";

#[cfg(codex_status_channel = "development")]
const MAIN_CLASS: PCWSTR = w!("CodexStatus.Development.MainWindow.v1");
#[cfg(codex_status_channel = "development")]
const FLYOUT_CLASS: PCWSTR = w!("CodexStatus.Development.FlyoutWindow.v1");
#[cfg(codex_status_channel = "development")]
const MUTEX_NAME: PCWSTR =
    w!("Local\\CodexStatus.Development.3E70297E-FB9B-4F98-AF42-DE19BD4824EC");
#[cfg(codex_status_channel = "development")]
const TRAY_GUID: GUID = GUID::from_u128(0x3e70297e_fb9b_4f98_af42_de19bd4824ec);
#[cfg(codex_status_channel = "development")]
const TRAY_IDENTITY_TEXT: &str = "GUID 3e70297e-fb9b-4f98-af42-de19bd4824ec";

#[cfg(codex_status_channel = "portable")]
const MAIN_CLASS: PCWSTR = w!("CodexStatus.Portable.MainWindow.v1");
#[cfg(codex_status_channel = "portable")]
const FLYOUT_CLASS: PCWSTR = w!("CodexStatus.Portable.FlyoutWindow.v1");
#[cfg(codex_status_channel = "portable")]
const MUTEX_NAME: PCWSTR = w!("Local\\CodexStatus.Portable.0D826064-8DC6-4308-BED6-7CB279AE4C9D");
#[cfg(codex_status_channel = "portable")]
const TRAY_IDENTITY_TEXT: &str = "HWND + uID 1";

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH_COMPLETE: u32 = WM_APP + 2;
const WM_SHOW_EXISTING: u32 = WM_APP + 3;
const WM_TOGGLE_FLYOUT: u32 = WM_APP + 4;
const WM_UPDATE_COMPLETE: u32 = WM_APP + 5;

const TIMER_REFRESH: usize = 1;
const TIMER_STARTUP: usize = 2;
const TIMER_CARD: usize = 3;
const TIMER_FLYOUT_ACTIVATE: usize = 4;

const TRAY_ACTIVATION_DEBOUNCE: Duration = Duration::from_millis(300);
const FLYOUT_ACTIVATION_GUARD: Duration = Duration::from_millis(220);
const TRAY_CLOSE_COALESCE: Duration = Duration::from_millis(250);

const CMD_REFRESH: u32 = 100;
const CMD_USAGE: u32 = 101;
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

thread_local! {
    static STATE: Cell<*mut AppState> = const { Cell::new(ptr::null_mut()) };
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("CodexStatus received unsupported command-line arguments")]
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

struct UpdateOutcome {
    result: Result<Option<updater::StagedUpdate>, updater::UpdateError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayQuotaState {
    primary_kind: QuotaKind,
    primary_percent: Option<u8>,
    indicator_percent: Option<u8>,
    refresh_state: RefreshState,
}

fn tray_quota_state(display: &DisplayState, preferred: QuotaKind) -> TrayQuotaState {
    let primary_kind = display.resolved_quota_kind(preferred);
    let weekly_percent = display.quota_percent(QuotaKind::Weekly);
    let five_hour_percent = display.quota_percent(QuotaKind::FiveHour);
    let indicator_percent = if five_hour_percent.is_none() {
        weekly_percent
    } else {
        display.quota_percent(primary_kind.other())
    };
    TrayQuotaState {
        primary_kind,
        primary_percent: display.quota_percent(primary_kind),
        indicator_percent,
        refresh_state: display.refresh_state,
    }
}

enum LaunchMode {
    Normal,
    Background,
    ApplyUpdate { parent_pid: u32, target: PathBuf },
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
    tray_quota_state: Option<TrayQuotaState>,
    tray_added: bool,
    tray_failure_logged: bool,
    refreshing: bool,
    refresh_pending: bool,
    update_checking: bool,
    failures: u8,
    last_tray_activation: Option<Instant>,
    flyout_ignore_inactive_until: Option<Instant>,
    flyout_hidden_for_tray_activation: Option<Instant>,
    hero_pressed: bool,
}

pub fn run() -> Result<(), AppError> {
    diagnostic("run:enter");
    let launch_mode = parse_arguments()?;
    if let LaunchMode::ApplyUpdate { parent_pid, target } = launch_mode {
        updater::apply_update_silently(parent_pid, &target);
        return Ok(());
    }
    let background = matches!(launch_mode, LaunchMode::Background);
    diagnostic("run:arguments");
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // The app has no editable controls. Disabling text services before the
        // first window is created prevents third-party IME/TIP modules from
        // being injected merely because the flyout receives focus.
        let _ = ImmDisableIME(0);
    }
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    let mutex = unsafe { InstanceHandle(CreateMutexW(None, false, MUTEX_NAME)?) };
    let mutex_was_existing = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
    diagnostic("run:mutex");
    if mutex_was_existing {
        if !background {
            if let Ok(existing) = unsafe { FindWindowW(MAIN_CLASS, PCWSTR::null()) } {
                unsafe {
                    let _ = PostMessageW(Some(existing), WM_SHOW_EXISTING, WPARAM(0), LPARAM(0));
                }
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
    let theme = ui::detect_theme(&settings.theme);
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
        theme,
        display,
        client: AppServerClient::new(),
        tray_icon: None,
        tray_quota_state: None,
        tray_added: false,
        tray_failure_logged: false,
        refreshing: false,
        refresh_pending: false,
        update_checking: false,
        failures: 0,
        last_tray_activation: None,
        flyout_ignore_inactive_until: None,
        flyout_hidden_for_tray_activation: None,
        hero_pressed: false,
    });
    let raw = Box::into_raw(state);
    STATE.with(|slot| slot.set(raw));
    diagnostic("run:state");

    let tray_ready = unsafe {
        let state = &mut *raw;
        ui::configure_flyout(state.flyout, state.theme);
        diagnostic("run:dwm");
        state.update_tray(true)
    };
    diagnostic("run:tray-returned");

    unsafe {
        let state = &mut *raw;
        state.reset_refresh_timer(state.settings.refresh_minutes.saturating_mul(60_000));
        let _ = SetTimer(Some(hwnd), TIMER_CARD, 30_000, None);
        if background {
            let _ = SetTimer(Some(hwnd), TIMER_STARTUP, 30_000, None);
        } else {
            state.start_refresh(false);
            if !tray_ready {
                state.show_flyout();
            }
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

fn parse_arguments() -> Result<LaunchMode, AppError> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [] => Ok(LaunchMode::Normal),
        [argument] if argument == "--background" => Ok(LaunchMode::Background),
        [mode, parent_pid, target] if mode == "--apply-update" => {
            let parent_pid =
                parent_pid.to_string_lossy().parse().map_err(|_| AppError::InvalidArguments)?;
            Ok(LaunchMode::ApplyUpdate { parent_pid, target: PathBuf::from(target) })
        }
        _ => Err(AppError::InvalidArguments),
    }
}

fn pointer_coordinates(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam.0 as u32;
    let x = i32::from(packed as u16 as i16);
    let y = i32::from((packed >> 16) as u16 as i16);
    (x, y)
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
                        WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_SELECT => {
                            state.request_toggle_flyout()
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => state.show_menu(),
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_TOGGLE_FLYOUT => {
                    state.toggle_flyout();
                    return LRESULT(0);
                }
                WM_REFRESH_COMPLETE => {
                    if lparam.0 != 0 {
                        let outcome = *Box::from_raw(lparam.0 as *mut RefreshOutcome);
                        state.finish_refresh(outcome);
                    }
                    return LRESULT(0);
                }
                WM_UPDATE_COMPLETE => {
                    if lparam.0 != 0 {
                        let outcome = *Box::from_raw(lparam.0 as *mut UpdateOutcome);
                        state.finish_update_check(outcome);
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
                            state.refresh_time_sensitive_state();
                            if IsWindowVisible(state.flyout).as_bool() {
                                let _ = InvalidateRect(Some(state.flyout), None, false);
                            }
                        }
                        TIMER_FLYOUT_ACTIVATE => {
                            let _ = KillTimer(Some(hwnd), TIMER_FLYOUT_ACTIVATE);
                            state.finish_flyout_activation();
                        }
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                    state.theme = ui::detect_theme(&state.settings.theme);
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
                ui::paint_card(
                    hwnd,
                    &state.display,
                    state.settings.display_quota,
                    state.locale,
                    state.theme,
                    state.hero_pressed,
                );
                return LRESULT(0);
            }
            WM_LBUTTONDOWN if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                let (x, y) = pointer_coordinates(lparam);
                let dpi = GetDpiForWindow(hwnd).max(96);
                if ui::hero_hit_test(x, y, dpi) {
                    state.hero_pressed = true;
                    let _ = SetCapture(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
            }
            WM_LBUTTONUP if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.hero_pressed {
                    let (x, y) = pointer_coordinates(lparam);
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    let activate = ui::hero_hit_test(x, y, dpi);
                    state.hero_pressed = false;
                    let _ = ReleaseCapture();
                    if activate {
                        state.toggle_display_quota();
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    return LRESULT(0);
                }
            }
            WM_CAPTURECHANGED if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.hero_pressed {
                    state.hero_pressed = false;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_ACTIVATE if (wparam.0 as u32 & 0xffff) == WA_INACTIVE => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    state.handle_flyout_inactive();
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    state.hide_flyout();
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    state.hide_flyout();
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
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

    fn refresh_time_sensitive_state(&mut self) {
        let now = Utc::now().timestamp();
        if self.display.refresh_state != RefreshState::Live
            && self.display.snapshot.as_ref().is_some_and(|value| !value.is_cache_valid(now))
        {
            self.display.snapshot = None;
            self.display.refresh_state = RefreshState::Unavailable;
        }
        if self.tray_quota_state != Some(self.current_tray_quota_state()) {
            let _ = self.update_tray(false);
        }
    }

    fn reset_refresh_timer(&self, milliseconds: u32) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
            let _ = SetTimer(Some(self.hwnd), TIMER_REFRESH, milliseconds.max(1_000), None);
        }
    }

    fn start_update_check(&mut self) {
        if self.update_checking {
            self.show_balloon(
                self.locale.text("Update in progress", "正在更新"),
                self.locale
                    .text("Please wait for the current update check.", "请等待当前更新检查完成。"),
            );
            return;
        }
        self.update_checking = true;
        self.show_balloon(
            self.locale.text("Checking for updates", "正在检查更新"),
            self.locale.text(
                "CodexStatus is checking your GitHub release.",
                "CodexStatus 正在检查你的 GitHub 发布版本。",
            ),
        );

        let hwnd_value = self.hwnd.0 as isize;
        let updates_directory = self.store.updates_directory();
        let spawn_result = thread::Builder::new()
            .name("codex-status-update".to_owned())
            .stack_size(512 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
                let outcome =
                    UpdateOutcome { result: updater::check_and_stage(&updates_directory) };
                let raw = Box::into_raw(Box::new(outcome));
                let posted = unsafe {
                    PostMessageW(Some(hwnd), WM_UPDATE_COMPLETE, WPARAM(0), LPARAM(raw as isize))
                };
                if posted.is_err() {
                    unsafe {
                        drop(Box::from_raw(raw));
                    }
                }
            });
        if let Err(error) = spawn_result {
            self.update_checking = false;
            self.show_update_failure(&error.to_string());
        }
    }

    fn finish_update_check(&mut self, outcome: UpdateOutcome) {
        self.update_checking = false;
        match outcome.result {
            Ok(Some(update)) => {
                self.show_balloon(
                    self.locale.text("Update ready", "更新已准备好"),
                    self.locale.text(
                        "The verified update will restart CodexStatus.",
                        "已验证更新，CodexStatus 将重启并完成替换。",
                    ),
                );
                match updater::launch_staged_update(&update) {
                    Ok(()) => unsafe {
                        let _ = DestroyWindow(self.hwnd);
                    },
                    Err(error) => self.show_update_failure(&error.to_string()),
                }
            }
            Ok(None) => self.show_balloon(
                self.locale.text("CodexStatus is up to date", "CodexStatus 已是最新版本"),
                &format!(
                    "{} {}",
                    self.locale.text("Current version", "当前版本"),
                    env!("CARGO_PKG_VERSION")
                ),
            ),
            Err(error) => self.show_update_failure(&error.to_string()),
        }
    }

    fn show_update_failure(&self, error: &str) {
        self.show_balloon(
            self.locale.text("Update failed", "更新失败"),
            &format!("{}: {error}", self.locale.text("Reason", "原因")),
        );
    }

    fn update_tray(&mut self, force_add: bool) -> bool {
        diagnostic("tray:render");
        let dpi = unsafe { GetDpiForSystem().max(96) };
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32 };
        let quota_state = self.current_tray_quota_state();
        let icon = match create_icon(
            quota_state.primary_percent,
            quota_state.indicator_percent,
            tone_for(&self.display, quota_state.primary_percent),
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
            &ui::tooltip(&self.display, self.settings.display_quota, self.locale),
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
                    self.locale.text("CodexStatus is ready", "CodexStatus 已就绪"),
                    self.locale.text(
                        "Your selected quota is shown in the tray. Drag the icon out of the overflow area to keep it visible.",
                        "你选择的额度会直接显示在托盘图标中。可将图标从折叠区拖出，保持常显。",
                    ),
                );
                self.settings.onboarding_shown = true;
                let _ = self.store.save_settings(&self.settings);
            }
        }
        true
    }

    fn current_tray_quota_state(&self) -> TrayQuotaState {
        tray_quota_state(&self.display, self.settings.display_quota)
    }

    fn toggle_display_quota(&mut self) {
        self.settings.display_quota = self.settings.display_quota.other();
        self.persist_settings();
        let _ = self.update_tray(false);
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    fn notify_data(&self) -> NOTIFYICONDATAW {
        #[cfg(codex_status_channel = "portable")]
        {
            NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ID,
                ..Default::default()
            }
        }
        #[cfg(not(codex_status_channel = "portable"))]
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

    fn record_tray_failure(&mut self, operation: &str, win32_error: u32, detail: &str) {
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

    fn show_balloon(&self, title: &str, body: &str) {
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

    fn request_toggle_flyout(&mut self) {
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

    fn toggle_flyout(&mut self) {
        if unsafe { IsWindowVisible(self.flyout) }.as_bool() {
            self.hide_flyout();
        } else {
            self.show_flyout();
        }
    }

    fn hide_flyout(&mut self) {
        self.flyout_ignore_inactive_until = None;
        self.flyout_hidden_for_tray_activation = None;
        self.hero_pressed = false;
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
            let _ = ShowWindow(self.flyout, SW_HIDE);
        }
    }

    fn handle_flyout_inactive(&mut self) {
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

    fn finish_flyout_activation(&mut self) {
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

    fn show_flyout(&mut self) {
        self.flyout_hidden_for_tray_activation = None;
        self.theme = ui::detect_theme(&self.settings.theme);
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
        self.flyout_ignore_inactive_until = Some(Instant::now() + FLYOUT_ACTIVATION_GUARD);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
            let _ = SetWindowPos(self.flyout, None, x, y, width, height, SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(self.flyout);
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    fn tray_rect(&self) -> Option<RECT> {
        #[cfg(codex_status_channel = "portable")]
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        #[cfg(not(codex_status_channel = "portable"))]
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
            separator(menu)?;
            append(
                menu,
                CMD_THEME_SYSTEM,
                self.locale.text("Theme: system", "主题：跟随系统"),
                self.settings.theme == "system",
            )?;
            append(
                menu,
                CMD_THEME_LIGHT,
                self.locale.text("Theme: light", "主题：浅色"),
                self.settings.theme == "light",
            )?;
            append(
                menu,
                CMD_THEME_DARK,
                self.locale.text("Theme: dark", "主题：深色"),
                self.settings.theme == "dark",
            )?;
            separator(menu)?;
            append(menu, CMD_UPDATE, self.locale.text("Check for updates", "检查更新"), false)?;
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
                CMD_UPDATE => self.start_update_check(),
                CMD_DISPLAY_FIVE_HOUR | CMD_DISPLAY_WEEKLY => {
                    self.settings.display_quota = if command == CMD_DISPLAY_WEEKLY {
                        QuotaKind::Weekly
                    } else {
                        QuotaKind::FiveHour
                    };
                    self.persist_settings();
                    let _ = self.update_tray(false);
                    let _ = InvalidateRect(Some(self.flyout), None, false);
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
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
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
    use crate::model::{AccountSummary, QuotaWindow, SESSION_MINUTES, WEEK_MINUTES};

    fn display_with_quotas(weekly: u8, five_hour: Option<u8>) -> DisplayState {
        let window = |percent: u8, window_minutes| QuotaWindow {
            used_percent: f64::from(100 - percent),
            remaining_percent: f64::from(percent),
            window_minutes,
            resets_at: Some(i64::MAX),
        };
        DisplayState::live(QuotaSnapshot {
            weekly: Some(window(weekly, WEEK_MINUTES)),
            session: five_hour.map(|percent| window(percent, SESSION_MINUTES)),
            account: AccountSummary::default(),
            fetched_at: 0,
        })
    }

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

    #[test]
    fn pointer_coordinates_sign_extend_both_axes() {
        let packed = u32::from(0xfffe_u16) | (u32::from(0xfffd_u16) << 16);
        assert_eq!(pointer_coordinates(LPARAM(packed as isize)), (-2, -3));
    }

    #[test]
    fn tray_indicator_uses_weekly_quota_when_five_hour_is_unavailable() {
        let display = display_with_quotas(66, None);
        for preferred in [QuotaKind::FiveHour, QuotaKind::Weekly] {
            let state = tray_quota_state(&display, preferred);
            assert_eq!(state.primary_kind, QuotaKind::Weekly);
            assert_eq!(state.primary_percent, Some(66));
            assert_eq!(state.indicator_percent, Some(66));
        }
    }

    #[test]
    fn tray_indicator_uses_the_other_quota_when_both_are_available() {
        let display = display_with_quotas(66, Some(85));
        let five_hour = tray_quota_state(&display, QuotaKind::FiveHour);
        assert_eq!(five_hour.primary_percent, Some(85));
        assert_eq!(five_hour.indicator_percent, Some(66));

        let weekly = tray_quota_state(&display, QuotaKind::Weekly);
        assert_eq!(weekly.primary_percent, Some(66));
        assert_eq!(weekly.indicator_percent, Some(85));
    }
}
