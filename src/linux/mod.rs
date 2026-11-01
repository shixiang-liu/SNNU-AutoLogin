//! Linux 无头端口：门户协议、调度、状态全部复用共享模块，这里只补平台耦合点。
//! 没有托盘也没有图形界面，进程入口是 linux::cli。
// autostart / network / process_lock / tray 与 macOS 端口一样由 main.rs 的
// path 属性挂到 crate 根（crate::autostart 等），这里只放无头端口独有的模块。
pub mod cli;
pub mod runtime;
pub mod storage;
pub mod wifi;

/// 与 macos::LocalTime 同名字段：config/portal 的共享代码按字段读取本地时间。
#[allow(non_snake_case)]
pub struct LocalTime {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
}

/// 本地时间：libc::localtime_r 尊重 TZ，日志时间戳和文件名时间戳都用它。
pub fn local_time() -> LocalTime {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut t: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut t);
        LocalTime {
            wYear: (t.tm_year + 1900) as u16,
            wMonth: (t.tm_mon + 1) as u16,
            wDay: t.tm_mday as u16,
            wHour: t.tm_hour as u16,
            wMinute: t.tm_min as u16,
            wSecond: t.tm_sec as u16,
        }
    }
}

pub fn run() {
    std::process::exit(cli::run());
}
