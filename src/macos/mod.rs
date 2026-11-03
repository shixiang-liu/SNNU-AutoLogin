//! Small native macOS shell; authentication remains in the shared Rust modules.
pub mod app;
pub mod appearance;
pub mod runtime;
pub mod storage;
pub mod validation;

/// macOS releases have their own version, independent of the Windows package.
pub const VERSION: &str = "1.0.1";

#[allow(non_snake_case)]
pub struct LocalTime {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
}
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
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--fixture-check") {
        validation::run();
        return;
    }
    if args.iter().any(|a| a == "--fixture-setup") {
        validation::setup();
        return;
    }
    if args.iter().any(|a| a == "--version") {
        println!("SNNU-AutoLogin v{}", crate::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--stop") {
        println!("{}", crate::agent::stop_daemon());
        return;
    }
    if args.iter().any(|a| a == "--doctor") {
        runtime::publish(crate::portal::status(std::time::Duration::from_secs(5)).0);
        println!("{}", app::diagnostic());
        return;
    }
    if args.iter().any(|a| a == "--probe") {
        println!(
            "{:?}",
            crate::portal::status(std::time::Duration::from_secs(5))
        );
        return;
    }
    if args.iter().any(|a| a == "--logout") {
        println!("{}", crate::agent::logout_now());
        return;
    }
    app::run(&args);
}
