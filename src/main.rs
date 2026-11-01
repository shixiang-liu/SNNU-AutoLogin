#![cfg_attr(windows, windows_subsystem = "windows")]
// 无头端口不会调用共享模块里 Windows/macOS 专用的辅助函数
// （开机自启注册表、托盘模型、界面视图模型等），统一在 Linux 上关掉死代码告警。
#![cfg_attr(target_os = "linux", allow(dead_code))]
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
mod notification;
mod portal;
#[cfg_attr(target_os = "macos", path = "macos/process_lock.rs")]
#[cfg_attr(target_os = "linux", path = "linux/process_lock.rs")]
mod process_lock;
#[cfg_attr(target_os = "macos", path = "macos/tray.rs")]
#[cfg_attr(target_os = "linux", path = "linux/tray.rs")]
mod tray;
mod tray_model;
mod ui_model;
mod wifi_heal_core;
#[cfg(windows)]
mod win32_main;
#[cfg(windows)]
mod windows_wifi;
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
    #[cfg(target_os = "linux")]
    linux::run();
}
