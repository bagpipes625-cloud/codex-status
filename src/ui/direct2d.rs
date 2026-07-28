//! Direct2D/DirectWrite renderer for the private compact quota flyout.
//!
//! The Win32 host, tray icon, menus, data model, layout, colors, and GDI
//! fallback remain unchanged. This module replaces only the pixel renderer and
//! is initialized lazily when the flyout is first shown.

use super::{
    HERO_DIVIDER_X, Locale, METRICS_LEFT_DIVIDER_X, METRICS_LEFT_X, Theme, accent_for, plan_label,
    projection_color, projection_label, quota_bar_color, reset_credit_detail, reset_details,
    updated_text,
};
use crate::model::DisplayState;
use chrono::Local;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
    ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection,
    IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::core::{BOOL, PCWSTR, Result, w};
use windows_numerics::Vector2;

thread_local! {
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

pub(super) struct PaintInput<'a> {
    pub hwnd: HWND,
    pub size: (i32, i32),
    pub dpi: u32,
    pub state: &'a DisplayState,
    pub locale: Locale,
    pub theme: Theme,
}

pub(super) fn paint(input: PaintInput<'_>) -> bool {
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match Renderer::new() {
                Ok(renderer) => *slot = Some(renderer),
                Err(error) => {
                    diagnostic_failure("initialize", &error);
                    return false;
                }
            }
        }

        let result = slot.as_mut().expect("renderer initialized").paint(&input);
        if let Err(error) = &result {
            diagnostic_failure("paint", error);
            if let Some(renderer) = slot.as_mut() {
                renderer.target = None;
            }
        }
        result.is_ok()
    })
}

pub(super) fn release() {
    RENDERER.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(feature = "diagnostics")]
fn diagnostic_failure(stage: &str, error: &windows::core::Error) {
    eprintln!("Direct2D {stage} failed: {:#x}", error.code().0);
}

#[cfg(not(feature = "diagnostics"))]
fn diagnostic_failure(_stage: &str, _error: &windows::core::Error) {}

struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    font_family: PCWSTR,
    target: Option<ID2D1HwndRenderTarget>,
    pixel_size: D2D_SIZE_U,
    dpi: u32,
    formats: Option<FormatSet>,
}

impl Renderer {
    fn new() -> Result<Self> {
        unsafe {
            let factory =
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)?;
            let font_family = select_font_family(&dwrite);
            Ok(Self {
                factory,
                dwrite,
                font_family,
                target: None,
                pixel_size: D2D_SIZE_U::default(),
                dpi: 0,
                formats: None,
            })
        }
    }

    fn paint(&mut self, input: &PaintInput<'_>) -> Result<()> {
        self.ensure_target(input.hwnd, input.size.0, input.size.1, input.dpi)?;
        self.ensure_formats(input.locale)?;

        let target = self.target.as_ref().expect("target initialized").clone();
        let formats = self.formats.as_ref().expect("formats initialized").clone();
        let dwrite = self.dwrite.clone();

        unsafe { target.BeginDraw() };
        let draw_result =
            draw_frame(&target, &dwrite, &formats, input.state, input.locale, input.theme);
        let end_result = unsafe { target.EndDraw(None, None) };
        if end_result.as_ref().is_err_and(|error| error.code() == D2DERR_RECREATE_TARGET) {
            self.target = None;
        }
        draw_result?;
        end_result
    }

    fn ensure_target(&mut self, hwnd: HWND, width: i32, height: i32, dpi: u32) -> Result<()> {
        let size = D2D_SIZE_U { width: width.max(1) as u32, height: height.max(1) as u32 };
        let wrong_window =
            self.target.as_ref().is_some_and(|target| unsafe { target.GetHwnd() != hwnd });
        if wrong_window {
            self.target = None;
        }

        if self.target.is_none() {
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                },
                dpiX: dpi as f32,
                dpiY: dpi as f32,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target =
                unsafe { self.factory.CreateHwndRenderTarget(&properties, &hwnd_properties)? };
            unsafe {
                target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            }
            self.target = Some(target);
            self.pixel_size = size;
            self.dpi = dpi;
            return Ok(());
        }

        let target = self.target.as_ref().expect("target initialized");
        if size != self.pixel_size {
            unsafe { target.Resize(&size)? };
            self.pixel_size = size;
        }
        if dpi != self.dpi {
            unsafe { target.SetDpi(dpi as f32, dpi as f32) };
            self.dpi = dpi;
        }
        Ok(())
    }

    fn ensure_formats(&mut self, locale: Locale) -> Result<()> {
        if self.formats.as_ref().is_none_or(|formats| formats.locale != locale) {
            self.formats = Some(FormatSet::new(&self.dwrite, self.font_family, locale)?);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct FormatSet {
    locale: Locale,
    header: IDWriteTextFormat,
    update: IDWriteTextFormat,
    label: IDWriteTextFormat,
    quota: IDWriteTextFormat,
    percent: IDWriteTextFormat,
    value: IDWriteTextFormat,
    secondary: IDWriteTextFormat,
    metric_label: IDWriteTextFormat,
    metric_value: IDWriteTextFormat,
    metric_detail: IDWriteTextFormat,
    footer: IDWriteTextFormat,
}

impl FormatSet {
    fn new(factory: &IDWriteFactory, family: PCWSTR, locale: Locale) -> Result<Self> {
        Ok(Self {
            locale,
            header: make_format(
                factory,
                family,
                locale,
                14.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                false,
            )?,
            update: make_format(
                factory,
                family,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                true,
            )?,
            label: make_format(
                factory,
                family,
                locale,
                12.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            quota: make_format(
                factory,
                family,
                locale,
                40.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                false,
            )?,
            percent: make_format(
                factory,
                family,
                locale,
                17.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                false,
            )?,
            value: make_format(
                factory,
                family,
                locale,
                17.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            secondary: make_format(
                factory,
                family,
                locale,
                12.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            metric_label: make_format(
                factory,
                family,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            metric_value: make_format(
                factory,
                family,
                locale,
                18.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            metric_detail: make_format(
                factory,
                family,
                locale,
                10.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            footer: make_format(
                factory,
                family,
                locale,
                13.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn make_format(
    factory: &IDWriteFactory,
    family: PCWSTR,
    locale: Locale,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    alignment: DWRITE_TEXT_ALIGNMENT,
    trim: bool,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = factory.CreateTextFormat(
            family,
            None::<&IDWriteFontCollection>,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            locale_name(locale),
        )?;
        format.SetTextAlignment(alignment)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        if trim {
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let sign = factory.CreateEllipsisTrimmingSign(&format)?;
            format.SetTrimming(&trimming, &sign)?;
        }
        Ok(format)
    }
}

fn select_font_family(factory: &IDWriteFactory) -> PCWSTR {
    let mut collection = None;
    if unsafe { factory.GetSystemFontCollection(&mut collection, false) }.is_ok()
        && let Some(collection) = collection
    {
        for candidate in [w!("Microsoft YaHei UI"), w!("Segoe UI")] {
            let mut index = 0;
            let mut exists = BOOL::default();
            if unsafe { collection.FindFamilyName(candidate, &mut index, &mut exists) }.is_ok()
                && exists.as_bool()
            {
                return candidate;
            }
        }
    }
    w!("Microsoft YaHei UI")
}

const fn locale_name(locale: Locale) -> PCWSTR {
    match locale {
        Locale::Chinese => w!("zh-CN"),
        Locale::English => w!("en-US"),
    }
}

struct Brushes {
    surface: ID2D1SolidColorBrush,
    surface_alt: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    line: ID2D1SolidColorBrush,
    status: ID2D1SolidColorBrush,
    quota: ID2D1SolidColorBrush,
}

impl Brushes {
    fn new(target: &ID2D1HwndRenderTarget, state: &DisplayState, theme: Theme) -> Result<Self> {
        Ok(Self {
            surface: solid_brush(target, theme.surface)?,
            surface_alt: solid_brush(target, theme.surface_alt)?,
            text: solid_brush(target, theme.text)?,
            muted: solid_brush(target, theme.muted)?,
            line: solid_brush(target, theme.line)?,
            status: solid_brush(target, accent_for(state, theme.high_contrast))?,
            quota: solid_brush(
                target,
                quota_bar_color(state.weekly_percent(), theme.high_contrast),
            )?,
        })
    }
}

fn draw_frame(
    target: &ID2D1HwndRenderTarget,
    dwrite: &IDWriteFactory,
    formats: &FormatSet,
    state: &DisplayState,
    locale: Locale,
    theme: Theme,
) -> Result<()> {
    let brushes = Brushes::new(target, state, theme)?;

    unsafe {
        target.Clear(Some(&color(theme.background)));
        draw_header(target, formats, &brushes, state);

        let hero = rounded_rect(16.0, 48.0, 360.0, 176.0, 14.0);
        target.FillRoundedRectangle(&hero, &brushes.surface);
        target.DrawRoundedRectangle(&hero, &brushes.line, 1.0, None::<&ID2D1StrokeStyle>);

        draw_text(
            target,
            locale.text("Weekly remaining", "本周剩余"),
            rect(30.0, 59.0, 178.0, 83.0),
            &formats.label,
            &brushes.muted,
        );
    }
    draw_percentage(target, dwrite, formats, &brushes, state.weekly_percent())?;

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

    unsafe {
        target.DrawLine(
            Vector2 { X: HERO_DIVIDER_X as f32 + 0.5, Y: 65.0 },
            Vector2 { X: HERO_DIVIDER_X as f32 + 0.5, Y: 132.0 },
            &brushes.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
        draw_text(
            target,
            locale.text("Reset in", "距离重置"),
            rect(220.0, 59.0, 346.0, 82.0),
            &formats.label,
            &brushes.muted,
        );
        draw_text(target, &reset.0, rect(220.0, 79.0, 346.0, 108.0), &formats.value, &brushes.text);
        draw_text(
            target,
            &reset.1,
            rect(220.0, 107.0, 346.0, 132.0),
            &formats.secondary,
            &brushes.muted,
        );
    }

    draw_quota_track(target, &brushes, state);
    draw_metrics(target, formats, &brushes, state, locale);
    draw_projection(target, formats, state, locale, theme)?;
    Ok(())
}

unsafe fn draw_header(
    target: &ID2D1HwndRenderTarget,
    formats: &FormatSet,
    brushes: &Brushes,
    state: &DisplayState,
) {
    unsafe {
        target.FillRoundedRectangle(&rounded_rect(18.0, 15.0, 20.0, 31.0, 2.0), &brushes.status);
        draw_text(
            target,
            "CodexStatus",
            rect(29.0, 7.0, 180.0, 41.0),
            &formats.header,
            &brushes.text,
        );
        draw_text(
            target,
            &updated_text(state, formats.locale),
            rect(190.0, 8.0, 358.0, 40.0),
            &formats.update,
            &brushes.muted,
        );
    }
}

fn draw_percentage(
    target: &ID2D1HwndRenderTarget,
    dwrite: &IDWriteFactory,
    formats: &FormatSet,
    brushes: &Brushes,
    percent: Option<u8>,
) -> Result<()> {
    let number = percent.map_or_else(|| "--".to_owned(), |value| value.to_string());
    let width = text_width(dwrite, &number, &formats.quota, 160.0, 58.0)?;
    unsafe {
        draw_text(target, &number, rect(30.0, 77.0, 160.0, 134.0), &formats.quota, &brushes.text);
        if percent.is_some() {
            draw_text(
                target,
                "%",
                rect(33.0 + width, 97.0, 177.0, 134.0),
                &formats.percent,
                &brushes.muted,
            );
        }
    }
    Ok(())
}

fn draw_quota_track(target: &ID2D1HwndRenderTarget, brushes: &Brushes, state: &DisplayState) {
    let left = 30.0;
    let right = 346.0;
    let top = 148.0;
    let bottom = 154.0;
    unsafe {
        target.FillRoundedRectangle(&rounded_rect(left, top, right, bottom, 6.0), &brushes.line)
    };

    if let Some(value) = state.weekly_percent()
        && value > 0
    {
        let filled = (left + (right - left) * f32::from(value) / 100.0).clamp(left + 6.0, right);
        unsafe {
            target.FillRoundedRectangle(
                &rounded_rect(left, top, filled, bottom, 6.0),
                &brushes.quota,
            );
        }
    }
}

fn draw_metrics(
    target: &ID2D1HwndRenderTarget,
    formats: &FormatSet,
    brushes: &Brushes,
    state: &DisplayState,
    locale: Locale,
) {
    let session = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.session.as_ref())
        .map_or_else(|| "--".to_owned(), |window| format!("{}%", window.display_percent()));
    let plan = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.account.plan_type.as_deref())
        .map(|plan| plan_label(plan, locale))
        .unwrap_or("--")
        .to_owned();
    let credits = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.account.reset_credits)
        .map(|credits| format!("{credits} {}", locale.text("resets", "次")))
        .unwrap_or_else(|| "--".to_owned());
    let credit_detail =
        state.snapshot.as_ref().and_then(|snapshot| reset_credit_detail(&snapshot.account, locale));

    unsafe {
        let metrics = rounded_rect(16.0, 184.0, 360.0, 252.0, 12.0);
        target.FillRoundedRectangle(&metrics, &brushes.surface_alt);
        target.DrawRoundedRectangle(&metrics, &brushes.line, 1.0, None::<&ID2D1StrokeStyle>);
    }
    draw_metric_divider(target, brushes, METRICS_LEFT_DIVIDER_X as f32 + 0.5);
    draw_metric_divider(target, brushes, HERO_DIVIDER_X as f32 + 0.5);
    draw_metric(
        target,
        formats,
        brushes,
        rect(METRICS_LEFT_X as f32, 184.0, METRICS_LEFT_DIVIDER_X as f32, 252.0),
        locale.text("5-hour quota", "5 小时额度"),
        &session,
        None,
    );
    draw_metric(
        target,
        formats,
        brushes,
        rect(METRICS_LEFT_DIVIDER_X as f32 + 1.0, 184.0, HERO_DIVIDER_X as f32, 252.0),
        locale.text("Plan", "套餐"),
        &plan,
        None,
    );
    draw_metric(
        target,
        formats,
        brushes,
        rect(HERO_DIVIDER_X as f32 + 1.0, 184.0, 360.0, 252.0),
        locale.text("Reset credits", "重置机会"),
        &credits,
        credit_detail.as_deref(),
    );
}

fn draw_metric_divider(target: &ID2D1HwndRenderTarget, brushes: &Brushes, x: f32) {
    unsafe {
        target.DrawLine(
            Vector2 { X: x, Y: 197.0 },
            Vector2 { X: x, Y: 239.0 },
            &brushes.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
    }
}

fn draw_metric(
    target: &ID2D1HwndRenderTarget,
    formats: &FormatSet,
    brushes: &Brushes,
    area: D2D_RECT_F,
    label: &str,
    value: &str,
    detail: Option<&str>,
) {
    unsafe {
        draw_text(
            target,
            label,
            rect(area.left + 12.0, area.top + 5.0, area.right - 10.0, area.top + 25.0),
            &formats.metric_label,
            &brushes.muted,
        );
        draw_text(
            target,
            value,
            rect(area.left + 12.0, area.top + 19.0, area.right - 10.0, area.bottom - 16.0),
            &formats.metric_value,
            &brushes.text,
        );
        if let Some(detail) = detail {
            draw_text(
                target,
                detail,
                rect(area.left + 12.0, area.top + 47.0, area.right - 10.0, area.bottom - 4.0),
                &formats.metric_detail,
                &brushes.muted,
            );
        }
    }
}

fn draw_projection(
    target: &ID2D1HwndRenderTarget,
    formats: &FormatSet,
    state: &DisplayState,
    locale: Locale,
    theme: Theme,
) -> Result<()> {
    let projection = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.weekly_usage_projection(Local::now().timestamp()))
        .map(|projection| projection_label(projection, locale));
    if let Some(projection) = projection {
        let brush = solid_brush(target, projection_color(projection.ample, theme.high_contrast))?;
        unsafe {
            draw_text(
                target,
                &projection.text,
                rect(18.0, 258.0, 358.0, 289.0),
                &formats.footer,
                &brush,
            );
        }
    }
    Ok(())
}

unsafe fn draw_text(
    target: &ID2D1HwndRenderTarget,
    value: &str,
    area: D2D_RECT_F,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
) {
    let text: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        target.DrawText(
            &text,
            format,
            &area,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        )
    };
}

fn text_width(
    factory: &IDWriteFactory,
    value: &str,
    format: &IDWriteTextFormat,
    max_width: f32,
    max_height: f32,
) -> Result<f32> {
    let text: Vec<u16> = value.encode_utf16().collect();
    let layout = unsafe { factory.CreateTextLayout(&text, format, max_width, max_height)? };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut metrics)? };
    Ok(metrics.widthIncludingTrailingWhitespace)
}

fn solid_brush(target: &ID2D1HwndRenderTarget, value: COLORREF) -> Result<ID2D1SolidColorBrush> {
    unsafe { target.CreateSolidColorBrush(&color(value), None) }
}

fn color(value: COLORREF) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (value.0 & 0xff) as f32 / 255.0,
        g: ((value.0 >> 8) & 0xff) as f32 / 255.0,
        b: ((value.0 >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

const fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

const fn rounded_rect(
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    radius: f32,
) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(left, top, right, bottom), radiusX: radius, radiusY: radius }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_geometry_stays_inside_the_logical_surface() {
        let hero = rounded_rect(16.0, 48.0, 360.0, 176.0, 14.0);
        assert!(hero.rect.left >= 0.0);
        assert!(hero.rect.right <= super::super::CARD_WIDTH as f32);
        assert!(hero.rect.bottom <= super::super::CARD_HEIGHT as f32);
    }
}
