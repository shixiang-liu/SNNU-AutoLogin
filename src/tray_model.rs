//! 托盘事件与菜单规则，不在打开菜单时执行网络请求。
#[cfg(windows)]
use windows::Win32::UI::{
    Shell::{
        NIN_BALLOONHIDE, NIN_BALLOONSHOW, NIN_BALLOONTIMEOUT, NIN_BALLOONUSERCLICK, NIN_SELECT,
    },
    WindowsAndMessaging::{WM_CONTEXTMENU, WM_LBUTTONUP, WM_RBUTTONUP},
};
// SDK 宏 NIN_SELECT | NINF_KEY，windows 0.58 未导出该组合宏。
#[cfg(windows)]
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Open,
    Menu,
    NoticeShown,
    NoticeClosed,
    Ignore,
}

#[cfg(windows)]
pub fn event(modern: bool, wp: usize, lp: u32, icon: u32) -> Event {
    if (modern && lp >> 16 != icon) || (!modern && wp != icon as usize) {
        return Event::Ignore;
    }
    match lp & 0xffff {
        NIN_SELECT | NIN_KEYSELECT | NIN_BALLOONUSERCLICK => Event::Open,
        WM_CONTEXTMENU if modern => Event::Menu,
        WM_LBUTTONUP if !modern => Event::Open,
        WM_RBUTTONUP if !modern => Event::Menu,
        NIN_BALLOONSHOW => Event::NoticeShown,
        NIN_BALLOONHIDE | NIN_BALLOONTIMEOUT => Event::NoticeClosed,
        _ => Event::Ignore,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Link {
    Checking = 0,
    Online = 1,
    Offline = 2,
    Waiting = 3,
    Connecting = 4,
    Setup = 5,
}
impl Link {
    pub fn from_byte(value: u8) -> Self {
        match value {
            1 => Self::Online,
            2 => Self::Offline,
            3 => Self::Waiting,
            4 => Self::Connecting,
            5 => Self::Setup,
            _ => Self::Checking,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
pub enum Command {
    Quit = 5,
    Automatic = 6,
    Settings = 7,
    Devices = 8,
}

/// 「关于」里的固定信息：作者、联系方式与免责说明。三端共用同一份文案。
pub const ABOUT: &str = "\
作者：Shixiang Liu
邮箱：shixiang.liu@foxmail.com
项目主页：github.com/shixiang-liu/SNNU-AutoLogin

非学校官方工具；账号只保存在本机，不会上传。";
/// 打开 Releases 列表而不是 /latest：最新发布的可能是 Linux 或 Android，不是当前平台。
pub const RELEASES_URL: &str = "https://github.com/shixiang-liu/SNNU-AutoLogin/releases";

pub struct Context {
    pub configured: bool,
    pub paused: bool,
    pub restricted: bool,
    pub no_internet: bool,
    pub busy: bool,
    pub link: Link,
}

pub enum Entry {
    Status(&'static str),
    Separator,
    Action {
        command: Command,
        label: &'static str,
        enabled: bool,
        checked: bool,
    },
}

pub fn status(c: &Context) -> &'static str {
    if c.busy {
        "正在处理…"
    } else if !c.configured {
        "尚未设置账号"
    } else if c.paused {
        "自动登录已关闭"
    } else if c.restricted {
        "账号需要处理"
    } else if c.no_internet {
        "网络暂不可用"
    } else {
        match c.link {
            Link::Online => "已连接",
            Link::Offline => "尚未连接",
            Link::Waiting => "等待校园网",
            Link::Connecting => "正在连接…",
            _ => "正在检测连接…",
        }
    }
}

pub fn menu(c: &Context) -> Vec<Entry> {
    let action = |command, label, enabled, checked| Entry::Action {
        command,
        label,
        enabled,
        checked,
    };
    let active = c.configured && !c.busy;
    let entries = vec![
        Entry::Status(status(c)),
        Entry::Separator,
        action(
            Command::Automatic,
            "自动登录",
            active,
            !c.paused && c.configured,
        ),
        // 只做跳转：设备列表与下线操作都留在门户里。
        action(
            Command::Devices,
            "在线设备管理",
            active && c.link == Link::Online,
            false,
        ),
        Entry::Separator,
        action(Command::Settings, "设置", true, false),
        Entry::Separator,
        action(Command::Quit, "退出程序", true, false),
    ];
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDBLCLK;
    fn online() -> Context {
        Context {
            configured: true,
            paused: false,
            restricted: false,
            no_internet: false,
            busy: false,
            link: Link::Online,
        }
    }
    fn action(c: &Context, id: Command) -> Option<(bool, bool)> {
        menu(c).into_iter().find_map(|e| match e {
            Entry::Action {
                command,
                enabled,
                checked,
                ..
            } if command == id => Some((enabled, checked)),
            _ => None,
        })
    }
    #[cfg(windows)]
    #[test]
    fn modern_single_click_and_keyboard_open_without_mouse_event_duplicates() {
        let pack = |event| (1 << 16) | event;
        assert_eq!(event(true, 0, pack(NIN_SELECT), 1), Event::Open);
        assert_eq!(event(true, 0, pack(NIN_KEYSELECT), 1), Event::Open);
        assert_eq!(event(true, 0, pack(NIN_BALLOONUSERCLICK), 1), Event::Open);
        assert_eq!(event(true, 0, pack(WM_LBUTTONUP), 1), Event::Ignore);
        assert_eq!(event(true, 0, pack(WM_LBUTTONDBLCLK), 1), Event::Ignore);
        assert_eq!(event(true, 0, pack(WM_CONTEXTMENU), 1), Event::Menu);
        assert_eq!(event(true, 0, pack(WM_RBUTTONUP), 1), Event::Ignore);
        assert_eq!(event(true, 0, (2 << 16) | NIN_SELECT, 1), Event::Ignore);
    }
    #[cfg(windows)]
    #[test]
    fn legacy_callbacks_keep_single_click_fallback() {
        assert_eq!(event(false, 1, WM_LBUTTONUP, 1), Event::Open);
        assert_eq!(event(false, 1, WM_RBUTTONUP, 1), Event::Menu);
        assert_eq!(event(false, 2, WM_LBUTTONUP, 1), Event::Ignore);
    }
    #[test]
    fn menu_keeps_only_automatic_devices_settings_and_quit() {
        let c = online();
        assert_eq!(status(&c), "已连接");
        assert_eq!(action(&c, Command::Automatic), Some((true, true)));
        assert_eq!(action(&c, Command::Devices), Some((true, false)));
        assert_eq!(action(&c, Command::Settings), Some((true, false)));
        assert_eq!(action(&c, Command::Quit), Some((true, false)));
        // 断开与重试回主界面，托盘不再重复（菜单里已不存在这两个命令）
    }
    #[test]
    fn pause_and_account_setup_never_claim_network_was_disconnected() {
        let mut c = online();
        c.paused = true;
        assert_eq!(status(&c), "自动登录已关闭");
        assert_eq!(action(&c, Command::Automatic), Some((true, false)));
        c.configured = false;
        assert_eq!(status(&c), "尚未设置账号");
        assert_eq!(action(&c, Command::Automatic), Some((false, false)));
    }
    #[test]
    fn busy_menu_prevents_repeated_actions_but_remains_navigable() {
        let mut c = online();
        c.link = Link::Offline;
        c.busy = true;
        assert!(!action(&c, Command::Automatic).unwrap().0);
        for id in [Command::Settings, Command::Quit] {
            assert!(action(&c, id).unwrap().0);
        }
        assert_eq!(status(&c), "正在处理…");
    }
    #[test]
    fn device_management_is_only_offered_while_online() {
        let mut c = online();
        assert_eq!(action(&c, Command::Devices), Some((true, false)));
        for link in [Link::Waiting, Link::Offline, Link::Connecting, Link::Setup] {
            c.link = link;
            assert_eq!(
                action(&c, Command::Devices),
                Some((false, false)),
                "{link:?} 状态下不应允许打开设备列表"
            );
        }
    }
    #[test]
    fn waiting_and_restricted_accounts_have_specific_statuses() {
        let mut c = online();
        c.link = Link::Waiting;
        assert_eq!(status(&c), "等待校园网");
        c.restricted = true;
        assert_eq!(status(&c), "账号需要处理");
    }
}
