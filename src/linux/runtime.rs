//! 无头常驻进程的退出信号：systemd stop 发 SIGTERM，前台 Ctrl-C 发 SIGINT。
//! 信号处理里只置一个原子标志，让主循环自己走完收尾（写日志、放开锁）。
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static QUIT: OnceLock<Arc<AtomicBool>> = OnceLock::new();

extern "C" fn on_stop_signal(_signal: libc::c_int) {
    if let Some(quit) = QUIT.get() {
        quit.store(true, Ordering::SeqCst);
    }
}

/// 退出与唤醒标志：退出由信号或控制端口 Q 置位，唤醒由网络变化置位。
pub fn signals() -> (Arc<AtomicBool>, Arc<AtomicBool>) {
    let quit = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(AtomicBool::new(false));
    let _ = QUIT.set(quit.clone());
    unsafe {
        let handler = on_stop_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
        // 日志写不出去（管道关了）不该把常驻进程带走
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    (quit, wake)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_signal_sets_quit_flag() {
        let (quit, _wake) = signals();
        assert!(!quit.load(Ordering::SeqCst));
        unsafe {
            libc::raise(libc::SIGTERM);
        }
        assert!(quit.load(Ordering::SeqCst));
    }
}
