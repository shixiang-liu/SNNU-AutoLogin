//! 无头端口没有托盘：状态直接写进本地日志。
//! API 与 Windows/macOS 端口同名同签名，共享的守护循环因此零改动。
pub use crate::tray_model::Link;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

static LINK: AtomicU8 = AtomicU8::new(0);

pub fn set_link(link: Link) {
    LINK.store(link as u8, Ordering::Relaxed);
}

/// 托盘生命周期在无头端口是空操作：进程本身就是常驻服务
pub fn start(_quit: Arc<AtomicBool>) {}

pub fn refresh_tip() {}

fn log(title: &str, text: &str) {
    if text.is_empty() {
        crate::config::logf(title);
    } else {
        crate::config::logf(&format!("{title}：{text}"));
    }
}

pub fn balloon(title: &str, text: &str) {
    log(title, text);
}

pub fn connecting(title: &str, text: &str) {
    log(title, text);
}

pub fn connected(text: &str) {
    log("已连接", text);
}

pub fn failed(text: &str) {
    log("暂未连接成功", text);
}
