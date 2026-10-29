// 守护循环：巡检、登录、退避、控制端口

use std::io::Read;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{load_config, load_state, update_state};
use crate::portal::{self, Status};

pub fn ctl_port() -> u16 {
    let s = load_state();
    if s.ctl_port > 0 {
        return s.ctl_port;
    }
    // Windows 用 USERNAME，macOS 用 USER；缺省值保持一致以便多用户区分。
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    let sum: u32 = user.bytes().map(|b| b as u32).sum();
    49721 + (sum % 400) as u16
}

#[cfg(windows)]
pub fn spawn_daemon() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new(crate::exe_path())
        .arg("--daemon")
        .creation_flags(0x00000008 | 0x08000000)
        .spawn();
}

pub fn stop_daemon() -> bool {
    use std::io::Write;
    match std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], ctl_port())),
        Duration::from_millis(300),
    ) {
        Ok(mut s) => {
            let _ = s.set_write_timeout(Some(Duration::from_millis(300)));
            s.write_all(b"Q").is_ok()
        }
        Err(_) => false,
    }
}

/// 监听系统网络地址变化（插拔网线、Wi-Fi 连断、换网），一有变化立刻叫醒巡检
#[cfg(windows)]
fn spawn_net_watch(wake: Arc<AtomicBool>) {
    use windows::Win32::NetworkManagement::IpHelper::NotifyAddrChange;
    std::thread::spawn(move || {
        let mut backoff: u64 = 1;
        loop {
            let started = std::time::Instant::now();
            unsafe {
                let _ = NotifyAddrChange(std::ptr::null_mut(), std::ptr::null());
            }
            // 连续变化做 1 秒去抖
            std::thread::sleep(Duration::from_secs(1));
            wake.store(true, Ordering::SeqCst);
            // 正常情况调用会一直阻塞到地址变化；很快返回说明注册失败，
            // 退避重试，别每秒空转
            if started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
            } else {
                backoff = 1;
            }
        }
    });
}

/// 后台守护只允许一个：避免多个进程各加一个托盘图标、互相抢着登录
#[cfg(windows)]
fn claim_daemon_instance() -> bool {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        match CreateMutexW(
            None,
            false,
            windows::core::w!("Local\\SNNU-AutoLogin-Daemon"),
        ) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    return false;
                }
                let _ = handle;
                true
            }
            Err(_) => true,
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn power_wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, CREATESTRUCTW, GWLP_USERDATA, WM_NCCREATE,
    };
    // 发送消息会直接进入窗口过程，不一定由 GetMessage 作为 MSG 返回。
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    if msg == 0x0218 && matches!(wp.0, 0x0012 | 0x0007 | 0x0006) {
        let wake = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const AtomicBool;
        if !wake.is_null() {
            (*wake).store(true, Ordering::SeqCst);
        }
        return windows::Win32::Foundation::LRESULT(1);
    }
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp)
}

/// 休眠/唤醒监听：恢复供电时等网卡就绪，立刻叫醒巡检
/// （message-only 窗口收不到电源广播，必须用隐藏的顶层窗口）
#[cfg(windows)]
fn spawn_power_watch(wake: Arc<AtomicBool>) {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassExW, TranslateMessage, MSG,
        WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW, WS_POPUP,
    };
    std::thread::spawn(move || unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let cls: Vec<u16> = "SNNUPowerWnd"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(power_wndproc),
            hInstance: windows::Win32::Foundation::HINSTANCE(hinst.0),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            crate::config::logf("电源监听：注册窗口类失败");
            return;
        }
        // 隐藏的顶层窗口：能收到 WM_POWERBROADCAST
        if CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            0,
            0,
            None,
            None,
            windows::Win32::Foundation::HINSTANCE(hinst.0),
            Some(Arc::as_ptr(&wake) as *const std::ffi::c_void),
        )
        .is_err()
        {
            crate::config::logf("电源监听：创建窗口失败");
            return;
        }
        crate::config::logf("电源监听已启动");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    });
}

pub fn run_daemon() {
    if !claim_daemon_instance() {
        crate::config::logf("后台守护已在运行，忽略重复启动");
        return;
    }
    let mut listener = None;
    let base = ctl_port();
    for p in base..base + 10 {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
            update_state(|s| s.ctl_port = p);
            listener = Some(l);
            break;
        }
    }
    let listener = match listener {
        Some(l) => l,
        None => {
            crate::config::logf("另一个实例已在运行");
            return;
        }
    };
    crate::config::logf(&format!("后台已启动 v{}", crate::VERSION));

    #[cfg(windows)]
    let (quit, wake) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    #[cfg(target_os = "macos")]
    let (quit, wake) = crate::macos::runtime::signals();
    #[cfg(windows)]
    {
        spawn_net_watch(wake.clone());
        spawn_power_watch(wake.clone());
    }
    crate::tray::start(quit.clone());
    {
        let quit = quit.clone();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 1];
                let received = s.read(&mut buf).is_ok();
                #[cfg(target_os = "macos")]
                if received && buf[0] == b'O' {
                    crate::macos::runtime::OPEN_REQUESTED.store(true, Ordering::SeqCst);
                }
                if received && buf[0] == b'Q' {
                    quit.store(true, Ordering::SeqCst);
                    #[cfg(target_os = "macos")]
                    crate::macos::runtime::wake();
                    break;
                }
            }
        });
    }

    let mut fail = 0i32;
    let mut beat = 0i32;
    let mut was_online = false;
    let mut last_wake = 0.0f64;
    let mut last_ping_ok = portal::unix_now();
    let mut reconnect_started: Option<std::time::Instant> = None;
    while !quit.load(Ordering::SeqCst) {
        if wake.swap(false, Ordering::SeqCst) {
            last_wake = portal::unix_now();
        }
        let just_woke = portal::unix_now() - last_wake < 30.0;
        let (cfg, pwd) = load_config();
        let st = load_state();
        if pwd.is_empty() {
            crate::tray::set_link(crate::tray::Link::Setup);
            // 设置完成后尽快接管状态与保活，避免托盘长时间停在“尚未设置账号”。
            sleep(2, &quit, &wake);
            continue;
        }
        if !portal::automatic_login_allowed(&st, portal::unix_now()) {
            if st.reconnect_paused {
                reconnect_started = None;
                was_online = false;
            }
            crate::tray::refresh_tip();
            #[cfg(windows)]
            sleep(1, &quit, &wake);
            #[cfg(target_os = "macos")]
            sleep(
                if st.reconnect_paused {
                    300
                } else {
                    (st.paused_until - portal::unix_now()).max(1.0).ceil() as u64
                },
                &quit,
                &wake,
            );
            continue;
        }
        #[cfg(target_os = "macos")]
        let probe_epoch = crate::macos::runtime::EPOCH.current();
        let (status, port) = portal::status(Duration::from_secs(4));
        #[cfg(target_os = "macos")]
        crate::macos::runtime::publish_probe(status.clone(), probe_epoch);
        if quit.load(Ordering::SeqCst) {
            break;
        }
        match status {
            Status::Unreachable => {
                if was_online {
                    reconnect_started.get_or_insert_with(std::time::Instant::now);
                    crate::tray::connecting("连接已断开", "网络恢复后会自动登录");
                    was_online = false;
                }
                crate::tray::set_link(crate::tray::Link::Waiting);
                // 刚唤醒时网卡可能还没就绪，密集重试几次
                sleep(if just_woke { 1 } else { 8 }, &quit, &wake)
            }
            Status::Online(session) => {
                // 其他任务或用户在网页上恢复连接，也要及时撤掉“正在重连”。
                if let Some(started) = reconnect_started.take() {
                    crate::tray::connected(&recovery_text(started.elapsed()));
                }
                fail = 0;
                beat += 1;
                was_online = true;
                crate::tray::set_link(crate::tray::Link::Online);
                update_state(|s| {
                    s.last_ok_ts = portal::unix_now();
                    s.last_error.clear();
                });
                // 保活：长时间没流量就补一次极小请求（generate_204，无正文）
                if cfg.keepalive
                    && portal::unix_now() - last_ping_ok
                        > cfg.keepalive_min.max(5).saturating_mul(60) as f64
                    && portal::internet_ping()
                {
                    last_ping_ok = portal::unix_now();
                }
                if beat % 4 == 0 {
                    let internet_ok = portal::internet_ok();
                    update_state(|s| record_internet_result(s, internet_ok));
                    let Ok(_connection) = portal::connection_lock() else {
                        sleep(5, &quit, &wake);
                        continue;
                    };
                    // 状态可能在检测或等待锁期间改变，修复前再次确认会话归属。
                    let repair_port = if repair_due(&load_state(), portal::unix_now())
                        && session_matches_config(&session, &cfg)
                    {
                        match portal::status(Duration::from_secs(4)) {
                            (Status::Online(current), current_port)
                                if session_matches_config(&current, &cfg) =>
                            {
                                Some(current_port)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let mut repair = false;
                    if crate::config::try_update_state(|s| {
                        if !quit.load(Ordering::SeqCst)
                            && repair_port.is_some()
                            && repair_due(s, portal::unix_now())
                        {
                            // 在发起修复前记录冷却，失败也不能立刻再次断网。
                            s.last_fix_ts = portal::unix_now();
                            repair = true;
                        }
                    })
                    .is_err()
                    {
                        crate::config::logf("无法保存修复冷却时间，跳过本次修复");
                        continue;
                    }
                    if let (true, Some(port)) = (repair, repair_port) {
                        crate::config::logf("检测到假在线，下线重登");
                        crate::tray::connecting("网络暂不可用", "正在尝试恢复连接");
                        if !portal::logout(port) {
                            // 下线请求没送达时会话仍在线，login_automatic 会直接返回成功，
                            // 导致假在线未修复却白等 20 分钟冷却。回滚冷却并稍后重试。
                            crate::config::logf("下线失败，跳过本次修复");
                            let _ = crate::config::try_update_state(|s| s.last_fix_ts = 0.0);
                            sleep(30, &quit, &wake);
                            continue;
                        }
                        crate::config::logf("已下线，重新登录");
                        std::thread::sleep(Duration::from_secs(1));
                        if !quit.load(Ordering::SeqCst) {
                            match portal::login_automatic(
                                &cfg.account,
                                &pwd,
                                &cfg.operator,
                                port,
                                || quit.load(Ordering::SeqCst),
                            ) {
                                Err(msg) => {
                                    record_login_failure(&msg);
                                    crate::tray::failed(&msg);
                                }
                                Ok(()) => {
                                    crate::tray::balloon("校园网已重新认证", "正在检查外网连接")
                                }
                            }
                        }
                    }
                }
                sleep(5, &quit, &wake);
            }
            Status::Offline => {
                crate::tray::set_link(crate::tray::Link::Connecting);
                reconnect_started.get_or_insert_with(std::time::Instant::now);
                if was_online {
                    crate::tray::connecting("连接已断开", "正在自动重连");
                    was_online = false;
                }
                crate::config::logf("未认证，提交登录");
                let started = std::time::Instant::now();
                match portal::login_automatic(&cfg.account, &pwd, &cfg.operator, port, || {
                    quit.load(Ordering::SeqCst)
                }) {
                    Ok(()) => {
                        crate::tray::set_link(crate::tray::Link::Online);
                        crate::config::logf(&format!("登录成功 {}", portal::mask(&cfg.account)));
                        let elapsed = reconnect_started
                            .take()
                            .map(|t| t.elapsed())
                            .unwrap_or_else(|| started.elapsed());
                        crate::tray::connected(&recovery_text(elapsed));
                        was_online = true;
                        update_state(|s| {
                            s.last_error.clear();
                            s.last_ok_ts = portal::unix_now();
                            s.no_internet = false;
                            s.net_bad = 0;
                        });
                        fail = 0;
                        sleep(5, &quit, &wake);
                    }
                    Err(msg) => {
                        if quit.load(Ordering::SeqCst) || load_state().reconnect_paused {
                            continue;
                        }
                        fail += 1;
                        let kind = portal::fail_kind(&msg);
                        crate::config::logf(&format!(
                            "登录失败({}): {}",
                            fail,
                            portal::scrub(&msg, &cfg.account)
                        ));
                        update_state(|s| s.last_error = msg.clone());
                        crate::tray::set_link(crate::tray::Link::Offline);
                        if fail == 1 {
                            crate::tray::failed(&msg);
                        }
                        match portal::next_delay(kind, fail) {
                            // 账号或认证限制：延后重试，避免反复提交无效认证
                            None => {
                                crate::config::logf("账号或认证问题，暂停自动重试 30 分钟");
                                update_state(|s| s.paused_until = portal::unix_now() + 1800.0);
                                sleep(1800, &quit, &wake);
                            }
                            Some(d) => sleep(d.as_secs(), &quit, &wake),
                        }
                    }
                }
            }
        }
    }
    crate::config::logf("后台已退出");
}

fn recovery_text(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("连接已恢复，用时 {:.1} 秒", elapsed.as_secs_f32())
    } else {
        "已重新连接校园网".into()
    }
}

#[cfg(windows)]
fn sleep(secs: u64, quit: &Arc<AtomicBool>, wake: &Arc<AtomicBool>) {
    let step = Duration::from_millis(200);
    let mut left = secs * 1000;
    while left > 0 && !quit.load(Ordering::SeqCst) {
        std::thread::sleep(step);
        left = left.saturating_sub(200);
        // 网络一变（插拔/换网/休眠唤醒）就结束等待，不必等满周期
        if wake.load(Ordering::SeqCst) {
            break;
        }
    }
}

pub fn running() -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], ctl_port())),
        Duration::from_millis(200),
    )
    .is_ok()
}

pub enum ConnectOutcome {
    Online,
    Waiting,
}

/// 手动连接保留具体错误；尚未连上学校网络时恢复自动登录并等待网络。
pub fn connect_now() -> Result<ConnectOutcome, String> {
    let (cfg, pwd) = crate::config::load_config();
    if pwd.is_empty() {
        return Err("请先设置校园网账号".into());
    }
    crate::config::try_update_state(|s| {
        s.reconnect_paused = false;
        s.paused_until = 0.0;
    })
    .map_err(|e| format!("未能开启自动登录：{}", e))?;
    if !running() {
        spawn_daemon();
    }
    let (st, port) = portal::status(Duration::from_secs(4));
    match st {
        Status::Unreachable => return Ok(ConnectOutcome::Waiting),
        Status::Online(_) => return Ok(ConnectOutcome::Online),
        Status::Offline => {}
    }
    match portal::login_automatic(&cfg.account, &pwd, &cfg.operator, port, || false) {
        Ok(()) => Ok(ConnectOutcome::Online),
        Err(msg) => {
            record_login_failure(&msg);
            Err(crate::ui_model::clean_copy(&portal::friendly_login_error(
                &portal::display_error(&msg),
            )))
        }
    }
}

/// 托盘菜单「断开连接」
pub fn logout_now() -> bool {
    // 先禁止新的自动提交，再等待已在途的登录结束并下线。
    if crate::config::try_update_state(|s| s.reconnect_paused = true).is_err() {
        return false;
    }
    let (status, port) = portal::status(Duration::from_secs(4));
    match status {
        Status::Offline => true,
        Status::Online(_) => portal::logout(port),
        Status::Unreachable => false,
    }
}

pub fn pause_automatic(paused: bool) -> Result<(), String> {
    crate::config::try_update_state(|s| {
        s.reconnect_paused = paused;
        if !paused {
            s.paused_until = 0.0;
        }
    })
    .map_err(|e| e.to_string())?;
    if paused {
        let _connection = portal::connection_lock()?;
    } else if !running() {
        spawn_daemon();
    }
    Ok(())
}

fn record_internet_result(s: &mut crate::config::State, ok: bool) {
    s.net_bad = if ok { 0 } else { s.net_bad.saturating_add(1) };
    s.no_internet = s.net_bad >= 2;
}

fn repair_due(s: &crate::config::State, now: f64) -> bool {
    s.net_bad >= 2
        && s.self_login_ok
        && portal::automatic_login_allowed(s, now)
        && (s.last_fix_ts == 0.0 || now - s.last_fix_ts >= 1200.0)
}

fn session_matches_config(session: &portal::Session, cfg: &crate::config::Config) -> bool {
    let (account, operator) = portal::split_account(&session.account);
    !account.is_empty() && account == cfg.account && operator == cfg.operator
}

pub fn record_login_failure(msg: &str) {
    update_state(|s| {
        s.last_error = msg.to_string();
        s.last_error_ts = portal::unix_now();
        if portal::next_delay(portal::fail_kind(msg), 1).is_none() {
            s.paused_until = portal::unix_now() + 1800.0;
        }
    });
    crate::config::logf(&format!(
        "登录失败：{}",
        portal::scrub(msg, &load_config().0.account)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_repair_cannot_take_over_another_account_or_operator() {
        let cfg = crate::config::Config {
            account: "20241234".into(),
            operator: "mobile".into(),
            ..Default::default()
        };
        for (account, eligible) in [
            ("20241234@mobile", true),
            ("20249999@mobile", false),
            ("20241234@unicom", false),
            ("", false),
        ] {
            let session = portal::Session {
                account: account.into(),
                ip: String::new(),
                login_time: String::new(),
            };
            assert_eq!(
                session_matches_config(&session, &cfg),
                eligible,
                "{account}"
            );
        }
    }

    #[test]
    fn waking_from_wait_preserves_event_for_main_loop() {
        let quit = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(AtomicBool::new(true));
        sleep(2, &quit, &wake);
        assert!(wake.swap(false, Ordering::SeqCst));
    }

    #[cfg(windows)]
    #[test]
    fn sent_resume_message_wakes_without_going_through_getmessage() {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
            let class = w!("SNNU-Power-Regression");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(power_wndproc),
                hInstance: windows::Win32::Foundation::HINSTANCE(hinst.0),
                lpszClassName: class,
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&wc), 0);
            let wake = AtomicBool::new(false);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                hinst,
                Some(&wake as *const _ as *const std::ffi::c_void),
            )
            .unwrap();
            SendMessageW(
                hwnd,
                0x0218,
                windows::Win32::Foundation::WPARAM(0x12),
                windows::Win32::Foundation::LPARAM(0),
            );
            assert!(wake.load(Ordering::SeqCst));
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn successful_probe_breaks_failure_streak_and_clears_warning() {
        let mut s = crate::config::State::default();
        record_internet_result(&mut s, false);
        assert!(!s.no_internet);
        record_internet_result(&mut s, true);
        record_internet_result(&mut s, false);
        assert_eq!(s.net_bad, 1);
        assert!(!s.no_internet);
        record_internet_result(&mut s, false);
        assert!(s.no_internet);
        record_internet_result(&mut s, true);
        assert_eq!(s.net_bad, 0);
        assert!(!s.no_internet);
    }

    #[test]
    fn repair_respects_cooldown_and_user_pause() {
        let mut s = crate::config::State {
            net_bad: 2,
            self_login_ok: true,
            ..Default::default()
        };
        assert!(repair_due(&s, 2000.0));
        s.last_fix_ts = 2000.0;
        assert!(!repair_due(&s, 2020.0));
        assert!(!repair_due(&s, 3199.0));
        assert!(repair_due(&s, 3200.0));
        s.reconnect_paused = true;
        assert!(!repair_due(&s, 4000.0));
        s.reconnect_paused = false;
        s.paused_until = 5000.0;
        assert!(!repair_due(&s, 4000.0));
    }
}

#[cfg(target_os = "macos")]
pub fn spawn_daemon() {
    crate::macos::runtime::start();
}
#[cfg(target_os = "macos")]
fn claim_daemon_instance() -> bool {
    true
}
#[cfg(target_os = "macos")]
fn sleep(secs: u64, quit: &Arc<AtomicBool>, wake: &Arc<AtomicBool>) {
    crate::macos::runtime::wait(Duration::from_secs(secs), quit, wake);
}
