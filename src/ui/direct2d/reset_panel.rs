use super::*;
use crate::reset::{ResetHit, ResetPanel};

pub(super) fn draw(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    f: &FormatSet,
    b: &Brushes,
    input: &PaintInput<'_>,
    panel: &ResetPanel,
) -> Result<()> {
    let dimensions = flyout_dimensions(input.state);
    let (w, h) = (dimensions.width as f32, dimensions.height as f32);
    let locale = input.locale;
    unsafe {
        target.FillRectangle(&rect(0.0, 0.0, w, h), &b.background);
        target.DrawRoundedRectangle(
            &rounded_rect(0.5, 0.5, w - 0.5, h - 0.5, FLYOUT_CORNER_RADIUS as f32),
            &b.line,
            1.0,
            None::<&ID2D1StrokeStyle>,
        );
    }
    draw_solid_triangle(target, factory, 24.0, 23.0, false, 12.0, &b.muted)?;
    draw_text(
        target,
        locale.text("Back", "返回"),
        rect(35.0, 8.0, 115.0, 38.0),
        &f.header,
        &b.text,
    );
    draw_text(
        target,
        &updated_text(input.state, locale, input.interaction.refresh_feedback),
        rect(w - 150.0, 8.0, w - 18.0, 38.0),
        &f.update,
        &b.muted,
    );
    let card = rounded_rect(16.5, 43.5, w - 16.5, h - 16.5, 10.0);
    if !input.theme.high_contrast {
        draw_card_shadow(target, &card, b);
    }
    unsafe {
        target.FillRoundedRectangle(&card, &b.metrics_surface);
        target.DrawRoundedRectangle(&card, &b.line, 1.0, None::<&ID2D1StrokeStyle>);
    }
    draw_text(
        target,
        locale.text("Reset credits", "重置机会"),
        rect(29.0, 47.0, 170.0, 73.0),
        &f.header,
        &b.text,
    );
    let account = input.state.snapshot.as_ref().map(|s| &s.account);
    let credits = account.and_then(|a| a.reset_credit_details.as_deref());
    let count = account.and_then(|a| a.reset_credits);
    let summary = if panel.pending.is_some() {
        locale.text("Check last result", "查询上次结果").into()
    } else {
        count
            .map(|n| {
                let listed = credits.map_or(0, <[crate::model::ResetCredit]>::len);
                if listed as u64 != n {
                    format!("{listed}/{n} {}", locale.text("listed", "张明细"))
                } else {
                    format!("{} {n}", locale.text("Available", "可用"))
                }
            })
            .unwrap_or_else(|| "--".into())
    };
    draw_text(
        target,
        &summary,
        rect(w - 150.0, 47.0, w - 29.0, 73.0),
        &f.update,
        if panel.pending.is_some() { &b.interactive } else { &b.muted },
    );
    let rows = credits.unwrap_or_default();
    unsafe {
        target.PushAxisAlignedClip(
            &rect(24.0, 76.0, w - 25.0, h - 17.0),
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
    }
    let scroll = panel.scroll.min(ResetPanel::max_scroll(rows.len(), h));
    for (index, credit) in rows.iter().enumerate() {
        let y = 76.0 + index as f32 * 60.0 - scroll;
        if y + 60.0 < 76.0 || y > h - 16.0 {
            continue;
        }
        let date = credit
            .expires_at
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| {
                t.with_timezone(&Local)
                    .format(locale.text("%b %-d %H:%M", "%-m月%-d日 %H:%M"))
                    .to_string()
            })
            .unwrap_or_else(|| locale.text("Expiry unavailable", "到期时间未提供").into());
        draw_text(target, &date, rect(30.0, y + 1.0, w - 120.0, y + 31.0), &f.header, &b.text);
        draw_text(
            target,
            locale.text("Expires", "到期"),
            rect(30.0, y + 28.0, w - 120.0, y + 49.0),
            &f.metric_label,
            &b.muted,
        );
        let enabled =
            credit.id.is_some() && credit.expires_at.is_none_or(|t| t > Local::now().timestamp());
        let button = rounded_rect(w - 111.0, y + 9.0, w - 32.0, y + 43.0, 6.0);
        unsafe {
            if panel.hovered == Some(ResetHit::Use(index)) && enabled {
                target.FillRoundedRectangle(&button, &b.line);
            }
            target.DrawRoundedRectangle(&button, &b.line, 1.0, None::<&ID2D1StrokeStyle>);
            target.DrawLine(
                Vector2 { X: 30.0, Y: y + 59.0 },
                Vector2 { X: w - 32.0, Y: y + 59.0 },
                &b.line,
                1.0,
                None::<&ID2D1StrokeStyle>,
            );
        }
        draw_text(
            target,
            if enabled {
                locale.text("Use reset", "使用重置")
            } else {
                locale.text("Unavailable", "不可用")
            },
            button.rect,
            &f.date,
            if enabled { &b.interactive } else { &b.muted },
        );
    }
    if rows.is_empty() {
        let message = if credits.is_none() || count.is_some_and(|n| n > 0) {
            locale.text("Credit details unavailable; refresh later", "暂未获取到券明细，请稍后刷新")
        } else {
            locale.text("No available reset credits", "暂无可用重置券")
        };
        draw_text(target, message, rect(28.0, 76.0, w - 28.0, h - 17.0), &f.date, &b.muted);
    }
    unsafe {
        target.PopAxisAlignedClip();
    }
    let max = ResetPanel::max_scroll(rows.len(), h);
    if max > 0.0 {
        let track = h - 100.0;
        let thumb = (track * (h - 92.0) / (rows.len() as f32 * 60.0)).max(24.0);
        let top = 80.0 + (track - thumb) * scroll / max;
        unsafe {
            target.FillRoundedRectangle(
                &rounded_rect(w - 23.0, top, w - 20.0, top + thumb, 1.5),
                &b.muted,
            );
        }
    }
    if let Some(credit) = &panel.confirmation {
        let overlay = alpha_brush(target, if input.theme.dark { 0.58 } else { 0.25 })?;
        let top = (h - 166.0) / 2.0;
        unsafe {
            target.FillRectangle(&rect(0.0, 0.0, w, h), &overlay);
            target.FillRoundedRectangle(
                &rounded_rect(24.0, top, w - 24.0, top + 166.0, 10.0),
                &b.metrics_surface,
            );
            target.DrawRoundedRectangle(
                &rounded_rect(24.0, top, w - 24.0, top + 166.0, 10.0),
                &b.line,
                1.0,
                None::<&ID2D1StrokeStyle>,
            );
        }
        draw_text(
            target,
            locale.text("Use this reset credit?", "确认使用这张重置券？"),
            rect(34.0, top + 13.0, w - 34.0, top + 43.0),
            &f.title,
            &b.text,
        );
        let date = credit
            .expires_at
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| {
                t.with_timezone(&Local)
                    .format(locale.text("%b %-d %H:%M", "%-m月%-d日 %H:%M"))
                    .to_string()
            })
            .unwrap_or_else(|| "--".into());
        draw_text(
            target,
            &format!("{} {date}", locale.text("Expires:", "到期：")),
            rect(34.0, top + 45.0, w - 34.0, top + 71.0),
            &f.date,
            &b.muted,
        );
        let message = panel.message.as_deref().unwrap_or_else(|| {
            locale.text("Consumes 1 credit. Cannot be undone.", "将消耗 1 张重置券，无法撤销。")
        });
        draw_text(
            target,
            message,
            rect(30.0, top + 74.0, w - 30.0, top + 104.0),
            &f.date,
            &b.muted,
        );
        for (left, right, label) in [
            (40.0, w / 2.0 - 5.0, locale.text("Cancel", "取消")),
            (
                w / 2.0 + 5.0,
                w - 40.0,
                if panel.busy {
                    locale.text("Processing…", "处理中…")
                } else {
                    locale.text("Confirm", "确认使用")
                },
            ),
        ] {
            let button = rounded_rect(left, top + 119.0, right, top + 151.0, 6.0);
            unsafe {
                target.DrawRoundedRectangle(&button, &b.line, 1.0, None::<&ID2D1StrokeStyle>);
            }
            draw_text(target, label, button.rect, &f.date, &b.text);
        }
    }
    Ok(())
}
