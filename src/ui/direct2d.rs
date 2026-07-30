//! Low-overhead Direct2D/DirectWrite renderer for the opaque flyout.
//!
//! An HWND render target avoids the D3D11/DXGI/DirectComposition device tree
//! used by transparent composition UIs. Device-independent factories and text
//! formats stay warm, while the HWND-sized target and its brushes are released
//! whenever the flyout is hidden.

use super::{
    AccountMetrics, HEADER_ACCENT_BOTTOM, HEADER_ACCENT_TOP, HEADER_TEXT_BOTTOM, HEADER_TEXT_TOP,
    HEADER_VERSION_BOTTOM, HEADER_VERSION_TOP, Locale, QuotaPanelGeometry, QuotaPanelSlot, Theme,
    accent_for, account_metrics, flyout_dimensions, inner_track_color, outer_track_color,
    quota_bar_color, quota_card_colors, quota_label, quota_panel_geometry, reset_details,
    theoretical_color, theoretical_remaining_percent, updated_text, version_text,
};
use crate::model::{DisplayState, QuotaAvailability, QuotaKind, QuotaWindow};
use chrono::Local;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID,
    D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_GAMMA_2_2, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINE_JOIN_ROUND,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE,
    D2D1CreateFactory, ID2D1Brush, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1LinearGradientBrush,
    ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_RANGE, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection,
    IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::core::{PCWSTR, Result, w};
use windows_numerics::Vector2;

thread_local! {
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
    static PERMANENT_FALLBACK: Cell<bool> = const { Cell::new(false) };
}

pub(super) struct PaintInput<'a> {
    pub hwnd: HWND,
    pub pixel_size: (i32, i32),
    pub dpi: u32,
    pub state: &'a DisplayState,
    pub preferred: QuotaKind,
    pub locale: Locale,
    pub theme: Theme,
    pub pressed_quota: Option<QuotaKind>,
}

pub(super) fn paint(input: PaintInput<'_>) -> bool {
    if PERMANENT_FALLBACK.with(Cell::get) {
        return false;
    }
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match Renderer::new() {
                Ok(renderer) => *slot = Some(renderer),
                Err(_) => {
                    PERMANENT_FALLBACK.with(|fallback| fallback.set(true));
                    return false;
                }
            }
        }
        let renderer = slot.as_mut().expect("renderer initialized");
        match renderer.paint(&input) {
            Ok(()) => true,
            Err(error) => {
                renderer.surface = None;
                if error.code() != D2DERR_RECREATE_TARGET {
                    PERMANENT_FALLBACK.with(|fallback| fallback.set(true));
                }
                false
            }
        }
    })
}

pub(super) fn release_surface() {
    RENDERER.with(|slot| {
        if let Some(renderer) = slot.borrow_mut().as_mut() {
            renderer.surface = None;
        }
    });
    PERMANENT_FALLBACK.with(|fallback| fallback.set(false));
}

pub(super) fn release_all() {
    RENDERER.with(|slot| *slot.borrow_mut() = None);
    PERMANENT_FALLBACK.with(|fallback| fallback.set(false));
}

struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    stroke_style: ID2D1StrokeStyle,
    formats: Option<FormatSet>,
    surface: Option<Surface>,
}

impl Renderer {
    fn new() -> Result<Self> {
        unsafe {
            let factory =
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)?;
            let stroke_style = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 1.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            Ok(Self { factory, dwrite, stroke_style, formats: None, surface: None })
        }
    }

    fn paint(&mut self, input: &PaintInput<'_>) -> Result<()> {
        self.ensure_surface(input.hwnd, input.pixel_size, input.dpi)?;
        self.ensure_formats(input.locale)?;
        let formats = self.formats.as_ref().expect("formats initialized");
        let surface = self.surface.as_mut().expect("surface initialized");
        let palette = Palette::new(input);
        surface.ensure_brushes(palette)?;
        let target = surface.target.clone();
        let brushes = surface.brushes.as_ref().expect("brushes initialized");

        unsafe {
            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            target.BeginDraw();
            target.Clear(Some(&color(input.theme.background)));
        }
        let draw_result = draw_frame(
            &target,
            &self.factory,
            &self.dwrite,
            &self.stroke_style,
            formats,
            brushes,
            input,
        );
        let end_result = unsafe { target.EndDraw(None, None) };
        end_result?;
        draw_result
    }

    fn ensure_formats(&mut self, locale: Locale) -> Result<()> {
        if self.formats.as_ref().is_none_or(|formats| formats.locale != locale) {
            self.formats = Some(FormatSet::new(&self.dwrite, locale)?);
        }
        Ok(())
    }

    fn ensure_surface(&mut self, hwnd: HWND, pixel_size: (i32, i32), dpi: u32) -> Result<()> {
        let pixel_size =
            D2D_SIZE_U { width: pixel_size.0.max(1) as u32, height: pixel_size.1.max(1) as u32 };
        let wrong_window = self.surface.as_ref().is_some_and(|surface| surface.hwnd != hwnd);
        if wrong_window {
            self.surface = None;
        }
        if let Some(surface) = &mut self.surface {
            if surface.pixel_size != pixel_size {
                unsafe { surface.target.Resize(&pixel_size)? };
                surface.pixel_size = pixel_size;
            }
            if surface.dpi != dpi {
                unsafe { surface.target.SetDpi(dpi as f32, dpi as f32) };
                surface.dpi = dpi;
            }
            return Ok(());
        }

        let render_properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_UNKNOWN,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi as f32,
            dpiY: dpi as f32,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: pixel_size,
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let target =
            unsafe { self.factory.CreateHwndRenderTarget(&render_properties, &hwnd_properties)? };
        self.surface =
            Some(Surface { hwnd, target, pixel_size, dpi, palette: None, brushes: None });
        Ok(())
    }
}

struct Surface {
    hwnd: HWND,
    target: ID2D1HwndRenderTarget,
    pixel_size: D2D_SIZE_U,
    dpi: u32,
    palette: Option<Palette>,
    brushes: Option<Brushes>,
}

impl Surface {
    fn ensure_brushes(&mut self, palette: Palette) -> Result<()> {
        if self.palette != Some(palette) {
            self.brushes = Some(Brushes::new(&self.target, palette)?);
            self.palette = Some(palette);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Palette {
    dark: bool,
    status: COLORREF,
    background: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    line: COLORREF,
    metrics_surface: COLORREF,
    five_surface: COLORREF,
    five_border: COLORREF,
    five_title: COLORREF,
    five_track: COLORREF,
    five_actual: COLORREF,
    weekly_surface: COLORREF,
    weekly_border: COLORREF,
    weekly_title: COLORREF,
    weekly_track: COLORREF,
    weekly_actual: COLORREF,
    inner_track: COLORREF,
    theoretical: COLORREF,
}

impl Palette {
    fn new(input: &PaintInput<'_>) -> Self {
        let effective = input.state.resolved_quota_kind(input.preferred);
        let status = accent_for(
            input.state,
            input.state.quota_percent(effective),
            input.theme.high_contrast,
        );
        let compact = matches!(input.state.quota_availability(), QuotaAvailability::Single(_));
        let five_selected = !compact && input.preferred == QuotaKind::FiveHour;
        let weekly_selected = !compact && input.preferred == QuotaKind::Weekly;
        let five_pressed = !compact && input.pressed_quota == Some(QuotaKind::FiveHour);
        let weekly_pressed = !compact && input.pressed_quota == Some(QuotaKind::Weekly);
        let five_actual = input.state.quota_percent(QuotaKind::FiveHour);
        let weekly_actual = input.state.quota_percent(QuotaKind::Weekly);
        let (five_surface, five_border) =
            quota_card_colors(input.theme, five_selected, five_pressed);
        let (weekly_surface, weekly_border) =
            quota_card_colors(input.theme, weekly_selected, weekly_pressed);
        Self {
            dark: input.theme.dark,
            status,
            background: input.theme.background,
            text: input.theme.text,
            muted: input.theme.muted,
            line: input.theme.line,
            metrics_surface: input.theme.surface_alt,
            five_surface,
            five_border,
            five_title: panel_title_color(input.theme, five_selected),
            five_track: outer_track_color(input.theme, five_selected),
            five_actual: quota_bar_color(five_actual, input.theme.high_contrast),
            weekly_surface,
            weekly_border,
            weekly_title: panel_title_color(input.theme, weekly_selected),
            weekly_track: outer_track_color(input.theme, weekly_selected),
            weekly_actual: quota_bar_color(weekly_actual, input.theme.high_contrast),
            inner_track: inner_track_color(input.theme),
            theoretical: theoretical_color(input.theme),
        }
    }
}

fn panel_title_color(theme: Theme, selected: bool) -> COLORREF {
    if !selected || theme.high_contrast {
        theme.muted
    } else if theme.dark {
        super::rgb(115, 204, 175)
    } else {
        super::rgb(8, 125, 97)
    }
}

struct Brushes {
    status: ID2D1SolidColorBrush,
    background: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    line: ID2D1SolidColorBrush,
    metrics_surface: ID2D1SolidColorBrush,
    five_surface: ID2D1SolidColorBrush,
    five_border: ID2D1SolidColorBrush,
    five_title: ID2D1SolidColorBrush,
    five_track: ID2D1SolidColorBrush,
    five_actual: ID2D1SolidColorBrush,
    weekly_surface: ID2D1SolidColorBrush,
    weekly_border: ID2D1SolidColorBrush,
    weekly_title: ID2D1SolidColorBrush,
    weekly_track: ID2D1SolidColorBrush,
    weekly_actual: ID2D1SolidColorBrush,
    inner_track: ID2D1SolidColorBrush,
    theoretical: ID2D1SolidColorBrush,
    shadow_far: ID2D1SolidColorBrush,
    shadow_near: ID2D1SolidColorBrush,
    healthy_gradient: ID2D1LinearGradientBrush,
    warning_gradient: ID2D1LinearGradientBrush,
    critical_gradient: ID2D1LinearGradientBrush,
}

impl Brushes {
    fn new(target: &ID2D1HwndRenderTarget, palette: Palette) -> Result<Self> {
        let (healthy, warning, critical) = if palette.dark {
            (
                ((43, 190, 154), (10, 137, 103)),
                ((230, 171, 61), (185, 113, 8)),
                ((228, 102, 109), (184, 55, 65)),
            )
        } else {
            (
                ((37, 181, 147), (7, 139, 105)),
                ((226, 161, 42), (184, 105, 0)),
                ((226, 91, 99), (184, 45, 56)),
            )
        };
        Ok(Self {
            status: brush(target, palette.status)?,
            background: brush(target, palette.background)?,
            text: brush(target, palette.text)?,
            muted: brush(target, palette.muted)?,
            line: brush(target, palette.line)?,
            metrics_surface: brush(target, palette.metrics_surface)?,
            five_surface: brush(target, palette.five_surface)?,
            five_border: brush(target, palette.five_border)?,
            five_title: brush(target, palette.five_title)?,
            five_track: brush(target, palette.five_track)?,
            five_actual: brush(target, palette.five_actual)?,
            weekly_surface: brush(target, palette.weekly_surface)?,
            weekly_border: brush(target, palette.weekly_border)?,
            weekly_title: brush(target, palette.weekly_title)?,
            weekly_track: brush(target, palette.weekly_track)?,
            weekly_actual: brush(target, palette.weekly_actual)?,
            inner_track: brush(target, palette.inner_track)?,
            theoretical: brush(target, palette.theoretical)?,
            shadow_far: alpha_brush(target, if palette.dark { 0.22 } else { 0.055 })?,
            shadow_near: alpha_brush(target, if palette.dark { 0.28 } else { 0.075 })?,
            healthy_gradient: gradient_brush(target, healthy.0, healthy.1)?,
            warning_gradient: gradient_brush(target, warning.0, warning.1)?,
            critical_gradient: gradient_brush(target, critical.0, critical.1)?,
        })
    }
}

#[derive(Clone)]
struct FormatSet {
    locale: Locale,
    header: IDWriteTextFormat,
    update: IDWriteTextFormat,
    title: IDWriteTextFormat,
    quota: IDWriteTextFormat,
    countdown: IDWriteTextFormat,
    date: IDWriteTextFormat,
    metric_label: IDWriteTextFormat,
    metric_value: IDWriteTextFormat,
    detail: IDWriteTextFormat,
    stacked_label: IDWriteTextFormat,
    stacked_value: IDWriteTextFormat,
    stacked_detail: IDWriteTextFormat,
}

impl FormatSet {
    fn new(factory: &IDWriteFactory, locale: Locale) -> Result<Self> {
        Ok(Self {
            locale,
            header: make_format(
                factory,
                locale,
                14.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                false,
            )?,
            update: make_format(
                factory,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                true,
            )?,
            title: make_format(
                factory,
                locale,
                15.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
            quota: make_format(
                factory,
                locale,
                36.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                false,
            )?,
            countdown: make_format(
                factory,
                locale,
                14.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
            date: make_format(
                factory,
                locale,
                12.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
            metric_label: make_format(
                factory,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            metric_value: make_format(
                factory,
                locale,
                20.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                true,
            )?,
            detail: make_format(
                factory,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                true,
            )?,
            stacked_label: make_format(
                factory,
                locale,
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
            stacked_value: make_format(
                factory,
                locale,
                20.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
            stacked_detail: make_format(
                factory,
                locale,
                10.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                true,
            )?,
        })
    }
}

fn make_format(
    factory: &IDWriteFactory,
    locale: Locale,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    alignment: DWRITE_TEXT_ALIGNMENT,
    trim: bool,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = factory.CreateTextFormat(
            w!("Microsoft YaHei UI"),
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

const fn locale_name(locale: Locale) -> PCWSTR {
    match locale {
        Locale::Chinese => w!("zh-CN"),
        Locale::English => w!("en-US"),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_frame(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    stroke_style: &ID2D1StrokeStyle,
    formats: &FormatSet,
    brushes: &Brushes,
    input: &PaintInput<'_>,
) -> Result<()> {
    let dimensions = flyout_dimensions(input.state);
    let width = dimensions.width as f32;
    let height = dimensions.height as f32;
    unsafe {
        target.FillRectangle(&rect(0.0, 0.0, width, height), &brushes.background);
        target.DrawRoundedRectangle(
            &rounded_rect(0.5, 0.5, width - 0.5, height - 0.5, 8.0),
            &brushes.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
        target.FillRoundedRectangle(
            &rounded_rect(18.0, HEADER_ACCENT_TOP as f32, 20.0, HEADER_ACCENT_BOTTOM as f32, 1.0),
            &brushes.status,
        );
    }
    draw_text(
        target,
        "CodexStatus",
        rect(29.0, HEADER_TEXT_TOP as f32, 180.0, HEADER_TEXT_BOTTOM as f32),
        &formats.header,
        &brushes.text,
    );
    let title_width =
        measure_text(dwrite, "CodexStatus", &formats.header)?.widthIncludingTrailingWhitespace;
    draw_text(
        target,
        &version_text(),
        rect(29.0 + title_width, HEADER_VERSION_TOP as f32, 190.0, HEADER_VERSION_BOTTOM as f32),
        &formats.metric_label,
        &brushes.muted,
    );
    draw_text(
        target,
        &updated_text(input.state, input.locale),
        rect(190.0, HEADER_TEXT_TOP as f32, width - 18.0, HEADER_TEXT_BOTTOM as f32),
        &formats.update,
        &brushes.muted,
    );

    let account = account_metrics(input.state, input.locale);
    match input.state.quota_availability() {
        QuotaAvailability::Single(kind) => {
            draw_quota_panel(
                target,
                factory,
                dwrite,
                stroke_style,
                formats,
                brushes,
                input,
                kind,
                QuotaPanelSlot::Left,
            )?;
            draw_stacked_metrics(target, formats, brushes, input, &account);
        }
        QuotaAvailability::None | QuotaAvailability::Both => {
            draw_quota_panel(
                target,
                factory,
                dwrite,
                stroke_style,
                formats,
                brushes,
                input,
                QuotaKind::FiveHour,
                QuotaPanelSlot::Left,
            )?;
            draw_quota_panel(
                target,
                factory,
                dwrite,
                stroke_style,
                formats,
                brushes,
                input,
                QuotaKind::Weekly,
                QuotaPanelSlot::Right,
            )?;
            draw_bottom_metrics(target, dwrite, formats, brushes, input, &account);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_quota_panel(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    stroke_style: &ID2D1StrokeStyle,
    formats: &FormatSet,
    brushes: &Brushes,
    input: &PaintInput<'_>,
    kind: QuotaKind,
    slot: QuotaPanelSlot,
) -> Result<()> {
    let compact = matches!(input.state.quota_availability(), QuotaAvailability::Single(_));
    let selected = !compact && input.preferred == kind;
    let pressed = !compact && input.pressed_quota == Some(kind);
    let geometry = quota_panel_geometry(slot);
    let QuotaPanelGeometry { left, right, center_x } = geometry;
    let (surface, border, title, track, actual_brush) = match kind {
        QuotaKind::FiveHour => (
            &brushes.five_surface,
            &brushes.five_border,
            &brushes.five_title,
            &brushes.five_track,
            &brushes.five_actual,
        ),
        QuotaKind::Weekly => (
            &brushes.weekly_surface,
            &brushes.weekly_border,
            &brushes.weekly_title,
            &brushes.weekly_track,
            &brushes.weekly_actual,
        ),
    };
    let left = left as f32;
    let right = right as f32;
    let center_x = center_x as f32;
    let card = rounded_rect(left + 0.5, 40.5, right - 0.5, 267.5, 10.0);
    if !input.theme.high_contrast {
        draw_card_shadow(target, &card, brushes);
    }
    unsafe {
        target.FillRoundedRectangle(&card, surface);
        target.DrawRoundedRectangle(&card, border, 1.0, None::<&ID2D1StrokeStyle>);
    }
    if input.theme.high_contrast && (selected || pressed) {
        let marker_width = if pressed { 52.0 } else { 36.0 };
        let marker_height = if pressed { 5.0 } else { 3.0 };
        unsafe {
            target.FillRoundedRectangle(
                &rounded_rect(
                    center_x - marker_width / 2.0,
                    260.0,
                    center_x + marker_width / 2.0,
                    260.0 + marker_height,
                    marker_height / 2.0,
                ),
                &brushes.text,
            );
        }
    }
    draw_text(
        target,
        quota_label(kind, input.locale),
        rect(left + 8.0, 47.0, right - 8.0, 78.0),
        &formats.title,
        title,
    );

    let window = input.state.quota_window(kind);
    let actual = window.map(QuotaWindow::display_percent);
    let theoretical =
        window.and_then(|window| theoretical_remaining_percent(window, Local::now().timestamp()));
    draw_arc(target, factory, stroke_style, center_x, 149.0, 66.0, 10.0, 100, track)?;
    if let Some(percent) = actual.filter(|percent| *percent > 0) {
        let arc_brush: &ID2D1Brush = if input.theme.high_contrast {
            actual_brush
        } else {
            let gradient = quota_gradient(brushes, percent);
            unsafe {
                gradient.SetStartPoint(Vector2 { X: center_x - 66.0, Y: 149.0 });
                gradient.SetEndPoint(Vector2 { X: center_x + 66.0, Y: 149.0 });
            }
            gradient
        };
        draw_arc(target, factory, stroke_style, center_x, 149.0, 66.0, 10.0, percent, arc_brush)?;
    }
    draw_arc(target, factory, stroke_style, center_x, 149.0, 54.0, 8.0, 100, &brushes.inner_track)?;
    if let Some(percent) = theoretical.filter(|percent| *percent > 0) {
        draw_arc(
            target,
            factory,
            stroke_style,
            center_x,
            149.0,
            54.0,
            8.0,
            percent,
            &brushes.theoretical,
        )?;
    }
    draw_percentage(target, dwrite, formats, &brushes.text, &brushes.muted, center_x, actual)?;

    let reset = window
        .map(|window| reset_details(window, input.locale))
        .unwrap_or_else(|| (input.locale.text("Unavailable", "暂无").to_owned(), "--".to_owned()));
    draw_text(
        target,
        &reset.0,
        rect(left + 8.0, 199.0, right - 8.0, 232.0),
        &formats.countdown,
        &brushes.text,
    );
    draw_text(
        target,
        &reset.1,
        rect(left + 8.0, 225.0, right - 8.0, 256.0),
        &formats.date,
        &brushes.muted,
    );
    Ok(())
}

fn draw_stacked_metrics(
    target: &ID2D1HwndRenderTarget,
    formats: &FormatSet,
    brushes: &Brushes,
    input: &PaintInput<'_>,
    account: &AccountMetrics,
) {
    let metrics = rounded_rect(192.5, 40.5, 319.5, 267.5, 10.0);
    if !input.theme.high_contrast {
        draw_card_shadow(target, &metrics, brushes);
    }
    unsafe {
        target.FillRoundedRectangle(&metrics, &brushes.five_surface);
        target.DrawRoundedRectangle(&metrics, &brushes.line, 1.0, None::<&ID2D1StrokeStyle>);
        target.DrawLine(
            Vector2 { X: 208.0, Y: 153.5 },
            Vector2 { X: 304.0, Y: 153.5 },
            &brushes.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
    }
    draw_text(
        target,
        input.locale.text("Plan", "套餐"),
        rect(200.0, 59.0, 312.0, 83.0),
        &formats.stacked_label,
        &brushes.muted,
    );
    draw_text(
        target,
        &account.plan,
        rect(200.0, 87.0, 312.0, 130.0),
        &formats.stacked_value,
        &brushes.text,
    );
    draw_text(
        target,
        input.locale.text("Reset credits", "重置机会"),
        rect(200.0, 173.0, 312.0, 197.0),
        &formats.stacked_label,
        &brushes.muted,
    );
    draw_text(
        target,
        &account.credits,
        rect(200.0, 198.0, 312.0, 236.0),
        &formats.stacked_value,
        &brushes.text,
    );
    if let Some(detail) = account.credit_detail.as_deref() {
        draw_text(
            target,
            detail,
            rect(197.0, 237.0, 315.0, 262.0),
            &formats.stacked_detail,
            &brushes.muted,
        );
    }
}

fn draw_bottom_metrics(
    target: &ID2D1HwndRenderTarget,
    dwrite: &IDWriteFactory,
    formats: &FormatSet,
    brushes: &Brushes,
    input: &PaintInput<'_>,
    account: &AccountMetrics,
) {
    let metrics = rounded_rect(16.5, 276.5, 359.5, 335.5, 10.0);
    if !input.theme.high_contrast {
        draw_card_shadow(target, &metrics, brushes);
    }
    unsafe {
        target.FillRoundedRectangle(&metrics, &brushes.metrics_surface);
        target.DrawRoundedRectangle(&metrics, &brushes.line, 1.0, None::<&ID2D1StrokeStyle>);
        target.DrawLine(
            Vector2 { X: 188.5, Y: 288.0 },
            Vector2 { X: 188.5, Y: 324.0 },
            &brushes.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
    }
    draw_text(
        target,
        input.locale.text("Plan", "套餐"),
        rect(30.0, 278.0, 178.0, 303.0),
        &formats.metric_label,
        &brushes.muted,
    );
    draw_text(
        target,
        &account.plan,
        rect(30.0, 298.0, 178.0, 334.0),
        &formats.metric_value,
        &brushes.text,
    );
    draw_text(
        target,
        input.locale.text("Reset credits", "重置机会"),
        rect(203.0, 278.0, 350.0, 303.0),
        &formats.metric_label,
        &brushes.muted,
    );
    draw_text(
        target,
        &account.credits,
        rect(203.0, 296.0, 350.0, 332.0),
        &formats.metric_value,
        &brushes.text,
    );
    if let Some(detail) = account.credit_detail.as_deref() {
        let value_width = measure_text(dwrite, &account.credits, &formats.metric_value)
            .map(|metrics| metrics.widthIncludingTrailingWhitespace)
            .unwrap_or(45.0);
        draw_text(
            target,
            detail,
            rect((203.0 + value_width + 10.0).min(300.0), 303.0, 350.0, 335.0),
            &formats.detail,
            &brushes.muted,
        );
    }
}

fn draw_percentage(
    target: &ID2D1HwndRenderTarget,
    dwrite: &IDWriteFactory,
    formats: &FormatSet,
    text_brush: &ID2D1SolidColorBrush,
    percent_brush: &ID2D1SolidColorBrush,
    center_x: f32,
    percent: Option<u8>,
) -> Result<()> {
    let value = percent.map_or_else(|| "--".to_owned(), |value| format!("{value}%"));
    let text: Vec<u16> = value.encode_utf16().collect();
    let layout = unsafe { dwrite.CreateTextLayout(&text, &formats.quota, 146.0, 72.0)? };
    if percent.is_some() {
        let range =
            DWRITE_TEXT_RANGE { startPosition: text.len().saturating_sub(1) as u32, length: 1 };
        unsafe {
            layout.SetFontSize(15.0, range)?;
            layout.SetDrawingEffect(percent_brush, range)?;
        }
    }
    unsafe {
        target.DrawTextLayout(
            Vector2 { X: center_x - 73.0, Y: 111.0 },
            &layout,
            text_brush,
            windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_arc(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    stroke_style: &ID2D1StrokeStyle,
    center_x: f32,
    center_y: f32,
    radius: f32,
    width: f32,
    percent: u8,
    brush: &ID2D1Brush,
) -> Result<()> {
    if percent == 0 {
        return Ok(());
    }
    let geometry = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { geometry.Open()? };
    let points = arc_points(center_x, center_y, radius, percent);
    unsafe {
        sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_HOLLOW);
        for point in &points[1..] {
            sink.AddLine(*point);
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        target.DrawGeometry(&geometry, brush, width, stroke_style);
    }
    Ok(())
}

fn quota_gradient(brushes: &Brushes, percent: u8) -> &ID2D1LinearGradientBrush {
    if percent > 49 {
        &brushes.healthy_gradient
    } else if percent > 19 {
        &brushes.warning_gradient
    } else {
        &brushes.critical_gradient
    }
}

fn draw_card_shadow(target: &ID2D1HwndRenderTarget, card: &D2D1_ROUNDED_RECT, brushes: &Brushes) {
    unsafe {
        target.FillRoundedRectangle(
            &rounded_rect(
                card.rect.left - 0.75,
                card.rect.top + 1.5,
                card.rect.right + 2.5,
                card.rect.bottom + 3.5,
                card.radiusX + 1.0,
            ),
            &brushes.shadow_far,
        );
        target.FillRoundedRectangle(
            &rounded_rect(
                card.rect.left - 0.25,
                card.rect.top + 0.75,
                card.rect.right + 1.5,
                card.rect.bottom + 2.0,
                card.radiusX + 0.5,
            ),
            &brushes.shadow_near,
        );
    }
}

fn arc_points(center_x: f32, center_y: f32, radius: f32, percent: u8) -> Vec<Vector2> {
    const START_DEGREES: f32 = 145.0;
    const SWEEP_DEGREES: f32 = 250.0;
    let sweep = SWEEP_DEGREES * f32::from(percent.min(100)) / 100.0;
    let segments = ((sweep / 2.0).ceil() as usize).max(1);
    (0..=segments)
        .map(|index| {
            let angle = (START_DEGREES + sweep * index as f32 / segments as f32).to_radians();
            Vector2 { X: center_x + radius * angle.cos(), Y: center_y + radius * angle.sin() }
        })
        .collect()
}

fn draw_text(
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
            windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

fn measure_text(
    factory: &IDWriteFactory,
    value: &str,
    format: &IDWriteTextFormat,
) -> Result<windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS> {
    let text: Vec<u16> = value.encode_utf16().collect();
    let layout = unsafe { factory.CreateTextLayout(&text, format, 1000.0, 100.0)? };
    let mut metrics = Default::default();
    unsafe { layout.GetMetrics(&mut metrics)? };
    Ok(metrics)
}

fn brush(target: &ID2D1HwndRenderTarget, value: COLORREF) -> Result<ID2D1SolidColorBrush> {
    unsafe { target.CreateSolidColorBrush(&color(value), None) }
}

fn alpha_brush(target: &ID2D1HwndRenderTarget, alpha: f32) -> Result<ID2D1SolidColorBrush> {
    unsafe { target.CreateSolidColorBrush(&rgba(0, 0, 0, alpha), None) }
}

fn gradient_brush(
    target: &ID2D1HwndRenderTarget,
    start: (u8, u8, u8),
    end: (u8, u8, u8),
) -> Result<ID2D1LinearGradientBrush> {
    let stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: rgba(start.0, start.1, start.2, 1.0) },
        D2D1_GRADIENT_STOP { position: 1.0, color: rgba(end.0, end.1, end.2, 1.0) },
    ];
    unsafe {
        let collection =
            target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        target.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: 0.0, Y: 0.0 },
                endPoint: Vector2 { X: 1.0, Y: 0.0 },
            },
            None,
            &collection,
        )
    }
}

fn rgba(red: u8, green: u8, blue: u8, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(red) / 255.0,
        g: f32::from(green) / 255.0,
        b: f32::from(blue) / 255.0,
        a: alpha,
    }
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
    fn direct2d_arc_keeps_the_existing_opening() {
        let points = arc_points(100.0, 149.0, 66.0, 100);
        let first = points.first().expect("start");
        let last = points.last().expect("end");
        assert!((first.X - 45.94).abs() < 0.1);
        assert!((first.Y - 186.86).abs() < 0.1);
        assert!((last.X - 154.06).abs() < 0.1);
        assert!((last.Y - 186.86).abs() < 0.1);
    }

    #[test]
    fn palette_channels_preserve_colorref_order() {
        let value = super::super::rgb(16, 163, 127);
        let converted = color(value);
        assert!((converted.r - 16.0 / 255.0).abs() < f32::EPSILON);
        assert!((converted.g - 163.0 / 255.0).abs() < f32::EPSILON);
        assert!((converted.b - 127.0 / 255.0).abs() < f32::EPSILON);
    }
}
