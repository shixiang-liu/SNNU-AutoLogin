// 开机自启：HKCU\Software\Microsoft\Windows\CurrentVersion\Run

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "SNNU-AutoLogin";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(access: u32) -> Option<HKEY> {
    unsafe {
        let mut h = HKEY::default();
        let rc = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(RUN_KEY).as_ptr()),
            0,
            windows::Win32::System::Registry::REG_SAM_FLAGS(access),
            &mut h,
        );
        if rc == ERROR_SUCCESS {
            Some(h)
        } else {
            None
        }
    }
}

pub fn enable() {
    if let Some(h) = open(KEY_SET_VALUE.0) {
        unsafe {
            let cmd = format!("\"{}\" --daemon", crate::exe_path());
            let data = wide(&cmd);
            let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
            let _ = RegSetValueExW(h, PCWSTR(wide(VALUE).as_ptr()), 0, REG_SZ, Some(bytes));
            let _ = RegCloseKey(h);
        }
    }
}

pub fn disable() {
    if let Some(h) = open(KEY_SET_VALUE.0) {
        unsafe {
            let _ = RegDeleteValueW(h, PCWSTR(wide(VALUE).as_ptr()));
            let _ = RegCloseKey(h);
        }
    }
}

pub fn enabled() -> bool {
    if let Some(h) = open(KEY_QUERY_VALUE.0) {
        // Query metadata without a fixed-size buffer, but reject empty/non-string values.
        let mut kind = REG_VALUE_TYPE::default();
        let mut len = 0;
        let rc = unsafe {
            windows::Win32::System::Registry::RegQueryValueExW(
                h,
                PCWSTR(wide(VALUE).as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut len),
            )
        };
        let _ = unsafe { RegCloseKey(h) };
        return rc == ERROR_SUCCESS && (kind == REG_SZ || kind == REG_EXPAND_SZ) && len > 2;
    }
    false
}

/// 程序被移动过时，把自启路径改回当前路径
pub fn ensure() {
    if enabled() {
        enable();
    }
}
