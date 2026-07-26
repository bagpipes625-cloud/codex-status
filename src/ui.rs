use crate::model::{DisplayState, QuotaWindow, RefreshState};
use chrono::{DateTime, Local};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Dwm::{
    DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_SWISS, FONT_QUALITY, FW_NORMAL, FW_SEMIBOLD,
    FillRect, FillRgn, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, MB_ICONERROR, MB_OK, MESSAGEBOX_STYLE, MessageBoxW, SPI_GETHIGHCONTRAST,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{PCWSTR, w};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

pub const CARD_WIDTH: i32 = 356;
pub const CARD_HEIGHT: i32 = 252;

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
    pub high_contrast: bool,
    background: COLORREF,
    surface: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    line: COLORREF,
}

pub fn detect_theme() -> Theme {
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
    let dark = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok()
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme").ok())
        .is_some_and(|value| value == 0);

    if high_contrast_enabled {
        Theme {
            dark: true,
            high_contrast: true,
            background: rgb(0, 0, 0),
            surface: rgb(0, 0, 0),
            text: rgb(255, 255, 255),
            muted: rgb(255, 255, 255),
            line: rgb(255, 255, 255),
        }
    } else if dark {
        Theme {
            dark,
            high_contrast: false,
            background: rgb(31, 33, 37),
            surface: rgb(40, 43, 48),
            text: rgb(247, 248, 250),
            muted: rgb(168, 175, 185),
            line: rgb(59, 63, 70),
        }
    } else {
        Theme {
            dark,
            high_contrast: false,
            background: rgb(245, 247, 250),
            surface: rgb(255, 255, 255),
            text: rgb(24, 27, 32),
            muted: rgb(99, 108, 121),
            line: rgb(226, 230, 235),
        }
    }
}

pub fn configure_flyout(hwnd: HWND, theme: Theme) {
    unsafe {
        let dark = i32::from(theme.dark);
        let corner = DWMWCP_ROUND;
        let backdrop = DWMSBT_TRANSIENTWINDOW;
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
        if !theme.high_contrast {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&backdrop as *const windows::Win32::Graphics::Dwm::DWM_SYSTEMBACKDROP_TYPE).cast(),
                size_of_val(&backdrop) as u32,
            );
        }
    }
}

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

pub fn tooltip(state: &DisplayState, locale: Locale) -> String {
    let status = match state.refresh_state {
        RefreshState::Loading => locale.text("refreshing", "刷新中"),
        RefreshState::Live => locale.text("live", "实时"),
        RefreshState::Cached => locale.text("cached", "缓存"),
        RefreshState::Unavailable => locale.text("unavailable", "不可用"),
    };
    match state.weekly_percent() {
        Some(percent) => format!(
            "CodexStatus · {} {}% · {status}",
            locale.text("weekly remaining", "周剩余"),
            percent
        ),
        None => format!("CodexStatus · {status}"),
    }
}

pub fn paint_card(hwnd: HWND, state: &DisplayState, locale: Locale, theme: Theme) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let width = (client.right - client.left).max(1);
        let height = (client.bottom - client.top).max(1);
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);

        let buffer = CreateCompatibleDC(Some(hdc));
        let bitmap = CreateCompatibleBitmap(hdc, width, height);
        if !buffer.is_invalid() && !bitmap.is_invalid() {
            let old_bitmap = SelectObject(buffer, HGDIOBJ(bitmap.0));
            draw_card(buffer, state, locale, theme, dpi);
            let _ = BitBlt(hdc, 0, 0, width, height, Some(buffer), 0, 0, SRCCOPY);
            let _ = SelectObject(buffer, old_bitmap);
        } else {
            draw_card(hdc, state, locale, theme, dpi);
        }
        if !bitmap.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        if !buffer.is_invalid() {
            let _ = DeleteDC(buffer);
        }
        let _ = EndPaint(hwnd, &paint);
    }
}

unsafe fn draw_card(hdc: HDC, state: &DisplayState, locale: Locale, theme: Theme, dpi: u32) {
    unsafe {
        let width = scale(CARD_WIDTH, dpi);
        let height = scale(CARD_HEIGHT, dpi);
        fill(hdc, RECT { left: 0, top: 0, right: width, bottom: height }, theme.background);
        let _ = SetBkMode(hdc, TRANSPARENT);

        let status_color = accent_for(state, theme.high_contrast);
        fill_rounded(
            hdc,
            RECT {
                left: scale(18, dpi),
                top: scale(19, dpi),
                right: scale(26, dpi),
                bottom: scale(27, dpi),
            },
            scale(8, dpi),
            status_color,
        );
        draw_text(
            hdc,
            "CodexStatus",
            RECT {
                left: scale(35, dpi),
                top: scale(9, dpi),
                right: scale(180, dpi),
                bottom: scale(38, dpi),
            },
            scale(13, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );

        draw_text_right(
            hdc,
            &updated_text(state, locale),
            RECT {
                left: scale(178, dpi),
                top: scale(10, dpi),
                right: width - scale(18, dpi),
                bottom: scale(38, dpi),
            },
            scale(10, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );

        let hero = RECT {
            left: scale(16, dpi),
            top: scale(46, dpi),
            right: width - scale(16, dpi),
            bottom: scale(165, dpi),
        };
        fill_rounded(hdc, hero, scale(14, dpi), theme.line);
        let border = scale(1, dpi).max(1);
        fill_rounded(
            hdc,
            RECT {
                left: hero.left + border,
                top: hero.top + border,
                right: hero.right - border,
                bottom: hero.bottom - border,
            },
            scale(13, dpi),
            theme.surface,
        );

        draw_text(
            hdc,
            locale.text("Weekly remaining", "本周剩余"),
            RECT {
                left: scale(32, dpi),
                top: scale(57, dpi),
                right: scale(169, dpi),
                bottom: scale(81, dpi),
            },
            scale(11, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.muted,
        );
        let percent = state.weekly_percent().map_or_else(|| "--".to_owned(), |v| v.to_string());
        let percent = if percent == "--" { percent } else { format!("{percent}%") };
        draw_text(
            hdc,
            &percent,
            RECT {
                left: scale(30, dpi),
                top: scale(76, dpi),
                right: scale(169, dpi),
                bottom: scale(130, dpi),
            },
            scale(36, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );

        fill(
            hdc,
            RECT {
                left: scale(177, dpi),
                top: scale(66, dpi),
                right: scale(178, dpi),
                bottom: scale(127, dpi),
            },
            theme.line,
        );

        let reset = state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.weekly.as_ref())
            .map(|window| reset_details(window, locale))
            .unwrap_or_else(|| {
                (
                    locale.text("Unavailable", "暂无").to_owned(),
                    locale.text("Reset time", "重置时间").to_owned(),
                )
            });
        draw_text(
            hdc,
            locale.text("Reset in", "距离重置"),
            RECT {
                left: scale(193, dpi),
                top: scale(58, dpi),
                right: width - scale(31, dpi),
                bottom: scale(80, dpi),
            },
            scale(10, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.muted,
        );
        draw_text(
            hdc,
            &reset.0,
            RECT {
                left: scale(193, dpi),
                top: scale(79, dpi),
                right: width - scale(31, dpi),
                bottom: scale(105, dpi),
            },
            scale(15, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        draw_text(
            hdc,
            &reset.1,
            RECT {
                left: scale(193, dpi),
                top: scale(104, dpi),
                right: width - scale(31, dpi),
                bottom: scale(127, dpi),
            },
            scale(10, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );

        let bar = RECT {
            left: scale(32, dpi),
            top: scale(140, dpi),
            right: width - scale(32, dpi),
            bottom: scale(147, dpi),
        };
        fill_rounded(hdc, bar, scale(7, dpi), theme.line);
        if let Some(value) = state.weekly_percent() {
            let filled = (bar.left + (bar.right - bar.left) * i32::from(value) / 100)
                .max(bar.left + (bar.bottom - bar.top));
            if value > 0 {
                fill_rounded(
                    hdc,
                    RECT { right: filled.min(bar.right), ..bar },
                    scale(7, dpi),
                    status_color,
                );
            }
        }

        let session = state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.session.as_ref())
            .map_or_else(|| "--".to_owned(), |window| format!("{}%", window.display_percent()));
        let plan = state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.account.plan_type.as_deref())
            .map(plan_label)
            .unwrap_or("--")
            .to_owned();
        let credits = state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.account.reset_credits)
            .map(|credits| format!("{credits} {}", locale.text("resets", "次")))
            .unwrap_or_else(|| "--".to_owned());

        metric(
            hdc,
            RECT {
                left: scale(24, dpi),
                top: scale(174, dpi),
                right: scale(110, dpi),
                bottom: scale(225, dpi),
            },
            locale.text("5-hour window", "5 小时"),
            &session,
            theme,
            dpi,
        );
        fill(
            hdc,
            RECT {
                left: scale(118, dpi),
                top: scale(181, dpi),
                right: scale(119, dpi),
                bottom: scale(220, dpi),
            },
            theme.line,
        );
        metric(
            hdc,
            RECT {
                left: scale(134, dpi),
                top: scale(174, dpi),
                right: scale(215, dpi),
                bottom: scale(225, dpi),
            },
            locale.text("Plan", "套餐"),
            &plan,
            theme,
            dpi,
        );
        fill(
            hdc,
            RECT {
                left: scale(226, dpi),
                top: scale(181, dpi),
                right: scale(227, dpi),
                bottom: scale(220, dpi),
            },
            theme.line,
        );
        metric(
            hdc,
            RECT {
                left: scale(242, dpi),
                top: scale(174, dpi),
                right: width - scale(24, dpi),
                bottom: scale(225, dpi),
            },
            locale.text("Reset credits", "重置机会"),
            &credits,
            theme,
            dpi,
        );

        let footer = footer_text(state, locale);
        draw_text(
            hdc,
            &footer,
            RECT {
                left: scale(18, dpi),
                top: scale(228, dpi),
                right: width - scale(18, dpi),
                bottom: scale(250, dpi),
            },
            scale(10, dpi),
            FW_NORMAL.0 as i32,
            if state.error.is_some() { status_color } else { theme.muted },
        );
    }
}

unsafe fn metric(hdc: HDC, rect: RECT, label: &str, value: &str, theme: Theme, dpi: u32) {
    unsafe {
        draw_text(
            hdc,
            label,
            RECT {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.top + scale(21, dpi),
            },
            scale(10, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
        draw_text(
            hdc,
            value,
            RECT {
                left: rect.left,
                top: rect.top + scale(20, dpi),
                right: rect.right,
                bottom: rect.bottom,
            },
            scale(17, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
    }
}

fn updated_text(state: &DisplayState, locale: Locale) -> String {
    if state.refresh_state == RefreshState::Loading {
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

fn footer_text(state: &DisplayState, locale: Locale) -> String {
    if let Some(error) = state.error.as_deref() {
        let prefix = if state.weekly_percent().is_some() {
            locale.text("Cached · ", "缓存 · ")
        } else {
            locale.text("Unavailable · ", "不可用 · ")
        };
        return format!("{prefix}{error}");
    }
    if state.refresh_state == RefreshState::Loading {
        return locale.text("Refreshing Codex quota…", "正在刷新 Codex 额度…").to_owned();
    }
    if state.snapshot.is_some() {
        locale.text("Read securely from local Codex", "数据安全读取自本机 Codex").to_owned()
    } else {
        locale.text("Waiting for Codex", "等待 Codex 数据").to_owned()
    }
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
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let countdown = if locale == Locale::Chinese {
        if days > 0 {
            format!("{days} 天 {hours} 小时")
        } else {
            format!("{hours} 小时 {minutes} 分")
        }
    } else if days > 0 {
        format!("{days}d {hours}h")
    } else {
        format!("{hours}h {minutes}m")
    };
    let local_time = DateTime::from_timestamp(reset, 0)
        .map(|time| time.with_timezone(&Local).format("%m/%d %H:%M").to_string())
        .unwrap_or_else(|| "--".to_owned());
    (countdown, local_time)
}

fn plan_label(plan: &str) -> &str {
    match plan.to_ascii_lowercase().as_str() {
        "plus" => "Plus",
        "pro" => "Pro",
        "team" => "Team",
        "business" => "Business",
        "enterprise" => "Enterprise",
        _ => plan,
    }
}

fn accent_for(state: &DisplayState, high_contrast: bool) -> COLORREF {
    if high_contrast {
        return rgb(255, 255, 255);
    }
    if state.refresh_state != RefreshState::Live {
        return rgb(91, 123, 153);
    }
    match state.weekly_percent() {
        Some(value) if value < 20 => rgb(220, 61, 73),
        Some(value) if value < 50 => rgb(218, 146, 0),
        Some(_) => rgb(14, 159, 110),
        None => rgb(104, 109, 118),
    }
}

unsafe fn draw_text(hdc: HDC, value: &str, rect: RECT, height: i32, weight: i32, color: COLORREF) {
    unsafe { draw_text_with_alignment(hdc, value, rect, height, weight, color, DT_LEFT) }
}

unsafe fn draw_text_right(
    hdc: HDC,
    value: &str,
    rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
) {
    unsafe { draw_text_with_alignment(hdc, value, rect, height, weight, color, DT_RIGHT) }
}

unsafe fn draw_text_with_alignment(
    hdc: HDC,
    value: &str,
    mut rect: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
    alignment: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    unsafe {
        let face = wide0(if height >= 28 {
            "Segoe UI Variable Display"
        } else {
            "Segoe UI Variable Text"
        });
        let font = CreateFontW(
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
        );
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetTextColor(hdc, color);
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

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let _ = FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

unsafe fn fill_rounded(hdc: HDC, rect: RECT, radius: i32, color: COLORREF) {
    unsafe {
        let region = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius.max(1),
            radius.max(1),
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
    use crate::model::QuotaWindow;

    #[test]
    fn localizes_tooltip_status() {
        let state =
            DisplayState { snapshot: None, refresh_state: RefreshState::Unavailable, error: None };
        assert!(tooltip(&state, Locale::English).contains("unavailable"));
        assert!(tooltip(&state, Locale::Chinese).contains("不可用"));
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
}
