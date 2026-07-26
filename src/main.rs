#![cfg(windows)]
#![cfg_attr(not(feature = "diagnostics"), windows_subsystem = "windows")]

mod app;
mod app_server;
mod icon;
mod model;
mod settings;
mod startup;
mod ui;
mod updater;

fn main() {
    #[cfg(feature = "diagnostics")]
    eprintln!("CodexStatus diagnostic build started");
    if let Err(error) = app::run() {
        ui::show_fatal_error(&error.to_string());
    }
}
