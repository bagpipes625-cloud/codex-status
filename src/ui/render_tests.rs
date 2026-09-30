//! Explicit offline native render validation; never queries an account.
use super::*;
use crate::history::{DailyUsageRecord, UsageHistory};
use crate::model::{CreditBalance, QuotaSnapshot};
use windows::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

#[test]
#[ignore = "opens scoped native preview windows; run explicitly on an interactive desktop"]
fn render_resource_matrix() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let now = Local::now().timestamp();
        let output =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/resource-render");
        std::fs::create_dir_all(&output).unwrap();
        let history = UsageHistory {
            days: vec![DailyUsageRecord {
                date: Local::now().format("%Y-%m-%d").to_string(),
                tokens: 78_980_000,
            }],
            last_observed_at: now,
        }
        .view(now);
        let mut cases = 0;
        for dpi in [96, 120, 144, 192] {
            for (layout, weekly, session) in
                [("dual", true, true), ("weekly", true, false), ("session", false, true)]
            {
                for theme_name in ["light", "dark", "contrast"] {
                    for (balance_name, balance) in [
                        ("normal", Some(CreditBalance::Amount("1200.25".into()))),
                        ("zero", Some(CreditBalance::Amount("0".into()))),
                        ("missing", None),
                        ("unlimited", Some(CreditBalance::Unlimited)),
                        (
                            "large",
                            Some(CreditBalance::Amount("123456789012345678901234567890".into())),
                        ),
                    ] {
                        let quota = |minutes, percent| QuotaWindow {
                            used_percent: 100.0 - percent,
                            remaining_percent: percent,
                            window_minutes: minutes,
                            resets_at: Some(now + 3600),
                        };
                        let state = DisplayState::live(QuotaSnapshot {
                            weekly: weekly.then(|| quota(10080, 66.0)),
                            session: session.then(|| quota(300, 85.0)),
                            fetched_at: now,
                            account: AccountSummary {
                                credit_balance: balance,
                                reset_credits: Some(2),
                                reset_credit_expires_at: Some(now + 86400),
                                ..Default::default()
                            },
                        });
                        let dims = flyout_dimensions(&state);
                        let width = scale(dims.width, dpi);
                        let height = scale(dims.height, dpi);
                        let hwnd = CreateWindowExW(
                            WS_EX_TOOLWINDOW,
                            w!("STATIC"),
                            w!("CodexStatus offline resource render"),
                            WS_POPUP,
                            80,
                            80,
                            width,
                            height,
                            None,
                            None,
                            Some(HINSTANCE(GetModuleHandleW(None).unwrap().0)),
                            None,
                        )
                        .unwrap();
                        let mut theme =
                            detect_theme(if theme_name == "dark" { "dark" } else { "light" });
                        if theme_name == "contrast" {
                            theme.high_contrast = true;
                            theme.background = rgb(0, 0, 0);
                            theme.surface = rgb(0, 0, 0);
                            theme.surface_alt = rgb(0, 0, 0);
                            theme.text = rgb(255, 255, 255);
                            theme.muted = rgb(255, 255, 255);
                            theme.line = rgb(255, 255, 255);
                        }
                        configure_flyout(hwnd, theme);
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        for page in [ResourcePage::Resets, ResourcePage::Points] {
                            let navigation =
                                HistoryNavigation { resource_page: page, ..Default::default() };
                            let interaction = CardInteraction {
                                pressed_quota: None,
                                refresh_feedback: false,
                                refresh_rotation_degrees: 0.0,
                                hovered_day: None,
                                hovered_history_values: false,
                            };
                            assert!(
                                direct2d::paint(direct2d::PaintInput {
                                    hwnd,
                                    pixel_size: (width, height),
                                    dpi,
                                    state: &state,
                                    preferred: QuotaKind::FiveHour,
                                    locale: Locale::Chinese,
                                    theme,
                                    history: Some(&history),
                                    reset: None,
                                    navigation: &navigation,
                                    interaction
                                }),
                                "Direct2D failed: {layout}/{theme_name}/{dpi}"
                            );
                            let _ = windows::Win32::Graphics::Dwm::DwmFlush();
                            if balance_name == "normal"
                                && [96, 192].contains(&dpi)
                                && theme_name != "contrast"
                            {
                                save_bmp(
                                    hwnd,
                                    width,
                                    height,
                                    &output.join(format!(
                                        "{layout}-{page:?}-{theme_name}-{dpi}-d2d.bmp"
                                    )),
                                );
                            }
                            let dc = GetDC(Some(hwnd));
                            draw_card(
                                dc,
                                &state,
                                QuotaKind::FiveHour,
                                Locale::Chinese,
                                theme,
                                CardView {
                                    reset: None,
                                    history: Some(&history),
                                    navigation: &navigation,
                                    interaction,
                                },
                                dpi,
                            );
                            let _ = GdiFlush();
                            if balance_name == "normal" && dpi == 96 && theme_name == "light" {
                                save_bmp(
                                    hwnd,
                                    width,
                                    height,
                                    &output.join(format!("{layout}-{page:?}-{dpi}-gdi.bmp")),
                                );
                            }
                            let _ = ReleaseDC(Some(hwnd), dc);
                            cases += 2;
                        }
                        release_card_renderer();
                        DestroyWindow(hwnd).unwrap();
                    }
                }
            }
        }
        println!(
            "PASS: {cases} native Direct2D/GDI renders; dual/single quota, four DPIs, three themes, zero/missing/unlimited/large/decimal balances. {}",
            output.display()
        );
    }
}

unsafe fn save_bmp(hwnd: HWND, width: i32, height: i32, path: &std::path::Path) {
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
