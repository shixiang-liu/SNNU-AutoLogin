//! Windows 桌面入口。
#[cfg(debug_assertions)]
use crate::tray;
use crate::VERSION;
use crate::{agent, autostart, config, desktop_ui, portal};
use desktop_ui::{run_win32, WM_DIAG_RESULT};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

fn console_out(text: &str) {
    use windows::Win32::System::Console::{
        AttachConsole, GetConsoleMode, GetStdHandle, WriteConsoleW, ATTACH_PARENT_PROCESS,
        CONSOLE_MODE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let h = match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) => h,
            Err(_) => return,
        };
        if h.is_invalid() {
            return;
        }
        let mut mode = CONSOLE_MODE(0);
        if GetConsoleMode(h, &mut mode).is_ok() {
            let wide: Vec<u16> = text
                .encode_utf16()
                .chain(std::iter::once(b'\n' as u16))
                .collect();
            let mut written = 0u32;
            let _ = WriteConsoleW(h, &wide, Some(&mut written), None);
        } else {
            use std::io::Write;
            use std::os::windows::io::FromRawHandle;
            let file = std::fs::File::from_raw_handle(h.0 as _);
            let mut f = std::mem::ManuallyDrop::new(file);
            let _ = f.write_all(text.as_bytes());
            let _ = f.write_all(b"\n");
            let _ = f.flush();
        }
    }
}

/// 自检内容。redact=true 时掩掉账号（用于会分享出去的诊断报告）
fn doctor_lines(redact: bool) -> Vec<String> {
    let (cfg, pwd) = config::load_config();
    let st = config::load_state();
    let (status, port) = portal::status(Duration::from_secs(4));
    let mut lines = Vec::new();

    if cfg.account.is_empty() {
        lines.push("账号　　还没配置".into());
    } else {
        let shown = if redact {
            portal::mask(&cfg.account)
        } else {
            cfg.account.clone()
        };
        let p = if pwd.is_empty() {
            "没有保存密码"
        } else {
            "密码已保存"
        };
        lines.push(format!("账号　　{}（{}）", shown, p));
    }
    if port > 0 {
        lines.push(format!("门户　　{}:{} 能连上", portal::host(), port));
    } else {
        lines.push(format!("门户　　{} 连不上", portal::host()));
    }
    let online = match &status {
        portal::Status::Online(s) => {
            let acct = if redact {
                portal::mask(&s.account)
            } else {
                s.account.clone()
            };
            let extra = if s.ip.is_empty() {
                String::new()
            } else {
                format!(" · IP {}", s.ip)
            };
            format!("已在线（{}）{}", acct, extra)
        }
        portal::Status::Offline => "连上校园网了，但还没认证".to_string(),
        portal::Status::Unreachable => "不在校园网内".to_string(),
    };
    lines.push(format!("状态　　{}", online));
    lines.push(format!(
        "外网　　{}",
        if portal::internet_ok() {
            "能上"
        } else {
            "上不了"
        }
    ));
    lines.push(format!(
        "后台　　{}",
        if agent::running() {
            format!("运行中（控制端口 {}）", agent::ctl_port())
        } else {
            "没在跑".to_string()
        }
    ));
    lines.push(format!(
        "开机自启　{}",
        if autostart::enabled() {
            "已开"
        } else {
            "已关"
        }
    ));
    if !st.last_error.is_empty() {
        let err = if redact {
            portal::scrub(&st.last_error, &cfg.account)
        } else {
            st.last_error.clone()
        };
        lines.push(format!("上次失败　{}", err));
    }
    lines.push(format!("配置　　{}", config::cfg_path().display()));
    lines.push(format!("日志　　{}", config::log_path().display()));
    lines
}

fn doctor() {
    for line in doctor_lines(false) {
        console_out(&line);
    }
}

/// 只读诊断：有线与 Wi-Fi 现状（自愈的判断依据），不做任何连接动作
fn wifi_info() {
    let cmd = crate::windows_wifi::SystemCmd;
    console_out(&format!(
        "有线　　{}",
        if crate::windows_wifi::wired_connected(&cmd) {
            "已连接"
        } else {
            "没连"
        }
    ));
    console_out(&format!(
        "Wi-Fi　　{}",
        crate::windows_wifi::current_wifi_ssid(&cmd).unwrap_or_else(|| "没连".into())
    ));
}

/// 一键诊断：--doctor 的内容 + 最近日志，写到桌面，方便反馈问题时直接附上
pub(crate) unsafe fn export_diag(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let mut report = String::new();
        report.push_str(&format!("SNNU-AutoLogin v{} 诊断报告\n", VERSION));
        report.push_str(&format!("生成时间　{}\n", config::now_stamp()));
        report.push_str(&"─".repeat(30));
        report.push('\n');
        for line in doctor_lines(true) {
            report.push_str(&line);
            report.push('\n');
        }
        report.push_str(&"─".repeat(30));
        report.push('\n');
        report.push_str("最近日志（最后 40 行）\n");
        if let Ok(text) = std::fs::read_to_string(config::log_path()) {
            let (cfg, _) = config::load_config();
            let tail: Vec<&str> = text.lines().rev().take(40).collect();
            for line in tail.iter().rev() {
                report.push_str(&portal::scrub(line, &cfg.account));
                report.push('\n');
            }
        } else {
            report.push_str("（没有日志文件）\n");
        }

        let (ok, msg) = match diag_desktop_path() {
            Some(dir) => {
                let path = dir.join(format!("SNNU诊断报告-{}.txt", config::stamp_compact()));
                match std::fs::write(&path, &report) {
                    Ok(()) => (true, path.display().to_string()),
                    Err(e) => (false, format!("写入失败：{}", e)),
                }
            }
            None => (false, "找不到桌面文件夹".into()),
        };
        let ptr = Box::into_raw(Box::new((ok, msg)));
        if PostMessageW(
            HWND(raw as *mut _),
            WM_DIAG_RESULT,
            WPARAM(0),
            LPARAM(ptr as isize),
        )
        .is_err()
        {
            let _ = Box::from_raw(ptr);
        }
    });
}

/// 桌面目录：先问系统（OneDrive 重定向也能对），失败再按环境变量猜
fn diag_desktop_path() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KNOWN_FOLDER_FLAG};
    // FOLDERID_Desktop = {B4BFCC3A-DB2C-424C-B029-7FE99A87C641}
    let desktop = windows::core::GUID::from_u128(0xB4BFCC3A_DB2C_424C_B029_7FE99A87C641);
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&desktop, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().ok().filter(|s| !s.is_empty());
            CoTaskMemFree(Some(p.0 as *const _));
            if let Some(s) = s {
                let dir = std::path::PathBuf::from(s);
                if dir.is_dir() {
                    return Some(dir);
                }
            }
        }
    }
    let home = std::env::var("USERPROFILE").ok()?;
    for name in ["Desktop", "OneDrive\\Desktop", "桌面"] {
        let dir = std::path::PathBuf::from(&home).join(name);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    None
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    #[cfg(debug_assertions)]
    if has("--notification-preview") {
        tray::preview_notifications();
        return;
    }

    if has("--daemon") {
        agent::run_daemon();
        return;
    }
    if has("--version") {
        console_out(&format!("SNNU-AutoLogin v{}", VERSION));
        return;
    }
    if has("--probe") {
        let (st, port) = portal::status(Duration::from_secs(5));
        console_out(&format!("{:?} port={}", st, port));
        return;
    }
    if has("--wifi-info") {
        wifi_info();
        return;
    }
    if has("--doctor") {
        doctor();
        return;
    }
    if has("--logout") {
        let (_, port) = portal::status(Duration::from_secs(5));
        if port == 0 {
            console_out("不在校园网内");
        } else {
            portal::logout(port);
            console_out("已断开");
        }
        return;
    }
    if has("--stop") {
        console_out(if agent::stop_daemon() {
            "stopped"
        } else {
            "not running"
        });
        return;
    }
    if let Err(e) = run_win32() {
        config::logf(&format!("Win32 界面启动失败: {}", e));
    }
}
