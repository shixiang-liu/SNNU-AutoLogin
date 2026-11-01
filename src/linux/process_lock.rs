//! 建议性文件锁（flock）：进程崩溃时由内核释放，不会留下死锁。
//! 与 macOS 端口同一套策略：同一线程可重入，跨进程互斥。
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

struct LockedFile {
    file: File,
}

// Windows 命名互斥量对持有线程是可重入的；这里对齐这个行为，
// 因为“假在线修复”会拿着连接锁再去调用会加锁的 logout/login。
thread_local! { static HELD: RefCell<HashMap<PathBuf, Weak<LockedFile>>> = RefCell::new(HashMap::new()); }

pub struct ProcessLock {
    _guard: Rc<LockedFile>,
}

impl ProcessLock {
    pub fn acquire(path: &Path, timeout: Duration) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let path = path.with_extension("lock");
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if let Some(guard) = HELD.with(|locks| locks.borrow().get(&key).and_then(Weak::upgrade)) {
            return Ok(Self { _guard: guard });
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        let deadline = Instant::now() + timeout;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                let guard = Rc::new(LockedFile { file });
                HELD.with(|locks| {
                    locks
                        .borrow_mut()
                        .insert(path.canonicalize().unwrap_or(key), Rc::downgrade(&guard))
                });
                return Ok(Self { _guard: guard });
            }
            let error = io::Error::last_os_error();
            if !matches!(
                error.raw_os_error(),
                Some(libc::EWOULDBLOCK) | Some(libc::EINTR)
            ) {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "等待程序锁超时"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同线程可重入而其他线程互斥() {
        let path = std::env::temp_dir().join(format!("snnu-linux-recursive-{}", std::process::id()));
        let outer = ProcessLock::acquire(&path, Duration::ZERO).unwrap();
        let inner = ProcessLock::acquire(&path, Duration::ZERO).unwrap();
        drop(inner);
        let other = path.clone();
        assert!(std::thread::spawn(move || ProcessLock::acquire(
            &other,
            Duration::from_millis(30)
        )
        .is_err())
        .join()
        .unwrap());
        drop(outer);
        assert!(ProcessLock::acquire(&path, Duration::ZERO).is_ok());
        std::fs::remove_file(path.with_extension("lock")).unwrap();
    }
}
