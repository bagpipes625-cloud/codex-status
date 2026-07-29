#![cfg(windows)]
#![cfg_attr(not(feature = "diagnostics"), windows_subsystem = "windows")]

use codex_status::app;

fn main() {
    #[cfg(feature = "diagnostics")]
    eprintln!("CodexStatus diagnostic build started");
    if let Err(error) = app::run() {
        #[cfg(feature = "diagnostics")]
        eprintln!("{error}");
        #[cfg(not(feature = "diagnostics"))]
        codex_status::ui::show_fatal_error(&error.to_string());
    }
}
