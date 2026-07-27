<div align="center">

# CodexStatus

**Your Codex weekly quota, readable at a glance in the Windows tray.**

[简体中文](README.zh-CN.md) · [Download](https://github.com/bagpipes625-cloud/codex-status/releases/latest) · [Report an issue](https://github.com/bagpipes625-cloud/codex-status/issues)

</div>

| Light | Dark |
|:--:|:--:|
| ![CodexStatus light quota flyout](assets/screenshots/codexstatus-light.png) | ![CodexStatus dark quota flyout](assets/screenshots/codexstatus-dark.png) |

CodexStatus is a tiny native Windows utility. Its notification-area icon is the number itself—`0` to `100`, or `--` when no trustworthy weekly value is available. Click it for reset timing, the optional five-hour window, plan information, and refresh status.

## Highlights

- Weekly remaining quota drawn directly into the standard tray icon.
- Transparent, theme-aware Segoe UI digits with a restrained green (≥50%), amber (20–49%), red (<20%), or muted status rule.
- Native rounded flyout that follows light, dark, high-contrast, and per-monitor DPI settings.
- System, light, and dark flyout themes selectable from the tray menu.
- Weekly depletion forecast based on the current cycle's elapsed time and used quota.
- Official Codex app-server RPC: `account/rateLimits/read`; no token scraping and no private endpoints.
- Event-driven Win32 process with no Electron, WebView, WPF, WinUI, local HTTP server, or resident async runtime.
- Five-minute default refresh, manual refresh, bounded failure backoff, safe cache expiry, and optional low-quota alerts.
- Single instance, Explorer-restart recovery, multi-monitor placement, and optional start with Windows.
- English and Simplified Chinese UI, selected from Windows automatically.

## Install

CodexStatus requires Windows 10/11 x64 and an already installed, signed-in [Codex CLI or Codex app](https://developers.openai.com/codex/cli/).

1. Download the per-user installer from [Releases](https://github.com/bagpipes625-cloud/codex-status/releases/latest).
2. Run it and confirm the installation directory. New installations default to `F:\CodexStatus` when the `F:` drive exists, or `%LOCALAPPDATA%\Programs\CodexStatus` otherwise. Existing installations keep their previous directory during upgrades.
3. The installer always registers start-with-Windows.
4. If Windows places the new icon behind the overflow arrow, open that area and drag CodexStatus onto the visible tray. Windows—not applications—controls notification icon visibility.

The installer is not yet code-signed, so Microsoft Defender SmartScreen may show an “unrecognized app” warning. Release assets include SHA-256 checksums. The portable ZIP makes no startup changes; enable startup from the right-click menu if desired.

## Use

- **Left-click:** open or close the quota card.
- **Right-click:** refresh now, open the Codex usage page, choose a 1/5/15-minute interval, configure a low-quota alert, select a theme, toggle startup, or exit.
- **Tray label:** weekly remaining percentage rounded to the nearest whole number.

CodexStatus only calls the locally installed `codex app-server`. Each refresh performs `initialize → account/read → account/rateLimits/read`, then closes the process tree using a Windows Job Object. It selects an exact 10,080-minute window first and only accepts a 6–8 day fallback; a short window is never mislabeled as weekly quota.

The footer extrapolates the average consumption rate observed since the weekly cycle began. It shows a green **Usage ample** status when the projected depletion is at or after reset, or a red estimated time to depletion when the quota is likely to run out first.

## Privacy

CodexStatus never reads or stores your OAuth token, email address, project content, prompts, or raw app-server response. It sends no telemetry or background network requests.

Two normal state files are stored under `%LOCALAPPDATA%\CodexStatus`:

- `settings.json`: refresh interval, UI language, theme, alert threshold, onboarding state, and alert deduplication state.
- `snapshot.json`: the latest non-sensitive parsed quota snapshot. It is discarded once its reset time passes.

Normal builds do not write activity logs. If Windows rejects a notification icon operation, CodexStatus additionally writes one failure-only `tray-errors.log` containing the operation, release channel, executable path, tray identity, PID, and Win32 error code. The optional `diagnostics` Cargo feature records lifecycle stages and filtered error summaries.

## Performance

Measured on Windows 11 24H2 x64 with the v0.3.0 release:

| State | CodexStatus working set | CPU | Child processes |
|---|---:|---:|---:|
| Idle after refresh | ~12 MB | ≤0.1% average | 0 |
| Refreshing | <15 MB for the tray process | brief | 1 temporary `codex app-server` tree |

The app-server process has a larger transient footprint because it is Codex itself; it exits immediately after the two account calls complete and is not part of the resident tray process.

## Build

The supported release target is stable Rust with `x86_64-pc-windows-msvc`:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
$env:CODEX_STATUS_CHANNEL = "stable"
cargo build --release --locked
Remove-Item Env:CODEX_STATUS_CHANNEL
```

Development builds default to an isolated development tray GUID. Set `CODEX_STATUS_CHANNEL` to `beta` or `stable` only when packaging that installed channel; each channel also has its own window classes and single-instance mutex. Do not run a stable-channel binary from the build or staging directory—install it first so its persistent GUID is initially registered from the fixed executable path. Portable packages use `CODEX_STATUS_CHANNEL=portable` and the supported `HWND + uID` identity because their executable path is intentionally not fixed.

For version tags, GitHub Actions builds a path-independent portable-channel ZIP and a stable-channel Inno Setup installer. Local development can also use the gnullvm target; llvm-mingw's `libunwind.dll` is then a development-only runtime dependency. Official release builds use MSVC and are a single executable.

## Design boundaries

CodexStatus intentionally does not inject private taskbar UI, collect cost/token history, support other providers, or expose a local server. Windows does not offer a supported API for forcing a tray icon to remain visible, so pinning is always the user's choice.

## Thanks

CodexStatus was informed by the interaction and information design of [CodexBar](https://github.com/steipete/CodexBar), [TaskbarQuota](https://github.com/zioder/TaskbarQuota), [CodexQuotaTaskbar](https://github.com/zHysie/CodexQuotaTaskbar), [codex-win-widget](https://github.com/Mauriciog87/codex-win-widget), and [Claude & Codex Battery](https://github.com/dennykim123/claude-codex-battery). Its compact flyout also takes cues from [Windows app design guidance](https://learn.microsoft.com/windows/apps/design/), [Twinkle Tray](https://github.com/xanderfrangos/twinkle-tray), and [EarTrumpet](https://github.com/File-New-Project/EarTrumpet). No source code was copied from those projects.

The quota transport follows the official [Codex app-server rate-limit documentation](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt). Notification-area behavior follows [Microsoft's guidance](https://learn.microsoft.com/windows/win32/uxguide/winenv-notification).

## License

[MIT](LICENSE)
