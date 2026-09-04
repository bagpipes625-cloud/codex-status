//! Explicit, network-free Direct2D visual regression fixture.
//! Run with `cargo test --test reset_render -- --ignored --nocapture`.
use codex_status::{model::*, reset::*, ui};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};
use windows::core::w;

#[test]
#[ignore = "opens a scoped preview window; run explicitly on an interactive desktop"]
fn render_reset_credit_states() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let now = chrono::Utc::now().timestamp();
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist/reset-render");
        std::fs::create_dir_all(&output).unwrap();
        for (name, compact, dark, count, confirm) in [
            ("dual-list", false, false, 2, false),
            ("dual-confirm", false, false, 2, true),
            ("compact-list", true, false, 2, false),
            ("compact-confirm", true, false, 2, true),
            ("dark-list", false, true, 8, false),
            ("dark-confirm", true, true, 2, true),
            ("empty", true, false, 0, false),
        ] {
            let quota = |minutes| QuotaWindow {
                used_percent: 35.0,
                remaining_percent: 65.0,
                window_minutes: minutes,
                resets_at: Some(now + 3600),
            };
            let credits: Vec<_> = (0..count)
                .map(|i| ResetCredit {
                    id: Some(format!("fixture-{i}")),
                    expires_at: Some(now + (i + 1) * 86400),
                })
                .collect();
            let panel = ResetPanel {
                open: true,
                confirmation: confirm.then(|| credits[0].clone()),
                ..Default::default()
            };
            let state = DisplayState::live(QuotaSnapshot {
                weekly: Some(quota(10080)),
                session: (!compact).then(|| quota(300)),
                account: AccountSummary {
                    reset_credits: Some(count as u64),
                    reset_credit_details: Some(credits),
                    ..Default::default()
                },
                fetched_at: now,
            });
            let size = ui::flyout_dimensions(&state);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("CodexStatus render fixture"),
                WS_POPUP,
                100,
                100,
                size.width,
                size.height,
                None,
                None,
                Some(HINSTANCE(GetModuleHandleW(None).unwrap().0)),
                None,
            )
            .unwrap();
            let theme = ui::detect_theme(if dark { "dark" } else { "light" });
            ui::configure_flyout(hwnd, theme);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
            let painted = ui::paint_card(
                hwnd,
                &state,
                QuotaKind::FiveHour,
                ui::Locale::Chinese,
                theme,
                ui::CardView {
                    reset: Some(&panel),
                    history: None,
                    navigation: &ui::HistoryNavigation::default(),
                    interaction: ui::CardInteraction {
                        pressed_quota: None,
                        refresh_feedback: false,
                        refresh_rotation_degrees: 0.0,
                        hovered_day: None,
                        hovered_history_values: false,
                    },
                },
            );
            assert!(painted, "Direct2D fallback in {name}");
            let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            save_bmp(hwnd, size.width, size.height, &output.join(format!("{name}.bmp")));
            ui::release_card_renderer();
            DestroyWindow(hwnd).unwrap();
        }
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
