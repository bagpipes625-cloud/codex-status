//! Theme detection and semantic UI color palettes.

use crate::model::{DisplayState, RefreshState};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub tray_dark: bool,
    pub high_contrast: bool,
    pub(super) background: COLORREF,
    pub(super) surface: COLORREF,
    pub(super) surface_alt: COLORREF,
    pub(super) text: COLORREF,
    pub(super) muted: COLORREF,
    pub(super) line: COLORREF,
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

pub(super) const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | ((green as u32) << 8) | ((blue as u32) << 16))
}

pub(super) fn refresh_icon_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.text
    } else if theme.dark {
        rgb(188, 198, 203)
    } else {
        rgb(102, 117, 124)
    }
}

pub(super) fn quota_card_colors(
    theme: Theme,
    selected: bool,
    pressed: bool,
) -> (COLORREF, COLORREF) {
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

pub(super) fn panel_title_color(theme: Theme, selected: bool) -> COLORREF {
    if !selected || theme.high_contrast {
        theme.muted
    } else if theme.dark {
        rgb(115, 204, 175)
    } else {
        rgb(8, 125, 97)
    }
}

pub(super) fn outer_track_color(theme: Theme, selected: bool) -> COLORREF {
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

pub(super) fn inner_track_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.surface
    } else if theme.dark {
        rgb(69, 75, 78)
    } else {
        rgb(217, 223, 226)
    }
}

pub(super) fn theoretical_color(theme: Theme) -> COLORREF {
    if theme.high_contrast {
        theme.text
    } else if theme.dark {
        rgb(145, 168, 180)
    } else {
        rgb(129, 150, 162)
    }
}

pub(super) fn quota_bar_color(percent: Option<u8>, high_contrast: bool) -> COLORREF {
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

pub(super) fn interactive_text_color(theme: Theme, hovered: bool, secondary: bool) -> COLORREF {
    if !hovered || theme.high_contrast {
        return if secondary { theme.muted } else { theme.text };
    }
    if theme.dark { rgb(112, 194, 175) } else { rgb(18, 126, 103) }
}

pub(super) fn accent_for(
    state: &DisplayState,
    percent: Option<u8>,
    high_contrast: bool,
) -> COLORREF {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
