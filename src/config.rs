// 配置、状态、DPAPI 加密、原子写

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{LocalFree, HLOCAL};
#[cfg(windows)]
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

pub const APP_NAME: &str = "SNNU-AutoLogin";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub password_enc: String,
    #[serde(default)]
    pub operator: String,
    /// 保活：长时间没流量时补一次极小请求，防止被门户踢下线
    #[serde(default = "default_keepalive")]
    pub keepalive: bool,
    #[serde(default = "default_keepalive_min")]
    pub keepalive_min: u64,
}

fn default_keepalive() -> bool {
    true
}
fn default_keepalive_min() -> u64 {
    15
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct State {
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub ctl_port: u16,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub last_error_ts: f64,
    #[serde(default)]
    pub last_ok_ts: f64,
    #[serde(default)]
    pub no_internet: bool,
    #[serde(default)]
    pub net_bad: i32,
    #[serde(default)]
    pub self_login_ok: bool,
    #[serde(default)]
    pub last_fix_ts: f64,
    #[serde(default)]
    pub paused_until: f64,
    /// 用户主动断开或正在修改账号时，禁止后台重新登录。
    #[serde(default)]
    pub reconnect_paused: bool,
    /// 最近确认的门户主机，用于认证地址缓存。
    #[serde(default)]
    pub portal_host: String,
}

pub fn app_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    return crate::macos::storage::app_dir();
    #[cfg(windows)]
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    #[cfg(windows)]
    {
        PathBuf::from(base).join(APP_NAME)
    }
}

pub fn cfg_path() -> PathBuf {
    app_dir().join("config.json")
}
pub fn state_path() -> PathBuf {
    app_dir().join("state.json")
}
pub fn log_path() -> PathBuf {
    app_dir().join("login.log")
}

#[cfg(windows)]
fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// 附加应用标识；保密性来自 DPAPI 用户保护，固定熵不等于额外秘密。
/// 这是固定的数据格式标识，与产品版本无关，变更会导致已有密码无法解密。
#[cfg(windows)]
const ENTROPY: &[u8] = b"SNNU-AutoLogin/v4";

#[cfg(windows)]
pub fn encrypt(plain: &str) -> String {
    unsafe {
        let input = blob(plain.as_bytes());
        let ent = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        let r = CryptProtectData(&input, PCWSTR::null(), Some(&ent), None, None, 0, &mut out);
        if r.is_err() {
            return String::new();
        }
        let slice = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let s = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, slice);
        let _ = LocalFree(HLOCAL(out.pbData as _));
        s
    }
}

#[cfg(windows)]
pub fn decrypt(token: &str) -> String {
    let raw = match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, token) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    unsafe {
        let input = blob(&raw);
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ent = blob(ENTROPY);
        let ok = CryptUnprotectData(&input, None, Some(&ent), None, None, 0, &mut out).is_ok()
            || CryptUnprotectData(&input, None, None, None, None, 0, &mut out).is_ok();
        if !ok {
            return String::new();
        }
        let slice = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let s = String::from_utf8_lossy(slice).to_string();
        // 释放 DPAPI 缓冲前擦除其中的明文。
        for b in slice.iter_mut() {
            *b = 0;
        }
        let _ = LocalFree(HLOCAL(out.pbData as _));
        s
    }
}

pub(crate) fn atomic_write(path: &Path, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(windows)]
pub fn load_config() -> (Config, String) {
    let raw = fs::read_to_string(cfg_path()).unwrap_or_default();
    let mut cfg: Config = serde_json::from_str(&raw).unwrap_or_default();
    // 电信出口已停用，旧配置回落到默认校园网
    if cfg.operator == "telecom" {
        cfg.operator.clear();
    }
    let pwd = decrypt(&cfg.password_enc);
    cfg.password_enc = String::new();
    (cfg, pwd)
}

#[cfg(windows)]
pub fn save_config(cfg: &Config, password: &str) -> std::io::Result<()> {
    let mut c = cfg.clone();
    c.password_enc = encrypt(password);
    if c.password_enc.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "密码加密失败",
        ));
    }
    let s = serde_json::to_string_pretty(&c).unwrap_or_default();
    atomic_write(&cfg_path(), &s)
}

pub fn load_state() -> State {
    let raw = fs::read_to_string(state_path()).unwrap_or_default();
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn update_state(f: impl FnOnce(&mut State)) {
    if let Err(e) = try_update_state(f) {
        logf(&format!("状态保存失败：{}", e));
    }
}

pub fn try_update_state(f: impl FnOnce(&mut State)) -> std::io::Result<()> {
    update_state_at(&state_path(), f)
}

fn update_state_at(path: &Path, f: impl FnOnce(&mut State)) -> std::io::Result<()> {
    let _lock = crate::process_lock::ProcessLock::acquire(path, Duration::from_secs(5))?;
    let mut s = match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                // 损坏的状态文件不能无限期阻塞状态更新：先备份原始内容再重建。
                let backup = path.with_file_name(format!(
                    "{}.corrupt-{}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("state"),
                    stamp_compact()
                ));
                let _ = fs::rename(path, &backup);
                logf(&format!(
                    "状态文件损坏，已备份为 {}：{}",
                    backup.display(),
                    e
                ));
                State::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e),
    };
    f(&mut s);
    let txt = serde_json::to_string(&s)?;
    atomic_write(path, &txt)
}

pub fn logf(msg: &str) {
    use std::io::Write;
    let _ = fs::create_dir_all(app_dir());
    if let Ok(meta) = fs::metadata(log_path()) {
        if meta.len() > 200 * 1024 {
            let _ = fs::rename(log_path(), app_dir().join("login.log.old"));
        }
    }
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "[{}] {}", now_stamp(), msg);
    }
}

/// 日志用的本地时间：MM-DD HH:MM:SS
pub fn now_stamp() -> String {
    let t = local_time();
    format!(
        "{:02}-{:02} {:02}:{:02}:{:02}",
        t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// 文件名用的紧凑时间戳（冒号不能进文件名）：MMDD-HHMMSS
pub fn stamp_compact() -> String {
    let t = local_time();
    format!(
        "{:02}{:02}-{:02}{:02}{:02}",
        t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

#[cfg(windows)]
fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    #[cfg(windows)]
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe { GetLocalTime() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "snnu-state-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("state.json")
    }

    #[cfg(windows)]
    #[test]
    fn fresh_profile_child() {
        let Some(profile) = std::env::var_os("SNNU_TEST_FRESH_PROFILE") else {
            return;
        };
        assert_eq!(app_dir(), PathBuf::from(profile).join(APP_NAME));
        let (cfg, password) = load_config();
        assert!(cfg.account.is_empty());
        assert!(cfg.operator.is_empty());
        assert!(password.is_empty());
        let state = load_state();
        assert!(state.last_error.is_empty());
        assert_eq!(state.last_ok_ts, 0.0);
        assert!(state.portal_host.is_empty());
        assert!(!app_dir().exists());
    }

    #[cfg(windows)]
    #[test]
    fn fresh_profile_has_no_saved_account_password_or_history() {
        let path = sandbox();
        let profile = path.parent().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "config::tests::fresh_profile_child"])
            .env("APPDATA", profile)
            .env("SNNU_TEST_FRESH_PROFILE", profile)
            .output()
            .unwrap();
        assert!(result.status.success(), "空配置环境校验失败");
        assert!(!profile.join(APP_NAME).exists());
        fs::remove_dir(profile).unwrap();
    }

    #[test]
    fn state_writer_child() {
        let Some(path) = std::env::var_os("SNNU_TEST_STATE_PATH") else {
            return;
        };
        for _ in 0..40 {
            update_state_at(Path::new(&path), |s| s.net_bad += 1).unwrap();
        }
    }

    #[test]
    fn concurrent_process_updates_preserve_every_change() {
        use std::process::{Command, Stdio};
        let path = sandbox();
        update_state_at(&path, |s| {
            s.reconnect_paused = true;
            s.ctl_port = 49999;
            s.portal_host = "fixture.invalid".into();
        })
        .unwrap();
        let mut children: Vec<_> = (0..3)
            .map(|_| {
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "config::tests::state_writer_child"])
                    .env("SNNU_TEST_STATE_PATH", &path)
                    .stdout(Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        // 原子替换期间的读者也必须始终读到完整 JSON。
        loop {
            let raw = fs::read_to_string(&path).unwrap();
            let _: State = serde_json::from_str(&raw).unwrap();
            if children.iter_mut().all(|c| c.try_wait().unwrap().is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let state: State = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(state.net_bad, 120);
        assert_eq!(state.ctl_port, 49999);
        assert_eq!(state.portal_host, "fixture.invalid");
        assert!(state.reconnect_paused);
        fs::remove_file(&path).unwrap();
        #[cfg(target_os = "macos")]
        fs::remove_file(path.with_extension("lock")).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn damaged_state_is_backed_up_and_rebuilt() {
        let path = sandbox();
        fs::write(&path, "broken JSON").unwrap();
        assert!(update_state_at(&path, |s| s.net_bad = 9).is_ok());
        // 状态重建后可正常更新。
        let rebuilt: State = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rebuilt.net_bad, 9);
        // 原始损坏内容被保留在备份文件里，便于排查。
        let backup = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .expect("损坏文件应被备份");
        assert_eq!(fs::read_to_string(backup.path()).unwrap(), "broken JSON");
        fs::remove_file(&path).unwrap();
        fs::remove_file(backup.path()).unwrap();
        #[cfg(target_os = "macos")]
        fs::remove_file(path.with_extension("lock")).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    /// 忽略未知字段，同时保留有效配置。
    #[test]
    fn 旧配置字段兼容() {
        let old = r#"{
            "account": "20241234",
            "password_enc": "xxx",
            "operator": "mobile",
            "interval": 30,
            "wifi_ssid": "SNNU",
            "keepalive": true,
            "keepalive_min": 15
        }"#;
        let cfg: Config = serde_json::from_str(old).unwrap();
        assert_eq!(cfg.account, "20241234");
        assert_eq!(cfg.operator, "mobile");
        assert!(cfg.keepalive);
        assert_eq!(cfg.keepalive_min, 15);
        // 未知字段不写回配置。
        let again = serde_json::to_string(&cfg).unwrap();
        assert!(!again.contains("interval"));
        assert!(!again.contains("wifi_ssid"));
    }

    #[test]
    fn 空配置用默认值() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.keepalive);
        assert_eq!(cfg.keepalive_min, 15);
        assert!(cfg.account.is_empty());
    }
}

#[cfg(target_os = "macos")]
pub use crate::macos::storage::{load_config, save_config};
#[cfg(target_os = "macos")]
fn local_time() -> crate::macos::LocalTime {
    crate::macos::local_time()
}
