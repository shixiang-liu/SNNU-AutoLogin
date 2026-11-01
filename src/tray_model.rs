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
    Open = 1,
    Retry = 2,
    Disconnect = 3,
    /// Windows 上只关闭界面，后台守护继续巡检；macOS 仍是「退出程序」
    Quit = 5,
    Automatic = 6,
    Settings = 7,
    /// 界面与后台一起退出，只有 Windows 托盘提供
    #[cfg(windows)]
    QuitAll = 8,
}

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
    let mut entries = vec![
        Entry::Status(status(c)),
        Entry::Separator,
        action(Command::Open, "打开主界面", true, false),
        action(Command::Settings, "设置", true, false),
        Entry::Separator,
        action(
            Command::Automatic,
            "自动登录",
            active,
            !c.paused && c.configured,
        ),
    ];
    if c.configured && !c.paused && (c.link != Link::Online || c.no_internet) {
        entries.push(action(
            Command::Retry,
            if c.no_internet {
                "检查网络"
            } else if c.link == Link::Waiting {
                "重新检测连接"
            } else {
                "立即重试"
            },
            active && c.link != Link::Connecting,
            false,
        ));
    }
    entries.push(action(
        Command::Disconnect,
        "断开校园网认证",
        active && c.link == Link::Online,
        false,
    ));
    entries.push(Entry::Separator);
    #[cfg(windows)]
    {
        // 关界面不等于停后台：自愈要继续跑，真正的退出口单独一项。
        entries.push(action(Command::Quit, "关闭界面", true, false));
        entries.push(action(Command::QuitAll, "退出并停止后台", true, false));
    }
    #[cfg(not(windows))]
    entries.push(action(Command::Quit, "退出程序", true, false));
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
    fn online_menu_removes_redundant_retry_and_keeps_one_automatic_control() {
        let c = online();
        assert_eq!(status(&c), "已连接");
        assert_eq!(action(&c, Command::Retry), None);
        assert_eq!(action(&c, Command::Automatic), Some((true, true)));
        assert_eq!(action(&c, Command::Disconnect), Some((true, false)));
    }
    #[test]
    fn pause_and_account_setup_never_claim_network_was_disconnected() {
        let mut c = online();
        c.paused = true;
        assert_eq!(status(&c), "自动登录已关闭");
        assert_eq!(action(&c, Command::Automatic), Some((true, false)));
        assert_eq!(action(&c, Command::Retry), None);
        c.configured = false;
        assert_eq!(status(&c), "尚未设置账号");
        assert_eq!(action(&c, Command::Automatic), Some((false, false)));
        assert_eq!(action(&c, Command::Disconnect), Some((false, false)));
    }
    #[cfg(windows)]
    #[test]
    fn closing_the_window_is_separate_from_stopping_the_background() {
        let c = online();
        let labels: Vec<&str> = menu(&c)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Action { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        // 关界面不停后台，真正退出是单独一项，两项都不能缺。
        assert!(labels.contains(&"关闭界面"));
        assert!(labels.contains(&"退出并停止后台"));
        assert!(!labels.contains(&"退出程序"));
        assert_ne!(
            Command::Quit as u32,
            Command::QuitAll as u32,
            "两项必须走不同的菜单命令"
        );
        assert_eq!(action(&c, Command::Quit), Some((true, false)));
        assert_eq!(action(&c, Command::QuitAll), Some((true, false)));
    }

    #[test]
    fn busy_menu_prevents_repeated_actions_but_remains_navigable() {
        let mut c = online();
        c.link = Link::Offline;
        c.busy = true;
        for id in [Command::Automatic, Command::Retry, Command::Disconnect] {
            assert!(!action(&c, id).unwrap().0);
        }
        for id in [Command::Open, Command::Settings, Command::Quit] {
            assert!(action(&c, id).unwrap().0);
        }
        assert_eq!(status(&c), "正在处理…");
    }
    #[test]
    fn waiting_and_restricted_accounts_have_specific_statuses() {
        let mut c = online();
        c.link = Link::Waiting;
        assert_eq!(status(&c), "等待校园网");
        assert_eq!(action(&c, Command::Disconnect), Some((false, false)));
        c.restricted = true;
        assert_eq!(status(&c), "账号需要处理");
    }
}
