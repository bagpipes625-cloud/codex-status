use crate::model::{DisplayState, RefreshState};
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, RECT};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_TOP, DeleteDC, DeleteObject, DrawTextW,
    FF_SWISS, FW_SEMIBOLD, GdiFlush, HGDIOBJ, OPAQUE, OUT_DEFAULT_PRECIS, SelectObject, SetBkColor,
    SetBkMode, SetTextColor,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON};
use windows::core::w;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconTone {
    Healthy,
    Warning,
    Critical,
    Stale,
    Unavailable,
}

impl IconTone {
    fn accent(self, high_contrast: bool, dark_taskbar: bool) -> [u8; 4] {
        if high_contrast {
            return foreground(dark_taskbar, false);
        }
        match self {
            Self::Healthy => [16, 163, 127, 255],
            Self::Warning => [210, 134, 0, 255],
            Self::Critical => [211, 64, 73, 255],
            Self::Stale => [112, 122, 134, 255],
            Self::Unavailable => [104, 109, 118, 255],
        }
    }

    fn is_muted(self) -> bool {
        matches!(self, Self::Stale | Self::Unavailable)
    }
}

pub fn tone_for(state: &DisplayState, percent: Option<u8>) -> IconTone {
    if state.refresh_state != RefreshState::Live {
        return if percent.is_some() { IconTone::Stale } else { IconTone::Unavailable };
    }
    quota_tone(percent)
}

fn quota_tone(percent: Option<u8>) -> IconTone {
    match percent {
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
    display_percent: Option<u8>,
    indicator_percent: Option<u8>,
    tone: IconTone,
    size: u32,
    high_contrast: bool,
    dark_taskbar: bool,
) -> windows::core::Result<OwnedIcon> {
    let size = size.clamp(16, 32);
    let xor =
        render_bgra(display_percent, indicator_percent, tone, size, high_contrast, dark_taskbar);
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

pub fn render_bgra(
    display_percent: Option<u8>,
    indicator_percent: Option<u8>,
    tone: IconTone,
    size: u32,
    high_contrast: bool,
    dark_taskbar: bool,
) -> Vec<u8> {
    let size = size.clamp(16, 32);
    let pixels =
        render_rgba(display_percent, indicator_percent, tone, size, high_contrast, dark_taskbar);
    let mut bytes = Vec::with_capacity((size * size * 4) as usize);

    // CreateIcon's 32-bpp XOR input is consumed in the same top-left order as
    // our logical canvas. Reversing scan lines here made a 2 look like a 5 and
    // moved the status rule above the number in Explorer.
    for y in 0..size {
        for x in 0..size {
            let [r, g, b, a] = pixels[(y * size + x) as usize];
            bytes.extend_from_slice(&[b, g, r, a]);
        }
    }
    bytes
}

fn render_rgba(
    display_percent: Option<u8>,
    indicator_percent: Option<u8>,
    tone: IconTone,
    size: u32,
    high_contrast: bool,
    dark_taskbar: bool,
) -> Vec<[u8; 4]> {
    let mut pixels = vec![[0_u8; 4]; (size * size) as usize];
    let label = display_percent.map_or_else(|| "--".to_owned(), |value| value.min(100).to_string());
    let text = foreground(dark_taskbar, tone.is_muted() && !high_contrast);

    if label == "100" {
        if let (Some(one), Some(zero)) = (rasterize_label("1", size), rasterize_label("0", size)) {
            composite_hundred(&mut pixels, size, &one, &zero, text);
        } else {
            draw_fallback_label(&mut pixels, size, &label, text);
        }
    } else if let Some(mask) = rasterize_label(&label, size) {
        let two_digit_scale =
            (label.chars().count() == 1).then(|| rasterize_label("65", size)).flatten();
        composite_mask(&mut pixels, size, &mask, two_digit_scale.as_ref(), text);
    } else {
        draw_fallback_label(&mut pixels, size, &label, text);
    }

    draw_quota_status_bar(&mut pixels, size, indicator_percent, high_contrast, dark_taskbar);
    pixels
}

fn foreground(dark_taskbar: bool, muted: bool) -> [u8; 4] {
    match (dark_taskbar, muted) {
        (true, true) => [183, 190, 199, 255],
        (true, false) => [248, 249, 250, 255],
        (false, true) => [94, 103, 114, 255],
        (false, false) => [31, 34, 38, 255],
    }
}

fn draw_quota_status_bar(
    pixels: &mut [[u8; 4]],
    size: u32,
    percent: Option<u8>,
    high_contrast: bool,
    dark_taskbar: bool,
) {
    let left = 1;
    let width = size.saturating_sub(2);
    let top = size.saturating_sub(2);
    let color = percent.map_or_else(
        || quota_track(high_contrast, dark_taskbar),
        |value| quota_tone(Some(value.min(100))).accent(high_contrast, dark_taskbar),
    );

    for y in top..size {
        for x in left..left + width {
            set_pixel(pixels, size, x as i32, y as i32, color);
        }
    }
}

fn quota_track(high_contrast: bool, dark_taskbar: bool) -> [u8; 4] {
    if high_contrast {
        return foreground(dark_taskbar, false);
    }
    if dark_taskbar { [61, 61, 61, 255] } else { [226, 226, 226, 255] }
}

struct GrayMask {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

struct MaskPlacement {
    width: usize,
    height: usize,
    x: usize,
    y: usize,
}

fn composite_hundred(
    pixels: &mut [[u8; 4]],
    size: u32,
    one: &GrayMask,
    zero: &GrayMask,
    color: [u8; 4],
) {
    let target_height = scaled_icon_unit(10, size);
    let glyph_widths =
        [scaled_icon_unit(3, size), scaled_icon_unit(4, size), scaled_icon_unit(4, size)];
    let gap = scaled_icon_unit(1, size);
    let total_width = glyph_widths.iter().sum::<usize>() + gap * 2;
    let mut origin_x = (size as usize - total_width) / 2;
    let origin_y = (size as usize - 3 - target_height) / 2;

    for (index, (mask, target_width)) in [one, zero, zero].into_iter().zip(glyph_widths).enumerate()
    {
        composite_resized_mask(
            pixels,
            size,
            mask,
            MaskPlacement { width: target_width, height: target_height, x: origin_x, y: origin_y },
            color,
        );
        origin_x += target_width;
        if index < 2 {
            origin_x += gap;
        }
    }
}

fn scaled_icon_unit(value_at_16: usize, size: u32) -> usize {
    ((value_at_16 * size as usize + 8) / 16).max(1)
}

fn rasterize_label(label: &str, target_size: u32) -> Option<GrayMask> {
    const SUPERSAMPLE: i32 = 8;
    let canvas_height = target_size as i32 * SUPERSAMPLE;
    let canvas_width = target_size as i32 * SUPERSAMPLE * 2;
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: canvas_width,
            biHeight: -canvas_height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = ptr::null_mut::<c_void>();

    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            return None;
        }
        let bitmap =
            match CreateDIBSection(Some(dc), &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            return None;
        }

        let font = CreateFontW(
            -(canvas_height * 7 / 8),
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            w!("Segoe UI"),
        );
        if font.0.is_null() {
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            return None;
        }

        let old_bitmap = SelectObject(dc, bitmap.into());
        let old_font = SelectObject(dc, font.into());
        let _ = SetBkMode(dc, OPAQUE);
        let _ = SetBkColor(dc, COLORREF(0));
        let _ = SetTextColor(dc, COLORREF(0x00ff_ffff));
        let mut text: Vec<u16> = label.encode_utf16().collect();
        let mut rect = RECT { left: 0, top: 0, right: canvas_width, bottom: canvas_height };
        let _ = DrawTextW(dc, &mut text, &mut rect, DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX);
        let _ = GdiFlush();

        let source = std::slice::from_raw_parts(
            bits.cast::<u8>(),
            (canvas_width * canvas_height * 4) as usize,
        );
        let result = crop_grayscale(source, canvas_width as usize, canvas_height as usize);

        if !old_font.0.is_null() {
            let _ = SelectObject(dc, old_font);
        }
        if !old_bitmap.0.is_null() {
            let _ = SelectObject(dc, old_bitmap);
        }
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        result
    }
}

fn crop_grayscale(source: &[u8], width: usize, height: usize) -> Option<GrayMask> {
    let mut left = width;
    let mut top = height;
    let mut right = 0;
    let mut bottom = 0;
    for y in 0..height {
        for x in 0..width {
            let pixel = &source[(y * width + x) * 4..][..3];
            let value = *pixel.iter().max().unwrap_or(&0);
            if value <= 3 {
                continue;
            }
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if left >= right || top >= bottom {
        return None;
    }

    let cropped_width = right - left;
    let cropped_height = bottom - top;
    let mut pixels = vec![0_u8; cropped_width * cropped_height];
    for y in 0..cropped_height {
        for x in 0..cropped_width {
            let source_index = ((top + y) * width + left + x) * 4;
            pixels[y * cropped_width + x] =
                source[source_index..source_index + 3].iter().copied().max().unwrap_or(0);
        }
    }
    Some(GrayMask { pixels, width: cropped_width, height: cropped_height })
}

fn composite_mask(
    pixels: &mut [[u8; 4]],
    size: u32,
    mask: &GrayMask,
    scale_reference: Option<&GrayMask>,
    color: [u8; 4],
) {
    let max_width = size.saturating_sub(2) as usize;
    let max_height = size.saturating_sub(3) as usize;
    // A lone digit must use the same font scale as the established two-digit
    // label. Use a representative two-digit mask only to choose the height;
    // the actual digit keeps its own proportions and is centered below.
    let sizing_mask = scale_reference.unwrap_or(mask);
    let width_limited_height = max_width * sizing_mask.height / sizing_mask.width.max(1);
    let target_height = max_height.min(width_limited_height.max(1));
    let target_width = (target_height * mask.width / mask.height.max(1)).clamp(1, max_width);
    let origin_x = (size as usize - target_width) / 2;
    let origin_y = (max_height.saturating_sub(target_height)) / 2;

    composite_resized_mask(
        pixels,
        size,
        mask,
        MaskPlacement { width: target_width, height: target_height, x: origin_x, y: origin_y },
        color,
    );
}

fn composite_resized_mask(
    pixels: &mut [[u8; 4]],
    size: u32,
    mask: &GrayMask,
    placement: MaskPlacement,
    color: [u8; 4],
) {
    for y in 0..placement.height {
        for x in 0..placement.width {
            let source_x0 = x * mask.width / placement.width;
            let source_x1 = ((x + 1) * mask.width / placement.width).max(source_x0 + 1);
            let source_y0 = y * mask.height / placement.height;
            let source_y1 = ((y + 1) * mask.height / placement.height).max(source_y0 + 1);
            let mut coverage = 0_u32;
            let mut samples = 0_u32;
            for source_y in source_y0..source_y1.min(mask.height) {
                for source_x in source_x0..source_x1.min(mask.width) {
                    coverage += u32::from(mask.pixels[source_y * mask.width + source_x]);
                    samples += 1;
                }
            }
            let alpha = coverage.checked_div(samples).unwrap_or(0) as u8;
            if alpha <= 5 {
                continue;
            }
            set_pixel(
                pixels,
                size,
                (placement.x + x) as i32,
                (placement.y + y) as i32,
                premultiply(color, alpha),
            );
        }
    }
}

fn premultiply(color: [u8; 4], alpha: u8) -> [u8; 4] {
    let scale = u16::from(alpha);
    [
        ((u16::from(color[0]) * scale + 127) / 255) as u8,
        ((u16::from(color[1]) * scale + 127) / 255) as u8,
        ((u16::from(color[2]) * scale + 127) / 255) as u8,
        alpha,
    ]
}

fn draw_fallback_label(pixels: &mut [[u8; 4]], size: u32, label: &str, color: [u8; 4]) {
    let glyph_width = 3_i32;
    let glyph_height = 7_i32;
    let glyph_count = label.chars().count() as i32;
    let units_width = glyph_count * glyph_width + (glyph_count - 1).max(0);
    let layout_units_width = if glyph_count == 1 { glyph_width * 2 + 1 } else { units_width };
    let scale_x = ((size as i32 - 2) / layout_units_width).max(1);
    let scale_y = ((size as i32 - 3) / glyph_height).max(1);
    let width = units_width * scale_x;
    let height = glyph_height * scale_y;
    let origin_x = (size as i32 - width) / 2;
    let origin_y = (size as i32 - 3 - height) / 2;

    for (index, character) in label.chars().enumerate() {
        let rows = fallback_glyph(character);
        let offset_x = origin_x + index as i32 * (glyph_width + 1) * scale_x;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..glyph_width {
                if bits & (1 << (glyph_width - 1 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale_y {
                    for dx in 0..scale_x {
                        set_pixel(
                            pixels,
                            size,
                            offset_x + column * scale_x + dx,
                            origin_y + row as i32 * scale_y + dy,
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

fn fallback_glyph(character: char) -> [u8; 7] {
    match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b001, 0b111, 0b100, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b001, 0b111, 0b001, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b101, 0b111, 0b001, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b100, 0b111, 0b001, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b100, 0b111, 0b101, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b010, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b101, 0b111, 0b101, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b101, 0b111, 0b001, 0b001, 0b111],
        '-' => [0b000, 0b000, 0b000, 0b111, 0b000, 0b000, 0b000],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_boundary_labels_at_supported_sizes_and_themes() {
        for size in [16, 20, 24, 32] {
            for dark in [false, true] {
                for value in
                    [None, Some(0), Some(9), Some(20), Some(49), Some(50), Some(87), Some(100)]
                {
                    let pixels = render_bgra(value, value, IconTone::Healthy, size, false, dark);
                    assert_eq!(pixels.len(), (size * size * 4) as usize);
                    assert!(pixels.iter().any(|byte| *byte != 0));
                }
            }
        }
    }

    #[test]
    fn common_two_digit_value_is_readable_but_keeps_a_transparent_background() {
        let pixels = render_rgba(Some(87), Some(87), IconTone::Healthy, 16, false, false);
        let visible: Vec<_> =
            pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 20).collect();
        let left = visible.iter().map(|(index, _)| index % 16).min().unwrap();
        let right = visible.iter().map(|(index, _)| index % 16).max().unwrap();
        assert!(right - left + 1 >= 12);
        assert!(pixels.iter().filter(|pixel| pixel[3] == 0).count() > 16 * 8);
    }

    #[test]
    fn one_digit_label_uses_the_same_height_as_two_digit_labels() {
        fn text_bounds(pixels: &[[u8; 4]], size: usize) -> (usize, usize, usize) {
            let visible: Vec<_> = pixels[..(size - 2) * size]
                .iter()
                .enumerate()
                .filter(|(_, pixel)| pixel[3] > 20)
                .collect();
            let left = visible.iter().map(|(index, _)| index % size).min().unwrap();
            let right = visible.iter().map(|(index, _)| index % size).max().unwrap();
            let top = visible.iter().map(|(index, _)| index / size).min().unwrap();
            let bottom = visible.iter().map(|(index, _)| index / size).max().unwrap();
            (left, right, bottom - top + 1)
        }

        for size in [16_usize, 20, 24, 32] {
            let single =
                render_rgba(Some(5), Some(5), IconTone::Healthy, size as u32, false, false);
            let double =
                render_rgba(Some(65), Some(65), IconTone::Healthy, size as u32, false, false);
            let (single_left, single_right, single_height) = text_bounds(&single, size);
            let (_, _, double_height) = text_bounds(&double, size);

            assert!(single_height.abs_diff(double_height) <= 1, "size={size}");
            assert!(single_left.abs_diff(size - 1 - single_right) <= 1, "size={size}");
        }
    }

    #[test]
    fn hundred_uses_the_dedicated_ten_pixel_layout_at_base_size() {
        let pixels = render_rgba(Some(100), Some(100), IconTone::Healthy, 16, false, false);
        let visible: Vec<_> =
            pixels[..14 * 16].iter().enumerate().filter(|(_, pixel)| pixel[3] > 20).collect();
        let left = visible.iter().map(|(index, _)| index % 16).min().unwrap();
        let right = visible.iter().map(|(index, _)| index % 16).max().unwrap();
        let top = visible.iter().map(|(index, _)| index / 16).min().unwrap();
        let bottom = visible.iter().map(|(index, _)| index / 16).max().unwrap();

        assert_eq!(bottom - top + 1, 10);
        assert!(right - left < 14);
        assert!(left.abs_diff(15 - right) <= 1);
    }

    #[test]
    fn light_and_dark_taskbars_get_opposite_foreground_colors() {
        let light = render_rgba(Some(87), Some(87), IconTone::Healthy, 16, false, false);
        let dark = render_rgba(Some(87), Some(87), IconTone::Healthy, 16, false, true);
        let sample = light
            .iter()
            .zip(&dark)
            .find(|(light, dark)| light[3] > 200 && dark[3] > 200 && light[0] != 16)
            .unwrap();
        assert!(sample.0[0] < sample.1[0]);
    }

    #[test]
    fn quota_status_bar_is_two_pixels_high_and_keeps_horizontal_margins() {
        let pixels = render_rgba(Some(50), Some(50), IconTone::Healthy, 16, false, false);
        let accent = quota_tone(Some(50)).accent(false, false);
        for y in [14, 15] {
            assert_eq!(pixels[y * 16], [0, 0, 0, 0]);
            assert!(pixels[y * 16 + 1..y * 16 + 15].iter().all(|pixel| *pixel == accent));
            assert_eq!(pixels[y * 16 + 15], [0, 0, 0, 0]);
        }
    }

    #[test]
    fn create_icon_bytes_keep_the_quota_status_bar_on_the_bottom() {
        let pixels = render_bgra(Some(50), Some(50), IconTone::Healthy, 16, false, false);
        let accent = quota_tone(Some(50)).accent(false, false);
        let accent_bgra = [accent[2], accent[1], accent[0], accent[3]];
        let pixel = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..][..4];
        assert_ne!(pixel(4, 13), accent_bgra);
        assert_eq!(pixel(4, 14), accent_bgra);
        assert_eq!(pixel(4, 15), accent_bgra);
    }

    #[test]
    fn quota_status_bar_stays_full_width_and_color_follows_the_panel_rules() {
        let cases = [
            (100, IconTone::Healthy),
            (50, IconTone::Healthy),
            (49, IconTone::Warning),
            (20, IconTone::Warning),
            (19, IconTone::Critical),
            (1, IconTone::Critical),
            (0, IconTone::Critical),
        ];
        for (percent, expected_tone) in cases {
            let pixels = render_rgba(Some(percent), Some(percent), expected_tone, 16, false, false);
            let accent = expected_tone.accent(false, false);
            let bar = &pixels[15 * 16 + 1..15 * 16 + 15];
            assert!(bar.iter().all(|pixel| *pixel == accent), "percent={percent}");
        }
    }

    #[test]
    fn status_bar_uses_the_non_displayed_quota() {
        let pixels = render_rgba(Some(85), Some(19), IconTone::Healthy, 16, false, false);
        let critical = quota_tone(Some(19)).accent(false, false);
        assert!(pixels[15 * 16 + 1..15 * 16 + 15].iter().all(|pixel| *pixel == critical));
    }

    #[test]
    fn unavailable_quota_uses_a_full_width_neutral_status_bar() {
        let pixels = render_rgba(None, None, IconTone::Unavailable, 16, false, false);
        let track = quota_track(false, false);
        assert!(pixels[15 * 16 + 1..15 * 16 + 15].iter().all(|pixel| *pixel == track));
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
        assert_eq!(tone_for(&state(19), Some(19)), IconTone::Critical);
        assert_eq!(tone_for(&state(20), Some(20)), IconTone::Warning);
        assert_eq!(tone_for(&state(49), Some(49)), IconTone::Warning);
        assert_eq!(tone_for(&state(50), Some(50)), IconTone::Healthy);
    }
}
