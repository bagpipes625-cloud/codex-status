# Changelog

All notable changes to CodexStatus are documented here.

## [Unreleased]

## [0.5.4] - 2026-07-30

- Strengthen the right edge of the lightweight card shadow while preserving its
  existing downward depth, opacity, layout, and cached-brush implementation.

## [0.5.3] - 2026-07-30

- Add subtle cached Direct2D shadows to quota and account cards without adding
  an offscreen surface or composition device tree.
- Render healthy, warning, and critical quota arcs with restrained light-to-dark
  gradients while preserving the existing green, yellow, and red thresholds.
- Align the visible bottoms of the plan, reset-credit count, and expiration
  timestamp in the dual-quota footer.
- Keep high-contrast rendering free of decorative shadows and retain the solid
  system-readable quota colors.

## [0.5.2] - 2026-07-30

- Match the Windows 10 fallback window shape and the rendered outer border to
  the Windows 11 8-DIP corner-radius baseline.
- Standardize every inner quota and account card on a 10-DIP radius without
  changing any established layout coordinates or dimensions.

## [0.5.1] - 2026-07-30

- Align the title, version, and refresh metadata with the header accent while
  preserving a shared visible text baseline.
- Keep draining app-server stderr after retaining the bounded diagnostic prefix,
  preventing a full diagnostic pipe from disrupting quota RPC.
- Bound app-server stdout lines, discard notifications while continuously
  draining the pipe, and forward at most one valid response for each expected
  RPC identifier.
- Isolate non-stable state by release channel and use process-unique atomic
  temporary files so parallel development, beta, portable, and stable instances
  cannot race while saving settings or snapshots.
- Remove validated stale update staging directories before each user-initiated
  update check, including the legacy pre-channel directory layout, without
  blocking updates when a file is locked. Hold non-reparse directory handles
  without delete sharing throughout cleanup so junction swaps cannot redirect it.
- Abort update replacement when the helper cannot verify that the previous
  process exited; only a confirmed nonexistent PID may bypass the wait.
- Deliver refresh and update results through bounded in-process channels so
  window messages never carry owned pointers.
- Exclusively create both update staging and final replacement files, clean
  failed atomic-save temporaries, report every settings persistence failure,
  bound each HTTP request, and retain a bounded diagnostic when an update
  helper must recover or cannot restart the app.

## [0.5.0] - 2026-07-30

- Replace the single primary quota panel with selectable five-hour and weekly
  cards that remain visible side by side.
- Show actual remaining quota on each outer gauge and time-paced remaining quota
  on a neutral inner gauge, while preserving the green, amber, and red thresholds.
- Move plan and reset-credit details into an equal two-column footer and remove
  the obsolete weekly depletion forecast.
- Render the flyout with a reusable Direct2D/DirectWrite HWND target for smooth
  gauges and consistent text on Windows 10, while retaining GDI as a fallback.
- Add a Windows 10 rounded window region and release all renderer resources plus
  trim the working set shortly after the flyout is hidden.
- Raise the three content cards by eight pixels to tighten the header gap and
  balance the bottom margin against the sixteen-pixel side margins.
- Adapt to accounts that expose only one valid quota window with a compact
  336-by-284 flyout: preserve the quota card geometry and stack plan and reset
  credits in the narrower right-hand card.
- Hide quota-switch controls when only one quota is available and restore them
  automatically when both quota windows return.
- Make single-window fallback symmetric: both the tray number and its two-pixel
  status rule follow whichever quota is available, while dual-window mode keeps
  the number on the selected quota and the rule on the other quota.
- Make low-quota alerts follow the selected window or the only available window,
  and trigger strictly below the configured threshold.
- Bound app-server reader cleanup after closing the complete Windows Job tree,
  atomically replace settings and snapshots, and validate startup entries against
  the current executable.
- Isolate startup registry values by release channel, support resource builds
  from Unicode repository paths, and reject release tags that disagree with the
  package version.

## [0.4.4] - 2026-07-29

- Make the primary quota card clickable so users can switch between the five-hour
  and weekly quota without opening the tray menu.
- Add pressed-state feedback and cancel the switch when the pointer is released
  outside the card.
- Move the primary reset countdown down by one pixel to equalize its visual
  spacing between the label and timestamp.

## [0.4.3] - 2026-07-29

- Rename the on-demand update menu command to **Check for updates** for clearer behavior.
- Move both lower-card reset timestamps down by one pixel for balanced vertical spacing.
- When the five-hour quota is unavailable, use the weekly quota for both the tray
  number and its two-pixel status rule regardless of the saved display preference.

## [0.4.2] - 2026-07-29

- Display the running package version beside the CodexStatus title.
- Measure the title with GDI before placing the version label, preventing overlap
  across DPI settings.
- Bottom-align the title, version, and refresh status on one shared baseline.
- Move the primary quota number and percent sign, secondary quota number and
  percent sign, reset-credit value, and plan value down by two pixels.

## [0.4.1] - 2026-07-29

- Add persistent tray-menu selection between the five-hour and weekly quota,
  defaulting to the five-hour window and falling back to weekly when unavailable.
- Swap the flyout's primary and secondary quota sections with the selected window,
  including percentage, reset timing, progress bar, and forecast.
- Keep the tray number on the displayed quota while its two-pixel status rule tracks
  the other quota window using the existing green, amber, and red thresholds.
- Refine reset countdowns for day, hour, minute, and sub-minute intervals.
- Align lower-card values and render the percentage sign at timestamp size and
  normal weight.

## [0.4.0] - 2026-07-29

- Keep one-digit tray labels at the established two-digit font size and center
  them horizontally, without changing the existing status bar or flyout.
- Add an on-demand **Update now** tray command backed by this repository's
  GitHub Releases, with release-channel isolation, strict download boundaries,
  SHA-256 digest verification, in-place replacement, and automatic restart.
- Keep update checks entirely user initiated; no update timer or background
  GitHub request is added.

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
