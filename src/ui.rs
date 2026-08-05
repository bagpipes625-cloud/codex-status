use crate::history::UsageHistoryView;
use crate::model::{
    AccountSummary, DisplayState, QuotaAvailability, QuotaKind, QuotaWindow, RefreshState,
};
use chrono::{DateTime, Datelike, Local};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BS_SOLID, BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_BOTTOM, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, ExtCreatePen, FF_SWISS,
    FONT_QUALITY, FW_NORMAL, FW_SEMIBOLD, FillRect, FillRgn, GetTextExtentPoint32W, HDC, HGDIOBJ,
    LOGBRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_ENDCAP_ROUND, PS_GEOMETRIC, PS_JOIN_ROUND,
    Polyline, SRCCOPY, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, MB_ICONERROR, MB_OK, MESSAGEBOX_STYLE, MessageBoxW, SPI_GETHIGHCONTRAST,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{PCWSTR, w};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

mod direct2d;
mod history_view;

pub use history_view::{
    HistoryHit, HistoryNavigation, HistoryPage, HoveredCycle, UsageSummaryDay,
    hit_test as history_hit_test, hovered_cycle,
};

pub const CARD_WIDTH: i32 = 376;
pub const CARD_HEIGHT: i32 = 352;
pub const COMPACT_CARD_WIDTH: i32 = 336;
pub const COMPACT_CARD_HEIGHT: i32 = 284;
pub(super) const HEADER_ACCENT_TOP: i32 = 15;
pub(super) const HEADER_ACCENT_BOTTOM: i32 = 31;
pub(super) const HEADER_TEXT_TOP: i32 = 8;
pub(super) const HEADER_TEXT_BOTTOM: i32 = 38;
pub(super) const HEADER_VERSION_TOP: i32 = HEADER_TEXT_TOP + 1;
pub(super) const HEADER_VERSION_BOTTOM: i32 = HEADER_TEXT_BOTTOM + 1;
pub(super) const REFRESH_BUTTON_RIGHT: i32 = 18;
pub(super) const REFRESH_BUTTON_RADIUS: i32 = 12;
pub(super) const REFRESH_BUTTON_GAP: i32 = 4;
pub(super) const REFRESH_ARC_START_DEGREES: f32 = 10.0;
pub(super) const REFRESH_ARC_SWEEP_DEGREES: f32 = 305.0;
pub(super) const FLYOUT_CORNER_RADIUS: i32 = 8;
const REFRESH_HIT_RADIUS: i32 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlyoutDimensions {
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct CardInteraction {
    pub pressed_quota: Option<QuotaKind>,
    pub refresh_feedback: bool,
    pub refresh_rotation_degrees: f32,
    pub hovered_cycle: Option<HoveredCycle>,
    pub hovered_history_values: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CardView<'a> {
    pub history: Option<&'a UsageHistoryView>,
    pub navigation: &'a HistoryNavigation,
    pub interaction: CardInteraction,
}

pub fn flyout_dimensions(state: &DisplayState) -> FlyoutDimensions {
    if matches!(state.quota_availability(), QuotaAvailability::Single(_)) {
        FlyoutDimensions { width: COMPACT_CARD_WIDTH, height: COMPACT_CARD_HEIGHT }
    } else {
        FlyoutDimensions { width: CARD_WIDTH, height: CARD_HEIGHT }
    }
}

#[derive(Debug, Clone, Copy)]
struct QuotaPanelState {
    kind: QuotaKind,
    slot: QuotaPanelSlot,
    selected: bool,
    pressed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuotaPanelSlot {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct QuotaPanelGeometry {
    left: i32,
    right: i32,
    center_x: i32,
}

fn quota_panel_geometry(slot: QuotaPanelSlot) -> QuotaPanelGeometry {
    match slot {
        QuotaPanelSlot::Left => QuotaPanelGeometry { left: 16, right: 184, center_x: 100 },
        QuotaPanelSlot::Right => QuotaPanelGeometry { left: 192, right: 360, center_x: 276 },
    }
}

struct AccountMetrics {
    credits: String,
    credit_detail: Option<String>,
}

struct DailyUsageMetrics {
    percent: String,
    tokens: String,
    selection: UsageSummaryDay,
}

fn daily_usage_metrics(
    history: Option<&UsageHistoryView>,
    selection: UsageSummaryDay,
    locale: Locale,
) -> DailyUsageMetrics {
    let Some(history) = history else {
        return DailyUsageMetrics { percent: "--".to_owned(), tokens: "--".to_owned(), selection };
    };
    let date = match selection {
        UsageSummaryDay::Yesterday => history.today - chrono::Duration::days(1),
        UsageSummaryDay::Today => history.today,
    };
    let Some(day) = history.day(date) else {
        return DailyUsageMetrics { percent: "--".to_owned(), tokens: "--".to_owned(), selection };
    };
    DailyUsageMetrics {
        percent: estimated_text(format_percent(day.weekly_consumed_percent), !day.quota_complete),
        tokens: day
            .tokens
            .map(|value| format_tokens(value, locale))
            .map(|value| estimated_text(value, !day.token_complete))
            .unwrap_or_else(|| "--".to_owned()),
        selection,
    }
}

fn estimated_text(value: String, estimated: bool) -> String {
    if estimated { format!("≈{value}") } else { value }
}

fn format_percent(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}%", value)
    } else {
        format!("{value:.1}%")
    }
}

fn format_tokens(value: u64, locale: Locale) -> String {
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
    if (scaled - scaled.round()).abs() < 0.05 {
        format!("{scaled:.0}{suffix}")
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

fn account_metrics(state: &DisplayState, locale: Locale) -> AccountMetrics {
    let snapshot = state.snapshot.as_ref();
    let credits = snapshot
        .and_then(|snapshot| snapshot.account.reset_credits)
        .map(|credits| format!("{credits} {}", locale.text("resets", "次")))
        .unwrap_or_else(|| "--".to_owned());
    let credit_detail =
        snapshot.and_then(|snapshot| reset_credit_detail(&snapshot.account, locale));
    AccountMetrics { credits, credit_detail }
}

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

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub tray_dark: bool,
    pub high_contrast: bool,
    background: COLORREF,
    surface: COLORREF,
    surface_alt: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    line: COLORREF,
}

pub fn detect_theme(preference: &str) -> Theme {
    let mut high_contrast =
        HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let high_contrast_enabled = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            high_contrast.cbSize,
            Some((&mut high_contrast as *mut HIGHCONTRASTW).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && high_contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
    };
    let personalize = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok();
    let system_dark = personalize
        .as_ref()
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme").ok())
        .is_some_and(|value| value == 0);
    let dark = match preference {
        "light" => false,
        "dark" => true,
        _ => system_dark,
    };
    let tray_dark = personalize
        .as_ref()
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme").ok())
        .is_some_and(|value| value == 0);

    if high_contrast_enabled {
        Theme {
            dark: true,
            tray_dark: true,
            high_contrast: true,
            background: rgb(0, 0, 0),
            surface: rgb(0, 0, 0),
            surface_alt: rgb(0, 0, 0),
            text: rgb(255, 255, 255),
            muted: rgb(255, 255, 255),
            line: rgb(255, 255, 255),
        }
    } else if dark {
        Theme {
            dark,
            tray_dark,
            high_contrast: false,
            background: rgb(32, 32, 32),
            surface: rgb(43, 43, 43),
            surface_alt: rgb(48, 48, 48),
            text: rgb(245, 245, 245),
            muted: rgb(190, 190, 190),
            line: rgb(61, 61, 61),
        }
    } else {
        Theme {
            dark,
            tray_dark,
            high_contrast: false,
            background: rgb(243, 243, 243),
            surface: rgb(255, 255, 255),
            surface_alt: rgb(248, 248, 248),
            text: rgb(24, 24, 24),
            muted: rgb(94, 94, 94),
            line: rgb(226, 226, 226),
        }
    }
}

pub fn configure_flyout(hwnd: HWND, theme: Theme) {
    unsafe {
        let dark = i32::from(theme.dark);
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of_val(&corner) as u32,
        );
    }
}

pub fn apply_flyout_shape(hwnd: HWND, dpi: u32) {
    unsafe {
        let corner = DWMWCP_ROUND;
        let set_native_corner = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of_val(&corner) as u32,
        )
        .is_ok();
        let mut applied_corner =
            windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE::default();
        let native_corner = set_native_corner
            && DwmGetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&mut applied_corner
                    as *mut windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE)
                    .cast(),
                size_of_val(&applied_corner) as u32,
            )
            .is_ok()
            && applied_corner == corner;
        if native_corner {
            let _ = SetWindowRgn(hwnd, None, true);
            return;
        }

        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let region = CreateRoundRectRgn(
            0,
            0,
            client.right.max(1) + 1,
            client.bottom.max(1) + 1,
            scale(FLYOUT_CORNER_RADIUS.saturating_mul(2), dpi).max(2),
            scale(FLYOUT_CORNER_RADIUS.saturating_mul(2), dpi).max(2),
        );
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

#[cfg_attr(feature = "diagnostics", allow(dead_code))]
pub fn show_fatal_error(message: &str) {
    let body = wide0(message);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            w!("CodexStatus"),
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONERROR.0),
        );
    }
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
            format!("CodexStatus · {} {}% · {status}", quota_remaining_label(kind, locale), percent)
        }
        None => format!("CodexStatus · {status}"),
    }
}

pub fn paint_card(
    hwnd: HWND,
    state: &DisplayState,
    preferred: QuotaKind,
    locale: Locale,
    theme: Theme,
    view: CardView<'_>,
) -> bool {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let width = (client.right - client.left).max(1);
        let height = (client.bottom - client.top).max(1);
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);

        let painted = direct2d::paint(direct2d::PaintInput {
            hwnd,
            pixel_size: (width, height),
            dpi,
            state,
            preferred,
            locale,
            theme,
            history: view.history,
            navigation: view.navigation,
            interaction: view.interaction,
        });
        if !painted {
            let buffer = CreateCompatibleDC(Some(hdc));
            let bitmap = CreateCompatibleBitmap(hdc, width, height);
            if !buffer.is_invalid() && !bitmap.is_invalid() {
                let old_bitmap = SelectObject(buffer, HGDIOBJ(bitmap.0));
                draw_card(buffer, state, preferred, locale, theme, view, dpi);
                let _ = BitBlt(hdc, 0, 0, width, height, Some(buffer), 0, 0, SRCCOPY);
                let _ = SelectObject(buffer, old_bitmap);
            } else {
                draw_card(hdc, state, preferred, locale, theme, view, dpi);
            }
            if !bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            if !buffer.is_invalid() {
                let _ = DeleteDC(buffer);
            }
        }
        let _ = EndPaint(hwnd, &paint);
        painted
    }
}

pub fn prewarm_card_renderer(
    hwnd: HWND,
    state: &DisplayState,
    preferred: QuotaKind,
    locale: Locale,
    theme: Theme,
    view: CardView<'_>,
) {
    unsafe {
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        direct2d::prewarm(direct2d::PaintInput {
            hwnd,
            pixel_size: ((client.right - client.left).max(1), (client.bottom - client.top).max(1)),
            dpi: windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96),
            state,
            preferred,
            locale,
            theme,
            history: view.history,
            navigation: view.navigation,
            interaction: view.interaction,
        });
    }
}

pub fn release_card_renderer() {
    direct2d::release_all();
}

unsafe fn draw_card(
    hdc: HDC,
    state: &DisplayState,
    preferred: QuotaKind,
    locale: Locale,
    theme: Theme,
    view: CardView<'_>,
    dpi: u32,
) {
    unsafe {
        let history = view.history;
        let navigation = view.navigation;
        let interaction = view.interaction;
        let dimensions = flyout_dimensions(state);
        let width = scale(dimensions.width, dpi);
        let height = scale(dimensions.height, dpi);
        outlined_surface(
            hdc,
            RECT { left: 0, top: 0, right: width, bottom: height },
            FLYOUT_CORNER_RADIUS,
            theme.background,
            theme.line,
            dpi,
        );
        let _ = SetBkMode(hdc, TRANSPARENT);

        if navigation.page != HistoryPage::Main {
            draw_history_fallback(hdc, history, navigation, locale, theme, dpi, dimensions);
            return;
        }

        let effective_kind = state.resolved_quota_kind(preferred);
        let selected_percent = state.quota_percent(effective_kind);
        let status_color = accent_for(state, selected_percent, theme.high_contrast);
        fill_rounded(
            hdc,
            RECT {
                left: scale(18, dpi),
                top: scale(HEADER_ACCENT_TOP, dpi),
                right: scale(20, dpi),
                bottom: scale(HEADER_ACCENT_BOTTOM, dpi),
            },
            scale(2, dpi),
            status_color,
        );
        draw_text_bottom(
            hdc,
            locale,
            "CodexStatus",
            RECT {
                left: scale(29, dpi),
                top: scale(HEADER_TEXT_TOP, dpi),
                right: scale(180, dpi),
                bottom: scale(HEADER_TEXT_BOTTOM, dpi),
            },
            scale(14, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        let title_right = scale(29, dpi)
            + measure_text_width(hdc, locale, "CodexStatus", scale(14, dpi), FW_SEMIBOLD.0 as i32);
        draw_text_bottom(
            hdc,
            locale,
            &version_text(),
            RECT {
                left: title_right,
                top: scale(HEADER_VERSION_TOP, dpi),
                right: scale(188, dpi),
                bottom: scale(HEADER_VERSION_BOTTOM, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );

        draw_text_right_bottom(
            hdc,
            locale,
            &updated_text(state, locale, interaction.refresh_feedback),
            RECT {
                left: scale(190, dpi),
                top: scale(HEADER_TEXT_TOP, dpi),
                right: width
                    - scale(
                        REFRESH_BUTTON_RIGHT + REFRESH_BUTTON_RADIUS * 2 + REFRESH_BUTTON_GAP,
                        dpi,
                    ),
                bottom: scale(HEADER_TEXT_BOTTOM, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        draw_refresh_icon(
            hdc,
            width - scale(REFRESH_BUTTON_RIGHT + REFRESH_BUTTON_RADIUS, dpi),
            scale((HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM) / 2, dpi),
            interaction.refresh_rotation_degrees,
            refresh_icon_color(theme),
            dpi,
        );

        let account = account_metrics(state, locale);
        let daily = daily_usage_metrics(history, navigation.summary_day, locale);
        match state.quota_availability() {
            QuotaAvailability::Single(kind) => {
                draw_quota_panel(
                    hdc,
                    state,
                    QuotaPanelState {
                        kind,
                        slot: QuotaPanelSlot::Left,
                        selected: false,
                        pressed: false,
                    },
                    locale,
                    theme,
                    dpi,
                );
                draw_stacked_metrics(
                    hdc,
                    locale,
                    &daily,
                    &account,
                    theme,
                    dpi,
                    interaction.hovered_history_values,
                );
            }
            QuotaAvailability::None | QuotaAvailability::Both => {
                draw_quota_panel(
                    hdc,
                    state,
                    QuotaPanelState {
                        kind: QuotaKind::FiveHour,
                        slot: QuotaPanelSlot::Left,
                        selected: preferred == QuotaKind::FiveHour,
                        pressed: interaction.pressed_quota == Some(QuotaKind::FiveHour),
                    },
                    locale,
                    theme,
                    dpi,
                );
                draw_quota_panel(
                    hdc,
                    state,
                    QuotaPanelState {
                        kind: QuotaKind::Weekly,
                        slot: QuotaPanelSlot::Right,
                        selected: preferred == QuotaKind::Weekly,
                        pressed: interaction.pressed_quota == Some(QuotaKind::Weekly),
                    },
                    locale,
                    theme,
                    dpi,
                );

                let metrics = RECT {
                    left: scale(16, dpi),
                    top: scale(276, dpi),
                    right: width - scale(16, dpi),
                    bottom: height - scale(16, dpi),
                };
                outlined_surface(hdc, metrics, 10, theme.surface_alt, theme.line, dpi);
                let divider_width = scale(1, dpi).max(1);
                let divider = scale(188, dpi);
                fill(
                    hdc,
                    RECT {
                        left: divider,
                        top: metrics.top + scale(12, dpi),
                        right: divider + divider_width,
                        bottom: metrics.bottom - scale(12, dpi),
                    },
                    theme.line,
                );
                draw_bottom_metrics(
                    hdc,
                    locale,
                    metrics,
                    divider,
                    divider_width,
                    &daily.percent,
                    &daily.tokens,
                    navigation.summary_day,
                    &account.credits,
                    account.credit_detail.as_deref(),
                    theme,
                    dpi,
                    interaction.hovered_history_values,
                );
            }
        }
    }
}

unsafe fn draw_history_fallback(
    hdc: HDC,
    history: Option<&UsageHistoryView>,
    navigation: &HistoryNavigation,
    locale: Locale,
    theme: Theme,
    dpi: u32,
    dimensions: FlyoutDimensions,
) {
    unsafe {
        let width = scale(dimensions.width, dpi);
        let height = scale(dimensions.height, dpi);
        draw_text_bottom(
            hdc,
            locale,
            locale.text("◀ Back", "◀ 返回"),
            RECT {
                left: scale(16, dpi),
                top: scale(8, dpi),
                right: scale(82, dpi),
                bottom: scale(38, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.text,
        );
        let panel = RECT {
            left: scale(16, dpi),
            top: scale(40, dpi),
            right: width - scale(16, dpi),
            bottom: height - scale(42, dpi),
        };
        outlined_surface(hdc, panel, 10, theme.surface_alt, theme.line, dpi);
        match navigation.page {
            HistoryPage::Month => {
                let title = match locale {
                    Locale::Chinese => {
                        format!("{}年{}月", navigation.month.year(), navigation.month.month())
                    }
                    Locale::English => navigation.month.format("%B %Y").to_string(),
                };
                draw_text_center(
                    hdc,
                    locale,
                    &title,
                    RECT {
                        left: panel.left,
                        top: panel.top + scale(6, dpi),
                        right: panel.right,
                        bottom: panel.top + scale(38, dpi),
                    },
                    scale(15, dpi),
                    FW_SEMIBOLD.0 as i32,
                    theme.text,
                );
                let message = if history.is_some_and(|value| !value.cycles.is_empty()) {
                    locale.text(
                        "Direct2D is unavailable; history remains recorded",
                        "Direct2D 不可用，历史数据仍会正常记录",
                    )
                } else {
                    locale.text("No history yet", "暂无历史数据")
                };
                draw_text_center(
                    hdc,
                    locale,
                    message,
                    RECT {
                        left: panel.left + scale(16, dpi),
                        top: panel.top + scale(54, dpi),
                        right: panel.right - scale(16, dpi),
                        bottom: panel.bottom - scale(20, dpi),
                    },
                    scale(11, dpi),
                    FW_NORMAL.0 as i32,
                    theme.muted,
                );
            }
            HistoryPage::Cycle => {
                let cycle = history.and_then(|view| {
                    navigation.selected_cycle.and_then(|index| view.cycles.get(index)).or_else(
                        || view.current_cycle_index().and_then(|index| view.cycles.get(index)),
                    )
                });
                let title = cycle
                    .map(|cycle| match locale {
                        Locale::Chinese => format!(
                            "{}月{}日 - {}月{}日",
                            cycle.start_date.month(),
                            cycle.start_date.day(),
                            cycle.display_end_date.month(),
                            cycle.display_end_date.day()
                        ),
                        Locale::English => format!(
                            "{} - {}",
                            cycle.start_date.format("%b %d"),
                            cycle.display_end_date.format("%b %d")
                        ),
                    })
                    .unwrap_or_else(|| locale.text("Daily usage", "单日消耗").to_owned());
                draw_text_center(
                    hdc,
                    locale,
                    &title,
                    RECT {
                        left: panel.left,
                        top: panel.top + scale(6, dpi),
                        right: panel.right,
                        bottom: panel.top + scale(38, dpi),
                    },
                    scale(15, dpi),
                    FW_SEMIBOLD.0 as i32,
                    theme.text,
                );
                draw_text_center(
                    hdc,
                    locale,
                    locale.text(
                        "Direct2D is unavailable; history remains recorded",
                        "Direct2D 不可用，历史数据仍会正常记录",
                    ),
                    RECT {
                        left: panel.left + scale(16, dpi),
                        top: panel.top + scale(54, dpi),
                        right: panel.right - scale(16, dpi),
                        bottom: panel.bottom - scale(20, dpi),
                    },
                    scale(11, dpi),
                    FW_NORMAL.0 as i32,
                    theme.muted,
                );
            }
            HistoryPage::Main => {}
        }
    }
}

unsafe fn draw_quota_panel(
    hdc: HDC,
    state: &DisplayState,
    panel: QuotaPanelState,
    locale: Locale,
    theme: Theme,
    dpi: u32,
) {
    unsafe {
        let geometry = quota_panel_geometry(panel.slot);
        let rect = quota_card_rect(panel.slot, dpi);
        let (surface, border) = quota_card_colors(theme, panel.selected, panel.pressed);
        outlined_surface(hdc, rect, 10, surface, border, dpi);
        if theme.high_contrast && (panel.selected || panel.pressed) {
            let marker_width = scale(if panel.pressed { 52 } else { 36 }, dpi);
            let marker_height = scale(if panel.pressed { 5 } else { 3 }, dpi).max(1);
            let center = (rect.left + rect.right) / 2;
            fill_rounded(
                hdc,
                RECT {
                    left: center - marker_width / 2,
                    top: rect.bottom - scale(8, dpi),
                    right: center + marker_width / 2,
                    bottom: rect.bottom - scale(8, dpi) + marker_height,
                },
                marker_height,
                theme.text,
            );
        }

        let center_x = scale(geometry.center_x, dpi);
        let center_y = scale(149, dpi);
        let window = state.quota_window(panel.kind);
        let actual = window.map(QuotaWindow::display_percent);
        let theoretical = window
            .and_then(|window| theoretical_remaining_percent(window, Local::now().timestamp()));
        let title_color = if !panel.selected || theme.high_contrast {
            theme.muted
        } else if theme.dark {
            rgb(115, 204, 175)
        } else {
            rgb(8, 125, 97)
        };

        draw_text_center(
            hdc,
            locale,
            quota_label(panel.kind, locale),
            RECT {
                left: rect.left + scale(8, dpi),
                top: scale(47, dpi),
                right: rect.right - scale(8, dpi),
                bottom: scale(78, dpi),
            },
            scale(15, dpi),
            FW_SEMIBOLD.0 as i32,
            title_color,
        );

        draw_progress_arc(
            hdc,
            center_x,
            center_y,
            scale(66, dpi),
            scale(10, dpi).max(1),
            100,
            outer_track_color(theme, panel.selected),
        );
        if let Some(percent) = actual.filter(|percent| *percent > 0) {
            draw_progress_arc(
                hdc,
                center_x,
                center_y,
                scale(66, dpi),
                scale(10, dpi).max(1),
                percent,
                quota_bar_color(actual, theme.high_contrast),
            );
        }
        draw_progress_arc(
            hdc,
            center_x,
            center_y,
            scale(54, dpi),
            scale(8, dpi).max(1),
            100,
            inner_track_color(theme),
        );
        if let Some(percent) = theoretical.filter(|percent| *percent > 0) {
            draw_progress_arc(
                hdc,
                center_x,
                center_y,
                scale(54, dpi),
                scale(8, dpi).max(1),
                percent,
                theoretical_color(theme),
            );
        }
        draw_centered_percentage(hdc, center_x, actual, locale, theme, dpi);

        let reset = window
            .map(|window| reset_details(window, locale))
            .unwrap_or_else(|| (locale.text("Unavailable", "暂无").to_owned(), "--".to_owned()));
        draw_text_center(
            hdc,
            locale,
            &reset.0,
            RECT {
                left: rect.left + scale(8, dpi),
                top: scale(199, dpi),
                right: rect.right - scale(8, dpi),
                bottom: scale(232, dpi),
            },
            scale(14, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        draw_text_center(
            hdc,
            locale,
            &reset.1,
            RECT {
                left: rect.left + scale(8, dpi),
                top: scale(225, dpi),
                right: rect.right - scale(8, dpi),
                bottom: scale(256, dpi),
            },
            scale(12, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
    }
}

unsafe fn draw_centered_percentage(
    hdc: HDC,
    center_x: i32,
    percent: Option<u8>,
    locale: Locale,
    theme: Theme,
    dpi: u32,
) {
    unsafe {
        let Some(percent) = percent else {
            draw_text_center(
                hdc,
                locale,
                "--",
                RECT {
                    left: center_x - scale(55, dpi),
                    top: scale(103, dpi),
                    right: center_x + scale(55, dpi),
                    bottom: scale(173, dpi),
                },
                scale(36, dpi),
                FW_SEMIBOLD.0 as i32,
                theme.text,
            );
            return;
        };
        let number = percent.to_string();
        let number_width =
            measure_text_width(hdc, locale, &number, scale(36, dpi), FW_SEMIBOLD.0 as i32);
        let percent_width =
            measure_text_width(hdc, locale, "%", scale(15, dpi), FW_SEMIBOLD.0 as i32);
        let gap = scale(3, dpi);
        let group_width = number_width + gap + percent_width;
        let left = center_x - group_width / 2;
        let bottom = scale(172, dpi);
        draw_text_bottom(
            hdc,
            locale,
            &number,
            RECT { left, top: scale(102, dpi), right: left + number_width, bottom },
            scale(36, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        draw_text_bottom(
            hdc,
            locale,
            "%",
            RECT {
                left: left + number_width + gap,
                top: scale(122, dpi),
                right: left + group_width,
                bottom,
            },
            scale(15, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.muted,
        );
    }
}

unsafe fn draw_stacked_metrics(
    hdc: HDC,
    locale: Locale,
    daily: &DailyUsageMetrics,
    account: &AccountMetrics,
    theme: Theme,
    dpi: u32,
    hovered_history_values: bool,
) {
    unsafe {
        let metrics = RECT {
            left: scale(192, dpi),
            top: scale(40, dpi),
            right: scale(320, dpi),
            bottom: scale(268, dpi),
        };
        outlined_surface(hdc, metrics, 10, theme.surface, theme.line, dpi);
        let divider_top = scale(153, dpi);
        fill(
            hdc,
            RECT {
                left: scale(208, dpi),
                top: divider_top,
                right: scale(304, dpi),
                bottom: divider_top + scale(1, dpi).max(1),
            },
            theme.line,
        );
        draw_text_center(
            hdc,
            locale,
            match daily.selection {
                UsageSummaryDay::Yesterday => locale.text("Yesterday ▶", "昨日消耗 ▶"),
                UsageSummaryDay::Today => locale.text("◀ Today", "◀ 今日消耗"),
            },
            RECT {
                left: metrics.left + scale(8, dpi),
                top: scale(59, dpi),
                right: metrics.right - scale(8, dpi),
                bottom: scale(83, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        draw_text_center(
            hdc,
            locale,
            &daily.percent,
            RECT {
                left: metrics.left + scale(8, dpi),
                top: scale(87, dpi),
                right: metrics.right - scale(8, dpi),
                bottom: scale(130, dpi),
            },
            scale(20, dpi),
            FW_SEMIBOLD.0 as i32,
            interactive_text_color(theme, hovered_history_values, false),
        );
        draw_text_center(
            hdc,
            locale,
            &daily.tokens,
            RECT {
                left: metrics.left + scale(8, dpi),
                top: scale(120, dpi),
                right: metrics.right - scale(8, dpi),
                bottom: scale(145, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            interactive_text_color(theme, hovered_history_values, true),
        );
        draw_text_center(
            hdc,
            locale,
            locale.text("Reset credits", "重置机会"),
            RECT {
                left: metrics.left + scale(8, dpi),
                top: scale(173, dpi),
                right: metrics.right - scale(8, dpi),
                bottom: scale(197, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        draw_text_center(
            hdc,
            locale,
            &account.credits,
            RECT {
                left: metrics.left + scale(8, dpi),
                top: scale(198, dpi),
                right: metrics.right - scale(8, dpi),
                bottom: scale(236, dpi),
            },
            scale(20, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        if let Some(detail) = account.credit_detail.as_deref() {
            draw_text_center(
                hdc,
                locale,
                detail,
                RECT {
                    left: metrics.left + scale(5, dpi),
                    top: scale(237, dpi),
                    right: metrics.right - scale(5, dpi),
                    bottom: scale(262, dpi),
                },
                scale(11, dpi),
                FW_NORMAL.0 as i32,
                theme.muted,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_bottom_metrics(
    hdc: HDC,
    locale: Locale,
    metrics: RECT,
    divider: i32,
    divider_width: i32,
    daily_percent: &str,
    daily_tokens: &str,
    selection: UsageSummaryDay,
    credits: &str,
    credit_detail: Option<&str>,
    theme: Theme,
    dpi: u32,
    hovered_history_values: bool,
) {
    unsafe {
        let left = metrics.left + scale(14, dpi);
        let right_left = divider + divider_width + scale(14, dpi);
        draw_text(
            hdc,
            locale,
            match selection {
                UsageSummaryDay::Yesterday => locale.text("Yesterday ▶", "昨日消耗 ▶"),
                UsageSummaryDay::Today => locale.text("◀ Today", "◀ 今日消耗"),
            },
            RECT {
                left,
                top: metrics.top + scale(3, dpi),
                right: divider - scale(10, dpi),
                bottom: metrics.top + scale(25, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        draw_text_bottom(
            hdc,
            locale,
            daily_percent,
            RECT {
                left,
                top: metrics.top + scale(20, dpi),
                right: divider - scale(10, dpi),
                bottom: metrics.bottom - scale(8, dpi),
            },
            scale(20, dpi),
            FW_SEMIBOLD.0 as i32,
            interactive_text_color(theme, hovered_history_values, false),
        );
        let percent_width =
            measure_text_width(hdc, locale, daily_percent, scale(20, dpi), FW_SEMIBOLD.0 as i32);
        draw_text_bottom(
            hdc,
            locale,
            daily_tokens,
            RECT {
                left: (left + percent_width + scale(10, dpi)).min(divider - scale(50, dpi)),
                top: metrics.top + scale(24, dpi),
                right: divider - scale(10, dpi),
                bottom: metrics.bottom - scale(7, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            interactive_text_color(theme, hovered_history_values, true),
        );
        draw_text(
            hdc,
            locale,
            locale.text("Reset credits", "重置机会"),
            RECT {
                left: right_left,
                top: metrics.top + scale(3, dpi),
                right: metrics.right - scale(10, dpi),
                bottom: metrics.top + scale(25, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        let value_width =
            measure_text_width(hdc, locale, credits, scale(20, dpi), FW_SEMIBOLD.0 as i32);
        let value_right = (right_left + value_width).min(metrics.right - scale(10, dpi));
        let value_bottom = metrics.bottom - scale(10, dpi);
        draw_text_bottom(
            hdc,
            locale,
            credits,
            RECT {
                left: right_left,
                top: metrics.top + scale(20, dpi),
                right: value_right,
                bottom: value_bottom,
            },
            scale(20, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        if let Some(detail) = credit_detail {
            draw_text_right_bottom(
                hdc,
                locale,
                detail,
                RECT {
                    left: value_right + scale(10, dpi),
                    top: metrics.top + scale(24, dpi),
                    right: metrics.right - scale(10, dpi),
                    bottom: metrics.bottom - scale(7, dpi),
                },
                scale(11, dpi),
                FW_NORMAL.0 as i32,
                theme.muted,
            );
        }
    }
}

fn theoretical_remaining_percent(window: &QuotaWindow, now: i64) -> Option<u8> {
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

fn arc_points(center_x: i32, center_y: i32, radius: i32, percent: u8) -> Vec<POINT> {
    const START_DEGREES: f64 = 145.0;
    const SWEEP_DEGREES: f64 = 250.0;
    let percent = percent.min(100);
    let sweep = SWEEP_DEGREES * f64::from(percent) / 100.0;
    let segments = ((sweep / 4.0).ceil() as usize).max(1);
    (0..=segments)
        .map(|index| {
            let fraction = index as f64 / segments as f64;
            let radians = (START_DEGREES + sweep * fraction).to_radians();
            POINT {
                x: center_x + (f64::from(radius) * radians.cos()).round() as i32,
                y: center_y + (f64::from(radius) * radians.sin()).round() as i32,
            }
        })
        .collect()
}

unsafe fn draw_progress_arc(
    hdc: HDC,
    center_x: i32,
    center_y: i32,
    radius: i32,
    width: i32,
    percent: u8,
    color: COLORREF,
) {
    if percent == 0 || radius <= 0 || width <= 0 {
        return;
    }
    let points = arc_points(center_x, center_y, radius, percent);
    let brush = LOGBRUSH { lbStyle: BS_SOLID, lbColor: color, lbHatch: 0 };
    unsafe {
        let pen = ExtCreatePen(
            PS_GEOMETRIC | PS_ENDCAP_ROUND | PS_JOIN_ROUND,
            width as u32,
            &brush,
            None,
        );
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, HGDIOBJ(pen.0));
        let _ = Polyline(hdc, &points);
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

unsafe fn draw_refresh_icon(
    hdc: HDC,
    center_x: i32,
    center_y: i32,
    rotation_degrees: f32,
    color: COLORREF,
    dpi: u32,
) {
    let radius = scale(6, dpi);
    let rotation = f64::from(rotation_degrees);
    let arc: [POINT; 22] = std::array::from_fn(|index| {
        let angle = (rotation
            + f64::from(REFRESH_ARC_START_DEGREES)
            + f64::from(REFRESH_ARC_SWEEP_DEGREES) * index as f64 / 21.0)
            .to_radians();
        POINT {
            x: center_x + (f64::from(radius) * angle.cos()).round() as i32,
            y: center_y + (f64::from(radius) * angle.sin()).round() as i32,
        }
    });
    let arrow = [
        POINT { x: scale(5, dpi), y: -scale(7, dpi) },
        POINT { x: scale(5, dpi), y: -scale(3, dpi) },
        POINT { x: scale(1, dpi), y: -scale(3, dpi) },
    ]
    .map(|point| {
        let radians = rotation.to_radians();
        let cosine = radians.cos();
        let sine = radians.sin();
        POINT {
            x: center_x + (f64::from(point.x) * cosine - f64::from(point.y) * sine).round() as i32,
            y: center_y + (f64::from(point.x) * sine + f64::from(point.y) * cosine).round() as i32,
        }
    });
    let brush = LOGBRUSH { lbStyle: BS_SOLID, lbColor: color, lbHatch: 0 };
    unsafe {
        let pen = ExtCreatePen(
            PS_GEOMETRIC | PS_ENDCAP_ROUND | PS_JOIN_ROUND,
            scale(2, dpi).max(1) as u32,
            &brush,
            None,
        );
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, HGDIOBJ(pen.0));
        let _ = Polyline(hdc, &arc);
        let _ = Polyline(hdc, &arrow);
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

fn refresh_icon_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.text
    } else if theme.dark {
        rgb(188, 198, 203)
    } else {
        rgb(102, 117, 124)
    }
}

fn quota_card_colors(theme: Theme, selected: bool, pressed: bool) -> (COLORREF, COLORREF) {
    if theme.high_contrast {
        return (theme.surface, theme.text);
    }
    match (theme.dark, selected, pressed) {
        (false, true, true) => (rgb(226, 244, 238), rgb(120, 196, 175)),
        (false, true, false) => (rgb(237, 248, 245), rgb(159, 212, 197)),
        (false, false, true) => (theme.surface_alt, rgb(207, 209, 209)),
        (false, false, false) => (theme.surface, theme.line),
        (true, true, true) => (rgb(36, 72, 62), rgb(70, 151, 127)),
        (true, true, false) => (rgb(32, 59, 52), rgb(55, 128, 106)),
        (true, false, true) => (theme.surface_alt, rgb(91, 91, 91)),
        (true, false, false) => (theme.surface, theme.line),
    }
}

fn outer_track_color(theme: Theme, selected: bool) -> COLORREF {
    if theme.high_contrast {
        theme.surface
    } else if theme.dark {
        if selected { rgb(62, 86, 79) } else { rgb(68, 72, 71) }
    } else if selected {
        rgb(216, 228, 224)
    } else {
        rgb(223, 227, 226)
    }
}

fn inner_track_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.surface
    } else if theme.dark {
        rgb(69, 75, 78)
    } else {
        rgb(217, 223, 226)
    }
}

fn theoretical_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.text
    } else if theme.dark {
        rgb(145, 168, 180)
    } else {
        rgb(129, 150, 162)
    }
}

unsafe fn outlined_surface(
    hdc: HDC,
    rect: RECT,
    radius_dip: i32,
    surface: COLORREF,
    border_color: COLORREF,
    dpi: u32,
) {
    unsafe {
        let diameter = scale(radius_dip.saturating_mul(2), dpi);
        fill_rounded(hdc, rect, diameter, border_color);
        let border = scale(1, dpi).max(1);
        fill_rounded(
            hdc,
            RECT {
                left: rect.left + border,
                top: rect.top + border,
                right: rect.right - border,
                bottom: rect.bottom - border,
            },
            (diameter - border.saturating_mul(2)).max(1),
            surface,
        );
    }
}

fn updated_text(state: &DisplayState, locale: Locale, refresh_feedback: bool) -> String {
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

pub fn refresh_button_hit_test(state: &DisplayState, x: i32, y: i32, dpi: u32) -> bool {
    let dimensions = flyout_dimensions(state);
    let center_x = scale(dimensions.width - REFRESH_BUTTON_RIGHT - REFRESH_BUTTON_RADIUS, dpi);
    let center_y = scale((HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM) / 2, dpi);
    let radius = scale(REFRESH_HIT_RADIUS, dpi);
    x >= center_x - radius
        && x < center_x + radius
        && y >= center_y - radius
        && y < center_y + radius
}

fn version_text() -> String {
    format!(" - v{}", env!("CARGO_PKG_VERSION"))
}

fn reset_details(window: &QuotaWindow, locale: Locale) -> (String, String) {
    let Some(reset) = window.resets_at else {
        return (
            locale.text("Unavailable", "暂无").to_owned(),
            locale.text("Reset time", "重置时间").to_owned(),
        );
    };
    let now = Local::now().timestamp();
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

fn quota_label(kind: QuotaKind, locale: Locale) -> &'static str {
    match kind {
        QuotaKind::FiveHour => locale.text("5-hour quota", "5小时额度"),
        QuotaKind::Weekly => locale.text("Weekly quota", "本周额度"),
    }
}

fn quota_bar_color(percent: Option<u8>, high_contrast: bool) -> COLORREF {
    if high_contrast {
        return rgb(255, 255, 255);
    }
    match percent {
        Some(value) if value > 49 => rgb(16, 163, 127),
        Some(value) if value > 19 => rgb(210, 134, 0),
        Some(_) => rgb(211, 64, 73),
        None => rgb(104, 109, 118),
    }
}

fn interactive_text_color(theme: Theme, hovered: bool, secondary: bool) -> COLORREF {
    if !hovered || theme.high_contrast {
        return if secondary { theme.muted } else { theme.text };
    }
    if theme.dark { rgb(112, 194, 175) } else { rgb(18, 126, 103) }
}

fn accent_for(state: &DisplayState, percent: Option<u8>, high_contrast: bool) -> COLORREF {
    if high_contrast {
        return rgb(255, 255, 255);
    }
    if state.refresh_state != RefreshState::Live {
        return rgb(91, 123, 153);
    }
    match percent {
        Some(value) if value < 20 => rgb(211, 64, 73),
        Some(value) if value < 50 => rgb(210, 134, 0),
        Some(_) => rgb(16, 163, 127),
        None => rgb(104, 109, 118),
    }
}

fn quota_card_rect(slot: QuotaPanelSlot, dpi: u32) -> RECT {
    let geometry = quota_panel_geometry(slot);
    RECT {
        left: scale(geometry.left, dpi),
        top: scale(40, dpi),
        right: scale(geometry.right, dpi),
        bottom: scale(268, dpi),
    }
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

unsafe fn draw_text(
    hdc: HDC,
    locale: Locale,
    value: &str,
    rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
) {
    let style = TextStyle { height, weight, color };
    unsafe { draw_text_with_alignment(hdc, locale, value, rect, style, DT_LEFT) }
}

unsafe fn draw_text_bottom(
    hdc: HDC,
    locale: Locale,
    value: &str,
    rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
) {
    let style = TextStyle { height, weight, color };
    unsafe { draw_text_with_bottom_alignment(hdc, locale, value, rect, style, DT_LEFT) }
}

unsafe fn draw_text_right_bottom(
    hdc: HDC,
    locale: Locale,
    value: &str,
    rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
) {
    let style = TextStyle { height, weight, color };
    unsafe { draw_text_with_bottom_alignment(hdc, locale, value, rect, style, DT_RIGHT) }
}

unsafe fn draw_text_center(
    hdc: HDC,
    locale: Locale,
    value: &str,
    rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
) {
    let style = TextStyle { height, weight, color };
    unsafe { draw_text_with_alignment(hdc, locale, value, rect, style, DT_CENTER) }
}

#[derive(Clone, Copy)]
struct TextStyle {
    height: i32,
    weight: i32,
    color: COLORREF,
}

unsafe fn draw_text_with_alignment(
    hdc: HDC,
    locale: Locale,
    value: &str,
    mut rect: RECT,
    style: TextStyle,
    alignment: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    unsafe {
        let font = create_ui_font(locale, style.height, style.weight);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetTextColor(hdc, style.color);
        let mut text: Vec<u16> = value.encode_utf16().collect();
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut rect,
            alignment | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

unsafe fn draw_text_with_bottom_alignment(
    hdc: HDC,
    locale: Locale,
    value: &str,
    mut rect: RECT,
    style: TextStyle,
    alignment: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    unsafe {
        let font = create_ui_font(locale, style.height, style.weight);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetTextColor(hdc, style.color);
        let mut text: Vec<u16> = value.encode_utf16().collect();
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut rect,
            alignment | DT_BOTTOM | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

unsafe fn measure_text_width(
    hdc: HDC,
    locale: Locale,
    value: &str,
    height: i32,
    weight: i32,
) -> i32 {
    unsafe {
        let font = create_ui_font(locale, height, weight);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let text: Vec<u16> = value.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &text, &mut size);
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        size.cx.max(0)
    }
}

unsafe fn create_ui_font(
    locale: Locale,
    height: i32,
    weight: i32,
) -> windows::Win32::Graphics::Gdi::HFONT {
    let face = wide0(ui_font_face(locale));
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            FONT_QUALITY(CLEARTYPE_QUALITY.0),
            u32::from(DEFAULT_PITCH.0 | FF_SWISS.0),
            PCWSTR(face.as_ptr()),
        )
    }
}

const fn ui_font_face(_locale: Locale) -> &'static str {
    "Microsoft YaHei UI"
}

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let _ = FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

unsafe fn fill_rounded(hdc: HDC, rect: RECT, diameter: i32, color: COLORREF) {
    unsafe {
        let region = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            diameter.max(1),
            diameter.max(1),
        );
        let brush = CreateSolidBrush(color);
        let _ = FillRgn(hdc, region, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
}

pub fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | ((green as u32) << 8) | ((blue as u32) << 16))
}

fn wide0(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::DailyUsageRecord;
    use crate::model::{AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, WEEK_MINUTES};
    use chrono::NaiveDate;

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
    fn header_text_row_is_centered_on_the_status_accent() {
        assert_eq!(HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM, HEADER_ACCENT_TOP + HEADER_ACCENT_BOTTOM);
        assert_eq!(HEADER_VERSION_TOP, HEADER_TEXT_TOP + 1);
        assert_eq!(HEADER_VERSION_BOTTOM, HEADER_TEXT_BOTTOM + 1);
    }

    #[test]
    fn refresh_feedback_replaces_the_timestamp_without_changing_display_state() {
        let state = display(true, true);
        assert_eq!(updated_text(&state, Locale::Chinese, true), "刷新中…");
        assert_eq!(state.refresh_state, RefreshState::Live);
    }

    #[test]
    fn refresh_hit_target_tracks_compact_and_full_widths() {
        for state in [display(true, false), display(true, true)] {
            let width = flyout_dimensions(&state).width;
            let center_x = width - REFRESH_BUTTON_RIGHT - REFRESH_BUTTON_RADIUS;
            let center_y = (HEADER_TEXT_TOP + HEADER_TEXT_BOTTOM) / 2;
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
    fn arc_geometry_keeps_the_requested_opening_and_progress() {
        let full = arc_points(100, 149, 66, 100);
        let half = arc_points(100, 149, 66, 50);
        assert_eq!(full.first(), Some(&POINT { x: 46, y: 187 }));
        assert_eq!(full.last(), Some(&POINT { x: 154, y: 187 }));
        assert_eq!(half.first(), full.first());
        assert!(half.last().is_some_and(|point| point.y < 100));
    }

    #[test]
    fn reset_countdown_never_goes_negative() {
        let window = QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes: 10_080,
            resets_at: Some(1),
        };
        let (countdown, _) = reset_details(&window, Locale::English);
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
        use chrono::TimeZone;

        let reset = Local.with_ymd_and_hms(2026, 8, 2, 11, 0, 0).single().unwrap().timestamp();
        let window = QuotaWindow {
            used_percent: 0.0,
            remaining_percent: 100.0,
            window_minutes: 10_080,
            resets_at: Some(reset),
        };

        assert_eq!(reset_details(&window, Locale::Chinese).1, "8月2日 11:00");
        assert_eq!(reset_details(&window, Locale::English).1, "08/02 11:00");
    }

    #[test]
    fn formats_the_nearest_reset_credit_expiration_to_minutes() {
        use chrono::TimeZone;

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
    fn colors_quota_bar_at_requested_thresholds() {
        assert_eq!(quota_bar_color(Some(50), false), rgb(16, 163, 127));
        assert_eq!(quota_bar_color(Some(49), false), rgb(210, 134, 0));
        assert_eq!(quota_bar_color(Some(20), false), rgb(210, 134, 0));
        assert_eq!(quota_bar_color(Some(19), false), rgb(211, 64, 73));
        assert_eq!(quota_bar_color(Some(0), false), rgb(211, 64, 73));
    }

    #[test]
    fn history_values_gain_subtle_hover_feedback_without_overriding_high_contrast() {
        for dark in [false, true] {
            let theme = Theme {
                dark,
                tray_dark: dark,
                high_contrast: false,
                background: rgb(240, 240, 240),
                surface: rgb(255, 255, 255),
                surface_alt: rgb(248, 248, 248),
                text: rgb(24, 24, 24),
                muted: rgb(96, 96, 96),
                line: rgb(220, 220, 220),
            };
            assert_eq!(interactive_text_color(theme, false, false), theme.text);
            assert_eq!(interactive_text_color(theme, false, true), theme.muted);
            assert_ne!(interactive_text_color(theme, true, false), theme.text);
            assert_ne!(interactive_text_color(theme, true, true), theme.muted);

            let high_contrast = Theme { high_contrast: true, ..theme };
            assert_eq!(interactive_text_color(high_contrast, true, false), theme.text);
            assert_eq!(interactive_text_color(high_contrast, true, true), theme.muted);
        }
    }

    #[test]
    fn high_contrast_tracks_do_not_mask_the_progress_arcs() {
        let theme = Theme {
            dark: true,
            tray_dark: true,
            high_contrast: true,
            background: rgb(0, 0, 0),
            surface: rgb(0, 0, 0),
            surface_alt: rgb(0, 0, 0),
            text: rgb(255, 255, 255),
            muted: rgb(255, 255, 255),
            line: rgb(255, 255, 255),
        };
        assert_ne!(outer_track_color(theme, true), quota_bar_color(Some(50), true));
        assert_ne!(inner_track_color(theme), theoretical_color(theme));
    }

    #[test]
    fn daily_summary_uses_the_selected_day_and_compact_units() {
        let view = UsageHistoryView {
            days: vec![
                DailyUsageRecord {
                    date: "2026-08-04".to_owned(),
                    weekly_consumed_percent: 7.0,
                    tokens: Some(120_000_000),
                    quota_complete: true,
                    token_complete: true,
                },
                DailyUsageRecord {
                    date: "2026-08-05".to_owned(),
                    weekly_consumed_percent: 2.25,
                    tokens: Some(25_000),
                    quota_complete: true,
                    token_complete: true,
                },
            ],
            cycles: Vec::new(),
            today: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
        };

        let yesterday =
            daily_usage_metrics(Some(&view), UsageSummaryDay::Yesterday, Locale::Chinese);
        assert_eq!(yesterday.percent, "7%");
        assert_eq!(yesterday.tokens, "1.2亿");
        let today = daily_usage_metrics(Some(&view), UsageSummaryDay::Today, Locale::Chinese);
        assert_eq!(today.percent, "2.2%");
        assert_eq!(today.tokens, "2.5万");
    }

    #[test]
    fn daily_summary_distinguishes_no_history_from_a_zero_day() {
        let mut view = UsageHistoryView {
            days: vec![DailyUsageRecord {
                date: "2026-08-05".to_owned(),
                weekly_consumed_percent: 0.0,
                tokens: Some(0),
                quota_complete: true,
                token_complete: true,
            }],
            cycles: Vec::new(),
            today: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
        };
        let missing = daily_usage_metrics(Some(&view), UsageSummaryDay::Yesterday, Locale::Chinese);
        assert_eq!(missing.percent, "--");
        assert_eq!(missing.tokens, "--");
        let zero = daily_usage_metrics(Some(&view), UsageSummaryDay::Today, Locale::Chinese);
        assert_eq!(zero.percent, "0%");
        assert_eq!(zero.tokens, "0");
        view.days[0].quota_complete = false;
        view.days[0].token_complete = false;
        let estimated = daily_usage_metrics(Some(&view), UsageSummaryDay::Today, Locale::Chinese);
        assert_eq!(estimated.percent, "≈0%");
        assert_eq!(estimated.tokens, "≈0");
        let unavailable = daily_usage_metrics(None, UsageSummaryDay::Yesterday, Locale::Chinese);
        assert_eq!(unavailable.percent, "--");
        assert_eq!(unavailable.tokens, "--");
    }
}
