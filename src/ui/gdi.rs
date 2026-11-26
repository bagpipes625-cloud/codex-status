//! GDI fallback renderer for environments where Direct2D is unavailable.

use super::CardView;
use super::format::{
    AccountMetrics, ClaudeSummaryRows, ClaudeWindowSummary, Locale, WeeklyUsageMetrics,
    account_metrics, claude_summary_rows, header_title, quota_label, reset_details, resource_label,
    theoretical_remaining_percent, ui_font_face, updated_text, version_text, weekly_usage_metrics,
};
use super::history_view::{HistoryNavigation, HistoryPage, ResourcePage, UsageSummaryWeek};
use super::layout::{
    BOTTOM_DIVIDER_X, BOTTOM_METRICS_CARD, COMPACT_CLAUDE_FIVE_HOUR, COMPACT_CLAUDE_WEEKLY,
    COMPACT_RESOURCE_HEADING, COMPACT_USAGE_HEADING, FLYOUT_CORNER_RADIUS, FULL_CLAUDE_FIVE_HOUR,
    FULL_CLAUDE_WEEKLY, FULL_RESOURCE_HEADING, FULL_USAGE_HEADING, FlyoutDimensions,
    HEADER_ACCENT_BOTTOM, HEADER_ACCENT_TOP, HEADER_TEXT_BOTTOM, HEADER_TEXT_TOP,
    HEADER_VERSION_BOTTOM, HEADER_VERSION_TOP, QUOTA_ARC_CENTER_Y, QuotaPanelSlot,
    REFRESH_ARC_START_DEGREES, REFRESH_ARC_SWEEP_DEGREES, REFRESH_BUTTON_GAP,
    REFRESH_BUTTON_RADIUS, REFRESH_BUTTON_RIGHT, STACKED_DIVIDER_LEFT, STACKED_DIVIDER_RIGHT,
    STACKED_DIVIDER_Y, STACKED_METRICS_CARD, flyout_dimensions, quota_card_rect,
    quota_panel_geometry, refresh_button_center, scale,
};
use super::theme::{
    Theme, accent_for, inner_track_color, interactive_text_color, outer_track_color,
    panel_title_color, quota_bar_color, quota_card_colors, refresh_icon_color, theoretical_color,
};
use crate::history::UsageHistoryView;
use crate::model::{DisplayState, ProviderKind, QuotaAvailability, QuotaKind, QuotaWindow};
use crate::winutil::wide0;
use chrono::{Datelike, Local};
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BS_SOLID, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_BOTTOM, DT_CENTER, DT_END_ELLIPSIS,
    DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW,
    ExtCreatePen, FF_SWISS, FONT_QUALITY, FW_NORMAL, FW_SEMIBOLD, FillRect, FillRgn,
    GetTextExtentPoint32W, HDC, HGDIOBJ, LOGBRUSH, OUT_DEFAULT_PRECIS, PS_ENDCAP_ROUND,
    PS_GEOMETRIC, PS_JOIN_ROUND, Polyline, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::core::PCWSTR;

#[derive(Debug, Clone, Copy)]
struct QuotaPanelState {
    kind: QuotaKind,
    slot: QuotaPanelSlot,
    selected: bool,
    pressed: bool,
}

#[derive(Clone, Copy)]
struct TextStyle {
    height: i32,
    weight: i32,
    color: COLORREF,
}

pub(super) unsafe fn draw_card(
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
        let title = header_title(view.provider);
        draw_text_bottom(
            hdc,
            locale,
            title,
            RECT {
                left: scale(29, dpi),
                top: scale(HEADER_TEXT_TOP, dpi),
                right: scale(205, dpi),
                bottom: scale(HEADER_TEXT_BOTTOM, dpi),
            },
            scale(14, dpi),
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        let title_right = scale(29, dpi)
            + measure_text_width(hdc, locale, title, scale(14, dpi), FW_SEMIBOLD.0 as i32);
        draw_text_bottom(
            hdc,
            locale,
            &version_text(),
            RECT {
                left: title_right,
                top: scale(HEADER_VERSION_TOP, dpi),
                right: scale(240, dpi),
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
        let (refresh_x, refresh_y) = refresh_button_center(dimensions.width);
        draw_refresh_icon(
            hdc,
            scale(refresh_x, dpi),
            scale(refresh_y, dpi),
            interaction.refresh_rotation_degrees,
            refresh_icon_color(theme),
            dpi,
        );

        let account = account_metrics(state, locale, navigation.resource_page);
        let claude = claude_summary_rows(state, locale);
        let weekly = weekly_usage_metrics(history, navigation.summary_week, locale);
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
                    &weekly,
                    &account,
                    view.provider,
                    &claude,
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

                let metrics = BOTTOM_METRICS_CARD.to_gdi(dpi);
                outlined_surface(hdc, metrics, 10, theme.surface_alt, theme.line, dpi);
                let divider_width = scale(1, dpi).max(1);
                let divider = scale(BOTTOM_DIVIDER_X, dpi);
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
                    &weekly.tokens,
                    &weekly.range,
                    navigation.summary_week,
                    &account.credits,
                    account.credit_detail.as_deref(),
                    account.page,
                    view.provider,
                    &claude,
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
                top: scale(HEADER_TEXT_TOP, dpi),
                right: scale(82, dpi),
                bottom: scale(HEADER_TEXT_BOTTOM, dpi),
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
                let message = if history.is_some_and(|value| !value.days.is_empty()) {
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
            HistoryPage::Week => {
                let start = navigation.selected_week;
                let end = start + chrono::Duration::days(6);
                let title = match locale {
                    Locale::Chinese => format!(
                        "{}月{}日 - {}月{}日",
                        start.month(),
                        start.day(),
                        end.month(),
                        end.day()
                    ),
                    Locale::English => {
                        format!("{} - {}", start.format("%b %d"), end.format("%b %d"))
                    }
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
        let center_y = scale(QUOTA_ARC_CENTER_Y, dpi);
        let window = state.quota_window(panel.kind);
        let actual = window.map(QuotaWindow::display_percent);
        let theoretical = window
            .and_then(|window| theoretical_remaining_percent(window, Local::now().timestamp()));
        let title_color = panel_title_color(theme, panel.selected);

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

#[allow(clippy::too_many_arguments)]
unsafe fn draw_stacked_metrics(
    hdc: HDC,
    locale: Locale,
    weekly: &WeeklyUsageMetrics,
    account: &AccountMetrics,
    provider: ProviderKind,
    claude: &ClaudeSummaryRows,
    theme: Theme,
    dpi: u32,
    hovered_history_values: bool,
) {
    unsafe {
        let metrics = STACKED_METRICS_CARD.to_gdi(dpi);
        outlined_surface(hdc, metrics, 10, theme.surface, theme.line, dpi);
        let divider_top = scale(STACKED_DIVIDER_Y, dpi);
        fill(
            hdc,
            RECT {
                left: scale(STACKED_DIVIDER_LEFT, dpi),
                top: divider_top,
                right: scale(STACKED_DIVIDER_RIGHT, dpi),
                bottom: divider_top + scale(1, dpi).max(1),
            },
            theme.line,
        );
        if provider == ProviderKind::Gemini {
            draw_gdi_claude_slot(
                hdc,
                locale,
                COMPACT_CLAUDE_FIVE_HOUR.to_gdi(dpi),
                locale.text("Claude 5H", "Claude 5H"),
                &claude.five_hour,
                theme,
                dpi,
            );
            draw_gdi_claude_slot(
                hdc,
                locale,
                COMPACT_CLAUDE_WEEKLY.to_gdi(dpi),
                locale.text("Claude Wk", "Claude 周"),
                &claude.weekly,
                theme,
                dpi,
            );
        } else {
            draw_gdi_paged_heading(
                hdc,
                locale,
                match weekly.selection {
                    UsageSummaryWeek::Previous => locale.text("Last week usage", "上周消耗"),
                    UsageSummaryWeek::Current => locale.text("This week usage", "本周消耗"),
                },
                COMPACT_USAGE_HEADING.to_gdi(dpi),
                theme,
                dpi,
                weekly.selection == UsageSummaryWeek::Previous,
            );
            draw_text_center(
                hdc,
                locale,
                &weekly.tokens,
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
                &weekly.range,
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
            draw_gdi_paged_heading(
                hdc,
                locale,
                resource_label(account.page, locale),
                COMPACT_RESOURCE_HEADING.to_gdi(dpi),
                theme,
                dpi,
                account.page == ResourcePage::Resets,
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
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_bottom_metrics(
    hdc: HDC,
    locale: Locale,
    metrics: RECT,
    divider: i32,
    divider_width: i32,
    weekly_tokens: &str,
    weekly_range: &str,
    selection: UsageSummaryWeek,
    credits: &str,
    credit_detail: Option<&str>,
    page: ResourcePage,
    provider: ProviderKind,
    claude: &ClaudeSummaryRows,
    theme: Theme,
    dpi: u32,
    hovered_history_values: bool,
) {
    unsafe {
        let left = metrics.left + scale(14, dpi);
        let right_left = divider + divider_width + scale(14, dpi);
        if provider == ProviderKind::Gemini {
            draw_gdi_claude_slot(
                hdc,
                locale,
                FULL_CLAUDE_FIVE_HOUR.to_gdi(dpi),
                locale.text("Claude 5H", "Claude 5H"),
                &claude.five_hour,
                theme,
                dpi,
            );
            draw_gdi_claude_slot(
                hdc,
                locale,
                FULL_CLAUDE_WEEKLY.to_gdi(dpi),
                locale.text("Claude Wk", "Claude 周"),
                &claude.weekly,
                theme,
                dpi,
            );
            return;
        }
        draw_gdi_paged_heading(
            hdc,
            locale,
            match selection {
                UsageSummaryWeek::Previous => locale.text("Last week usage", "上周消耗"),
                UsageSummaryWeek::Current => locale.text("This week usage", "本周消耗"),
            },
            FULL_USAGE_HEADING.to_gdi(dpi),
            theme,
            dpi,
            selection == UsageSummaryWeek::Previous,
        );
        let token_width =
            measure_text_width(hdc, locale, weekly_tokens, scale(20, dpi), FW_SEMIBOLD.0 as i32);
        let token_right = (left + token_width).min(divider - scale(34, dpi));
        draw_text_bottom(
            hdc,
            locale,
            weekly_tokens,
            RECT {
                left,
                top: metrics.top + scale(20, dpi),
                right: token_right,
                bottom: metrics.bottom - scale(8, dpi),
            },
            scale(20, dpi),
            FW_SEMIBOLD.0 as i32,
            interactive_text_color(theme, hovered_history_values, false),
        );
        draw_text_right_bottom(
            hdc,
            locale,
            weekly_range,
            RECT {
                left: token_right + scale(4, dpi),
                top: metrics.top + scale(24, dpi),
                right: divider - scale(10, dpi),
                bottom: metrics.bottom - scale(7, dpi),
            },
            scale(11, dpi),
            FW_NORMAL.0 as i32,
            interactive_text_color(theme, hovered_history_values, true),
        );
        draw_gdi_paged_heading(
            hdc,
            locale,
            resource_label(page, locale),
            FULL_RESOURCE_HEADING.to_gdi(dpi),
            theme,
            dpi,
            page == ResourcePage::Resets,
        );
        let mut value_size = scale(20, dpi);
        let mut value_width =
            measure_text_width(hdc, locale, credits, value_size, FW_SEMIBOLD.0 as i32);
        let detail_width = credit_detail.map_or(0, |detail| {
            measure_text_width(hdc, locale, detail, scale(11, dpi), FW_NORMAL.0 as i32)
        });
        if value_width + detail_width + scale(4, dpi) > metrics.right - scale(10, dpi) - right_left
        {
            value_size = scale(16, dpi);
            value_width =
                measure_text_width(hdc, locale, credits, value_size, FW_SEMIBOLD.0 as i32);
        }
        let value_right = (right_left + value_width).min(metrics.right - scale(34, dpi));
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
            value_size,
            FW_SEMIBOLD.0 as i32,
            theme.text,
        );
        if let Some(detail) = credit_detail {
            draw_text_right_bottom(
                hdc,
                locale,
                detail,
                RECT {
                    left: value_right + scale(4, dpi),
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

unsafe fn draw_gdi_claude_slot(
    hdc: HDC,
    locale: Locale,
    bounds: RECT,
    label: &str,
    summary: &ClaudeWindowSummary,
    theme: Theme,
    dpi: u32,
) {
    unsafe {
        let pct_text = summary.percent.map_or_else(|| "--".to_owned(), |v| format!("{v}%"));
        let bar_color = quota_bar_color(summary.percent, theme.high_contrast);
        let pct_color = if summary.percent.is_some() { bar_color } else { theme.muted };
        let pct_width = scale(34, dpi);
        let header_bottom = bounds.top + scale(18, dpi);
        draw_text_with_alignment(
            hdc,
            locale,
            label,
            RECT {
                left: bounds.left,
                top: bounds.top,
                right: bounds.right - pct_width,
                bottom: header_bottom,
            },
            TextStyle { height: scale(11, dpi), weight: FW_NORMAL.0 as i32, color: theme.muted },
            DT_LEFT,
        );
        draw_text_with_alignment(
            hdc,
            locale,
            &pct_text,
            RECT {
                left: bounds.right - pct_width,
                top: bounds.top,
                right: bounds.right,
                bottom: header_bottom,
            },
            TextStyle { height: scale(11, dpi), weight: FW_SEMIBOLD.0 as i32, color: pct_color },
            DT_RIGHT,
        );
        let bar_h = scale(4, dpi).max(2);
        let bar_top = bounds.top + scale(22, dpi);
        let bar_rect =
            RECT { left: bounds.left, top: bar_top, right: bounds.right, bottom: bar_top + bar_h };
        fill_rounded(hdc, bar_rect, bar_h, outer_track_color(theme, false));
        if let Some(value) = summary.percent
            && value > 0
        {
            let fill_w = ((bar_rect.right - bar_rect.left) * i32::from(value)) / 100;
            if fill_w > 0 {
                fill_rounded(
                    hdc,
                    RECT {
                        right: (bar_rect.left + fill_w.max(bar_h)).min(bar_rect.right),
                        ..bar_rect
                    },
                    bar_h,
                    bar_color,
                );
            }
        }
        draw_text_with_alignment(
            hdc,
            locale,
            &summary.reset_hint,
            RECT {
                left: bounds.left,
                top: bounds.top + scale(29, dpi),
                right: bounds.right,
                bottom: bounds.bottom,
            },
            TextStyle { height: scale(10, dpi), weight: FW_NORMAL.0 as i32, color: theme.muted },
            DT_LEFT,
        );
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_gdi_paged_heading(
    hdc: HDC,
    locale: Locale,
    label: &str,
    area: RECT,
    theme: Theme,
    dpi: u32,
    points_right: bool,
) {
    unsafe {
        let size = scale(11, dpi);
        let width = measure_text_width(hdc, locale, label, size, FW_NORMAL.0 as i32);
        draw_text_center(hdc, locale, label, area, size, FW_NORMAL.0 as i32, theme.muted);
        let center = (area.left + area.right) / 2;
        let direction = if points_right { 1 } else { -1 };
        let arrow = (center + direction * (width / 2 + scale(11, dpi)))
            .clamp(area.left + scale(6, dpi), area.right - scale(6, dpi));
        draw_text_center(
            hdc,
            locale,
            if points_right { "▶" } else { "◀" },
            RECT { left: arrow - scale(6, dpi), right: arrow + scale(6, dpi), ..area },
            scale(9, dpi),
            FW_NORMAL.0 as i32,
            theme.muted,
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_geometry_keeps_the_requested_opening_and_progress() {
        let full = arc_points(100, 149, 66, 100);
        let half = arc_points(100, 149, 66, 50);
        assert_eq!(full.first(), Some(&POINT { x: 46, y: 187 }));
        assert_eq!(full.last(), Some(&POINT { x: 154, y: 187 }));
        assert_eq!(half.first(), full.first());
        assert!(half.last().is_some_and(|point| point.y < 100));
    }
}
