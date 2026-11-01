//! 小型原生桌面界面：状态主页、设置、账号、帮助
use crate::{
    agent, autostart, config, network, portal,
    ui_model::{self, Action, Tone},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::Duration;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{SetScrollInfo, ShowScrollBar, DRAWITEMSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

const BG: u32 = 0x00f9f7f6;
const WHITE: u32 = 0x00ffffff;
const TEXT: u32 = 0x00302218;
const MUTED: u32 = 0x00857066;
const BLUE: u32 = 0x00eb6325;
const LINE: u32 = 0x00ece7e4;
const SOFT: u32 = 0x00f3efeb;
const GREEN: u32 = 0x004a7a16;
const RED: u32 = 0x003b35bd;
const AMBER: u32 = 0x00185c99;
const WIDTH: i32 = 380;
const WM_SNAPSHOT: u32 = WM_APP + 1;
const WM_DONE: u32 = WM_APP + 2;
pub const WM_DIAG_RESULT: u32 = WM_APP + 5;
pub const WM_EXIT_APP: u32 = WM_APP + 6;
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 7;

const NETWORK: i32 = 101;
const SETTINGS: i32 = 102;
const TITLE: i32 = 103;
const SUB: i32 = 104;
const ACCOUNT_CAPTION: i32 = 105;
const ACCOUNT_VALUE: i32 = 106;
const OP_VALUE: i32 = 107;
const PRIMARY: i32 = 108;
const FOOT: i32 = 109;
const BACK: i32 = 110;
const PAGE_TITLE: i32 = 111;
const NOTE: i32 = 112;
const ACCOUNT_ROW: i32 = 201;
const AUTO_CONNECT: i32 = 202;
const AUTO_START: i32 = 203;
const CHECK_NETWORK: i32 = 113;
const HELP: i32 = 206;
const DELETE: i32 = 208;
const FEATURES: i32 = 209;
const FEATURE_START: i32 = 510;
const FEATURE_ITEMS: [(&str, &str); 9] = [
    ("自动登录", "连接学校网络后自动完成登录"),
    ("开机运行", "随 Windows 启动后台"),
    ("自动重连", "掉线、换网或唤醒后重新检查连接"),
    ("连接保活", "定时发送少量请求，维持连接活动"),
    ("异常处理", "尝试恢复网络，账号异常时暂缓重试"),
    ("认证地址自动识别", "学校认证地址变化时自动重新寻找"),
    ("密码加密保存", "密码在本机加密保存"),
    ("托盘运行", "关闭窗口后继续运行"),
    ("诊断报告", "导出连接信息与日志，方便排查问题"),
];
const FEATURE_END: i32 = FEATURE_START + FEATURE_ITEMS.len() as i32 * 2;
const FEATURE_GROUP_START: i32 = 550;
const FEATURE_GROUPS: [&str; 3] = ["自动连接", "连接维护", "日常使用"];
const INTRO: i32 = 301;
const ACC_LABEL: i32 = 302;
const ACC_EDIT: i32 = 303;
const PWD_LABEL: i32 = 304;
const PWD_EDIT: i32 = 305;
const PWD_SHOW: i32 = 306;
const OP_LABEL: i32 = 307;
const SEG0: i32 = 308;
const SEG1: i32 = 309;
const SEG2: i32 = 310;
const OP_HINT: i32 = 311;
const SAVE: i32 = 312;
const PRIVACY: i32 = 313;
const INFO_TITLE: i32 = 410;
const INFO_SUB: i32 = 411;
const INFO_FIELD_START: i32 = 430;
const INFO_FIELD_END: i32 = INFO_FIELD_START + 10;
const INFO_LABELS: [&str; 5] = ["网络名称", "当前账号", "上网方式", "IP 地址", "在线时长"];
const DIAG_HINT: i32 = 404;
const DIAG: i32 = 402;
const VERSION: i32 = 403;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Settings,
    Account,
    Help,
    Features,
}
#[derive(Clone, Copy, PartialEq)]
enum Style {
    Label,
    Center,
    CardLabel,
    Value,
    Edit,
    Primary,
    Power,
    Link,
    Row,
    Toggle,
    Segment,
}
struct Control {
    id: i32,
    page: Option<Page>,
    rect: (i32, i32, i32, i32),
    text: &'static str,
    style: Style,
}

fn controls() -> Vec<Control> {
    use Page::*;
    use Style::*;
    let c = |id, page, rect, text, style| Control {
        id,
        page,
        rect,
        text,
        style,
    };
    let mut list = vec![
        c(NETWORK, Some(Home), (24, 20, 250, 28), "当前网络", Label),
        c(SETTINGS, Some(Home), (292, 16, 64, 36), "设置", Link),
        c(
            TITLE,
            Some(Home),
            (24, 228, 332, 38),
            "正在检测连接",
            Center,
        ),
        c(
            SUB,
            Some(Home),
            (24, 272, 332, 42),
            "稍候，正在检查当前网络",
            Center,
        ),
        c(
            ACCOUNT_CAPTION,
            Some(Home),
            (40, 342, 272, 18),
            "当前账号",
            CardLabel,
        ),
        c(
            ACCOUNT_VALUE,
            Some(Home),
            (40, 364, 216, 24),
            "尚未设置",
            CardLabel,
        ),
        c(OP_VALUE, Some(Home), (268, 364, 72, 24), "", CardLabel),
        c(PRIMARY, Some(Home), (122, 72, 136, 136), "检测中", Power),
        c(
            CHECK_NETWORK,
            Some(Home),
            (130, 310, 120, 30),
            "检查网络",
            Link,
        ),
        c(
            FOOT,
            Some(Home),
            (24, 420, 332, 34),
            "关闭窗口后仍可自动连接",
            Center,
        ),
        c(BACK, None, (16, 16, 64, 36), "‹ 返回", Link),
        c(PAGE_TITLE, None, (90, 20, 200, 28), "设置", Center),
        c(NOTE, None, (24, 438, 332, 40), "", Center),
        c(
            ACCOUNT_ROW,
            Some(Settings),
            (24, 76, 332, 76),
            "账号设置\n更换学号、密码或上网方式",
            Row,
        ),
        c(
            AUTO_CONNECT,
            Some(Settings),
            (24, 172, 332, 62),
            "自动登录\n已开启，掉线后自动恢复",
            Toggle,
        ),
        c(
            AUTO_START,
            Some(Settings),
            (24, 246, 332, 62),
            "开机自动运行\n登录 Windows 后自动运行",
            Toggle,
        ),
        c(
            FEATURES,
            Some(Settings),
            (24, 328, 332, 42),
            "功能介绍",
            Row,
        ),
        c(HELP, Some(Settings), (24, 382, 332, 42), "网络信息", Row),
        c(
            INTRO,
            Some(Account),
            (24, 70, 332, 24),
            "填一次，以后自动登录",
            Center,
        ),
        c(ACC_LABEL, Some(Account), (24, 116, 332, 20), "学号", Label),
        c(ACC_EDIT, Some(Account), (38, 151, 304, 24), "", Edit),
        c(
            PWD_LABEL,
            Some(Account),
            (24, 202, 200, 20),
            "校园网密码",
            Label,
        ),
        c(PWD_EDIT, Some(Account), (38, 237, 304, 24), "", Edit),
        c(PWD_SHOW, Some(Account), (286, 194, 70, 32), "显示", Link),
        c(
            OP_LABEL,
            Some(Account),
            (24, 286, 332, 20),
            "上网方式",
            Label,
        ),
        c(SEG0, Some(Account), (28, 320, 106, 34), "校园网", Segment),
        c(SEG1, Some(Account), (137, 320, 106, 34), "移动", Segment),
        c(SEG2, Some(Account), (246, 320, 106, 34), "联通", Segment),
        c(
            OP_HINT,
            Some(Account),
            (24, 366, 332, 22),
            "与网页登录时的选择一致",
            Label,
        ),
        c(
            SAVE,
            Some(Account),
            (24, 438, 332, 44),
            "保存并连接",
            Primary,
        ),
        c(
            PRIVACY,
            Some(Account),
            (24, 498, 332, 20),
            "密码在本机加密保存",
            Center,
        ),
        c(
            DELETE,
            Some(Account),
            (24, 536, 332, 32),
            "删除保存的账号",
            Link,
        ),
        c(INFO_TITLE, Some(Help), (62, 88, 278, 26), "", Label),
        c(INFO_SUB, Some(Help), (40, 124, 300, 32), "", Label),
        c(
            DIAG,
            Some(Help),
            (24, 432, 332, 44),
            "导出诊断报告",
            Primary,
        ),
        c(
            DIAG_HINT,
            Some(Help),
            (24, 492, 332, 38),
            "包含连接信息与最近日志\n密码不写入报告，账号会脱敏",
            Center,
        ),
        c(VERSION, Some(Help), (24, 552, 332, 20), "", Center),
    ];
    for (i, label) in INFO_LABELS.iter().enumerate() {
        let y = 208 + i as i32 * 40;
        list.push(c(
            INFO_FIELD_START + i as i32 * 2,
            Some(Help),
            (40, y, 92, 22),
            label,
            Label,
        ));
        list.push(c(
            INFO_FIELD_START + i as i32 * 2 + 1,
            Some(Help),
            (134, y, 206, 22),
            "",
            Value,
        ));
    }
    for (i, group) in FEATURE_GROUPS.iter().enumerate() {
        list.push(c(
            FEATURE_GROUP_START + i as i32,
            Some(Features),
            (24, 72 + i as i32 * 212, 332, 20),
            group,
            Label,
        ));
    }
    for (i, (title, description)) in FEATURE_ITEMS.iter().enumerate() {
        let top = 112 + i as i32 / 3 * 212 + i as i32 % 3 * 56;
        list.push(c(
            FEATURE_START + i as i32 * 2,
            Some(Features),
            (40, top, 300, 20),
            title,
            Label,
        ));
        list.push(c(
            FEATURE_START + i as i32 * 2 + 1,
            Some(Features),
            (40, top + 24, 300, 18),
            description,
            Label,
        ));
    }
    list
}

#[derive(Clone)]
struct Snapshot {
    revision: u64,
    status: portal::Status,
    name: String,
    account: String,
    operator: String,
    configured: bool,
    running: bool,
    startup: bool,
    state: config::State,
}
impl Snapshot {
    fn preview(kind: &str) -> Self {
        let state = config::State {
            reconnect_paused: matches!(kind, "paused" | "paused-online"),
            no_internet: kind == "warning",
            last_error: match kind {
                "error" => "您的账号密码有误",
                "captcha" => "需要验证码",
                "balance" => "账号已欠费",
                _ => "",
            }
            .into(),
            ..Default::default()
        };
        Self {
            revision: REFRESH_EPOCH.current(),
            status: if matches!(
                kind,
                "error" | "paused" | "setup" | "offline" | "captcha" | "balance" | "connecting"
            ) {
                portal::Status::Offline
            } else if kind == "waiting" {
                portal::Status::Unreachable
            } else {
                portal::Status::Online(portal::Session {
                    account: if kind == "other" {
                        "20249999@mobile"
                    } else {
                        "20241234@mobile"
                    }
                    .into(),
                    ip: "10.1.2.3".into(),
                    login_time: String::new(),
                })
            },
            name: network::current_name(),
            account: "20241234".into(),
            operator: "mobile".into(),
            configured: kind != "setup",
            running: true,
            startup: true,
            state,
        }
    }
}

#[derive(Clone, Copy)]
struct Gfx {
    heading: HFONT,
    title: HFONT,
    body: HFONT,
    small: HFONT,
    button: HFONT,
    bg: HBRUSH,
    white: HBRUSH,
}
unsafe impl Send for Gfx {}
struct Ui {
    dpi: i32,
    page: Page,
    busy: bool,
    diag_busy: bool,
    loading: bool,
    extra_check: bool,
    sub_extra: i32,
    info_extra: i32,
    info_tone: Tone,
    spin: u16,
    preview: bool,
    hover: i32,
    scroll: i32,
    viewport: i32,
    content_height: i32,
    op: i32,
    shown: bool,
    action: Action,
    tone: Tone,
    note: String,
    note_error: bool,
    note_until: Option<std::time::Instant>,
    invalid_field: Option<i32>,
    busy_id: i32,
    busy_label: String,
    initializing_inputs: bool,
    draft_touched: bool,
    snapshot: Snapshot,
    gfx: Gfx,
}
static UI: OnceLock<Mutex<Ui>> = OnceLock::new();
static PREVIEW: OnceLock<String> = OnceLock::new();
static PREVIEW_DPI: OnceLock<i32> = OnceLock::new();
static SAVING: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicBool = AtomicBool::new(false);
static EXIT_WITH_DAEMON: AtomicBool = AtomicBool::new(false);
static LAST_START: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REFRESH_EPOCH: ui_model::RefreshEpoch = ui_model::RefreshEpoch::new();
fn ui() -> std::sync::MutexGuard<'static, Ui> {
    UI.get().unwrap().lock().unwrap_or_else(|e| e.into_inner())
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn s(dpi: i32, n: i32) -> i32 {
    (n * dpi + 48) / 96
}
unsafe fn child(hwnd: HWND, id: i32) -> HWND {
    GetDlgItem(hwnd, id).unwrap_or_default()
}
unsafe fn text(hwnd: HWND, id: i32, value: &str) {
    let value = wide(value);
    let _ = SetWindowTextW(child(hwnd, id), PCWSTR(value.as_ptr()));
}
unsafe fn get(hwnd: HWND, id: i32) -> String {
    let control = child(hwnd, id);
    let n = GetWindowTextLengthW(control);
    let mut v = vec![0u16; n.max(0) as usize + 1];
    let len = GetWindowTextW(control, &mut v);
    String::from_utf16_lossy(&v[..len as usize])
}
fn color(tone: Tone) -> u32 {
    match tone {
        Tone::Good => GREEN,
        Tone::Error => RED,
        Tone::Warning => AMBER,
        Tone::Neutral => MUTED,
    }
}
fn op_value(i: i32) -> &'static str {
    match i {
        1 => "mobile",
        2 => "unicom",
        _ => "",
    }
}
fn op_index(op: &str) -> i32 {
    match op {
        "mobile" => 1,
        "unicom" => 2,
        _ => 0,
    }
}
fn op_label(op: &str) -> &'static str {
    match op {
        "mobile" => "移动",
        "unicom" => "联通",
        _ => "校园网",
    }
}
fn font(dpi: i32, points: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -((points * dpi + 36) / 72),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            w!("Microsoft YaHei UI"),
        )
    }
}
fn make_gfx(dpi: i32) -> Gfx {
    unsafe {
        Gfx {
            heading: font(dpi, 13, 600),
            title: font(dpi, 22, 650),
            body: font(dpi, 10, 400),
            small: font(dpi, 9, 400),
            button: font(dpi, 10, 600),
            bg: CreateSolidBrush(COLORREF(BG)),
            white: CreateSolidBrush(COLORREF(WHITE)),
        }
    }
}
fn control_font(gfx: &Gfx, id: i32) -> HFONT {
    if (FEATURE_START..FEATURE_END).contains(&id) {
        return if (id - FEATURE_START) % 2 == 0 {
            gfx.button
        } else {
            gfx.small
        };
    }
    if (FEATURE_GROUP_START..FEATURE_GROUP_START + 3).contains(&id) {
        return gfx.small;
    }
    match id {
        INFO_TITLE => gfx.heading,
        INFO_SUB => gfx.small,
        TITLE => gfx.title,
        NETWORK | PAGE_TITLE | ACCOUNT_VALUE => gfx.heading,
        FOOT | PRIVACY | OP_HINT | ACCOUNT_CAPTION | OP_VALUE | DIAG_HINT | VERSION | NOTE => {
            gfx.small
        }
        _ => gfx.body,
    }
}

unsafe fn create_controls(hwnd: HWND) {
    let (dpi, gfx) = {
        let u = ui();
        (u.dpi, u.gfx)
    };
    let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
    for c in controls() {
        let is_button = matches!(
            c.style,
            Style::Power
                | Style::Primary
                | Style::Link
                | Style::Row
                | Style::Toggle
                | Style::Segment
        );
        let mut style = WS_CHILD.0;
        let class = if c.style == Style::Edit {
            w!("EDIT")
        } else if is_button {
            w!("BUTTON")
        } else {
            w!("STATIC")
        };
        if is_button {
            style |= WS_TABSTOP.0 | 0x000b | 0x4000;
        } else if c.style == Style::Edit {
            style |= WS_TABSTOP.0 | 0x0080;
            if c.id == PWD_EDIT {
                style |= 0x0020;
            }
        } else if matches!(c.style, Style::Center) {
            style |= 0x0001;
        } else if c.style == Style::Value {
            style |= 0x0002 | 0x4000;
        }
        if !is_button && c.style != Style::Edit {
            style |= 0x0080;
        } // SS_NOPREFIX，SSID 中的 & 按原文显示
        if c.id == NETWORK {
            style |= 0x4000;
        } // SS_ENDELLIPSIS
        let t = wide(c.text);
        let (x, y, w, h) = c.rect;
        let control = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(style),
            s(dpi, x),
            s(dpi, y),
            s(dpi, w),
            s(dpi, h),
            hwnd,
            HMENU(c.id as isize as *mut _),
            hinst,
            None,
        )
        .unwrap_or_default();
        let _ = SendMessageW(
            control,
            WM_SETFONT,
            WPARAM(control_font(&gfx, c.id).0 as usize),
            LPARAM(0),
        );
        if c.style == Style::Edit {
            let _ = SendMessageW(
                control,
                0x00c5,
                WPARAM(if c.id == ACC_EDIT { 32 } else { 256 }),
                LPARAM(0),
            );
        }
        if c.style == Style::Edit {
            let cue = wide(if c.id == ACC_EDIT {
                "输入学号"
            } else {
                "输入校园网密码"
            });
            let _ = SendMessageW(control, 0x1501, WPARAM(1), LPARAM(cue.as_ptr() as isize));
        }
    }
}

unsafe fn layout(hwnd: HWND) {
    let (page, dpi, busy, diag_busy, loading, extra_check, sub_extra, info_extra, note, configured) = {
        let u = ui();
        (
            u.page,
            u.dpi,
            u.busy,
            u.diag_busy,
            u.loading,
            u.extra_check,
            u.sub_extra,
            u.info_extra,
            u.note.clone(),
            u.snapshot.configured,
        )
    };
    let note_height = feedback_height(hwnd, &note, dpi);
    let home_extra = sub_extra + if extra_check { 32 } else { 0 };
    let account_extra = if page == Page::Account && !note.is_empty() {
        note_height.saturating_sub(32)
    } else {
        0
    };
    let mut height = match page {
        Page::Home => 472 + home_extra,
        Page::Settings => 448,
        Page::Account => {
            if configured {
                592
            } else {
                540
            }
        }
        Page::Help => 596 + info_extra,
        Page::Features => 724,
    };
    height += account_extra;
    if !note.is_empty() {
        height = height.max(match page {
            Page::Settings => 440 + note_height + 24,
            Page::Help => 584 + info_extra + note_height + 18,
            Page::Features => 712 + note_height + 18,
            Page::Home => 420 + home_extra + note_height + 18,
            _ => height,
        });
    }
    let area = work_area(hwnd);
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: s(dpi, WIDTH),
        bottom: 0,
    };
    let base_style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let _ = AdjustWindowRectEx(&mut frame, base_style, false, WS_EX_CONTROLPARENT);
    let viewport_px = s(dpi, height)
        .min((area.bottom - area.top - (frame.bottom - frame.top) - 12).max(s(dpi, 220)));
    let viewport = viewport_px * 96 / dpi;
    let scroll = {
        let mut u = ui();
        u.viewport = viewport;
        u.content_height = height;
        u.scroll = u.scroll.clamp(0, (height - viewport).max(0));
        u.scroll
    };
    for c in controls() {
        let visible = match c.id {
            BACK | PAGE_TITLE => page != Page::Home,
            NOTE => !note.is_empty(),
            FOOT => page == Page::Home && note.is_empty(),
            DELETE => page == Page::Account && configured,
            CHECK_NETWORK => page == Page::Home && extra_check,
            _ => c.page == Some(page),
        };
        let control = child(hwnd, c.id);
        let _ = ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE });
        let (x, mut y, w, mut h) = c.rect;
        if c.id == NOTE {
            h = note_height;
            y = match page {
                Page::Home => 420 + home_extra,
                Page::Account => 397,
                Page::Settings => 440,
                Page::Help => 584 + info_extra,
                Page::Features => 712,
            };
        }
        if page == Page::Home {
            if c.id == SUB {
                h += sub_extra;
            }
            if c.id == CHECK_NETWORK {
                y += sub_extra;
            }
            if matches!(c.id, ACCOUNT_CAPTION | ACCOUNT_VALUE | OP_VALUE | FOOT) {
                y += home_extra;
            }
        }
        if page == Page::Help {
            if c.id == INFO_SUB {
                h += info_extra;
            }
            if (INFO_FIELD_START..INFO_FIELD_END).contains(&c.id)
                || matches!(c.id, DIAG | DIAG_HINT | VERSION)
            {
                y += info_extra;
            }
        }
        if page == Page::Account && matches!(c.id, SAVE | PRIVACY | DELETE) {
            y += account_extra;
        }
        let _ = MoveWindow(
            control,
            s(dpi, x),
            s(dpi, y - scroll),
            s(dpi, w),
            s(dpi, h),
            true,
        );
        // EnableWindow 可同步触发重绘消息，调用时不能持有 UI 锁。
        let _ = EnableWindow(
            control,
            !busy && !(c.id == DIAG && diag_busy) && !(c.id == PRIMARY && loading),
        );
        if c.style == Style::Power {
            let region = CreateEllipticRgn(0, 0, s(dpi, w), s(dpi, h));
            if SetWindowRgn(control, region, true) == 0 {
                let _ = DeleteObject(region);
            }
        }
    }
    text(
        hwnd,
        PAGE_TITLE,
        match page {
            Page::Settings => "设置",
            Page::Account => "账号设置",
            Page::Help => "网络信息",
            Page::Features => "功能介绍",
            _ => "",
        },
    );
    text(hwnd, NOTE, &note);
    if page == Page::Settings && !configured {
        for id in [AUTO_CONNECT, AUTO_START] {
            let _ = EnableWindow(child(hwnd, id), false);
        }
    }
    let overflow = viewport < height;
    let _ = ShowScrollBar(hwnd, SB_VERT, overflow);
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: height - 1,
        nPage: viewport as u32,
        nPos: scroll,
        ..Default::default()
    };
    let _ = SetScrollInfo(hwnd, SB_VERT, &info, true);
    let style = base_style
        | if overflow {
            WS_VSCROLL
        } else {
            WINDOW_STYLE(0)
        };
    let mut r = RECT {
        left: 0,
        top: 0,
        right: s(dpi, WIDTH),
        bottom: viewport_px,
    };
    let _ = AdjustWindowRectEx(&mut r, style, false, WS_EX_CONTROLPARENT);
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        r.right - r.left,
        r.bottom - r.top,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let mut window = RECT::default();
    let _ = GetWindowRect(hwnd, &mut window);
    let x = window.left.clamp(
        area.left,
        (area.right - (window.right - window.left)).max(area.left),
    );
    let y = window.top.clamp(
        area.top,
        (area.bottom - (window.bottom - window.top)).max(area.top),
    );
    if x != window.left || y != window.top {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let _ = InvalidateRect(hwnd, None, true);
}

unsafe fn work_area(hwnd: HWND) -> RECT {
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        return info.rcWork;
    }
    let mut area = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut area as *mut _ as *mut _),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    area
}

unsafe fn feedback_height(hwnd: HWND, note: &str, dpi: i32) -> i32 {
    let font = ui().gfx.small;
    measured_height(hwnd, note, dpi, font, 112, 332)
}

unsafe fn measured_height(
    hwnd: HWND,
    note: &str,
    dpi: i32,
    font: HFONT,
    max: i32,
    width: i32,
) -> i32 {
    if note.is_empty() {
        return 32;
    }
    let dc = GetDC(hwnd);
    let old = SelectObject(dc, font);
    let mut bounds = RECT {
        left: 0,
        top: 0,
        right: s(dpi, width),
        bottom: 0,
    };
    let mut value: Vec<u16> = note.encode_utf16().collect();
    let _ = DrawTextW(
        dc,
        &mut value,
        &mut bounds,
        DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
    );
    SelectObject(dc, old);
    let _ = ReleaseDC(hwnd, dc);
    (((bounds.bottom - bounds.top) * 96 + dpi - 1) / dpi + 2).clamp(32, max)
}

unsafe fn ensure_visible(hwnd: HWND, id: i32) -> bool {
    let mut bounds = RECT::default();
    if GetWindowRect(child(hwnd, id), &mut bounds).is_err() {
        return false;
    }
    let mut origin = POINT {
        x: bounds.left,
        y: bounds.top,
    };
    let _ = ScreenToClient(hwnd, &mut origin);
    let mut u = ui();
    let top = origin.y * 96 / u.dpi + u.scroll;
    let height = (bounds.bottom - bounds.top) * 96 / u.dpi;
    let old = u.scroll;
    if top < old {
        u.scroll = (top - 12).max(0);
    } else if top + height > old + u.viewport {
        u.scroll = top + height - u.viewport + 12;
    }
    u.scroll != old
}

unsafe fn page(hwnd: HWND, target: Page) {
    let hide_password = {
        let u = ui();
        u.page == Page::Account && target != Page::Account && u.shown
    };
    if hide_password {
        ui().shown = false;
        let _ = SendMessageW(child(hwnd, PWD_EDIT), 0x00cc, WPARAM(0x25cf), LPARAM(0));
        text(hwnd, PWD_SHOW, "显示");
    }

    {
        let mut u = ui();
        u.page = target;
        u.scroll = 0;
        u.note.clear();
        u.note_error = false;
        u.note_until = None;
        u.invalid_field = None;
    }
    if target == Page::Help {
        update_help(hwnd);
    }
    if target == Page::Account {
        update_save_label(hwnd);
    }
    layout(hwnd);
    if target == Page::Account {
        let _ = SetFocus(child(hwnd, ACC_EDIT));
    }
}

unsafe fn apply_snapshot(hwnd: HWND, mut snap: Snapshot) {
    let preview = cfg!(debug_assertions) && ui().preview;
    if !preview {
        snap.state = config::load_state();
        snap.startup = autostart::enabled();
    }
    let view = ui_model::present(
        &snap.status,
        snap.configured,
        snap.running,
        snap.state.reconnect_paused,
        snap.state.no_internet,
        &snap.state.last_error,
        &snap.account,
    )
    .check_operator(
        &snap.status,
        &snap.account,
        &snap.operator,
        snap.state.reconnect_paused,
    );
    let font = ui().gfx.body;
    let dpi = ui().dpi;
    let sub_extra = measured_height(hwnd, &view.subtitle, dpi, font, 144, 332).saturating_sub(42);
    let resize = {
        let mut u = ui();
        let resize = u.loading || u.extra_check != view.network_check || u.sub_extra != sub_extra;
        u.extra_check = view.network_check;
        u.sub_extra = sub_extra;
        u.action = view.action;
        u.tone = view.tone;
        u.snapshot = snap.clone();
        u.loading = false;
        resize
    };
    text(hwnd, NETWORK, &snap.name);
    text(hwnd, TITLE, view.title);
    text(hwnd, SUB, &view.subtitle);
    let footer = if snap.name == "有线网络" {
        view.footer.replace("Wi-Fi", "网线")
    } else {
        view.footer.to_string()
    };
    text(hwnd, FOOT, &footer);
    if !snap.configured {
        if let portal::Status::Online(session) = &snap.status {
            if get(hwnd, ACC_EDIT).trim().is_empty() {
                let (account, operator) = portal::split_account(&session.account);
                if !account.is_empty() {
                    let focus_account = GetFocus() == child(hwnd, ACC_EDIT);
                    ui().initializing_inputs = true;
                    text(hwnd, ACC_EDIT, &account);
                    {
                        ui().op = op_index(&operator);
                    }
                    ui().initializing_inputs = false;
                    update_save_label(hwnd);
                    if focus_account {
                        let _ = SetFocus(child(hwnd, PWD_EDIT));
                    }
                }
            }
        }
    }
    if !ui().busy {
        text(hwnd, PRIMARY, view.button);
    }
    let (display_account, display_operator) = match &snap.status {
        portal::Status::Online(session) => portal::split_account(&session.account),
        _ => (snap.account.clone(), snap.operator.clone()),
    };
    let masked = portal::mask(&display_account);
    text(
        hwnd,
        ACCOUNT_VALUE,
        if display_account.is_empty() {
            "尚未设置"
        } else {
            &masked
        },
    );
    text(
        hwnd,
        OP_VALUE,
        if display_account.is_empty() {
            ""
        } else {
            op_label(&display_operator)
        },
    );
    text(
        hwnd,
        ACCOUNT_ROW,
        &format!(
            "账号设置\n{}",
            if snap.configured {
                format!(
                    "{} · {}",
                    portal::mask(&snap.account),
                    op_label(&snap.operator)
                )
            } else {
                "添加校园网账号".into()
            }
        ),
    );
    text(
        hwnd,
        AUTO_CONNECT,
        if snap.state.reconnect_paused {
            "自动登录\n已关闭，不影响当前连接"
        } else if snap.state.paused_until > portal::unix_now() {
            "自动登录\n已开启，账号异常时暂缓重试"
        } else {
            "自动登录\n已开启，掉线后自动恢复"
        },
    );
    text(
        hwnd,
        AUTO_START,
        if snap.startup {
            "开机自动运行\n已开启，登录 Windows 后运行"
        } else {
            "开机自动运行\n已关闭，需要时手动打开"
        },
    );
    if ui().page == Page::Help {
        update_help(hwnd);
    }
    let _ = InvalidateRect(hwnd, None, false);
    for id in [AUTO_CONNECT, AUTO_START, ACCOUNT_ROW, PRIMARY] {
        let _ = InvalidateRect(child(hwnd, id), None, false);
    }
    apply_busy_label(hwnd);
    if resize {
        layout(hwnd);
    }
}

fn probe() -> Snapshot {
    let revision = REFRESH_EPOCH.current();
    let (cfg, pwd) = config::load_config();
    let configured = !pwd.is_empty();
    let mut running = agent::running();
    let state = config::load_state();
    if configured
        && !running
        && !state.reconnect_paused
        && !SAVING.load(Ordering::SeqCst)
        && !CLOSED.load(Ordering::SeqCst)
    {
        let now = portal::unix_now() as u64;
        if now.saturating_sub(LAST_START.load(Ordering::SeqCst)) > 10 {
            LAST_START.store(now, Ordering::SeqCst);
            agent::spawn_daemon();
            std::thread::sleep(Duration::from_millis(300));
            running = agent::running();
        }
    }
    let name = network::current_name();
    Snapshot {
        revision,
        status: portal::status(Duration::from_secs(4)).0,
        name,
        account: cfg.account,
        operator: cfg.operator,
        configured,
        running,
        startup: autostart::enabled(),
        state: config::load_state(),
    }
}
unsafe fn start_probe(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        while !CLOSED.load(Ordering::SeqCst) {
            let snapshot = probe();
            let ptr = Box::into_raw(Box::new(snapshot));
            if PostMessageW(
                HWND(raw as *mut _),
                WM_SNAPSHOT,
                WPARAM(0),
                LPARAM(ptr as isize),
            )
            .is_err()
            {
                let _ = Box::from_raw(ptr);
                break;
            }
            for _ in 0..15 {
                if CLOSED.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

struct Done {
    result: Result<(), String>,
    next: Option<Page>,
    message: String,
    status: Option<portal::Status>,
    busy_id: i32,
    previous_label: String,
}
unsafe fn job(
    hwnd: HWND,
    label: &str,
    next: Option<Page>,
    success: &str,
    refresh_network: bool,
    f: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    if ui().busy {
        return;
    }
    if cfg!(debug_assertions) && ui().preview {
        {
            let mut u = ui();
            u.note = "预览模式，未执行网络或账号操作".into();
        }
        if let Some(p) = next {
            page(hwnd, p);
        } else {
            layout(hwnd);
        }
        return;
    }
    let busy_id = busy_target(ui().page, GetDlgCtrlID(GetFocus()));
    let previous_label = if busy_id > 0 {
        get(hwnd, busy_id)
    } else {
        String::new()
    };
    REFRESH_EPOCH.advance();
    {
        let mut u = ui();
        u.busy = true;
        u.note = label.into();
        u.note_error = false;
        u.note_until = None;
        u.busy_id = busy_id;
        u.busy_label = label.into();
    }
    text(hwnd, PRIMARY, label);
    text(hwnd, SAVE, label);
    layout(hwnd);
    apply_busy_label(hwnd);
    let raw = hwnd.0 as isize;
    let message = success.to_string();
    std::thread::spawn(move || {
        let result = f();
        let status = if refresh_network {
            Some(
                portal::status_quick(Duration::from_secs(2)).unwrap_or(portal::Status::Unreachable),
            )
        } else {
            None
        };
        let done = Done {
            result,
            next,
            message,
            status,
            busy_id,
            previous_label,
        };
        let ptr = Box::into_raw(Box::new(done));
        if PostMessageW(
            HWND(raw as *mut _),
            WM_DONE,
            WPARAM(0),
            LPARAM(ptr as isize),
        )
        .is_err()
        {
            let _ = Box::from_raw(ptr);
        }
    });
}

fn busy_target(page: Page, focused: i32) -> i32 {
    if [
        PRIMARY,
        SAVE,
        AUTO_CONNECT,
        AUTO_START,
        CHECK_NETWORK,
        DELETE,
    ]
    .contains(&focused)
    {
        focused
    } else if page == Page::Account {
        SAVE
    } else {
        PRIMARY
    }
}

unsafe fn apply_busy_label(hwnd: HWND) {
    let (busy, id, label) = {
        let u = ui();
        (u.busy, u.busy_id, u.busy_label.clone())
    };
    if !busy || id <= 0 {
        return;
    }
    let Some(control) = controls().into_iter().find(|c| c.id == id) else {
        return;
    };
    if matches!(control.style, Style::Row | Style::Toggle) {
        text(
            hwnd,
            id,
            &format!(
                "{}\n{}",
                control.text.split('\n').next().unwrap_or(""),
                label
            ),
        );
    } else {
        text(hwnd, id, &label);
    }
}

unsafe fn inputs_changed(hwnd: HWND) -> bool {
    let (old, pwd) = if cfg!(debug_assertions) && ui().preview {
        (
            config::Config {
                account: "20241234".into(),
                operator: "mobile".into(),
                ..Default::default()
            },
            "preview-only".into(),
        )
    } else {
        config::load_config()
    };
    ui_model::account_changed(
        &old.account,
        &pwd,
        &old.operator,
        &get(hwnd, ACC_EDIT),
        &get(hwnd, PWD_EDIT),
        op_value(ui().op),
    )
}
unsafe fn update_save_label(hwnd: HWND) {
    let (busy, configured) = {
        let u = ui();
        (u.busy, u.snapshot.configured)
    };
    if !busy {
        let unchanged = configured && !inputs_changed(hwnd);
        text(
            hwnd,
            SAVE,
            if unchanged {
                "完成"
            } else {
                "保存并连接"
            },
        );
    }
}
unsafe fn load_inputs(hwnd: HWND) {
    let (cfg, pwd) = if cfg!(debug_assertions) && ui().preview {
        if ui().snapshot.configured {
            (
                config::Config {
                    account: "20241234".into(),
                    operator: "mobile".into(),
                    ..Default::default()
                },
                "preview-only".into(),
            )
        } else {
            (config::Config::default(), String::new())
        }
    } else {
        config::load_config()
    };
    {
        let mut u = ui();
        u.op = op_index(&cfg.operator);
        u.shown = false;
        u.initializing_inputs = true;
    }
    text(hwnd, ACC_EDIT, &cfg.account);
    text(hwnd, PWD_EDIT, &pwd);
    let _ = SendMessageW(child(hwnd, PWD_EDIT), 0x00cc, WPARAM(0x25cf), LPARAM(0));
    text(hwnd, PWD_SHOW, "显示");
    {
        let mut u = ui();
        u.initializing_inputs = false;
        u.draft_touched = false;
    }
    update_save_label(hwnd);
}

unsafe fn save_account(hwnd: HWND) {
    if ui().busy {
        return;
    }
    let account = get(hwnd, ACC_EDIT).trim().to_string();
    let password = get(hwnd, PWD_EDIT);
    let operator = op_value(ui().op).to_string();
    let configured = ui().snapshot.configured;
    if configured && !inputs_changed(hwnd) {
        page(hwnd, Page::Home);
        return;
    }
    let error = if account.len() < 4
        || account.len() > 20
        || !account.chars().all(|c| c.is_ascii_alphanumeric())
    {
        Some((ACC_EDIT, "学号需为 4–20 位字母或数字"))
    } else if password.is_empty() {
        Some((PWD_EDIT, "请输入校园网密码"))
    } else if password.contains(';') {
        Some((PWD_EDIT, "门户不支持包含分号的密码"))
    } else {
        None
    };
    if let Some((id, msg)) = error {
        {
            let mut u = ui();
            u.note = msg.into();
            u.note_error = true;
            u.note_until = None;
            u.invalid_field = Some(id);
        }
        layout(hwnd);
        let _ = SetFocus(child(hwnd, id));
        return;
    }
    let (online, preview) = {
        let u = ui();
        (
            matches!(u.snapshot.status, portal::Status::Online(_)),
            u.preview,
        )
    };
    if online
        && !preview
        && MessageBoxW(
            hwnd,
            w!("保存后会短暂断网，并使用新设置重新连接"),
            w!("保存并重新连接"),
            MB_OKCANCEL | MB_ICONQUESTION,
        ) != IDOK
    {
        return;
    }
    if preview {
        page(hwnd, Page::Home);
        return;
    }
    SAVING.store(true, Ordering::SeqCst);
    job(
        hwnd,
        "正在保存并连接…",
        Some(Page::Home),
        "账号已更新",
        true,
        move || {
            let mut was_paused = false;
            config::try_update_state(|s| {
                was_paused = s.reconnect_paused;
                s.reconnect_paused = true;
            })
            .map_err(|e| e.to_string())?;
            let result = (|| {
                let _connection = portal::connection_lock()?;
                let (status, port) = portal::status(Duration::from_secs(4));
                match status {
                    portal::Status::Unreachable => {
                        return Err("请先连接学校网络，再保存账号".into())
                    }
                    portal::Status::Online(_) => {
                        if !online {
                            return Err("当前已恢复连接，请再次保存以确认重新连接".into());
                        }
                        if !portal::logout(port) {
                            return Err("当前连接未能断开，原账号设置已保留".into());
                        }
                    }
                    portal::Status::Offline => {}
                }
                portal::login(&account, &password, &operator, port)
                    .map_err(|e| ui_model::clean_copy(&portal::friendly_login_error(&e)))?;
                let (old, old_pwd) = config::load_config();
                let first = old_pwd.is_empty();
                let cfg = config::Config {
                    account,
                    operator,
                    keepalive: if first { true } else { old.keepalive },
                    keepalive_min: if old.keepalive_min == 0 {
                        15
                    } else {
                        old.keepalive_min.max(5)
                    },
                    wifi_self_heal: if first { true } else { old.wifi_self_heal },
                    ssid: if old.ssid.trim().is_empty() {
                        crate::windows_wifi::DEFAULT_SSID.to_string()
                    } else {
                        old.ssid.clone()
                    },
                    password_enc: String::new(),
                };
                config::save_config(&cfg, &password).map_err(|e| format!("账号保存失败：{}", e))?;
                config::try_update_state(|s| {
                    s.reconnect_paused = false;
                    s.paused_until = 0.0;
                    s.last_error.clear();
                })
                .map_err(|e| e.to_string())?;
                if first {
                    autostart::enable();
                }
                if !agent::running() {
                    agent::spawn_daemon();
                }
                Ok(())
            })();
            if result.is_err() {
                config::update_state(|s| s.reconnect_paused = was_paused);
            }
            result
        },
    );
}

unsafe fn toggle_pause(hwnd: HWND) {
    let pause = !ui().snapshot.state.reconnect_paused;
    if cfg!(debug_assertions) && ui().preview {
        let mut snap = ui().snapshot.clone();
        snap.state.reconnect_paused = pause;
        apply_snapshot(hwnd, snap);
        return;
    }
    job(
        hwnd,
        if pause {
            "正在关闭…"
        } else {
            "正在恢复…"
        },
        None,
        if pause {
            "已关闭自动登录，当前连接保持不变"
        } else {
            "已开启自动登录"
        },
        false,
        move || agent::pause_automatic(pause),
    );
}

unsafe fn disconnect(hwnd: HWND) {
    if cfg!(debug_assertions) && ui().preview {
        let mut snap = ui().snapshot.clone();
        snap.status = portal::Status::Offline;
        snap.state.reconnect_paused = true;
        snap.state.no_internet = false;
        snap.state.last_error.clear();
        apply_snapshot(hwnd, snap);
        return;
    }
    job(
        hwnd,
        "正在断开…",
        Some(Page::Home),
        "已断开校园网，自动登录已关闭",
        true,
        || {
            if agent::logout_now() {
                Ok(())
            } else if config::load_state().reconnect_paused {
                Err("自动登录已关闭，暂未确认认证是否断开".into())
            } else {
                Err("未能断开校园网，请稍后再试".into())
            }
        },
    );
}

unsafe fn connect(hwnd: HWND) {
    let check_internet = {
        let u = ui();
        u.snapshot.state.no_internet && matches!(u.snapshot.status, portal::Status::Online(_))
    };
    if cfg!(debug_assertions) && ui().preview {
        let mut snap = ui().snapshot.clone();
        snap.status = portal::Status::Online(portal::Session {
            account: format!("{}@{}", snap.account, snap.operator),
            ip: "10.1.2.3".into(),
            login_time: String::new(),
        });
        if !check_internet {
            snap.state.reconnect_paused = false;
        }
        snap.state.no_internet = false;
        snap.state.last_error.clear();
        apply_snapshot(hwnd, snap);
        return;
    }
    job(
        hwnd,
        if check_internet {
            "正在检查…"
        } else {
            "正在连接…"
        },
        Some(Page::Home),
        "连接检查已完成",
        true,
        move || {
            if check_internet {
                let ok = portal::internet_ok();
                config::update_state(|s| {
                    s.no_internet = !ok;
                    if ok {
                        s.net_bad = 0;
                    }
                });
                if !ok {
                    return Err("校园网已认证，外网暂时无法访问".into());
                }
                return Ok(());
            }
            agent::connect_now().map(|_| ())
        },
    );
}

unsafe fn command(hwnd: HWND, id: i32) {
    if ui().busy {
        return;
    }
    match id {
        SETTINGS => page(hwnd, Page::Settings),
        BACK => {
            let (current, configured) = {
                let u = ui();
                (u.page, u.snapshot.configured)
            };
            let target = if current == Page::Settings || !configured {
                Page::Home
            } else {
                Page::Settings
            };
            page(hwnd, target);
        }
        ACCOUNT_ROW => page(hwnd, Page::Account),
        PRIMARY => {
            let action = ui().action;
            match action {
                Action::Account => page(hwnd, Page::Account),
                Action::Connect => connect(hwnd),
                Action::Disconnect => disconnect(hwnd),
            }
        }
        SAVE => save_account(hwnd),
        SEG0 | SEG1 | SEG2 => {
            {
                let mut u = ui();
                if u.op != id - SEG0 {
                    u.draft_touched = true;
                }
                u.op = id - SEG0;
            }
            update_save_label(hwnd);
            for c in [SEG0, SEG1, SEG2] {
                let _ = InvalidateRect(child(hwnd, c), None, false);
            }
        }
        PWD_SHOW => {
            let shown = {
                let mut u = ui();
                u.shown = !u.shown;
                u.shown
            };
            let _ = SendMessageW(
                child(hwnd, PWD_EDIT),
                0x00cc,
                WPARAM(if shown { 0 } else { 0x25cf }),
                LPARAM(0),
            );
            text(hwnd, PWD_SHOW, if shown { "隐藏" } else { "显示" });
            let _ = InvalidateRect(child(hwnd, PWD_EDIT), None, true);
        }
        AUTO_CONNECT => toggle_pause(hwnd),
        AUTO_START => {
            let enable = !ui().snapshot.startup;
            if cfg!(debug_assertions) && ui().preview {
                let mut snap = ui().snapshot.clone();
                snap.startup = enable;
                apply_snapshot(hwnd, snap);
                return;
            }
            job(
                hwnd,
                "正在更新…",
                None,
                if enable {
                    "已开启开机运行"
                } else {
                    "已关闭开机运行，当前自动连接不受影响"
                },
                false,
                move || {
                    if enable {
                        autostart::enable();
                    } else {
                        autostart::disable();
                    }
                    if autostart::enabled() == enable {
                        Ok(())
                    } else {
                        Err("未能更改开机运行设置，请重试".into())
                    }
                },
            );
        }
        CHECK_NETWORK => connect(hwnd),
        HELP => page(hwnd, Page::Help),
        FEATURES => page(hwnd, Page::Features),
        DIAG => {
            let can_export = {
                let u = ui();
                !u.preview && !u.diag_busy
            };
            if can_export {
                ui().diag_busy = true;
                text(hwnd, DIAG, "正在导出…");
                let _ = EnableWindow(child(hwnd, DIAG), false);
                crate::export_diag(hwnd);
            }
        }
        DELETE => {
            let message = "删除本机保存的账号密码，并关闭自动登录";
            if cfg!(debug_assertions) && ui().preview {
                return;
            }
            let message = wide(message);
            if MessageBoxW(
                hwnd,
                PCWSTR(message.as_ptr()),
                w!("删除保存的账号"),
                MB_OKCANCEL | MB_ICONQUESTION,
            ) != IDOK
            {
                return;
            }
            SAVING.store(true, Ordering::SeqCst);
            job(
                hwnd,
                "正在删除…",
                Some(Page::Account),
                "已删除保存的账号",
                false,
                move || {
                    config::try_update_state(|s| s.reconnect_paused = true)
                        .map_err(|e| e.to_string())?;
                    autostart::disable();
                    if autostart::enabled() {
                        return Err("未能关闭开机运行，请重试".into());
                    }
                    agent::stop_daemon();
                    let _connection = portal::connection_lock()?;
                    match std::fs::remove_file(config::cfg_path()) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.to_string()),
                    }
                    Ok(())
                },
            );
        }
        _ => {}
    }
}

unsafe fn update_help(hwnd: HWND) {
    let (snap, loading, dpi, font) = {
        let u = ui();
        (u.snapshot.clone(), u.loading, u.dpi, u.gfx.small)
    };
    let summary = ui_model::network_summary(
        &snap.status,
        snap.configured,
        snap.state.reconnect_paused,
        snap.state.no_internet,
        &snap.state.last_error,
        &snap.account,
        loading,
    );
    let extra = measured_height(hwnd, &summary.description, dpi, font, 112, 300).saturating_sub(32);
    let resize = {
        let mut u = ui();
        let changed = u.info_extra != extra;
        u.info_extra = extra;
        u.info_tone = summary.tone;
        changed
    };
    text(hwnd, INFO_TITLE, summary.title);
    text(hwnd, INFO_SUB, &summary.description);
    let (account, operator, ip, duration) = match &snap.status {
        portal::Status::Online(session) => {
            let (account, operator) = portal::split_account(&session.account);
            (
                account,
                operator,
                if session.ip.is_empty() {
                    "暂不可读".into()
                } else {
                    session.ip.clone()
                },
                portal::online_minutes(&session.login_time)
                    .map(portal::fmt_duration)
                    .unwrap_or_else(|| "暂不可读".into()),
            )
        }
        _ => (
            snap.account.clone(),
            snap.operator.clone(),
            "—".into(),
            "—".into(),
        ),
    };
    let values = [
        snap.name,
        if account.is_empty() {
            "尚未设置".into()
        } else {
            portal::mask(&account)
        },
        if account.is_empty() {
            "—".into()
        } else {
            op_label(&operator).into()
        },
        ip,
        duration,
    ];
    for (i, value) in values.iter().enumerate() {
        text(hwnd, INFO_FIELD_START + i as i32 * 2 + 1, value);
    }
    text(
        hwnd,
        VERSION,
        &format!("校园网自动登录  v{}", crate::VERSION),
    );
    if resize {
        layout(hwnd);
    }
}

unsafe fn rounded(hdc: HDC, r: RECT, radius: i32, fill: u32, border: u32) {
    let brush = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(
        PS_SOLID,
        if matches!(border, BLUE | RED) { 2 } else { 1 },
        COLORREF(border),
    );
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}
unsafe fn draw_text(
    hdc: HDC,
    value: &str,
    mut rect: RECT,
    font: HFONT,
    color: u32,
    flags: DRAW_TEXT_FORMAT,
) {
    let old = SelectObject(hdc, font);
    let _ = SetTextColor(hdc, COLORREF(color));
    let _ = SetBkMode(hdc, TRANSPARENT);
    let mut v: Vec<u16> = value.encode_utf16().collect();
    let _ = DrawTextW(hdc, &mut v, &mut rect, flags | DT_NOPREFIX);
    SelectObject(hdc, old);
}

unsafe fn draw_power(dis: &DRAWITEMSTRUCT) {
    let (dpi, gfx, tone, action, active, spin) = {
        let u = ui();
        (u.dpi, u.gfx, u.tone, u.action, u.busy || u.loading, u.spin)
    };
    let r = dis.rcItem;
    let focused = dis.itemState.0 & 0x0010 != 0;
    let pressed = dis.itemState.0 & 0x0001 != 0;
    let hover = ui().hover == PRIMARY;
    let fg = if active || tone == Tone::Neutral {
        BLUE
    } else {
        color(tone)
    };
    let fill = if active {
        SOFT
    } else {
        match tone {
            Tone::Good => {
                if hover || pressed {
                    0x00d5ecd9
                } else {
                    0x00e5f4e8
                }
            }
            Tone::Warning => {
                if hover || pressed {
                    0x00cee6fa
                } else {
                    0x00dff1fc
                }
            }
            Tone::Error => {
                if hover || pressed {
                    0x00d9dcfa
                } else {
                    0x00ebebfd
                }
            }
            Tone::Neutral => {
                if hover || pressed {
                    0x00f6e6dc
                } else {
                    0x00fcf0e8
                }
            }
        }
    };
    FillRect(dis.hDC, &r, gfx.bg);
    // 焦点使用圆环，点击区域也由椭圆窗口区域限定。
    rounded(
        dis.hDC,
        RECT {
            left: r.left + s(dpi, 2),
            top: r.top + s(dpi, 2),
            right: r.right - s(dpi, 2),
            bottom: r.bottom - s(dpi, 2),
        },
        s(dpi, 136),
        fill,
        if focused { BLUE } else { fill },
    );
    let cx = (r.left + r.right) / 2;
    let cy = r.top + s(dpi, 52 + if pressed { 1 } else { 0 });
    let radius = s(dpi, 20);
    if !active && action == Action::Account {
        draw_text(
            dis.hDC,
            if tone == Tone::Neutral { "+" } else { "!" },
            RECT {
                top: cy - s(dpi, 26),
                bottom: cy + s(dpi, 25),
                ..r
            },
            gfx.title,
            fg,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    } else {
        let pen = CreatePen(PS_SOLID, s(dpi, 2), COLORREF(fg));
        let old = SelectObject(dis.hDC, pen);
        let (start, end) = if active {
            let angle = (spin as f64).to_radians();
            let p = |a: f64| {
                (
                    cx + (radius as f64 * a.cos()) as i32,
                    cy + (radius as f64 * a.sin()) as i32,
                )
            };
            (p(angle), p(angle + std::f64::consts::FRAC_PI_2))
        } else {
            (
                (cx - s(dpi, 13), cy - s(dpi, 16)),
                (cx + s(dpi, 13), cy - s(dpi, 16)),
            )
        };
        let _ = Arc(
            dis.hDC,
            cx - radius,
            cy - radius,
            cx + radius,
            cy + radius,
            start.0,
            start.1,
            end.0,
            end.1,
        );
        if !active {
            let _ = MoveToEx(dis.hDC, cx, cy - s(dpi, 25), None);
            let _ = LineTo(dis.hDC, cx, cy - s(dpi, 2));
        }
        SelectObject(dis.hDC, old);
        let _ = DeleteObject(pen);
    }
    let value = get(GetParent(dis.hwndItem).unwrap_or_default(), PRIMARY);
    draw_text(
        dis.hDC,
        &value,
        RECT {
            left: r.left + s(dpi, 9),
            right: r.right - s(dpi, 9),
            top: r.top + s(dpi, 93),
            bottom: r.top + s(dpi, 119),
        },
        gfx.button,
        fg,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
}

unsafe fn draw_button(dis: &DRAWITEMSTRUCT) {
    let c = controls().into_iter().find(|c| c.id == dis.CtlID as i32);
    let Some(c) = c else {
        return;
    };
    if c.style == Style::Power {
        draw_power(dis);
        return;
    }
    let u = ui();
    let dpi = u.dpi;
    let r = dis.rcItem;
    let enabled = dis.itemState.0 & 0x0004 == 0;
    let pressed = dis.itemState.0 & 0x0001 != 0;
    let hover = u.hover == c.id;
    let mut fill = WHITE;
    let mut border = LINE;
    let mut fg = TEXT;
    match c.style {
        Style::Primary => {
            fill = if !enabled {
                0x00d7cbbd
            } else if pressed {
                0x00c64f1d
            } else if hover {
                0x00db5820
            } else {
                BLUE
            };
            border = fill;
            fg = WHITE;
        }
        Style::Link => {
            fill = if hover { SOFT } else { BG };
            border = fill;
            fg = if c.id == DELETE { MUTED } else { BLUE };
        }
        Style::Segment => {
            let selected = u.op == c.id - SEG0;
            fill = if selected { WHITE } else { SOFT };
            border = if selected { LINE } else { SOFT };
            fg = if selected { BLUE } else { MUTED };
        }
        _ => {
            if hover {
                fill = 0x00fcfaf9;
            }
        }
    }
    if !enabled && c.style != Style::Primary {
        fg = 0x00b5aaa0;
    }
    rounded(dis.hDC, r, s(dpi, 10), fill, border);
    let n = GetWindowTextLengthW(dis.hwndItem);
    let mut buf = vec![0u16; n.max(0) as usize + 1];
    let len = GetWindowTextW(dis.hwndItem, &mut buf);
    let value = String::from_utf16_lossy(&buf[..len as usize]);
    if matches!(c.style, Style::Row | Style::Toggle) {
        let mut lines = value.split('\n');
        let first = lines.next().unwrap_or("");
        let second = lines.next();
        let tx = RECT {
            left: r.left + s(dpi, 16),
            top: r.top + if second.is_some() { s(dpi, 12) } else { 0 },
            right: r.right - s(dpi, 62),
            bottom: if second.is_some() {
                r.top + s(dpi, 33)
            } else {
                r.bottom
            },
        };
        draw_text(
            dis.hDC,
            first,
            tx,
            u.gfx.button,
            fg,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        if let Some(second) = second {
            draw_text(
                dis.hDC,
                second,
                RECT {
                    top: r.top + s(dpi, 36),
                    bottom: r.bottom - s(dpi, 8),
                    ..tx
                },
                u.gfx.small,
                MUTED,
                DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        if c.style == Style::Toggle {
            let on = if c.id == AUTO_CONNECT {
                !u.snapshot.state.reconnect_paused
            } else {
                u.snapshot.startup
            };
            let x = r.right - s(dpi, 54);
            let y = (r.top + r.bottom) / 2 - s(dpi, 11);
            rounded(
                dis.hDC,
                RECT {
                    left: x,
                    top: y,
                    right: x + s(dpi, 38),
                    bottom: y + s(dpi, 22),
                },
                s(dpi, 22),
                if on { BLUE } else { 0x00c9c0b8 },
                if on { BLUE } else { 0x00c9c0b8 },
            );
            rounded(
                dis.hDC,
                RECT {
                    left: x + s(dpi, if on { 19 } else { 3 }),
                    top: y + s(dpi, 3),
                    right: x + s(dpi, if on { 35 } else { 19 }),
                    bottom: y + s(dpi, 19),
                },
                s(dpi, 16),
                WHITE,
                WHITE,
            );
        } else {
            draw_text(
                dis.hDC,
                "›",
                RECT {
                    left: r.right - s(dpi, 38),
                    right: r.right - s(dpi, 12),
                    ..r
                },
                u.gfx.heading,
                MUTED,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
    } else {
        draw_text(
            dis.hDC,
            &value,
            r,
            u.gfx.button,
            fg,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if dis.itemState.0 & 0x0010 != 0 {
        let focus = RECT {
            left: r.left + s(dpi, 4),
            top: r.top + s(dpi, 4),
            right: r.right - s(dpi, 4),
            bottom: r.bottom - s(dpi, 4),
        };
        let _ = DrawFocusRect(dis.hDC, &focus);
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let u = ui();
    let dpi = u.dpi;
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    FillRect(hdc, &r, u.gfx.bg);
    let rect = |x, y, w, h| RECT {
        left: s(dpi, x),
        top: s(dpi, y - u.scroll),
        right: s(dpi, x + w),
        bottom: s(dpi, y + h - u.scroll),
    };
    match u.page {
        Page::Home => {
            let extra = u.sub_extra + if u.extra_check { 32 } else { 0 };
            rounded(hdc, rect(24, 330 + extra, 332, 74), s(dpi, 12), WHITE, LINE);
        }
        Page::Help => {
            rounded(
                hdc,
                rect(24, 72, 332, 104 + u.info_extra),
                s(dpi, 12),
                WHITE,
                LINE,
            );
            rounded(
                hdc,
                rect(40, 96, 12, 12),
                s(dpi, 12),
                color(u.info_tone),
                color(u.info_tone),
            );
            rounded(
                hdc,
                rect(24, 192 + u.info_extra, 332, 216),
                s(dpi, 12),
                WHITE,
                LINE,
            );
            let pen = CreatePen(PS_SOLID, 1, COLORREF(LINE));
            let old = SelectObject(hdc, pen);
            for row in 1..5 {
                let y = s(dpi, 192 + u.info_extra + row * 40 - u.scroll);
                let _ = MoveToEx(hdc, s(dpi, 40), y, None);
                let _ = LineTo(hdc, s(dpi, 340), y);
            }
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
        Page::Account => {
            let focus = GetFocus();
            for (y, id) in [(140, ACC_EDIT), (226, PWD_EDIT)] {
                let border = if u.invalid_field == Some(id) {
                    RED
                } else if focus == child(hwnd, id) {
                    BLUE
                } else {
                    LINE
                };
                rounded(hdc, rect(24, y, 332, 46), s(dpi, 10), WHITE, border);
            }
            rounded(hdc, rect(24, 316, 332, 42), s(dpi, 10), SOFT, SOFT);
        }
        Page::Features => {
            let pen = CreatePen(PS_SOLID, 1, COLORREF(LINE));
            let old = SelectObject(hdc, pen);
            for group in 0..3 {
                let top = 100 + group * 212;
                rounded(hdc, rect(24, top, 332, 176), s(dpi, 12), WHITE, LINE);
                for row in 1..3 {
                    let y = s(dpi, top + 4 + row * 56 - u.scroll);
                    let _ = MoveToEx(hdc, s(dpi, 40), y, None);
                    let _ = LineTo(hdc, s(dpi, 340), y);
                }
            }
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
        _ => {}
    }
    drop(u);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn apply_dpi(hwnd: HWND, dpi: i32) {
    let new = make_gfx(dpi);
    let old = {
        let mut u = ui();
        u.dpi = dpi;
        std::mem::replace(&mut u.gfx, new)
    };
    for c in controls() {
        let f = control_font(&ui().gfx, c.id);
        let _ = SendMessageW(
            child(hwnd, c.id),
            WM_SETFONT,
            WPARAM(f.0 as usize),
            LPARAM(1),
        );
    }
    for f in [old.heading, old.title, old.body, old.small, old.button] {
        let _ = DeleteObject(f);
    }
    let _ = DeleteObject(old.bg);
    let _ = DeleteObject(old.white);
    layout(hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if UI.get().is_none() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DRAWITEM => {
            draw_button(&*(lp.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let hdc = HDC(wp.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lp.0 as *mut _));
            let u = ui();
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(
                hdc,
                COLORREF(match id {
                    i if (FEATURE_START..FEATURE_END).contains(&i)
                        && (i - FEATURE_START) % 2 == 1 =>
                    {
                        MUTED
                    }
                    i if (FEATURE_GROUP_START..FEATURE_GROUP_START + 3).contains(&i) => MUTED,
                    i if (INFO_FIELD_START..INFO_FIELD_END).contains(&i)
                        && (i - INFO_FIELD_START) % 2 == 0 =>
                    {
                        MUTED
                    }
                    INFO_SUB => MUTED,
                    NOTE => {
                        if u.note_error {
                            RED
                        } else {
                            MUTED
                        }
                    }
                    FOOT | SUB | PRIVACY | OP_HINT | ACCOUNT_CAPTION | OP_VALUE | VERSION
                    | DIAG_HINT => MUTED,
                    _ => TEXT,
                }),
            );
            let card = (FEATURE_START..FEATURE_END).contains(&id)
                || (INFO_FIELD_START..INFO_FIELD_END).contains(&id)
                || matches!(
                    id,
                    ACCOUNT_CAPTION
                        | ACCOUNT_VALUE
                        | OP_VALUE
                        | ACC_EDIT
                        | PWD_EDIT
                        | INFO_TITLE
                        | INFO_SUB
                );
            let _ = SetBkColor(hdc, COLORREF(if card { WHITE } else { BG }));
            LRESULT(if card {
                u.gfx.white.0 as isize
            } else {
                u.gfx.bg.0 as isize
            })
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let notification = (wp.0 >> 16) & 0xffff;
            if notification == 0 {
                command(hwnd, id);
            } else if notification == 0x0300 && matches!(id, ACC_EDIT | PWD_EDIT) {
                {
                    let mut u = ui();
                    if !u.initializing_inputs {
                        u.draft_touched = true;
                    }
                }
                update_save_label(hwnd);
                let clear = {
                    let mut u = ui();
                    if !u.busy && !u.initializing_inputs && u.note_error {
                        u.note.clear();
                        u.note_until = None;
                        u.invalid_field = None;
                        true
                    } else {
                        false
                    }
                };
                if clear {
                    layout(hwnd);
                }
            } else if notification == 0x0100 || notification == 6 {
                let _ = InvalidateRect(hwnd, None, false);
                let changed = ensure_visible(hwnd, id);
                if changed {
                    layout(hwnd);
                }
            } else if notification == 0x0200 || notification == 7 {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_VSCROLL => {
            {
                let mut u = ui();
                let delta = if msg == WM_MOUSEWHEEL {
                    -((wp.0 >> 16) as u16 as i16 as i32) / 120 * 48
                } else {
                    match wp.0 & 0xffff {
                        0 => -32,
                        1 => 32,
                        2 => -u.viewport,
                        3 => u.viewport,
                        4 | 5 => {
                            let mut info = SCROLLINFO {
                                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                                fMask: SIF_TRACKPOS,
                                ..Default::default()
                            };
                            let _ = GetScrollInfo(hwnd, SB_VERT, &mut info);
                            info.nTrackPos - u.scroll
                        }
                        6 => -u.scroll,
                        7 => u.content_height - u.viewport - u.scroll,
                        _ => 0,
                    }
                };
                u.scroll = (u.scroll + delta).clamp(0, (u.content_height - u.viewport).max(0));
            }
            layout(hwnd);
            LRESULT(0)
        }
        WM_SNAPSHOT => {
            if lp.0 != 0 {
                let snap = *Box::from_raw(lp.0 as *mut Snapshot);
                if REFRESH_EPOCH.accepts(snap.revision, ui().busy) {
                    apply_snapshot(hwnd, snap);
                }
            }
            LRESULT(0)
        }
        WM_DONE => {
            if lp.0 == 0 {
                return LRESULT(0);
            }
            let done = *Box::from_raw(lp.0 as *mut Done);
            REFRESH_EPOCH.advance();
            SAVING.store(false, Ordering::SeqCst);
            {
                ui().busy = false;
            }
            if done.busy_id > 0 {
                text(hwnd, done.busy_id, &done.previous_label);
            }
            let mut focus_after = None;
            match done.result {
                Ok(()) => {
                    if let Some(p) = done.next {
                        page(hwnd, p);
                    }
                    {
                        let mut u = ui();
                        u.note = done.message;
                        u.note_error = false;
                        u.note_until = Some(std::time::Instant::now() + Duration::from_secs(4));
                        u.invalid_field = None;
                    }
                    if done.busy_id == SAVE || done.busy_id == DELETE {
                        load_inputs(hwnd);
                    }
                }
                Err(e) => {
                    {
                        let mut u = ui();
                        u.note = ui_model::clean_copy(&e);
                        u.note_error = true;
                        u.note_until = None;
                    }
                    if ui().page == Page::Account && (e.contains("密码") || e.contains("验证码"))
                    {
                        focus_after = Some(PWD_EDIT);
                        ui().invalid_field = Some(PWD_EDIT);
                    }
                }
            }
            let mut snap = ui().snapshot.clone();
            snap.revision = REFRESH_EPOCH.current();
            if let Some(status) = done.status {
                snap.status = status;
            }
            let (cfg, pwd) = config::load_config();
            snap.account = cfg.account;
            snap.operator = cfg.operator;
            snap.configured = !pwd.is_empty();
            snap.running = agent::running();
            apply_snapshot(hwnd, snap);
            update_save_label(hwnd);
            layout(hwnd);
            if let Some(id) = focus_after {
                let _ = SetFocus(child(hwnd, id));
            }
            LRESULT(0)
        }
        WM_DIAG_RESULT => {
            ui().diag_busy = false;
            text(hwnd, DIAG, "导出诊断报告");
            let busy = ui().busy;
            let _ = EnableWindow(child(hwnd, DIAG), !busy);
            if lp.0 != 0 {
                let (ok, message) = *Box::from_raw(lp.0 as *mut (bool, String));
                let value = wide(&format!(
                    "{}\n{}",
                    if ok {
                        "诊断信息已导出"
                    } else {
                        "导出失败"
                    },
                    message
                ));
                let _ = MessageBoxW(
                    hwnd,
                    PCWSTR(value.as_ptr()),
                    w!("诊断信息"),
                    if ok {
                        MB_ICONINFORMATION
                    } else {
                        MB_ICONWARNING
                    },
                );
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            if PREVIEW_DPI.get().is_some() {
                return LRESULT(0);
            }
            let r = *(lp.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            apply_dpi(hwnd, (wp.0 & 0xffff) as i32);
            LRESULT(0)
        }
        WM_TIMER => {
            let animate = {
                let mut u = ui();
                u.spin = (u.spin + 24) % 360;
                u.page == Page::Home && (u.busy || u.loading)
            };
            if animate {
                let _ = InvalidateRect(child(hwnd, PRIMARY), None, false);
            }
            let clear_note = {
                let mut u = ui();
                if !u.busy && u.note_until.is_some_and(|t| std::time::Instant::now() >= t) {
                    u.note.clear();
                    u.note_until = None;
                    true
                } else {
                    false
                }
            };
            if clear_note {
                layout(hwnd);
            }
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let h = WindowFromPoint(p);
            let id = if GetParent(h).ok() == Some(hwnd) {
                GetDlgCtrlID(h)
            } else {
                0
            };
            let previous = {
                let mut u = ui();
                let previous = u.hover;
                u.hover = id;
                previous
            };
            if previous != id {
                for i in [previous, id] {
                    if i > 0 {
                        let _ = InvalidateRect(child(hwnd, i), None, false);
                    }
                }
            }
            LRESULT(0)
        }
        #[cfg(debug_assertions)]
        WM_CONTEXTMENU if PREVIEW.get().is_some_and(|kind| kind == "tray") => {
            crate::tray::preview_menu(hwnd);
            LRESULT(0)
        }
        WM_OPEN_SETTINGS => {
            if !ui().busy {
                page(hwnd, Page::Settings);
            }
            LRESULT(0)
        }
        WM_EXIT_APP => {
            EXIT_WITH_DAEMON.store(true, Ordering::SeqCst);
            let _ = SendMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_CLOSE => {
            if ui().busy {
                EXIT_WITH_DAEMON.store(false, Ordering::SeqCst);
                {
                    ui().note = "正在处理连接，请稍候".into();
                }
                layout(hwnd);
                return LRESULT(0);
            }
            let (preview, draft_touched) = {
                let u = ui();
                (u.preview, u.draft_touched)
            };
            if draft_touched
                && inputs_changed(hwnd)
                && MessageBoxW(
                    hwnd,
                    w!("关闭后不会保存本次账号修改"),
                    w!("放弃修改并关闭"),
                    MB_OKCANCEL | MB_ICONQUESTION,
                ) != IDOK
            {
                EXIT_WITH_DAEMON.store(false, Ordering::SeqCst);
                return LRESULT(0);
            }
            CLOSED.store(true, Ordering::SeqCst);
            if EXIT_WITH_DAEMON.load(Ordering::SeqCst) && !preview {
                agent::stop_daemon();
            }
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOSED.store(true, Ordering::SeqCst);
            let _ = KillTimer(hwnd, 1);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn claim_instance(settings: bool) -> bool {
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        if let Ok(hwnd) = FindWindowW(None, w!("校园网自动登录")) {
            if IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                if settings {
                    let _ = PostMessageW(hwnd, WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
                }
                let _ = SetForegroundWindow(hwnd);
                return false;
            }
        }
        match CreateMutexW(None, false, w!("Local\\SNNU-AutoLogin-GUI")) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(h);
                    false
                } else {
                    true
                }
            }
            Err(_) => false,
        }
    }
}

pub fn run_win32() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    // 模拟状态只在调试构建中开放，发布程序始终展示真实连接。
    let preview = if cfg!(debug_assertions) {
        args.iter()
            .position(|a| a == "--preview")
            .map(|i| args.get(i + 1).cloned().unwrap_or_else(|| "online".into()))
    } else {
        None
    };
    let settings = args.iter().any(|a| a == "--settings");
    if preview.is_none() && !claim_instance(settings) {
        return Ok(());
    }
    if let Some(kind) = &preview {
        let _ = PREVIEW.set(kind.clone());
    }
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("SNNUDesktop");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(hinst.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: crate::tray::app_icon(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("窗口初始化失败".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            if preview.is_some() {
                w!("校园网自动登录 · 预览")
            } else {
                w!("校园网自动登录")
            },
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            460,
            None,
            None,
            hinst,
            None,
        )
        .map_err(|e| e.to_string())?;
        let dc = GetDC(hwnd);
        let mut dpi = GetDeviceCaps(dc, LOGPIXELSY).max(96);
        let _ = ReleaseDC(hwnd, dc);
        if preview.is_some() {
            if let Some(i) = args.iter().position(|a| a == "--preview-dpi") {
                if let Some(value) = args.get(i + 1).and_then(|v| v.parse::<i32>().ok()) {
                    dpi = value.clamp(96, 288);
                    let _ = PREVIEW_DPI.set(dpi);
                }
            }
            let label = wide(&format!("校园网自动登录 · 预览 · {}%", dpi * 100 / 96));
            let _ = SetWindowTextW(hwnd, PCWSTR(label.as_ptr()));
        }
        let snap = if let Some(kind) = &preview {
            Snapshot::preview(kind)
        } else {
            let (cfg, pwd) = config::load_config();
            Snapshot {
                revision: REFRESH_EPOCH.current(),
                status: portal::Status::Unreachable,
                name: network::current_name(),
                account: cfg.account,
                operator: cfg.operator,
                configured: !pwd.is_empty(),
                running: agent::running(),
                startup: autostart::enabled(),
                state: config::load_state(),
            }
        };
        let mut initial = match preview.as_deref() {
            None if settings => Page::Settings,
            Some("setup") => Page::Account,
            Some("settings") => Page::Settings,
            Some("account") => Page::Account,
            Some("features") => Page::Features,
            Some("help") => Page::Help,
            _ => {
                if snap.configured {
                    Page::Home
                } else {
                    Page::Account
                }
            }
        };
        if preview.is_some() {
            if let Some(page) = args
                .iter()
                .position(|a| a == "--preview-page")
                .and_then(|i| args.get(i + 1))
            {
                initial = match page.as_str() {
                    "help" => Page::Help,
                    "features" => Page::Features,
                    "settings" => Page::Settings,
                    "account" => Page::Account,
                    _ => Page::Home,
                };
            }
        }
        UI.set(Mutex::new(Ui {
            dpi,
            page: initial,
            busy: false,
            diag_busy: false,
            loading: true,
            extra_check: false,
            sub_extra: 0,
            info_extra: 0,
            info_tone: Tone::Neutral,
            spin: 0,
            preview: preview.is_some(),
            hover: 0,
            scroll: 0,
            viewport: 0,
            content_height: 0,
            op: op_index(&snap.operator),
            shown: false,
            action: Action::Connect,
            tone: Tone::Neutral,
            note: String::new(),
            note_error: false,
            note_until: None,
            invalid_field: None,
            busy_id: 0,
            busy_label: String::new(),
            initializing_inputs: false,
            draft_touched: false,
            snapshot: snap.clone(),
            gfx: make_gfx(dpi),
        }))
        .map_err(|_| "界面已初始化")?;
        create_controls(hwnd);
        load_inputs(hwnd);
        apply_snapshot(hwnd, snap);
        if preview.is_some() {
            if let Some(note) = args
                .iter()
                .position(|a| a == "--preview-note")
                .and_then(|i| args.get(i + 1))
            {
                let mut u = ui();
                u.note = note.clone();
                u.note_error = args.iter().any(|a| a == "--preview-error");
            }
        }
        layout(hwnd);
        if preview.is_none() {
            if ui().snapshot.configured {
                autostart::ensure();
            }
            ui().loading = true;
            text(hwnd, PRIMARY, "检测中");
            layout(hwnd);
            text(hwnd, TITLE, "正在检测连接");
            text(hwnd, SUB, "稍候，正在检查当前网络");
            start_probe(hwnd);
        }
        if preview.as_deref() == Some("connecting") {
            ui().busy = true;
            text(hwnd, PRIMARY, "连接中");
            text(hwnd, TITLE, "正在连接");
            text(hwnd, SUB, "连接结果会自动更新，无需重复点击");
            layout(hwnd);
        }
        let area = work_area(hwnd);
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let _ = SetWindowPos(
            hwnd,
            None,
            area.left + (area.right - area.left - (rect.right - rect.left)) / 2,
            area.top + (area.bottom - area.top - (rect.bottom - rect.top)) / 2,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if initial == Page::Account {
            let first = if get(hwnd, ACC_EDIT).trim().is_empty() {
                ACC_EDIT
            } else {
                PWD_EDIT
            };
            let _ = SetFocus(child(hwnd, first));
        }
        let _ = SetTimer(hwnd, 1, 80, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let (current_page, busy) = {
                let u = ui();
                (u.page, u.busy)
            };
            if msg.message == WM_KEYDOWN && matches!(current_page, Page::Help | Page::Features) {
                let key = msg.wParam.0;
                if matches!(key, 0x21 | 0x22 | 0x23 | 0x24 | 0x26 | 0x28) {
                    let overflow = {
                        let mut u = ui();
                        let overflow = u.content_height > u.viewport;
                        let target = match key {
                            0x21 => u.scroll - u.viewport + 32,
                            0x22 => u.scroll + u.viewport - 32,
                            0x23 => u.content_height,
                            0x24 => 0,
                            0x26 => u.scroll - 32,
                            _ => u.scroll + 32,
                        };
                        if overflow {
                            u.scroll = target.clamp(0, (u.content_height - u.viewport).max(0));
                        }
                        overflow
                    };
                    if overflow {
                        layout(hwnd);
                        continue;
                    }
                }
            }
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 == 0x1b
                && current_page != Page::Home
                && !busy
            {
                command(hwnd, BACK);
                continue;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == 0x0d && !ui().busy {
                let focused = GetFocus();
                let id = GetDlgCtrlID(focused);
                if current_page == Page::Home && focused == hwnd {
                    command(hwnd, PRIMARY);
                    continue;
                }
                if ui().page == Page::Account && matches!(id, ACC_EDIT | PWD_EDIT) {
                    save_account(hwnd);
                    continue;
                }
                if controls().iter().any(|c| {
                    c.id == id
                        && matches!(
                            c.style,
                            Style::Power
                                | Style::Primary
                                | Style::Link
                                | Style::Row
                                | Style::Toggle
                                | Style::Segment
                        )
                }) {
                    command(hwnd, id);
                    continue;
                }
            }
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enter_from_an_input_uses_the_save_button_as_busy_target() {
        assert_eq!(busy_target(Page::Account, PWD_EDIT), SAVE);
        assert_eq!(busy_target(Page::Account, ACC_EDIT), SAVE);
        assert_eq!(busy_target(Page::Home, 0), PRIMARY);
        assert_eq!(busy_target(Page::Settings, AUTO_START), AUTO_START);
    }

    #[test]
    fn interactive_controls_have_unique_identifiers_and_valid_geometry() {
        let controls = controls();
        let mut ids = std::collections::HashSet::new();
        for c in controls {
            assert!(ids.insert(c.id));
            assert!(c.rect.2 > 0 && c.rect.3 > 0);
            assert!(c.rect.0 >= 0 && c.rect.0 + c.rect.2 <= WIDTH);
        }
    }
}
