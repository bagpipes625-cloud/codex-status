use crate::model::{DisplayState, RefreshState};
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconTone {
    Healthy,
    Warning,
    Critical,
    Stale,
    Unavailable,
}

impl IconTone {
    fn colors(self, high_contrast: bool) -> ([u8; 4], [u8; 4], [u8; 4]) {
        if high_contrast {
            return ([0, 0, 0, 255], [255, 255, 255, 255], [255, 255, 255, 255]);
        }
        match self {
            Self::Healthy => ([14, 159, 110, 255], [7, 92, 67, 255], [255, 255, 255, 255]),
            Self::Warning => ([218, 146, 0, 255], [133, 83, 0, 255], [255, 255, 255, 255]),
            Self::Critical => ([220, 61, 73, 255], [129, 25, 37, 255], [255, 255, 255, 255]),
            Self::Stale => ([85, 112, 139, 255], [43, 62, 82, 255], [255, 255, 255, 255]),
            Self::Unavailable => ([104, 109, 118, 255], [54, 58, 66, 255], [255, 255, 255, 255]),
        }
    }
}

pub fn tone_for(state: &DisplayState) -> IconTone {
    if state.refresh_state != RefreshState::Live {
        return if state.weekly_percent().is_some() {
            IconTone::Stale
        } else {
            IconTone::Unavailable
        };
    }
    match state.weekly_percent() {
        Some(percent) if percent < 20 => IconTone::Critical,
        Some(percent) if percent < 50 => IconTone::Warning,
        Some(_) => IconTone::Healthy,
        None => IconTone::Unavailable,
    }
}

pub struct OwnedIcon(HICON);

impl OwnedIcon {
    pub fn handle(&self) -> HICON {
        self.0
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

pub fn create_icon(
    percent: Option<u8>,
    tone: IconTone,
    size: u32,
    high_contrast: bool,
) -> windows::core::Result<OwnedIcon> {
    let size = size.clamp(16, 32);
    let xor = render_bgra(percent, tone, size, high_contrast);
    let mask_stride = size.div_ceil(32) * 4;
    let and_mask = vec![0_u8; (mask_stride * size) as usize];
    let icon = unsafe {
        CreateIcon(
            None::<HINSTANCE>,
            size as i32,
            size as i32,
            1,
            32,
            and_mask.as_ptr(),
            xor.as_ptr(),
        )?
    };
    Ok(OwnedIcon(icon))
}

pub fn render_bgra(percent: Option<u8>, tone: IconTone, size: u32, high_contrast: bool) -> Vec<u8> {
    let size = size.clamp(16, 32);
    let (fill, border, text) = tone.colors(high_contrast);
    let mut pixels = vec![[0_u8; 4]; (size * size) as usize];
    let radius = (size as f32 * 0.22).max(3.0);
    for y in 0..size {
        for x in 0..size {
            let edge_x = (x.min(size - 1 - x)) as f32;
            let edge_y = (y.min(size - 1 - y)) as f32;
            let inside = if edge_x >= radius || edge_y >= radius {
                true
            } else {
                let dx = radius - edge_x - 0.5;
                let dy = radius - edge_y - 0.5;
                dx * dx + dy * dy <= radius * radius
            };
            if !inside {
                continue;
            }
            let border_pixel = x == 0
                || y == 0
                || x == size - 1
                || y == size - 1
                || (edge_x < 1.5 && edge_y >= radius)
                || (edge_y < 1.5 && edge_x >= radius);
            let color = if border_pixel { border } else { fill };
            set_pixel(&mut pixels, size, x as i32, y as i32, color);
        }
    }

    let label = percent.map_or_else(|| "--".to_owned(), |value| value.min(100).to_string());
    draw_label(&mut pixels, size, &label, text);

    let mut bytes = Vec::with_capacity((size * size * 4) as usize);
    for y in (0..size).rev() {
        for x in 0..size {
            let [r, g, b, a] = pixels[(y * size + x) as usize];
            bytes.extend_from_slice(&[b, g, r, a]);
        }
    }
    bytes
}

fn draw_label(pixels: &mut [[u8; 4]], size: u32, label: &str, color: [u8; 4]) {
    let glyph_width = 3_i32;
    let glyph_height = 5_i32;
    let glyph_count = label.chars().count() as i32;
    let units_w = glyph_count * glyph_width + (glyph_count - 1).max(0);
    let max_scale_x = ((size as i32 - 4) / units_w).max(1);
    let max_scale_y = ((size as i32 - 5) / glyph_height).max(1);
    let scale = max_scale_x.min(max_scale_y).min(4);
    let width = units_w * scale;
    let height = glyph_height * scale;
    let origin_x = (size as i32 - width) / 2;
    let origin_y = (size as i32 - height) / 2;
    let shadow = [20, 24, 28, 190];

    for (index, character) in label.chars().enumerate() {
        let rows = glyph(character);
        let offset_x = origin_x + index as i32 * (glyph_width + 1) * scale;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..glyph_width {
                if bits & (1 << (glyph_width - 1 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = offset_x + column * scale + dx;
                        let y = origin_y + row as i32 * scale + dy;
                        set_pixel(pixels, size, x + 1, y + 1, shadow);
                    }
                }
            }
        }
    }
    for (index, character) in label.chars().enumerate() {
        let rows = glyph(character);
        let offset_x = origin_x + index as i32 * (glyph_width + 1) * scale;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..glyph_width {
                if bits & (1 << (glyph_width - 1 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        set_pixel(
                            pixels,
                            size,
                            offset_x + column * scale + dx,
                            origin_y + row as i32 * scale + dy,
                            color,
                        );
                    }
                }
            }
        }
    }
}

fn set_pixel(pixels: &mut [[u8; 4]], size: u32, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
        return;
    }
    pixels[(y as u32 * size + x as u32) as usize] = color;
}

fn glyph(character: char) -> [u8; 5] {
    match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        _ => [0; 5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_boundary_labels_at_supported_sizes() {
        for size in [16, 20, 24, 32] {
            for value in [None, Some(0), Some(9), Some(20), Some(49), Some(50), Some(87), Some(100)]
            {
                let pixels = render_bgra(value, IconTone::Healthy, size, false);
                assert_eq!(pixels.len(), (size * size * 4) as usize);
                assert!(pixels.iter().any(|byte| *byte != 0));
            }
        }
    }

    #[test]
    fn classifies_thresholds_without_relying_on_color_alone() {
        fn state(percent: u8) -> DisplayState {
            use crate::model::{AccountSummary, QuotaSnapshot, QuotaWindow, WEEK_MINUTES};
            DisplayState {
                snapshot: Some(QuotaSnapshot {
                    weekly: Some(QuotaWindow {
                        used_percent: 100.0 - f64::from(percent),
                        remaining_percent: f64::from(percent),
                        window_minutes: WEEK_MINUTES,
                        resets_at: Some(i64::MAX),
                    }),
                    session: None,
                    account: AccountSummary::default(),
                    fetched_at: 0,
                }),
                refresh_state: RefreshState::Live,
                error: None,
            }
        }
        assert_eq!(tone_for(&state(19)), IconTone::Critical);
        assert_eq!(tone_for(&state(20)), IconTone::Warning);
        assert_eq!(tone_for(&state(49)), IconTone::Warning);
        assert_eq!(tone_for(&state(50)), IconTone::Healthy);
    }

    #[test]
    fn boundary_icon_pixels_have_a_stable_fingerprint() {
        let mut hash = 0xcbf29ce484222325_u64;
        for high_contrast in [false, true] {
            for tone in [
                IconTone::Healthy,
                IconTone::Warning,
                IconTone::Critical,
                IconTone::Stale,
                IconTone::Unavailable,
            ] {
                for size in [16, 20, 24, 32] {
                    for value in
                        [None, Some(0), Some(9), Some(20), Some(49), Some(50), Some(87), Some(100)]
                    {
                        for byte in render_bgra(value, tone, size, high_contrast) {
                            hash ^= u64::from(byte);
                            hash = hash.wrapping_mul(0x100000001b3);
                        }
                    }
                }
            }
        }
        assert_eq!(hash, 5_631_663_040_915_169_022);
    }
}
