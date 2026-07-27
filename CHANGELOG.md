# Changelog

All notable changes to CodexStatus are documented here.

## [Unreleased]

## [0.3.6] - 2026-07-27

- Display the nearest available reset credit's expiration time without adding
  another refresh or network request.
- Align the lower metric-card divider with the hero divider, split the two
  left columns evenly, and align all three metric values on one row.

## [0.3.5] - 2026-07-27

- Keep the tray icon's two-pixel bottom bar at full width and use its
  green, yellow, or red color alone to communicate the remaining-quota state.

## [0.3.4] - 2026-07-27

- Align the percent sign with the bottom of the weekly remaining number.
- Turn the tray icon's bottom rule into a two-pixel remaining-quota bar whose
  length and green, yellow, or red color match the main panel.

## [0.3.3] - 2026-07-27

- Display the server-reported `pro` plan as `Pro` because the available account and
  Codex quota fields do not distinguish 5x Pro from 20x Pro.

## [0.3.2] - 2026-07-27

- Format Chinese reset timestamps as `8月2日 11:00` instead of `08/02 11:00`.
- Increase the reset timestamp and quota forecast typography for easier reading.

## [0.3.1] - 2026-07-27

- Restore the installation-directory page so the destination can be selected on every
  computer.
- Default new installations to `F:\CodexStatus` when the `F:` drive exists, otherwise
  use `%LOCALAPPDATA%\Programs\CodexStatus`.
- Reuse an existing installation directory during upgrades and continue to register
  start-with-Windows for every installation.

## [0.3.0] - 2026-07-27

- Remove the automatic updater, release-check timers, network transport, and update metadata from the installed application.
- Add a weekly quota depletion forecast with green ample-use and red estimated-depletion indicators.
- Use Microsoft YaHei UI throughout the panel and distinguish Free, Go, Plus, 5x Pro, and 20x Pro plans.
- Apply green, yellow, and red remaining-quota thresholds and show an explicit exhausted state.
- Center the forecast footer and remove the local/cache diagnostic line.
- Give development, beta, and stable builds independent persistent tray GUIDs; the stable
  identity is only packaged for the fixed installation path.
- Keep portable builds path-independent with a constant `uID` under the traditional
  `HWND + uID` identity instead of reusing the installed product GUID.
- Keep tray operations on one consistent `NIF_GUID` identity, enforce one instance per
  channel, recover after Explorer restarts, and degrade without terminating if tray
  registration fails.
- Discover Codex Desktop's local app-server executable when the packaged WindowsApps
  binary cannot be launched by an unpackaged tray process.
- Fix the installation directory at `F:\CodexStatus` and always register start-with-Windows.

## [0.2.3] - 2026-07-27

- Generate the Windows manifest and executable version metadata from the Cargo package version so File Explorer reports the installed release correctly.

## [0.2.2] - 2026-07-27

- Use one Segoe UI Variable request throughout the flyout so Latin letters and quota numerals no longer change families between strings; Windows font linking supplies localized glyphs.
- Remove the nested reset panel and three separate metric cards in favor of a calmer split quota surface and one aligned metrics band.
- Refine light and dark semantic colors, dividers, spacing, status accents, and privacy copy for clearer hierarchy and stronger contrast.
- Refresh the documented light and dark screenshots to match the released interface.

## [0.2.1] - 2026-07-27

- Return the process working set to its low idle footprint shortly after the daily WinHTTP update check completes.

## [0.2.0] - 2026-07-27

- Replace the solid block tray badge with transparent, theme-aware Segoe UI quota digits and a restrained one-pixel status rule.
- Fix the tray bitmap orientation that could make a weekly value ending in `2` look like `5`.
- Follow the Windows system theme independently from the app theme so the number stays legible on light and dark taskbars.
- Recompose the flyout around a focused weekly-quota card, a quiet reset panel, three consistent metric cards, and a roomier Fluent-style spacing system.
- Add silent daily updates from verified GitHub Release executables, with SHA-256 digest validation, atomic replacement, and automatic restart.
- Add system, light, and dark flyout theme choices while preserving Windows high-contrast behavior.
- Use Microsoft YaHei UI for Simplified Chinese and Segoe UI Variable for Latin text and quota numerals, with larger supporting type.

## [0.1.1] - 2026-07-27

- Keep the flyout lightweight on systems with third-party input methods by disabling unused text services before window creation.
- Preserve the redesigned readable tray digits and reliable single-click flyout behavior.

## [0.1.0] - 2026-07-26

- Initial public release.
- Weekly quota digits in the standard Windows notification area.
- Native light, dark, high-contrast, and per-monitor-DPI flyout.
- Official Codex app-server quota transport with safe process cleanup.
- Cache expiry, refresh backoff, low-quota alerts, startup control, and single-instance behavior.
- Portable ZIP and per-user Inno Setup installer.
