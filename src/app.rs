use crate::app_server::{AppServerClient, AppServerFailure, AppServerSnapshot};
use crate::history::UsageLedger;
use crate::icon::OwnedIcon;
use crate::model::{DisplayState, ProviderKind, QuotaAvailability, QuotaKind, RefreshState};
use crate::settings::{AppStore, Settings};
use crate::{startup, ui, updater};
use chrono::Utc;
use std::cell::Cell;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    RECT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Gdi::{HBRUSH, InvalidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::Ime::ImmDisableIME;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::Shell::{NIM_DELETE, NIN_SELECT, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, FindWindowW, GetMessageW, IDC_ARROW, IsWindowVisible, KillTimer, LoadCursorW,
    MSG, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SW_HIDE,
    SWP_NOACTIVATE, SWP_NOZORDER, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_CONTEXTMENU,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT,
    WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_OVERLAPPED, WS_POPUP,
};
#[cfg(any(codex_status_channel = "stable", codex_status_channel = "beta"))]
use windows::core::GUID;
use windows::core::{PCWSTR, w};

mod flyout;
mod menu;
mod reset_panel;
mod tray;

use flyout::{HistoryWheelState, pointer_coordinates, standard_back_message, wheel_delta};
use menu::show_context_menu;
use tray::TrayQuotaState;

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
const TRAY_IDENTITY_TEXT: &str = "HWND + uID 1";

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
const WM_PREWARM_FLYOUT: u32 = WM_APP + 6;
const WM_RESTORE_TRAY: u32 = WM_APP + 7;
const WM_RECONCILE_DISPLAY: u32 = WM_APP + 8;
const WM_RECONCILE_FLYOUT: u32 = WM_APP + 9;

const TIMER_REFRESH: usize = 1;
const TIMER_STARTUP: usize = 2;
const TIMER_CARD: usize = 3;
const TIMER_FLYOUT_ACTIVATE: usize = 4;
const TIMER_REFRESH_ANIMATION: usize = 5;
const TIMER_RESET_RESULT: usize = 6;
const REFRESH_ANIMATION_INTERVAL_MS: u32 = 16;
const RESET_RESULT_POLL_INTERVAL_MS: u32 = 250;

const HISTORY_SAVE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKGROUND_PREFETCH_INTERVAL: Duration = Duration::from_secs(15 * 60);

thread_local! {
    static STATE: Cell<*mut AppState> = const { Cell::new(ptr::null_mut()) };
    static STATE_BORROWED: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED_MESSAGE: Cell<u32> = const { Cell::new(0) };
}

struct StateBorrow;

impl StateBorrow {
    fn acquire(state_ptr: *mut AppState) -> Option<Self> {
        // Win32 APIs can synchronously re-enter either window procedure. A nested
        // callback must fall back to default handling instead of aliasing AppState.
        if state_ptr.is_null() || STATE_BORROWED.with(|borrowed| borrowed.replace(true)) {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for StateBorrow {
    fn drop(&mut self) {
        STATE_BORROWED.with(|borrowed| borrowed.set(false));
    }
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
    provider: ProviderKind,
    result: Result<AppServerSnapshot, String>,
    account_key: Option<String>,
    complete: bool,
}

impl RefreshOutcome {
    fn from_fetch(
        provider: ProviderKind,
        result: Result<AppServerSnapshot, AppServerFailure>,
    ) -> Self {
        match result {
            Ok(snapshot) => Self {
                provider,
                account_key: snapshot.account_key.clone(),
                result: Ok(snapshot),
                complete: true,
            },
            Err(failure) => Self {
                provider,
                account_key: failure.account_key,
                result: Err(failure.error.to_string()),
                complete: true,
            },
        }
    }

    fn unscoped_error(provider: ProviderKind, error: String) -> Self {
        Self { provider, result: Err(error), account_key: None, complete: true }
    }
}

struct UpdateOutcome {
    result: Result<Option<updater::StagedUpdate>, updater::UpdateError>,
}

struct ProviderSlot {
    display: DisplayState,
    active_account_key: Option<String>,
    failures: u8,
    refreshing: bool,
    refresh_pending: bool,
    last_prefetched: Option<Instant>,
    reset_refresh: ResetRefreshState,
}

impl Default for ProviderSlot {
    fn default() -> Self {
        // A persisted quota snapshot cannot be verified against the account that is
        // active now. Start empty; in-process refresh failures still retain the
        // latest account-verified live snapshot through DisplayState::after_error.
        Self {
            display: DisplayState::loading(None),
            active_account_key: None,
            failures: 0,
            refreshing: false,
            refresh_pending: false,
            last_prefetched: None,
            reset_refresh: ResetRefreshState::default(),
        }
    }
}

fn should_background_prefetch(slot: &ProviderSlot, force: bool, now: Instant) -> bool {
    if slot.refreshing {
        return false;
    }
    if force {
        return true;
    }
    let has_snapshot = slot.display.snapshot.is_some();
    if !has_snapshot && slot.failures == 0 {
        return true;
    }
    slot.last_prefetched
        .is_none_or(|last| now.saturating_duration_since(last) >= BACKGROUND_PREFETCH_INTERVAL)
}

fn same_verified_account(active: Option<&str>, observed: Option<&str>) -> bool {
    observed.is_some() && active == observed
}

fn provider_index(provider: ProviderKind) -> usize {
    match provider {
        ProviderKind::Codex => 0,
        ProviderKind::Gemini => 1,
    }
}

enum LaunchMode {
    Normal,
    Background,
    ApplyUpdate { parent_pid: u32, target: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageLoopAction {
    Dispatch,
    Exit,
    Error,
}

fn classify_get_message(result: i32) -> MessageLoopAction {
    match result {
        value if value > 0 => MessageLoopAction::Dispatch,
        0 => MessageLoopAction::Exit,
        _ => MessageLoopAction::Error,
    }
}

#[derive(Default)]
struct ResetRefreshState {
    last_boundary: Option<i64>,
}

impl ResetRefreshState {
    fn begin_if_due(&mut self, display: &DisplayState, now: i64, refreshing: bool) -> bool {
        if refreshing {
            return false;
        }
        let Some(boundary) = display
            .snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| [snapshot.session.as_ref(), snapshot.weekly.as_ref()])
            .flatten()
            .filter_map(|window| window.resets_at)
            .filter(|reset| *reset <= now)
            .max()
        else {
            return false;
        };
        if self.last_boundary.is_some_and(|last| boundary <= last) {
            return false;
        }
        self.last_boundary = Some(boundary);
        true
    }
}

struct AppState {
    reset_panel: crate::reset::ResetPanel,
    reset_results: Receiver<Result<crate::reset::ResetOutcome, crate::app_server::ResetFailure>>,
    reset_results_tx:
        SyncSender<Result<crate::reset::ResetOutcome, crate::app_server::ResetFailure>>,
    hwnd: HWND,
    flyout: HWND,
    taskbar_created: u32,
    store: AppStore,
    usage_history: UsageLedger,
    history_view_cache: Option<crate::history::UsageHistoryView>,
    history_dirty: bool,
    last_history_save: Option<Instant>,
    history_navigation: ui::HistoryNavigation,
    full_history_render: bool,
    settings: Settings,
    locale: ui::Locale,
    theme: ui::Theme,
    client: AppServerClient,
    fallback: std::sync::Arc<std::sync::Mutex<crate::refresh::FallbackState>>,
    gemini: std::sync::Arc<std::sync::Mutex<crate::gemini::GeminiState>>,
    providers: [ProviderSlot; 2],
    refresh_results: Receiver<RefreshOutcome>,
    refresh_results_tx: SyncSender<RefreshOutcome>,
    update_results: Receiver<UpdateOutcome>,
    update_results_tx: SyncSender<UpdateOutcome>,
    tray_icon: Option<OwnedIcon>,
    tray_quota_state: Option<TrayQuotaState>,
    tray_added: bool,
    tray_failure_logged: bool,
    refresh_feedback: bool,
    refresh_animation_started: Option<Instant>,
    update_checking: bool,
    last_tray_activation: Option<Instant>,
    flyout_ignore_inactive_until: Option<Instant>,
    flyout_hidden_for_tray_activation: Option<Instant>,
    pressed_quota: Option<QuotaKind>,
    pressed_history: Option<ui::HistoryHit>,
    hovered_history_day: Option<ui::HoveredHistoryDay>,
    hovered_history_values: bool,
    refresh_button_pressed: bool,
    history_wheel: HistoryWheelState,
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
            w!("AgentStatus"),
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
            w!("AgentStatus"),
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
    let usage_history = store.load_usage_history();
    let locale = ui::Locale::detect(&settings.locale);
    let theme = ui::detect_theme(&settings.theme);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED_MESSAGE.with(|slot| slot.set(taskbar_created));
    let (refresh_results_tx, refresh_results) = sync_channel(8);
    let (update_results_tx, update_results) = sync_channel(1);
    let (reset_results_tx, reset_results) = sync_channel(1);
    let reset_record = store.load_reset_attempt();
    let reset_panel = crate::reset::ResetPanel {
        storage_blocked: reset_record.is_err(),
        pending: reset_record.ok().flatten(),
        ..Default::default()
    };
    let state = Box::new(AppState {
        reset_panel,
        reset_results,
        reset_results_tx,
        hwnd,
        flyout,
        taskbar_created,
        store,
        usage_history,
        history_view_cache: None,
        history_dirty: false,
        last_history_save: None,
        history_navigation: ui::HistoryNavigation::default(),
        full_history_render: true,
        settings,
        locale,
        theme,
        client: AppServerClient::new(),
        fallback: Default::default(),
        gemini: Default::default(),
        providers: Default::default(),
        refresh_results,
        refresh_results_tx,
        update_results,
        update_results_tx,
        tray_icon: None,
        tray_quota_state: None,
        tray_added: false,
        tray_failure_logged: false,
        refresh_feedback: false,
        refresh_animation_started: None,
        update_checking: false,
        last_tray_activation: None,
        flyout_ignore_inactive_until: None,
        flyout_hidden_for_tray_activation: None,
        pressed_quota: None,
        pressed_history: None,
        hovered_history_day: None,
        hovered_history_values: false,
        refresh_button_pressed: false,
        history_wheel: HistoryWheelState::default(),
    });
    let raw = Box::into_raw(state);
    STATE.with(|slot| slot.set(raw));
    diagnostic("run:state");

    let tray_ready = unsafe {
        let _state_borrow = StateBorrow::acquire(raw).expect("startup owns AppState");
        let state = &mut *raw;
        ui::configure_flyout(state.flyout, state.theme);
        diagnostic("run:dwm");
        let tray_ready = state.update_tray(true);
        state.position_flyout(false);
        tray_ready
    };
    diagnostic("run:tray-returned");

    unsafe {
        let _state_borrow = StateBorrow::acquire(raw).expect("startup owns AppState");
        let state = &mut *raw;
        state.reset_refresh_timer(state.settings.refresh_minutes.saturating_mul(60_000));
        let _ = SetTimer(Some(hwnd), TIMER_CARD, 30_000, None);
        if background {
            let _ = SetTimer(Some(hwnd), TIMER_STARTUP, 1_500, None);
        } else {
            state.start_refresh(false);
            state.prefetch_other_provider(true);
            if !tray_ready {
                state.show_flyout();
            }
        }
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_PREWARM_FLYOUT, WPARAM(0), LPARAM(0));
    }
    diagnostic("run:message-loop");

    let mut message = MSG::default();
    let mut message_loop_error = None;
    unsafe {
        loop {
            match classify_get_message(GetMessageW(&mut message, None, 0, 0).0) {
                MessageLoopAction::Dispatch => {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                MessageLoopAction::Exit => break,
                MessageLoopAction::Error => {
                    message_loop_error = Some(windows::core::Error::from_thread());
                    break;
                }
            }
        }
    }

    if message_loop_error.is_some() {
        // GetMessageW errors bypass the normal WM_CLOSE path. Destroy both
        // windows while STATE is still valid so WM_DESTROY releases the D2D
        // renderer and no unowned window survives into fatal-error reporting.
        unsafe {
            let _ = DestroyWindow(flyout);
            let _ = DestroyWindow(hwnd);
        }
    }

    STATE.with(|slot| slot.set(ptr::null_mut()));
    TASKBAR_CREATED_MESSAGE.with(|slot| slot.set(0));
    unsafe {
        (&mut *raw).persist_usage_history(true);
        drop(Box::from_raw(raw));
    }
    drop(mutex);
    match message_loop_error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
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
        // These messages synchronously re-enter the window procedure. Handle them
        // before borrowing AppState from the thread-local pointer.
        match message {
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
                let state_ptr = STATE.with(Cell::get);
                if !state_ptr.is_null()
                    && let Some(_state_borrow) = StateBorrow::acquire(state_ptr)
                {
                    (&mut *state_ptr).cleanup_window_resources();
                }
                ui::release_card_renderer();
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }

        let state_ptr = STATE.with(Cell::get);
        if !state_ptr.is_null() {
            if message == WM_TRAY {
                let event = lparam.0 as u32 & 0xffff;
                if matches!(event, WM_RBUTTONUP | WM_CONTEXTMENU) {
                    show_context_menu(state_ptr);
                    return LRESULT(0);
                }
            }

            let Some(_state_borrow) = StateBorrow::acquire(state_ptr) else {
                let taskbar_created = TASKBAR_CREATED_MESSAGE.with(Cell::get);
                if let Some(deferred) = deferred_main_message(message, taskbar_created) {
                    let _ = PostMessageW(Some(hwnd), deferred, WPARAM(0), LPARAM(0));
                }
                return DefWindowProcW(hwnd, message, wparam, lparam);
            };

            let state = &mut *state_ptr;
            if state.taskbar_created != 0 && message == state.taskbar_created {
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
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_TOGGLE_FLYOUT => {
                    state.toggle_flyout();
                    return LRESULT(0);
                }
                WM_PREWARM_FLYOUT => {
                    let history = state.history_view();
                    ui::prewarm_card_renderer(
                        state.flyout,
                        &state.active_slot().display,
                        state.settings.display_quota,
                        state.locale,
                        state.theme,
                        ui::CardView {
                            provider: state.settings.provider,
                            reset: Some(&state.reset_panel),
                            history,
                            navigation: &state.history_navigation,
                            interaction: ui::CardInteraction {
                                pressed_quota: None,
                                refresh_feedback: false,
                                refresh_rotation_degrees: 0.0,
                                hovered_day: None,
                                hovered_history_values: false,
                            },
                        },
                    );
                    return LRESULT(0);
                }
                WM_RESTORE_TRAY => {
                    state.tray_added = false;
                    let _ = state.update_tray(true);
                    return LRESULT(0);
                }
                WM_RECONCILE_DISPLAY => {
                    ui::reset_card_renderer_fallback();
                    state.theme = ui::detect_theme(&state.settings.theme);
                    ui::configure_flyout(state.flyout, state.theme);
                    let _ = state.update_tray(false);
                    let _ = InvalidateRect(Some(state.flyout), None, true);
                    return LRESULT(0);
                }
                WM_REFRESH_COMPLETE => {
                    state.finish_ready_background_work();
                    return LRESULT(0);
                }
                WM_UPDATE_COMPLETE => {
                    state.finish_ready_background_work();
                    return LRESULT(0);
                }
                WM_SHOW_EXISTING => {
                    state.show_flyout();
                    return LRESULT(0);
                }
                WM_TIMER => {
                    match wparam.0 {
                        TIMER_REFRESH => {
                            state.start_refresh(false);
                            state.prefetch_other_provider(false);
                        }
                        TIMER_STARTUP => {
                            let _ = KillTimer(Some(hwnd), TIMER_STARTUP);
                            state.start_refresh(false);
                            state.prefetch_other_provider(true);
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
                        TIMER_REFRESH_ANIMATION => state.advance_refresh_animation(),
                        _ => {}
                    }
                    state.finish_ready_background_work();
                    return LRESULT(0);
                }
                WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                    ui::reset_card_renderer_fallback();
                    state.theme = ui::detect_theme(&state.settings.theme);
                    ui::configure_flyout(state.flyout, state.theme);
                    let _ = state.update_tray(false);
                    let _ = InvalidateRect(Some(state.flyout), None, true);
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
        let _state_borrow = if state_ptr.is_null() {
            None
        } else if let Some(state_borrow) = StateBorrow::acquire(state_ptr) {
            Some(state_borrow)
        } else {
            return match message {
                WM_ERASEBKGND => LRESULT(1),
                WM_CLOSE => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    LRESULT(0)
                }
                WM_DPICHANGED => {
                    let _ = PostMessageW(Some(hwnd), WM_RECONCILE_FLYOUT, WPARAM(0), LPARAM(0));
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            };
        };
        match message {
            WM_PAINT if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                let history = state.history_view();
                let full_history_render = ui::paint_card(
                    hwnd,
                    &state.active_slot().display,
                    state.settings.display_quota,
                    state.locale,
                    state.theme,
                    ui::CardView {
                        provider: state.settings.provider,
                        reset: Some(&state.reset_panel),
                        history,
                        navigation: &state.history_navigation,
                        interaction: ui::CardInteraction {
                            pressed_quota: state.pressed_quota,
                            refresh_feedback: state.refresh_feedback,
                            refresh_rotation_degrees: state.refresh_rotation_degrees(),
                            hovered_day: state.hovered_history_day,
                            hovered_history_values: state.hovered_history_values,
                        },
                    },
                );
                state.full_history_render = full_history_render;
                if full_history_render && state.reset_panel.success {
                    state.reset_panel.success_interactive = true;
                }
                if !full_history_render && state.reset_panel.open {
                    let announce_success =
                        state.reset_panel.success && !state.reset_panel.success_notified;
                    state.reset_panel.close();
                    if announce_success {
                        state.show_reset_success_balloon();
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
            value if !state_ptr.is_null() && standard_back_message(value, wparam, lparam) => {
                let state = &mut *state_ptr;
                if state.reset_panel.open {
                    state.reset_back();
                    return LRESULT(1);
                }
                if state.history_navigation.page != ui::HistoryPage::Main {
                    state.activate_history_hit(ui::HistoryHit::Back);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return if value == WM_KEYDOWN { LRESULT(0) } else { LRESULT(1) };
                }
            }
            WM_MOUSEWHEEL if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.reset_panel.captures_pointer() {
                    state.scroll_reset(wheel_delta(wparam));
                    return LRESULT(0);
                }
                if let Some(changed) = state.handle_history_wheel(wheel_delta(wparam)) {
                    if changed {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    return LRESULT(0);
                }
            }
            WM_LBUTTONDOWN if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.reset_pointer_down(lparam) {
                    return LRESULT(0);
                }
                let (x, y) = pointer_coordinates(lparam);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let active_display = &state.active_slot().display;
                if (state.history_navigation.page == ui::HistoryPage::Main
                    || state.full_history_render)
                    && !state.active_slot().refreshing
                    && !state.refresh_feedback
                    && ui::refresh_button_hit_test(active_display, x, y, dpi)
                {
                    state.refresh_button_pressed = true;
                    let _ = SetCapture(hwnd);
                    return LRESULT(0);
                }
                if let Some(kind) = ui::quota_card_hit_test(active_display, x, y, dpi) {
                    if state.history_navigation.page == ui::HistoryPage::Main {
                        state.pressed_quota = Some(kind);
                        let _ = SetCapture(hwnd);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return LRESULT(0);
                    }
                }
                let history = state.history_view();
                let compact =
                    matches!(active_display.quota_availability(), QuotaAvailability::Single(_));
                if let Some(hit) = ui::history_hit_test(
                    &state.history_navigation,
                    history,
                    compact,
                    x,
                    y,
                    dpi,
                    state.full_history_render,
                ) {
                    state.pressed_history = Some(hit);
                    let _ = SetCapture(hwnd);
                    return LRESULT(0);
                }
            }
            WM_LBUTTONUP if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.reset_pointer_up(lparam) {
                    return LRESULT(0);
                }
                if state.refresh_button_pressed {
                    let (x, y) = pointer_coordinates(lparam);
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    let activate =
                        ui::refresh_button_hit_test(&state.active_slot().display, x, y, dpi);
                    state.refresh_button_pressed = false;
                    let _ = ReleaseCapture();
                    if activate && !state.active_slot().refreshing && !state.refresh_feedback {
                        state.start_refresh(true);
                        state.prefetch_other_provider(true);
                    }
                    return LRESULT(0);
                }
                if let Some(pressed) = state.pressed_quota {
                    let (x, y) = pointer_coordinates(lparam);
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    let activate = ui::quota_card_hit_test(&state.active_slot().display, x, y, dpi)
                        == Some(pressed);
                    state.pressed_quota = None;
                    let _ = ReleaseCapture();
                    if activate {
                        state.select_display_quota(pressed);
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    return LRESULT(0);
                }
                if let Some(pressed) = state.pressed_history.take() {
                    let (x, y) = pointer_coordinates(lparam);
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    let history = state.history_view();
                    let compact = matches!(
                        state.active_slot().display.quota_availability(),
                        QuotaAvailability::Single(_)
                    );
                    let activate = ui::history_hit_test(
                        &state.history_navigation,
                        history,
                        compact,
                        x,
                        y,
                        dpi,
                        state.full_history_render,
                    ) == Some(pressed);
                    let _ = ReleaseCapture();
                    if activate {
                        state.activate_history_hit(pressed);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
            }
            WM_MOUSEMOVE if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                if state.reset_pointer_move(lparam) {
                    return LRESULT(0);
                }
                let (x, y) = pointer_coordinates(lparam);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let history = state.history_view();
                let compact = matches!(
                    state.active_slot().display.quota_availability(),
                    QuotaAvailability::Single(_)
                );
                let hovered = ui::hovered_history_day(
                    &state.history_navigation,
                    history,
                    compact,
                    x,
                    y,
                    dpi,
                    state.full_history_render,
                );
                let hovered_history_values = state.settings.provider == ProviderKind::Codex
                    && matches!(
                        ui::history_hit_test(
                            &state.history_navigation,
                            history,
                            compact,
                            x,
                            y,
                            dpi,
                            state.full_history_render,
                        ),
                        Some(ui::HistoryHit::OpenHistory)
                    );
                if hovered != state.hovered_history_day
                    || hovered_history_values != state.hovered_history_values
                {
                    state.hovered_history_day = hovered;
                    state.hovered_history_values = hovered_history_values;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
            WM_CAPTURECHANGED if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                state.reset_panel.pressed = None;
                let had_capture = state.pressed_quota.take().is_some()
                    || state.pressed_history.take().is_some()
                    || state.refresh_button_pressed;
                state.refresh_button_pressed = false;
                if had_capture {
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
            WM_RECONCILE_FLYOUT if !state_ptr.is_null() => {
                let state = &mut *state_ptr;
                state.position_flyout(IsWindowVisible(hwnd).as_bool());
                let _ = InvalidateRect(Some(hwnd), None, true);
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
                ui::apply_flyout_shape(hwnd, GetDpiForWindow(hwnd).max(96));
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn deferred_main_message(message: u32, taskbar_created: u32) -> Option<u32> {
    if taskbar_created != 0 && message == taskbar_created {
        Some(WM_RESTORE_TRAY)
    } else if matches!(message, WM_SETTINGCHANGE | WM_DISPLAYCHANGE) {
        Some(WM_RECONCILE_DISPLAY)
    } else {
        None
    }
}

impl AppState {
    fn slot(&self, provider: ProviderKind) -> &ProviderSlot {
        &self.providers[provider_index(provider)]
    }

    fn slot_mut(&mut self, provider: ProviderKind) -> &mut ProviderSlot {
        &mut self.providers[provider_index(provider)]
    }

    fn active_slot(&self) -> &ProviderSlot {
        self.slot(self.settings.provider)
    }

    fn active_slot_mut(&mut self) -> &mut ProviderSlot {
        self.slot_mut(self.settings.provider)
    }

    fn history_view(&self) -> Option<&crate::history::UsageHistoryView> {
        self.history_view_cache.as_ref()
    }

    fn start_refresh(&mut self, force: bool) {
        if self.reset_panel.busy {
            self.active_slot_mut().refresh_pending = true;
            return;
        }
        diagnostic("refresh:start");
        if force {
            self.refresh_feedback = true;
            if self.refresh_animation_started.is_none()
                && unsafe { IsWindowVisible(self.flyout) }.as_bool()
            {
                self.start_refresh_animation();
            }
        }
        if self.active_slot().refreshing {
            self.active_slot_mut().refresh_pending |= force;
            if force {
                unsafe {
                    let _ = InvalidateRect(Some(self.flyout), None, false);
                }
            }
            return;
        }
        {
            let slot = self.active_slot_mut();
            slot.refreshing = true;
            slot.display.error = None;
            if slot.display.snapshot.is_none() {
                slot.display.refresh_state = RefreshState::Loading;
            }
        }
        let _ = self.update_tray(false);
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }

        let provider = self.settings.provider;
        if let Err(error) = self.spawn_refresh_worker(provider) {
            self.finish_refresh(RefreshOutcome::unscoped_error(
                provider,
                format!("Could not start refresh: {error}"),
            ));
        }
    }

    fn prefetch_other_provider(&mut self, force: bool) {
        let other = self.settings.provider.other();
        let now = Instant::now();
        if !should_background_prefetch(self.slot(other), force, now) {
            return;
        }
        {
            let slot = self.slot_mut(other);
            slot.refreshing = true;
            slot.last_prefetched = Some(now);
        }
        if self.spawn_refresh_worker(other).is_err() {
            self.slot_mut(other).refreshing = false;
        }
    }

    fn spawn_refresh_worker(&self, provider: ProviderKind) -> std::io::Result<()> {
        let hwnd_value = self.hwnd.0 as isize;
        let client = self.client.clone();
        let fallback = self.fallback.clone();
        let gemini = self.gemini.clone();
        let results = self.refresh_results_tx.clone();
        thread::Builder::new()
            .name("codex-status-refresh".to_owned())
            .stack_size(512 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
                diagnostic("refresh:worker");
                let outcome = match provider {
                    ProviderKind::Codex => RefreshOutcome::from_fetch(
                        provider,
                        crate::refresh::fetch(&client, &fallback, |snapshot| {
                            let mut interim = RefreshOutcome::from_fetch(provider, Ok(snapshot));
                            interim.complete = false;
                            if results.send(interim).is_ok() {
                                unsafe {
                                    let _ = PostMessageW(
                                        Some(hwnd),
                                        WM_REFRESH_COMPLETE,
                                        WPARAM(0),
                                        LPARAM(0),
                                    );
                                }
                            }
                        }),
                    ),
                    ProviderKind::Gemini => {
                        RefreshOutcome::from_fetch(provider, crate::gemini::fetch(&gemini))
                    }
                };
                diagnostic(if outcome.result.is_ok() {
                    "refresh:success"
                } else {
                    "refresh:error"
                });
                if results.send(outcome).is_ok() {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_REFRESH_COMPLETE, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .map(|_| ())
    }

    fn finish_refresh(&mut self, outcome: RefreshOutcome) {
        let is_active = outcome.provider == self.settings.provider;
        if is_active {
            self.reset_panel.pressed = None;
            self.reset_panel.hovered = None;
            if outcome.complete || outcome.result.is_ok() {
                self.stop_refresh_feedback();
            }
        }
        self.slot_mut(outcome.provider).refreshing = !outcome.complete;
        match outcome.result {
            Ok(snapshot) => {
                {
                    let slot = self.slot_mut(outcome.provider);
                    slot.failures = 0;
                    slot.active_account_key = snapshot.account_key.clone();
                    slot.display = DisplayState::live(snapshot.quota);
                }
                if outcome.provider == ProviderKind::Codex {
                    self.usage_history.record_refresh(
                        snapshot.account_key.as_deref(),
                        snapshot.token_usage.as_ref(),
                        Utc::now().timestamp(),
                    );
                    self.history_dirty = true;
                    if is_active {
                        self.refresh_history_view_cache();
                    }
                    self.persist_usage_history(false);
                }
                if is_active {
                    if outcome.complete {
                        self.reset_refresh_timer(
                            self.settings.refresh_minutes.saturating_mul(60_000),
                        );
                    }
                    self.maybe_alert();
                }
            }
            Err(error) => {
                let now = Utc::now().timestamp();
                let locale = self.locale;
                let (failures, same_account) = {
                    let slot = self.slot_mut(outcome.provider);
                    slot.failures = slot.failures.saturating_add(1);
                    let same = same_verified_account(
                        slot.active_account_key.as_deref(),
                        outcome.account_key.as_deref(),
                    );
                    let snapshot = if same {
                        slot.display.snapshot.take()
                    } else {
                        // A partial refresh may identify a different account, but its
                        // persisted history is not activated until the full snapshot
                        // succeeds. This prevents stale cross-account UI on failure.
                        slot.active_account_key = None;
                        None
                    };
                    slot.display =
                        DisplayState::after_error(snapshot, friendly_error(&error, locale), now);
                    (slot.failures, same)
                };
                if is_active {
                    if !same_account {
                        self.history_view_cache = None;
                    }
                    let backoff = match failures {
                        1 => 60_000,
                        2 => 5 * 60_000,
                        _ => 15 * 60_000,
                    };
                    self.reset_refresh_timer(backoff);
                    if same_account {
                        self.refresh_history_view_cache();
                    }
                }
            }
        }
        if !is_active {
            if outcome.complete && self.slot(outcome.provider).refresh_pending {
                {
                    let slot = self.slot_mut(outcome.provider);
                    slot.refresh_pending = false;
                    slot.refreshing = true;
                }
                if self.spawn_refresh_worker(outcome.provider).is_err() {
                    self.slot_mut(outcome.provider).refreshing = false;
                }
            }
            return;
        }
        let _ = self.update_tray(false);
        self.sync_visible_flyout_layout();
        self.reset_panel.scroll =
            self.reset_panel.scroll.min(crate::reset::ResetPanel::max_scroll(
                self.active_slot()
                    .display
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.account.reset_credit_details.as_ref())
                    .map_or(0, Vec::len),
                ui::flyout_dimensions(&self.active_slot().display).height as f32,
            ));
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
        if outcome.complete && self.active_slot().refresh_pending {
            self.active_slot_mut().refresh_pending = false;
            self.start_refresh(true);
        }
    }

    fn refresh_history_view_cache(&mut self) {
        let key = self.active_slot().active_account_key.as_deref();
        self.history_view_cache =
            self.usage_history.history_for(key).map(|history| history.view(Utc::now().timestamp()));
    }

    fn persist_usage_history(&mut self, force: bool) {
        if !self.history_dirty {
            return;
        }
        let due = self
            .last_history_save
            .is_none_or(|last| force || last.elapsed() >= HISTORY_SAVE_INTERVAL);
        if due && self.store.save_usage_history(&self.usage_history).is_ok() {
            self.history_dirty = false;
            self.last_history_save = Some(Instant::now());
        }
    }

    fn finish_ready_background_work(&mut self) {
        self.finish_reset_if_ready();
        while let Ok(outcome) = self.refresh_results.try_recv() {
            self.finish_refresh(outcome);
        }
        if let Ok(outcome) = self.update_results.try_recv() {
            self.finish_update_check(outcome);
        }
    }

    fn refresh_time_sensitive_state(&mut self) {
        let now = Utc::now().timestamp();
        let due_reset = {
            let slot = self.active_slot_mut();
            slot.reset_refresh.begin_if_due(&slot.display, now, slot.refreshing)
        };
        if due_reset {
            self.start_refresh(false);
        }
        if self
            .history_view_cache
            .as_ref()
            .is_some_and(|view| view.today != chrono::Local::now().date_naive())
        {
            self.refresh_history_view_cache();
        }
        {
            let display = &mut self.active_slot_mut().display;
            if display.refresh_state != RefreshState::Live
                && display.snapshot.as_ref().is_some_and(|value| !value.is_cache_valid(now))
            {
                display.snapshot = None;
                display.refresh_state = RefreshState::Unavailable;
            }
        }
        if self.tray_quota_state != Some(self.current_tray_quota_state()) {
            let _ = self.update_tray(false);
        }
        self.sync_visible_flyout_layout();
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
                "AgentStatus is checking your GitHub release.",
                "AgentStatus 正在检查你的 GitHub 发布版本。",
            ),
        );

        let hwnd_value = self.hwnd.0 as isize;
        let updates_directory = self.store.updates_directory();
        let results = self.update_results_tx.clone();
        let spawn_result = thread::Builder::new()
            .name("codex-status-update".to_owned())
            .stack_size(512 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
                let outcome =
                    UpdateOutcome { result: updater::check_and_stage(&updates_directory) };
                if results.send(outcome).is_ok() {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_UPDATE_COMPLETE, WPARAM(0), LPARAM(0));
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
                        "The verified update will restart AgentStatus.",
                        "已验证更新，AgentStatus 将重启并完成替换。",
                    ),
                );
                match updater::launch_staged_update(&update) {
                    Ok(()) => unsafe {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    },
                    Err(error) => self.show_update_failure(&error.to_string()),
                }
            }
            Ok(None) => self.show_balloon(
                self.locale.text("AgentStatus is up to date", "AgentStatus 已是最新版本"),
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

    fn persist_settings(&self) {
        self.save_settings(true);
    }

    fn persist_internal_settings(&self) {
        self.save_settings(false);
    }

    fn save_settings(&self, notify: bool) {
        if let Err(error) = self.store.save_settings(&self.settings) {
            let _ = self.store.write_settings_error(&format!(
                "{} settings:save error={error:?}\n",
                Utc::now().to_rfc3339()
            ));
            if notify {
                self.show_balloon(
                    self.locale.text("Setting was not saved", "设置未保存"),
                    &error.to_string(),
                );
            }
        }
    }

    fn cleanup_window_resources(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
            let _ = KillTimer(Some(self.hwnd), TIMER_STARTUP);
            let _ = KillTimer(Some(self.hwnd), TIMER_CARD);
            let _ = KillTimer(Some(self.hwnd), TIMER_FLYOUT_ACTIVATE);
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH_ANIMATION);
            let _ = KillTimer(Some(self.hwnd), TIMER_RESET_RESULT);
            if self.tray_added {
                let data = self.notify_data();
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                self.tray_added = false;
            }
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.cleanup_window_resources();
    }
}

fn friendly_error(error: &str, locale: ui::Locale) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("sign in to antigravity cli") {
        return locale
            .text(
                "Sign in to Antigravity CLI (agy), then refresh",
                "请先登录 Antigravity CLI (agy)，然后刷新",
            )
            .to_owned();
    }
    if lower.contains("antigravity language server is not running") {
        return locale
            .text("Start Antigravity, then refresh", "请先启动 Antigravity，然后刷新")
            .to_owned();
    }
    if lower.contains("antigravity") {
        return locale
            .text("Could not read local Antigravity quota", "无法读取本地 Antigravity 额度")
            .to_owned();
    }
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
    let truncate = path.metadata().is_ok_and(|metadata| metadata.len() >= 256 * 1024);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(path)
    {
        let _ = writeln!(file, "{} {stage}", Utc::now().to_rfc3339());
    }
}

#[cfg(not(feature = "diagnostics"))]
fn diagnostic(_stage: &str) {}

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

    fn display_with_reset_times(weekly: Option<i64>, five_hour: Option<i64>) -> DisplayState {
        let window = |resets_at, window_minutes| QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes,
            resets_at: Some(resets_at),
        };
        DisplayState::live(QuotaSnapshot {
            weekly: weekly.map(|reset| window(reset, WEEK_MINUTES)),
            session: five_hour.map(|reset| window(reset, SESSION_MINUTES)),
            account: AccountSummary::default(),
            fetched_at: 0,
        })
    }

    #[test]
    fn reset_refresh_triggers_once_for_each_crossed_boundary() {
        let display = display_with_reset_times(Some(200), Some(100));
        let mut state = ResetRefreshState::default();

        assert!(!state.begin_if_due(&display, 99, false));
        assert!(state.begin_if_due(&display, 100, false));
        assert!(!state.begin_if_due(&display, 150, false));
        assert!(state.begin_if_due(&display, 200, false));
        assert!(!state.begin_if_due(&display, 300, false));
    }

    #[test]
    fn reset_refresh_does_not_consume_a_boundary_while_refreshing() {
        let display = display_with_reset_times(Some(100), Some(100));
        let mut state = ResetRefreshState::default();

        assert!(!state.begin_if_due(&display, 100, true));
        assert!(state.begin_if_due(&display, 100, false));
        assert!(!state.begin_if_due(&display, 100, false));
    }

    #[test]
    fn reset_refresh_boundaries_are_isolated_per_provider() {
        let mut providers = [ProviderSlot::default(), ProviderSlot::default()];
        providers[provider_index(ProviderKind::Codex)].display =
            display_with_reset_times(Some(500), None);
        providers[provider_index(ProviderKind::Gemini)].display =
            display_with_reset_times(Some(200), None);

        let codex = &mut providers[provider_index(ProviderKind::Codex)];
        assert!(codex.reset_refresh.begin_if_due(&codex.display, 500, false));

        let gemini = &mut providers[provider_index(ProviderKind::Gemini)];
        assert!(gemini.reset_refresh.begin_if_due(&gemini.display, 500, false));
        assert!(!gemini.reset_refresh.begin_if_due(&gemini.display, 500, false));
    }

    #[test]
    fn rejects_unknown_options() {
        assert_eq!(
            friendly_error("Codex is not installed", ui::Locale::Chinese),
            "未找到 Codex，请先安装并加入 PATH"
        );
    }

    #[test]
    fn state_borrow_blocks_reentrant_aliasing_and_recovers_after_return() {
        let state_ptr = ptr::dangling_mut::<AppState>();
        let borrow = StateBorrow::acquire(state_ptr).expect("first callback owns the state");
        assert!(StateBorrow::acquire(state_ptr).is_none());
        drop(borrow);
        assert!(StateBorrow::acquire(state_ptr).is_some());
    }

    #[test]
    fn reentrant_system_changes_are_deferred_without_pointer_payloads() {
        let taskbar_created = 0xc123;
        assert_eq!(deferred_main_message(taskbar_created, taskbar_created), Some(WM_RESTORE_TRAY));
        assert_eq!(
            deferred_main_message(WM_SETTINGCHANGE, taskbar_created),
            Some(WM_RECONCILE_DISPLAY)
        );
        assert_eq!(
            deferred_main_message(WM_DISPLAYCHANGE, taskbar_created),
            Some(WM_RECONCILE_DISPLAY)
        );
        assert_eq!(deferred_main_message(WM_TIMER, taskbar_created), None);
    }

    #[test]
    fn failed_refresh_reuses_data_only_for_the_same_verified_account() {
        assert!(same_verified_account(Some("account-a"), Some("account-a")));
        assert!(!same_verified_account(Some("account-a"), Some("account-b")));
        assert!(!same_verified_account(Some("account-a"), None));
        assert!(!same_verified_account(None, None));
    }

    #[test]
    fn message_loop_distinguishes_dispatch_exit_and_error() {
        assert_eq!(classify_get_message(1), MessageLoopAction::Dispatch);
        assert_eq!(classify_get_message(0), MessageLoopAction::Exit);
        assert_eq!(classify_get_message(-1), MessageLoopAction::Error);
    }

    #[test]
    fn background_prefetch_throttles_periodic_runs_but_allows_forced_and_stale() {
        let now = Instant::now();
        let mut slot = ProviderSlot::default();
        assert!(should_background_prefetch(&slot, false, now));

        slot.display = display_with_quotas(Some(80), Some(90));
        slot.last_prefetched = Some(now);
        assert!(!should_background_prefetch(&slot, false, now + Duration::from_secs(5 * 60)));
        assert!(should_background_prefetch(&slot, true, now + Duration::from_secs(5 * 60)));
        assert!(should_background_prefetch(&slot, false, now + BACKGROUND_PREFETCH_INTERVAL));

        slot.display = DisplayState::loading(None);
        slot.failures = 1;
        assert!(!should_background_prefetch(&slot, false, now + Duration::from_secs(5 * 60)));
        assert!(should_background_prefetch(&slot, false, now + BACKGROUND_PREFETCH_INTERVAL));

        slot.refreshing = true;
        assert!(!should_background_prefetch(&slot, true, now + BACKGROUND_PREFETCH_INTERVAL));
    }
}
