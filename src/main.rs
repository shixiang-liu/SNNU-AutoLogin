#![cfg_attr(windows, windows_subsystem = "windows")]
mod agent;
#[cfg_attr(target_os = "macos", path = "macos/autostart.rs")]
#[cfg_attr(target_os = "linux", path = "linux/autostart.rs")]
mod autostart;
mod config;
#[cfg(windows)]
mod desktop_ui;
#[cfg(target_os = "linux")]
mod linux;
#[cfg_attr(target_os = "macos", path = "macos/network.rs")]
#[cfg_attr(target_os = "linux", path = "linux/network.rs")]
mod network;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod notification;
mod portal;
#[cfg_attr(target_os = "macos", path = "macos/process_lock.rs")]
#[cfg_attr(target_os = "linux", path = "linux/process_lock.rs")]
mod process_lock;
#[cfg_attr(target_os = "macos", path = "macos/tray.rs")]
#[cfg_attr(target_os = "linux", path = "linux/tray.rs")]
mod tray;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod tray_model;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod ui_model;
#[cfg(windows)]
mod wifi_recovery;
#[cfg(windows)]
mod win32_main;
#[cfg(windows)]
use win32_main::export_diag;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(target_os = "linux")]
pub use linux::VERSION;
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
    #[cfg(target_os = "linux")]
    linux::run();
}
