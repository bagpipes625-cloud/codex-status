use crate::history::UsageHistoryView;
use crate::model::{DisplayState, ProviderKind, QuotaKind};
use crate::winutil::wide0;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateRoundRectRgn, DeleteDC,
    DeleteObject, EndPaint, HGDIOBJ, PAINTSTRUCT, SRCCOPY, SelectObject, SetWindowRgn,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, MB_ICONERROR, MB_OK, MESSAGEBOX_STYLE, MessageBoxW,
};
use windows::core::{PCWSTR, w};

mod direct2d;
mod format;
mod gdi;
mod history_view;
pub(crate) mod layout;
mod theme;

#[cfg(test)]
mod render_tests;

pub use format::{Locale, tooltip};
pub use history_view::{
    HistoryHit, HistoryNavigation, HistoryPage, HoveredHistoryDay, ResourcePage, UsageSummaryWeek,
    hit_test as history_hit_test, hovered_day as hovered_history_day,
};
pub use layout::{
    CARD_HEIGHT, CARD_WIDTH, COMPACT_CARD_HEIGHT, COMPACT_CARD_WIDTH, FlyoutDimensions,
    flyout_dimensions, quota_card_hit_test, refresh_button_hit_test, scale,
};
pub use theme::{Theme, detect_theme};

#[derive(Debug, Clone, Copy)]
pub struct CardInteraction {
    pub pressed_quota: Option<QuotaKind>,
    pub refresh_feedback: bool,
    pub refresh_rotation_degrees: f32,
    pub hovered_day: Option<HoveredHistoryDay>,
    pub hovered_history_values: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CardView<'a> {
    pub provider: ProviderKind,
    pub reset: Option<&'a crate::reset::ResetPanel>,
    pub history: Option<&'a UsageHistoryView>,
    pub navigation: &'a HistoryNavigation,
    pub interaction: CardInteraction,
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
            scale(layout::FLYOUT_CORNER_RADIUS.saturating_mul(2), dpi).max(2),
            scale(layout::FLYOUT_CORNER_RADIUS.saturating_mul(2), dpi).max(2),
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
            w!("AgentStatus"),
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONERROR.0),
        );
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
            provider: view.provider,
            hwnd,
            pixel_size: (width, height),
            dpi,
            state,
            preferred,
            locale,
            theme,
            history: view.history,
            reset: view.reset,
            navigation: view.navigation,
            interaction: view.interaction,
        });
        if !painted {
            let buffer = CreateCompatibleDC(Some(hdc));
            let bitmap = CreateCompatibleBitmap(hdc, width, height);
            if !buffer.is_invalid() && !bitmap.is_invalid() {
                let old_bitmap = SelectObject(buffer, HGDIOBJ(bitmap.0));
                gdi::draw_card(buffer, state, preferred, locale, theme, view, dpi);
                let _ = BitBlt(hdc, 0, 0, width, height, Some(buffer), 0, 0, SRCCOPY);
                let _ = SelectObject(buffer, old_bitmap);
            } else {
                gdi::draw_card(hdc, state, preferred, locale, theme, view, dpi);
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
            provider: view.provider,
            hwnd,
            pixel_size: ((client.right - client.left).max(1), (client.bottom - client.top).max(1)),
            dpi: windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96),
            state,
            preferred,
            locale,
            theme,
            history: view.history,
            reset: view.reset,
            navigation: view.navigation,
            interaction: view.interaction,
        });
    }
}

pub fn release_card_renderer() {
    direct2d::release_all();
}

pub fn reset_card_renderer_fallback() {
    direct2d::reset_fallback();
}

/// Captures a window client surface into a 32-bit BMP file for offline visual tests.
///
/// # Safety
/// `hwnd` must be a valid window handle on the current thread with at least `width x height` pixels.
#[doc(hidden)]
pub unsafe fn save_window_bmp(hwnd: HWND, width: i32, height: i32, path: &std::path::Path) {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, ReleaseDC,
    };
    unsafe {
        let dc = GetDC(Some(hwnd));
        let mem = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, width, height);
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        BitBlt(mem, 0, 0, width, height, Some(dc), 0, 0, SRCCOPY).unwrap();
        let _ = SelectObject(mem, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: 40,
                biWidth: width,
                biHeight: height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        assert_ne!(
            GetDIBits(
                mem,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS
            ),
            0
        );
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&54u32.to_le_bytes());
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        bytes.extend_from_slice(&pixels);
        std::fs::write(path, bytes).unwrap();
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(Some(hwnd), dc);
    }
}
