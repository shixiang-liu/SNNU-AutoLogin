// 托盘图标：单击打开、原生状态菜单、异步操作反馈。

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub use crate::tray_model::Link;
use crate::tray_model::{self, Command, Context, Entry, Event};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_REALTIME,
    NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_NOTICE: u32 = WM_APP + 2;
const NID_UID: u32 = 1;

static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static MODERN_CALLBACKS: AtomicBool = AtomicBool::new(false);
static ACTION_BUSY: AtomicBool = AtomicBool::new(false);
static LINK: AtomicU8 = AtomicU8::new(Link::Checking as u8);
static TIP: OnceLock<Mutex<Vec<u16>>> = OnceLock::new();
static NOTICES: OnceLock<Mutex<crate::notification::Latest>> = OnceLock::new();
fn notices() -> &'static Mutex<crate::notification::Latest> {
    NOTICES.get_or_init(|| Mutex::new(crate::notification::Latest::default()))
}
fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn tip_buf() -> &'static Mutex<Vec<u16>> {
    TIP.get_or_init(|| Mutex::new(wide("校园网自动登录")))
}

// MAKEINTRESOURCEW encodes resource ID 1 as a pointer; it is never dereferenced.
#[allow(clippy::manual_dangling_ptr)]
pub(crate) fn app_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        // 资源 1 就是 app.rc 里嵌入的蓝色 WiFi 图标
        LoadIconW(hinst, PCWSTR(1 as *const u16))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn notify(flags: u32, msg: u32, action: u32) -> bool {
    let hwnd = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return false;
    }
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(hwnd as *mut _),
        uID: NID_UID,
        uFlags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS(flags),
        uCallbackMessage: msg,
        ..Default::default()
    };
    if action == NIM_SETVERSION.0 {
        nid.Anonymous.uVersion = 4;
    }
    if flags & NIF_TIP.0 != 0 {
        let g = tip_buf().lock().unwrap();
        let n = g.len().min(127);
        nid.szTip[..n].copy_from_slice(&g[..n]);
    }
    if flags & NIF_ICON.0 != 0 {
        nid.hIcon = app_icon();
    }
    if flags & NIF_INFO.0 != 0 {
        nid.dwInfoFlags = windows::Win32::UI::Shell::NOTIFY_ICON_INFOTIP_FLAGS(0x1);
    }
    unsafe { Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(action), &nid).as_bool() }
}

/// 普通提示也采用最新状态覆盖，不积压到网络状态已变化后才显示。
pub fn balloon(title: &str, text: &str) {
    publish(crate::notification::Kind::Info, title, text);
}
pub fn connecting(title: &str, text: &str) {
    publish(crate::notification::Kind::Connecting, title, text);
}
pub fn connected(text: &str) {
    publish(crate::notification::Kind::Connected, "已连接", text);
}
pub fn failed(text: &str) {
    publish(crate::notification::Kind::Failed, "暂未连接成功", text);
}

fn publish(kind: crate::notification::Kind, title: &str, text: &str) {
    if let Ok(mut latest) = notices().lock() {
        latest.publish(kind, title, text, now_ms());
    }
    let raw = TRAY_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(HWND(raw as *mut _), WM_NOTICE, WPARAM(0), LPARAM(0));
        }
    }
}

fn clear_balloon() {
    let raw = TRAY_HWND.load(Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    // Windows 官方接口：NIF_INFO + 空 szInfo 撤回当前气泡
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(raw as *mut _),
        uID: NID_UID,
        uFlags: NIF_INFO,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn expire_notice() {
    let expired = notices()
        .lock()
        .map(|mut n| n.expire(now_ms()))
        .unwrap_or(false);
    if expired {
        clear_balloon();
    }
}

fn copy_z(destination: &mut [u16], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let mut len = encoded.len().min(destination.len().saturating_sub(1));
    if len > 0 && len < encoded.len() && (0xd800..=0xdbff).contains(&encoded[len - 1]) {
        len -= 1;
    }
    destination[..len].copy_from_slice(&encoded[..len]);
    if len < destination.len() {
        destination[len] = 0;
    }
}

fn drain_balloons() {
    let hwnd_raw = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd_raw == 0 {
        return;
    }
    let item = notices().lock().ok().and_then(|mut n| n.take(now_ms()));
    let Some(notice) = item else {
        return;
    };
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(hwnd_raw as *mut _),
        uID: NID_UID,
        // NIF_REALTIME 禁止 Shell 把已经过时的连接状态延后播放
        // https://learn.microsoft.com/windows/win32/api/shellapi/ns-shellapi-notifyicondataw
        uFlags: NIF_INFO | NIF_REALTIME,
        dwInfoFlags: windows::Win32::UI::Shell::NOTIFY_ICON_INFOTIP_FLAGS(0x1 | 0x10),
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    copy_z(&mut nid.szInfoTitle, &notice.title);
    copy_z(&mut nid.szInfo, &notice.text);
    let accepted = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() };
    crate::config::logf(&format!(
        "状态通知：{} → {}",
        notice.title,
        if accepted {
            "已提交系统"
        } else {
            "系统未接受"
        }
    ));
}

pub fn set_tip(text: &str) {
    if let Ok(mut g) = tip_buf().lock() {
        *g = wide(text);
    }
    notify((NIF_TIP | NIF_SHOWTIP).0, WM_TRAY, NIM_MODIFY.0);
}

fn register_icon() -> bool {
    MODERN_CALLBACKS.store(false, Ordering::SeqCst);
    if !notify(
        (NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP).0,
        WM_TRAY,
        NIM_ADD.0,
    ) {
        return false;
    }
    // 每次添加图标都要重新协商，包括 Explorer 重启后的重新添加。
    let modern = notify(0, 0, NIM_SETVERSION.0);
    MODERN_CALLBACKS.store(modern, Ordering::SeqCst);
    crate::config::logf(if modern {
        "托盘交互：版本 4，单击打开"
    } else {
        "托盘交互：兼容模式，单击打开"
    });
    true
}

pub fn set_link(link: Link) {
    LINK.store(link as u8, Ordering::SeqCst);
    refresh_tip();
}

pub fn refresh_tip() {
    let state = crate::config::load_state();
    let link = Link::from_byte(LINK.load(Ordering::SeqCst));
    let context = Context {
        configured: link != Link::Setup,
        paused: state.reconnect_paused,
        restricted: state.paused_until > crate::portal::unix_now(),
        no_internet: state.no_internet,
        guardian: state.guardian,
        busy: ACTION_BUSY.load(Ordering::SeqCst),
        link,
    };
    set_tip(&format!(
        "校园网自动登录 · {}",
        tray_model::status(&context)
    ));
}

fn menu_context() -> Context {
    let (_, password) = crate::config::load_config();
    let state = crate::config::load_state();
    Context {
        configured: !password.is_empty(),
        paused: state.reconnect_paused,
        restricted: state.paused_until > crate::portal::unix_now(),
        no_internet: state.no_internet,
        guardian: state.guardian,
        busy: ACTION_BUSY.load(Ordering::SeqCst),
        link: Link::from_byte(LINK.load(Ordering::SeqCst)),
    }
}

unsafe fn create_menu(context: &Context) -> Option<HMENU> {
    let menu = CreatePopupMenu().ok()?;
    for entry in tray_model::menu(context) {
        let result = match entry {
            Entry::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
            Entry::Status(label) => {
                AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(wide(label).as_ptr()))
            }
            Entry::Action {
                command,
                label,
                enabled,
                checked,
            } => AppendMenuW(
                menu,
                MF_STRING
                    | if enabled {
                        MENU_ITEM_FLAGS(0)
                    } else {
                        MF_GRAYED
                    }
                    | if checked {
                        MF_CHECKED
                    } else {
                        MENU_ITEM_FLAGS(0)
                    },
                command as usize,
                PCWSTR(wide(label).as_ptr()),
            ),
        };
        if result.is_err() {
            let _ = DestroyMenu(menu);
            return None;
        }
    }
    let _ = SetMenuDefaultItem(menu, Command::Open as u32, 0);
    Some(menu)
}

unsafe fn track_menu(hwnd: HWND, context: &Context, tray: bool) -> u32 {
    let Some(menu) = create_menu(context) else {
        return 0;
    };
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    if tray {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: NID_UID,
            ..Default::default()
        };
        // 菜单锚在图标旁，键盘唤起时也不会跟随远处的鼠标。
        if let Ok(rect) = Shell_NotifyIconGetRect(&id) {
            pt.x = rect.right;
            pt.y = rect.top;
        }
    }
    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    )
    .0 as u32;
    let _ = DestroyMenu(menu);
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    if tray && command == 0 {
        notify(0, 0, NIM_SETFOCUS.0);
    }
    command
}

/// 离线预览复用正式菜单，不执行菜单操作，也不读取真实配置。
#[cfg(debug_assertions)]
pub unsafe fn preview_menu(hwnd: HWND) {
    let context = Context {
        configured: true,
        paused: false,
        restricted: false,
        no_internet: false,
        guardian: false,
        busy: false,
        link: Link::Online,
    };
    let _ = track_menu(hwnd, &context, false);
}

fn open_interface(settings: bool) {
    unsafe {
        if let Ok(window) = FindWindowW(None, windows::core::w!("校园网自动登录")) {
            let _ = ShowWindow(window, SW_RESTORE);
            if settings {
                let _ = PostMessageW(
                    window,
                    crate::desktop_ui::WM_OPEN_SETTINGS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            let _ = SetForegroundWindow(window);
            return;
        }
    }
    let mut process = std::process::Command::new(crate::exe_path());
    if settings {
        process.arg("--settings");
    }
    if process.spawn().is_err() {
        failed("未能打开窗口，请重新打开程序");
    }
}

fn show_menu(hwnd: HWND, quit: &Arc<AtomicBool>) {
    let context = menu_context();
    let command = unsafe { track_menu(hwnd, &context, true) };
    match command {
        1 => {
            open_interface(false);
            return;
        }
        7 => {
            open_interface(true);
            return;
        }
        5 => {
            unsafe {
                // 界面负责未保存的草稿；退出后不被界面重新拉起。
                if let Ok(window) = FindWindowW(None, windows::core::w!("校园网自动登录")) {
                    let _ =
                        PostMessageW(window, crate::desktop_ui::WM_EXIT_APP, WPARAM(0), LPARAM(0));
                    return;
                }
                quit.store(true, Ordering::SeqCst);
                let _ = PostMessageW(hwnd, WM_DESTROY, WPARAM(0), LPARAM(0));
            }
            return;
        }
        8 => {
            if crate::config::try_update_state(|s| {
                s.guardian = !s.guardian;
                s.guardian_failures = 0;
                s.net_bad = 0;
                s.no_internet = false;
            })
            .is_err()
            {
                failed("无法保存守卫者设置");
            }
            return;
        }
        9 => {
            let port = crate::config::load_state().port;
            if crate::portal::PORTS.contains(&port) && crate::portal::host() == crate::portal::HOST
            {
                let url = wide(&format!(
                    "http://{}:{port}/snnuportal/userstatus.jsp",
                    crate::portal::HOST
                ));
                let result = unsafe {
                    windows::Win32::UI::Shell::ShellExecuteW(
                        hwnd,
                        windows::core::w!("open"),
                        PCWSTR(url.as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    )
                };
                if result.0 as isize <= 32 {
                    failed("无法打开在线设备管理页面");
                }
            } else {
                failed("尚未连接到支持设备管理的校园网门户");
            }
            return;
        }
        2 | 3 | 6 => {}
        _ => return,
    }
    if ACTION_BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    refresh_tip();
    let paused = context.paused;
    connecting(
        match command {
            3 => "正在断开认证",
            6 if !paused => "正在关闭自动登录",
            6 => "正在开启自动登录",
            _ => "正在检查连接",
        },
        "稍候，完成后会更新结果",
    );
    std::thread::spawn(move || {
        use crate::notification::Kind;
        let (kind, title, message) = match command {
            2 => {
                let internet_check = crate::config::load_state().no_internet;
                let result = if internet_check {
                    let ok = crate::portal::internet_ok();
                    crate::config::update_state(|s| {
                        s.no_internet = !ok;
                        if ok {
                            s.net_bad = 0;
                        }
                    });
                    if ok {
                        Ok(crate::agent::ConnectOutcome::Online)
                    } else {
                        Err("校园网已认证，外网暂时无法访问".into())
                    }
                } else {
                    crate::agent::connect_now()
                };
                match result {
                    Ok(crate::agent::ConnectOutcome::Online) => {
                        set_link(Link::Online);
                        (Kind::Connected, "已连接", "已完成连接检查".into())
                    }
                    Ok(crate::agent::ConnectOutcome::Waiting) => {
                        set_link(Link::Waiting);
                        (
                            Kind::Info,
                            "等待校园网",
                            "连接学校 Wi-Fi 或网线后会自动登录".into(),
                        )
                    }
                    Err(message) => (Kind::Failed, "暂未连接成功", message),
                }
            }
            3 => {
                if crate::agent::logout_now() {
                    set_link(Link::Offline);
                    (
                        Kind::Info,
                        "已断开校园网认证",
                        "自动登录已关闭，Wi-Fi 保持连接".into(),
                    )
                } else if crate::config::load_state().reconnect_paused {
                    (
                        Kind::Failed,
                        "自动登录已关闭",
                        "暂未确认认证是否断开，请打开主界面检查".into(),
                    )
                } else {
                    (
                        Kind::Failed,
                        "操作未完成",
                        "未能断开认证，请打开主界面检查".into(),
                    )
                }
            }
            _ => match crate::agent::pause_automatic(!paused) {
                Ok(()) if paused => (
                    Kind::Info,
                    "已开启自动登录",
                    "连接校园网后会自动登录".into(),
                ),
                Ok(()) => (Kind::Info, "自动登录已关闭", "当前连接保持不变".into()),
                Err(message) => (Kind::Failed, "设置未能完成", message),
            },
        };
        ACTION_BUSY.store(false, Ordering::SeqCst);
        refresh_tip();
        publish(kind, title, &message);
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Explorer 重启 / 登录后任务栏就绪：把图标重新加回去
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::SeqCst) {
        register_icon();
        return LRESULT(0);
    }
    match msg {
        WM_NOTICE => {
            clear_balloon();
            LRESULT(0)
        }
        WM_TIMER => {
            expire_notice();
            drain_balloons();
            LRESULT(0)
        }
        WM_TRAY => {
            match tray_model::event(
                MODERN_CALLBACKS.load(Ordering::SeqCst),
                wp.0,
                lp.0 as u32,
                NID_UID,
            ) {
                Event::Open => open_interface(false),
                Event::Menu => {
                    if let Some(quit) = QUIT.get() {
                        show_menu(hwnd, quit);
                    }
                }
                Event::NoticeShown => crate::config::logf("状态通知：系统发出显示事件"),
                Event::NoticeClosed => crate::config::logf("状态通知：系统发出关闭事件"),
                Event::Ignore => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, 1);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

static QUIT: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// 独立通知预览，不读取账号或操作网络。
#[cfg(debug_assertions)]
pub fn preview_notifications() {
    let quit = Arc::new(AtomicBool::new(false));
    start(quit.clone());
    for _ in 0..30 {
        if TRAY_HWND.load(Ordering::SeqCst) != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    publish(
        crate::notification::Kind::Connecting,
        "通知预览 · 正在连接",
        "仅测试通知，不操作网络",
    );
    std::thread::sleep(Duration::from_millis(1500));
    publish(
        crate::notification::Kind::Connected,
        "通知预览 · 连接成功",
        "这是最新结果，旧提示应已撤下",
    );
    std::thread::sleep(Duration::from_secs(6));
    quit.store(true, Ordering::SeqCst);
    let raw = TRAY_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(HWND(raw as *mut _), WM_DESTROY, WPARAM(0), LPARAM(0));
        }
    }
    std::thread::sleep(Duration::from_millis(100));
}

pub fn start(quit: Arc<AtomicBool>) {
    let _ = QUIT.set(quit);
    std::thread::spawn(move || unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class_name = wide("SNNUTrayWnd");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: windows::Win32::Foundation::HINSTANCE(hinst.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            crate::config::logf("托盘注册类失败");
            return;
        }
        let hwnd = match CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("SNNU").as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                crate::config::logf(&format!("托盘建窗失败: {:?}", e));
                return;
            }
        };
        TRAY_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        crate::config::logf("托盘已启动");
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(windows::core::w!("TaskbarCreated")),
            Ordering::SeqCst,
        );

        // 开机自启时 Explorer 可能还没准备好，失败就短暂重试
        let mut added = false;
        for attempt in 0..6 {
            if register_icon() {
                added = true;
                break;
            }
            if attempt < 5 {
                std::thread::sleep(Duration::from_millis(800));
            }
        }
        // 图标注册后提交已缓存的悬停文本。
        if added {
            notify((NIF_TIP | NIF_SHOWTIP).0, WM_TRAY, NIM_MODIFY.0);
        }

        // 状态变化通过消息立即提交，计时器只负责撤掉过时提示
        let _ = SetTimer(hwnd, 1, 100, None);
        drain_balloons();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        notify(NIF_TIP.0, 0, NIM_DELETE.0);
        let _ = DestroyWindow(hwnd);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_menu_preserves_default_action_checkmark_and_disabled_state() {
        unsafe {
            let mut context = Context {
                configured: true,
                paused: false,
                restricted: false,
                no_internet: false,
                guardian: false,
                busy: false,
                link: Link::Online,
            };
            let menu = create_menu(&context).unwrap();
            assert_eq!(
                GetMenuDefaultItem(menu, 0, GET_MENU_DEFAULT_ITEM_FLAGS(0)),
                Command::Open as u32
            );
            assert_ne!(
                GetMenuState(menu, Command::Automatic as u32, MF_BYCOMMAND) & MF_CHECKED.0,
                0
            );
            assert_eq!(
                GetMenuState(menu, Command::Retry as u32, MF_BYCOMMAND),
                u32::MAX
            );
            DestroyMenu(menu).unwrap();
            context.busy = true;
            let menu = create_menu(&context).unwrap();
            assert_ne!(
                GetMenuState(menu, Command::Automatic as u32, MF_BYCOMMAND) & MF_GRAYED.0,
                0
            );
            DestroyMenu(menu).unwrap();
        }
    }
    #[test]
    fn notification_text_is_terminated_without_splitting_surrogates() {
        let mut target = [0u16; 3];
        copy_z(&mut target, "A😀");
        assert_eq!(target, [b'A' as u16, 0, 0]);
        let mut target = [0u16; 4];
        copy_z(&mut target, "A😀");
        assert_eq!(String::from_utf16(&target[..3]).unwrap(), "A😀");
        assert_eq!(target[3], 0);
    }
}
