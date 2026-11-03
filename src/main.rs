#![cfg_attr(windows, windows_subsystem = "windows")]
mod agent;
#[cfg_attr(target_os = "macos", path = "macos/autostart.rs")]
mod autostart;
mod config;
#[cfg(windows)]
mod desktop_ui;
#[cfg_attr(target_os = "macos", path = "macos/network.rs")]
mod network;
mod notification;
mod portal;
#[cfg_attr(target_os = "macos", path = "macos/process_lock.rs")]
mod process_lock;
#[cfg_attr(target_os = "macos", path = "macos/tray.rs")]
mod tray;
mod tray_model;
mod ui_model;
#[cfg(windows)]
mod wifi_recovery;
#[cfg(windows)]
mod win32_main;
#[cfg(windows)]
use win32_main::export_diag;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(target_os = "macos")]
pub use macos::VERSION;
pub fn exe_path() -> String {
    std::env::current_exe()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
fn main() {
    #[cfg(windows)]
    win32_main::run();
    #[cfg(target_os = "macos")]
    macos::run();
}
