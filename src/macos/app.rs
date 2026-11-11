//! AppKit owns every window, menu and control on the main thread.
//! Each retained object below is created with the documented Objective-C signature.
use super::{appearance, runtime, storage};
use crate::{agent, autostart, config, network, portal, tray, tray_model, ui_model};
use appearance::{rect, BG, MUTED, TEXT, WIDTH};
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use objc2_app_kit::{NSBackingStoreType, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Mutex},
    time::{Duration, Instant},
};

#[link(name = "AppKit", kind = "framework")]
extern "C" {}
#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}
static NOTIFICATIONS_ALLOWED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}

type Object = Retained<AnyObject>;
thread_local! { static UI: RefCell<Option<Ui>> = const { RefCell::new(None) }; }
static RESULT: Mutex<Option<Result<String, String>>> = Mutex::new(None);
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Settings,
    Account,
    Network,
    Features,
    Help,
}
struct AccountDraft {
    account: String,
    password: String,
    operator: isize,
}
struct Ui {
    app: Object,
    controller: Object,
    status_item: Object,
    window: Option<Object>,
    window_closed: bool,
    document: Option<Object>,
    scroller: Option<Object>,
    home_extra: f64,
    info_extra: f64,
    account_saved: bool,
    operator_index: isize,
    password_visible: bool,
    after_job: Option<Page>,
    account_draft: Option<AccountDraft>,
    layout_key: String,
    invalid_field: isize,
    controls: Vec<(isize, Object)>,
    page: Page,
    menu_key: String,
    note: String,
    note_error: bool,
    note_until: Option<Instant>,
    location: Option<Object>,
    tick: u64,
    smoke: Option<PathBuf>,
    started: Instant,
    measure: Option<(String, PathBuf, u64)>,
    samples: Vec<serde_json::Value>,
}
unsafe fn string(object: &AnyObject) -> String {
    let value: Retained<NSString> = msg_send![object, stringValue];
    value.to_string()
}
unsafe fn set_text(object: &AnyObject, value: &str) {
    let old: Retained<NSString> = msg_send![object, stringValue];
    if old.to_string() != value {
        let _: () = msg_send![object, setStringValue: &*NSString::from_str(value)];
    }
}
impl Ui {
    unsafe fn capture_draft(&mut self) {
        if self.page != Page::Account || self.control(201).is_none() {
            return;
        }
        self.account_draft = Some(AccountDraft {
            account: string(self.control(201).unwrap()),
            password: string(
                self.control(if self.password_visible { 203 } else { 202 })
                    .unwrap(),
            ),
            operator: self.operator_index,
        });
    }
    unsafe fn navigate(&mut self, page: Page) {
        self.capture_draft();
        self.open(page);
    }
    unsafe fn confirm_discard(&mut self) -> bool {
        self.capture_draft();
        let (cfg, password) = config::load_config();
        let changed = self.account_draft.as_ref().is_some_and(|draft| {
            ui_model::account_changed(
                &cfg.account,
                &password,
                &cfg.operator,
                &draft.account,
                &draft.password,
                operator_code(draft.operator),
            )
        });
        if !changed {
            return true;
        }
        let alert: Object = msg_send![class!(NSAlert), new];
        let _: () = msg_send![&alert,setMessageText:&*NSString::from_str("放弃修改并关闭")];
        let _: () =
            msg_send![&alert,setInformativeText:&*NSString::from_str("关闭后不会保存本次账号修改")];
        let _: *mut AnyObject = msg_send![&alert,addButtonWithTitle:&*NSString::from_str("取消")];
        let _: *mut AnyObject =
            msg_send![&alert,addButtonWithTitle:&*NSString::from_str("放弃修改")];
        let choice: isize = msg_send![&alert, runModal];
        choice == 1001
    }
    unsafe fn focus_field(&self, id: isize) {
        if let (Some(window), Some(field)) = (&self.window, self.control(id)) {
            let _: bool = msg_send![window,makeFirstResponder:field];
            let bounds: NSRect = msg_send![field, bounds];
            let _: bool = msg_send![field,scrollRectToVisible:bounds];
        }
    }
    unsafe fn control(&self, id: isize) -> Option<&AnyObject> {
        self.controls
            .iter()
            .find(|(tag, _)| *tag == id)
            .map(|(_, object)| &**object)
    }
    unsafe fn add(&mut self, view: &AnyObject, tag: isize, object: Object) {
        let _: () = msg_send![view, addSubview: &*object];
        self.controls.push((tag, object));
    }
    unsafe fn label(
        &mut self,
        view: &AnyObject,
        tag: isize,
        value: &str,
        frame: NSRect,
        size: f64,
        bold: bool,
        color: u32,
        alignment: isize,
    ) {
        let field: Allocated<AnyObject> = msg_send![class!(NSTextField), alloc];
        let frame = rect(
            frame.origin.x - 2.,
            frame.origin.y,
            frame.size.width + 4.,
            frame.size.height,
        );
        let field: Object = msg_send![field,initWithFrame:frame];
        let _: () = msg_send![&field,setBezeled:false];
        let _: () = msg_send![&field,setEditable:false];
        let _: () = msg_send![&field,setSelectable:true];
        let _: () = msg_send![&field,setDrawsBackground:false];
        appearance::configure_label(&field, size, bold, color, alignment);
        if frame.size.height <= 28. {
            let cell: *mut AnyObject = msg_send![&field, cell];
            let _: () = msg_send![cell,setLineBreakMode:4usize];
            let _: () = msg_send![cell,setUsesSingleLineMode:true];
        }
        set_text(&field, value);
        self.add(view, tag, field);
    }
    unsafe fn button(&mut self, view: &AnyObject, tag: isize, title: &str, frame: NSRect) {
        let button: Allocated<AnyObject> = msg_send![class!(SNNUDesktopButton), alloc];
        let button: Object = msg_send![button,initWithFrame:frame];
        let _: () = msg_send![&button,setButtonType:if tag==11||tag==12 {3usize}else if (30..=32).contains(&tag) {1usize}else{7usize}];
        let _: () = msg_send![&button,setBordered:false];
        let _: () = msg_send![&button,setTitle:&*NSString::from_str(title)];
        let _: () = msg_send![&button,setTag:tag];
        let _: () = msg_send![&button,setTarget:&*self.controller];
        let _: () = msg_send![&button,setAction:sel!(perform:)];
        let _: () = msg_send![&button,setFocusRingType:1usize];
        appearance::track(&button);
        self.add(view, tag, button);
    }
    unsafe fn edit(
        &mut self,
        view: &AnyObject,
        tag: isize,
        value: &str,
        frame: NSRect,
        secure: bool,
    ) {
        let cls = if secure {
            class!(NSSecureTextField)
        } else {
            class!(NSTextField)
        };
        let field: Allocated<AnyObject> = msg_send![cls, alloc];
        let field: Object = msg_send![field,initWithFrame:frame];
        let _: () = msg_send![&field,setBezeled:false];
        let _: () = msg_send![&field,setBordered:false];
        let _: () = msg_send![&field,setDrawsBackground:false];
        let _: () = msg_send![&field,setFocusRingType:1usize];
        let _: () = msg_send![&field,setFont:&*appearance::font(13.333,false)];
        let _: () = msg_send![&field,setTextColor:&*appearance::color(TEXT)];
        let _: () = msg_send![&field,setDelegate:&*self.controller];
        set_text(&field, value);
        self.add(view, tag, field);
    }
    unsafe fn title(&self, tag: isize, value: &str) {
        if let Some(button) = self.control(tag) {
            let old: Retained<NSString> = msg_send![button, title];
            if old.to_string() != value {
                let _: () = msg_send![button,setTitle:&*NSString::from_str(value)];
                let _: () = msg_send![button,setNeedsDisplay:true];
            }
        }
    }
    unsafe fn layout(&mut self) {
        let feedback = storage::last_error().unwrap_or_else(|| self.note.clone());
        let key = format!(
            "{}:{}:{}:{}:{}",
            self.page as u8, self.home_extra, self.info_extra, self.account_saved, feedback
        );
        if key == self.layout_key {
            return;
        }
        self.layout_key = key;
        let note_height = appearance::text_height(&feedback, 332., 13.333).max(32.);
        let account_extra = if feedback.is_empty() {
            0.
        } else {
            (note_height - 32.).max(0.)
        };
        let mut height = match self.page {
            Page::Home => 472. + self.home_extra,
            Page::Settings => 448.,
            Page::Account => {
                if self.account_saved {
                    592. + account_extra
                } else {
                    540. + account_extra
                }
            }
            Page::Network => 596. + self.info_extra,
            Page::Features => 724.,
            Page::Help => 596.,
        };
        let note_top = match self.page {
            Page::Home => 420. + self.home_extra,
            Page::Account => 397.,
            Page::Settings => 440.,
            Page::Network => 584. + self.info_extra,
            Page::Features => 712.,
            Page::Help => 528.,
        };
        if !feedback.is_empty() {
            let bottom = if matches!(self.page, Page::Home | Page::Network | Page::Features) {
                18.
            } else {
                24.
            };
            height = height.max(note_top + note_height + bottom);
        }
        if let Some(note) = self.control(104) {
            let _: () = msg_send![note,setFrame:rect(24.,note_top,332.,note_height)];
            let _: () = msg_send![note,setHidden:feedback.is_empty()];
        }
        if let Some(foot) = self.control(108) {
            let _: () = msg_send![foot,setHidden:!feedback.is_empty()];
        }
        if self.page == Page::Home {
            for (id, y) in [(105, 342.), (106, 364.), (107, 364.), (108, 420.)] {
                if let Some(control) = self.control(id) {
                    let frame: NSRect = msg_send![control, frame];
                    let _: () = msg_send![control,setFrameOrigin:NSPoint::new(frame.origin.x,y+self.home_extra)];
                }
            }
        }
        if self.page == Page::Account {
            for (id, y) in [(13, 438.), (109, 498.), (14, 536.)] {
                if let Some(control) = self.control(id) {
                    let frame: NSRect = msg_send![control, frame];
                    let _: () = msg_send![control,setFrameOrigin:NSPoint::new(frame.origin.x,y+account_extra)];
                }
            }
        }
        let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
        let area = if screen.is_null() {
            rect(0., 0., 1440., 900.)
        } else {
            msg_send![screen, visibleFrame]
        };
        let viewport = height.min((area.size.height - 48.).max(220.));
        if let Some(document) = &self.document {
            let _: () = msg_send![document,setFrameSize:NSSize::new(WIDTH,height)];
        }
        if let Some(scroller) = &self.scroller {
            let _: () = msg_send![scroller,setFrameSize:NSSize::new(WIDTH,viewport)];
            let _: () = msg_send![scroller,setHasVerticalScroller:viewport<height];
        }
        if let Some(window) = &self.window {
            let old: NSRect = msg_send![window, frame];
            let frame: NSRect =
                msg_send![window,frameRectForContentRect:rect(0.,0.,WIDTH,viewport)];
            let y = (old.origin.y + old.size.height - frame.size.height).clamp(
                area.origin.y,
                (area.origin.y + area.size.height - frame.size.height).max(area.origin.y),
            );
            let x = old.origin.x.clamp(
                area.origin.x,
                (area.origin.x + area.size.width - frame.size.width).max(area.origin.x),
            );
            let frame = rect(x, y, frame.size.width, frame.size.height);
            let _: () = msg_send![window,setFrame:frame,display:true];
        }
    }
    unsafe fn open(&mut self, page: Page) {
        if self.page != page {
            self.note.clear();
            self.note_error = false;
            self.note_until = None;
        }
        self.page = page;
        self.window_closed = false;
        let (cfg, password) = config::load_config();
        self.account_saved = !cfg.account.is_empty();
        let state = config::load_state();
        let status = runtime::status();
        let view = ui_model::present(
            &status,
            !password.is_empty(),
            agent::running(),
            state.reconnect_paused,
            state.no_internet,
            &state.last_error,
            &cfg.account,
        )
        .check_operator(&status, &cfg.account, &cfg.operator, state.reconnect_paused);
        let summary = ui_model::network_summary(
            &status,
            !password.is_empty(),
            state.reconnect_paused,
            state.no_internet,
            &state.last_error,
            &cfg.account,
            false,
        );
        self.home_extra = (appearance::text_height(&view.subtitle, 332., 13.333) - 42.).max(0.)
            + if view.network_check { 32. } else { 0. };
        self.info_extra = (appearance::text_height(&summary.description, 300., 12.) - 32.).max(0.);
        if self.window.is_none() {
            let window: Allocated<AnyObject> = msg_send![class!(NSWindow), alloc];
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;
            let window: Object = msg_send![window,initWithContentRect:rect(0.,0.,WIDTH,472.),styleMask:style,backing:NSBackingStoreType::Buffered,defer:false];
            let _: () = msg_send![&window,setTitle:&*NSString::from_str("校园网自动登录")];
            let _: () = msg_send![&window,setBackgroundColor:&*appearance::color(BG)];
            let _: () = msg_send![&window,setReleasedWhenClosed:false];
            let _: () = msg_send![&window,setDelegate:&*self.controller];
            let _: () = msg_send![&window, center];
            self.window = Some(window);
        }
        self.controls.clear();
        self.document.take();
        self.scroller.take();
        self.layout_key.clear();
        self.invalid_field = 0;
        appearance::update(appearance::Canvas {
            page: page as u8,
            home_extra: self.home_extra,
            info_extra: self.info_extra,
            tone: view.tone,
            info_tone: summary.tone,
            action: view.action,
            busy: false,
            spin: 0.,
            focus_field: 0,
            invalid_field: 0,
        });
        let document: Allocated<AnyObject> = msg_send![class!(SNNUContentView), alloc];
        let document: Object = msg_send![document,initWithFrame:rect(0.,0.,WIDTH,724.)];
        let title = match page {
            Page::Home => "",
            Page::Settings => "设置",
            Page::Account => "账号设置",
            Page::Network => "网络信息",
            Page::Features => "功能介绍",
            Page::Help => "使用帮助",
        };
        if page != Page::Home {
            self.button(&document, 40, "‹ 返回", rect(16., 16., 64., 36.));
            self.label(
                &document,
                100,
                title,
                rect(90., 20., 200., 28.),
                17.333,
                true,
                TEXT,
                2,
            );
        }
        match page {
            Page::Home => {
                self.label(
                    &document,
                    103,
                    &network::current_name(),
                    rect(24., 20., 250., 28.),
                    17.333,
                    true,
                    TEXT,
                    0,
                );
                self.button(&document, 1, "设置", rect(292., 16., 64., 36.));
                self.button(&document, 10, view.button, rect(122., 72., 136., 136.));
                self.label(
                    &document,
                    101,
                    view.title,
                    rect(24., 228., 332., 38.),
                    29.333,
                    true,
                    TEXT,
                    2,
                );
                self.label(
                    &document,
                    102,
                    &view.subtitle,
                    rect(
                        24.,
                        272.,
                        332.,
                        42. + self.home_extra - if view.network_check { 32. } else { 0. },
                    ),
                    13.333,
                    false,
                    MUTED,
                    2,
                );
                self.button(
                    &document,
                    20,
                    "检查网络",
                    rect(
                        130.,
                        310. + self.home_extra - if view.network_check { 32. } else { 0. },
                        120.,
                        30.,
                    ),
                );
                self.label(
                    &document,
                    105,
                    "当前账号",
                    rect(40., 342. + self.home_extra, 272., 18.),
                    12.,
                    false,
                    MUTED,
                    0,
                );
                self.label(
                    &document,
                    106,
                    "",
                    rect(40., 364. + self.home_extra, 216., 24.),
                    17.333,
                    true,
                    TEXT,
                    0,
                );
                self.label(
                    &document,
                    107,
                    "",
                    rect(268., 364. + self.home_extra, 72., 24.),
                    12.,
                    false,
                    MUTED,
                    1,
                );
                self.label(
                    &document,
                    108,
                    view.footer,
                    rect(24., 420. + self.home_extra, 332., 34.),
                    12.,
                    false,
                    MUTED,
                    2,
                );
            }
            Page::Settings => {
                self.button(
                    &document,
                    2,
                    "账号设置\n添加校园网账号",
                    rect(24., 76., 332., 76.),
                );
                self.button(
                    &document,
                    11,
                    "自动登录\n已开启，掉线后自动恢复",
                    rect(24., 172., 332., 62.),
                );
                self.button(
                    &document,
                    12,
                    "开机自动运行\n登录 Mac 后自动运行",
                    rect(24., 246., 332., 62.),
                );
                self.button(&document, 4, "功能介绍", rect(24., 328., 332., 42.));
                self.button(&document, 3, "网络信息", rect(24., 382., 332., 42.));
            }
            Page::Account => {
                self.password_visible = false;
                let draft_account = self
                    .account_draft
                    .as_ref()
                    .map(|d| d.account.clone())
                    .unwrap_or_else(|| cfg.account.clone());
                let draft_password = self
                    .account_draft
                    .as_ref()
                    .map(|d| d.password.clone())
                    .unwrap_or_else(|| password.clone());
                self.operator_index = self
                    .account_draft
                    .as_ref()
                    .map(|d| d.operator)
                    .unwrap_or_else(|| match cfg.operator.as_str() {
                        "mobile" => 1,
                        "unicom" => 2,
                        _ => 0,
                    });
                self.label(
                    &document,
                    -1,
                    "填一次，以后自动登录",
                    rect(24., 70., 332., 24.),
                    13.333,
                    false,
                    TEXT,
                    2,
                );
                self.label(
                    &document,
                    -1,
                    "学号",
                    rect(24., 116., 332., 20.),
                    13.333,
                    false,
                    TEXT,
                    0,
                );
                self.edit(
                    &document,
                    201,
                    &draft_account,
                    rect(38., 151., 304., 24.),
                    false,
                );
                self.label(
                    &document,
                    -1,
                    "校园网密码",
                    rect(24., 202., 200., 20.),
                    13.333,
                    false,
                    TEXT,
                    0,
                );
                self.edit(
                    &document,
                    202,
                    &draft_password,
                    rect(38., 237., 304., 24.),
                    true,
                );
                self.edit(&document, 203, "", rect(38., 237., 304., 24.), false);
                let _: () = msg_send![self.control(203).unwrap(),setHidden:true];
                self.button(&document, 16, "显示", rect(286., 194., 70., 32.));
                self.label(
                    &document,
                    -1,
                    "上网方式",
                    rect(24., 286., 332., 20.),
                    13.333,
                    false,
                    TEXT,
                    0,
                );
                for (i, name) in ["校园网", "移动", "联通"].iter().enumerate() {
                    self.button(
                        &document,
                        30 + i as isize,
                        name,
                        rect([28., 137., 246.][i], 320., 106., 34.),
                    );
                }
                self.label(
                    &document,
                    -1,
                    "与网页登录时的选择一致",
                    rect(24., 366., 332., 22.),
                    12.,
                    false,
                    MUTED,
                    0,
                );
                self.button(&document, 13, "保存并连接", rect(24., 438., 332., 44.));
                self.label(
                    &document,
                    109,
                    "密码在本机钥匙串中加密保存",
                    rect(24., 498., 332., 20.),
                    12.,
                    false,
                    MUTED,
                    2,
                );
                if self.account_saved {
                    self.button(&document, 14, "删除保存的账号", rect(24., 536., 332., 32.));
                }
            }
            Page::Network => {
                self.label(
                    &document,
                    401,
                    summary.title,
                    rect(62., 88., 278., 26.),
                    17.333,
                    true,
                    TEXT,
                    0,
                );
                self.label(
                    &document,
                    402,
                    &summary.description,
                    rect(40., 124., 300., 32. + self.info_extra),
                    12.,
                    false,
                    MUTED,
                    0,
                );
                let values = network_values();
                for (i, label) in ["网络名称", "当前账号", "上网方式", "IP 地址", "在线时长"]
                    .iter()
                    .enumerate()
                {
                    let y = 208. + self.info_extra + i as f64 * 40.;
                    self.label(
                        &document,
                        -1,
                        label,
                        rect(40., y, 92., 22.),
                        13.333,
                        false,
                        MUTED,
                        0,
                    );
                    if i == 0 {
                        self.button(&document, 17, &values[i], rect(134., y, 206., 22.));
                    } else {
                        self.label(
                            &document,
                            440 + i as isize,
                            &values[i],
                            rect(134., y, 206., 22.),
                            13.333,
                            false,
                            TEXT,
                            1,
                        );
                    }
                }
                self.button(
                    &document,
                    15,
                    "导出诊断报告",
                    rect(24., 432. + self.info_extra, 332., 44.),
                );
                self.label(
                    &document,
                    -1,
                    "包含连接信息与最近日志\n密码不写入报告，账号会脱敏",
                    rect(24., 492. + self.info_extra, 332., 38.),
                    12.,
                    false,
                    MUTED,
                    2,
                );
                self.label(
                    &document,
                    -1,
                    &format!("校园网自动登录  v{}", crate::VERSION),
                    rect(24., 552. + self.info_extra, 332., 20.),
                    12.,
                    false,
                    MUTED,
                    2,
                );
            }
            Page::Features => {
                let groups = ["自动连接", "连接维护", "日常使用"];
                let items = [
                    ("自动登录", "连接学校网络后自动完成登录"),
                    ("开机运行", "随 Mac 登录启动后台"),
                    ("自动重连", "掉线、换网或唤醒后重新检查连接"),
                    ("连接保活", "定时发送少量请求，维持连接活动"),
                    ("异常处理", "尝试恢复网络，账号异常时暂缓重试"),
                    ("认证地址自动识别", "学校认证地址变化时自动重新寻找"),
                    ("密码加密保存", "密码在本机钥匙串中加密保存"),
                    ("菜单栏运行", "关闭窗口后继续运行"),
                    ("诊断报告", "导出连接信息与日志，方便排查问题"),
                ];
                for (i, group) in groups.iter().enumerate() {
                    self.label(
                        &document,
                        -1,
                        group,
                        rect(24., 72. + i as f64 * 212., 332., 20.),
                        12.,
                        false,
                        MUTED,
                        0,
                    );
                }
                for (i, (name, description)) in items.iter().enumerate() {
                    let top = 112. + (i / 3) as f64 * 212. + (i % 3) as f64 * 56.;
                    self.label(
                        &document,
                        -1,
                        name,
                        rect(40., top, 300., 20.),
                        13.333,
                        true,
                        TEXT,
                        0,
                    );
                    self.label(
                        &document,
                        -1,
                        description,
                        rect(40., top + 24., 300., 18.),
                        12.,
                        false,
                        MUTED,
                        0,
                    );
                }
            }
            Page::Help => {
                self.label(&document,-1,"1. 连接学校 Wi-Fi 或有线网络。\n2. 在账号设置填写学号、密码和上网方式。\n3. 点击保存并连接。\n\n关闭窗口后继续在菜单栏运行。\n从菜单栏选择退出，可退出后台。\n\n钥匙串访问失败时，请解锁并允许访问，\n然后从应用菜单重新读取密码。\n\n连接失败时，请检查系统设置 →\n隐私与安全性 → 本地网络是否允许本应用。\n\n显示真实 Wi-Fi 名称需要定位授权，\n可点击网络信息中的网络名称进行授权。\n\n登录启动需要在系统登录项中获得允许。",rect(40.,88.,300.,400.),13.333,false,TEXT,0);
            }
        }
        self.label(
            &document,
            104,
            "",
            rect(24., 440., 332., 40.),
            13.333,
            false,
            MUTED,
            2,
        );
        let scroller: Allocated<AnyObject> = msg_send![class!(NSScrollView), alloc];
        let scroller: Object = msg_send![scroller,initWithFrame:rect(0.,0.,WIDTH,472.)];
        let _: () = msg_send![&scroller,setBorderType:0usize];
        let _: () = msg_send![&scroller,setDrawsBackground:true];
        let _: () = msg_send![&scroller,setBackgroundColor:&*appearance::color(BG)];
        let _: () = msg_send![&scroller,setScrollerStyle:1isize];
        let _: () = msg_send![&scroller,setAutohidesScrollers:true];
        let _: () = msg_send![&scroller,setDocumentView:&*document];
        let _: () = msg_send![self.window.as_ref().unwrap(),setContentView:&*scroller];
        self.document = Some(document);
        self.scroller = Some(scroller);
        self.layout();
        let _: () = msg_send![self.document.as_ref().unwrap(),scrollPoint:NSPoint::new(0.,0.)];
        let _: () = msg_send![self.window.as_ref().unwrap(),makeKeyAndOrderFront:std::ptr::null::<AnyObject>()];
        let _: () = msg_send![&self.app,activateIgnoringOtherApps:true];
        runtime::refresh_status();
        self.refresh();
        if page == Page::Account {
            self.focus_field(201);
        }
    }
    unsafe fn refresh(&mut self) {
        let (cfg, password) = config::load_config();
        let state = config::load_state();
        let status = runtime::status();
        let busy = runtime::BUSY.load(Ordering::SeqCst);
        if self.window.is_some()
            && self.tick % 5 == 0
            && (password.is_empty()
                || state.reconnect_paused
                || state.paused_until > portal::unix_now())
        {
            runtime::refresh_status();
        }
        let view = ui_model::present(
            &status,
            !password.is_empty(),
            agent::running(),
            state.reconnect_paused,
            state.no_internet,
            &state.last_error,
            &cfg.account,
        )
        .check_operator(&status, &cfg.account, &cfg.operator, state.reconnect_paused);
        let summary = ui_model::network_summary(
            &status,
            !password.is_empty(),
            state.reconnect_paused,
            state.no_internet,
            &state.last_error,
            &cfg.account,
            false,
        );
        let new_extra = (appearance::text_height(&view.subtitle, 332., 13.333) - 42.).max(0.)
            + if view.network_check { 32. } else { 0. };
        let new_info = (appearance::text_height(&summary.description, 300., 12.) - 32.).max(0.);
        if self.window.is_some()
            && ((self.page == Page::Home && new_extra != self.home_extra)
                || (self.page == Page::Network && new_info != self.info_extra))
        {
            self.open(self.page);
            return;
        }
        let (display_account, display_operator) = match &status {
            portal::Status::Online(session) => portal::split_account(&session.account),
            _ => (cfg.account.clone(), cfg.operator.clone()),
        };
        if self.page == Page::Home {
            if let Some(label) = self.control(101) {
                set_text(label, if busy { "正在处理…" } else { view.title });
            }
            if let Some(label) = self.control(102) {
                set_text(label, &view.subtitle);
            }
            if let Some(label) = self.control(103) {
                if self.tick % 5 == 0 {
                    set_text(label, &network::current_name());
                }
            }
            if let Some(label) = self.control(106) {
                let masked = portal::mask(&display_account);
                set_text(
                    label,
                    if display_account.is_empty() {
                        "尚未设置"
                    } else {
                        &masked
                    },
                );
            }
            if let Some(label) = self.control(107) {
                set_text(
                    label,
                    if display_account.is_empty() {
                        ""
                    } else {
                        operator_label(&display_operator)
                    },
                );
            }
            if let Some(label) = self.control(108) {
                let footer = if network::current_name() == "有线网络" {
                    view.footer.replace("Wi-Fi", "网线")
                } else {
                    view.footer.into()
                };
                set_text(label, &footer);
            }
            self.title(10, if busy { "正在处理…" } else { view.button });
            if let Some(button) = self.control(20) {
                let _: () = msg_send![button,setHidden:!view.network_check];
            }
        }
        if self.page == Page::Settings {
            self.title(
                2,
                &format!(
                    "账号设置\n{}",
                    if password.is_empty() {
                        "添加校园网账号".into()
                    } else {
                        format!(
                            "{} · {}",
                            portal::mask(&cfg.account),
                            operator_label(&cfg.operator)
                        )
                    }
                ),
            );
            self.title(
                11,
                if state.reconnect_paused {
                    "自动登录\n已关闭，不影响当前连接"
                } else if state.paused_until > portal::unix_now() {
                    "自动登录\n已开启，账号异常时暂缓重试"
                } else {
                    "自动登录\n已开启，掉线后自动恢复"
                },
            );
            self.title(
                12,
                match autostart::status() {
                    1 => "开机自动运行\n已开启，登录 Mac 后运行",
                    2 => "开机自动运行\n等待允许，请在系统登录项中开启",
                    _ => "开机自动运行\n已关闭，需要时手动打开",
                },
            );
            for (id, on) in [(11, !state.reconnect_paused), (12, autostart::registered())] {
                if let Some(button) = self.control(id) {
                    let _: () = msg_send![button,setState:if on{1isize}else{0isize}];
                }
            }
        }
        if self.page == Page::Account {
            for i in 0..3 {
                if let Some(button) = self.control(30 + i) {
                    let _: () =
                        msg_send![button,setState:if self.operator_index==i{1isize}else{0isize}];
                }
            }
            self.title(
                16,
                if self.password_visible {
                    "隐藏"
                } else {
                    "显示"
                },
            );
            let changed = ui_model::account_changed(
                &cfg.account,
                &password,
                &cfg.operator,
                &string(self.control(201).unwrap()),
                &string(
                    self.control(if self.password_visible { 203 } else { 202 })
                        .unwrap(),
                ),
                operator_code(self.operator_index),
            );
            self.title(
                13,
                if busy {
                    "正在保存并连接…"
                } else if !password.is_empty() && !changed {
                    "完成"
                } else if matches!(status, portal::Status::Online(_)) {
                    "保存并重新连接"
                } else {
                    "保存并连接"
                },
            );
        }
        if self.page == Page::Network {
            if let Some(label) = self.control(401) {
                set_text(label, summary.title);
            }
            if let Some(label) = self.control(402) {
                set_text(label, &summary.description);
            }
            let values = network_values();
            self.title(17, &values[0]);
            for (i, value) in values.iter().enumerate().skip(1) {
                if let Some(label) = self.control(440 + i as isize) {
                    set_text(label, value);
                }
            }
        }
        let feedback = storage::last_error().unwrap_or_else(|| self.note.clone());
        if let Some(note) = self.control(104) {
            set_text(note, &feedback);
            let rgb = if self.note_error || storage::last_error().is_some() {
                0xbd353b
            } else {
                MUTED
            };
            let _: () = msg_send![note,setTextColor:&*appearance::color(rgb)];
        }
        for tag in [10, 11, 12, 13, 14, 16, 18, 20, 30, 31, 32, 201, 202, 203] {
            if let Some(control) = self.control(tag) {
                let enabled = !busy && (!(tag == 11 || tag == 12) || !password.is_empty());
                let _: () = msg_send![control,setEnabled:enabled];
            }
        }
        let mut focused = 0isize;
        if let Some(window) = &self.window {
            let responder: *mut AnyObject = msg_send![window, firstResponder];
            if !responder.is_null() {
                let has_delegate: bool = msg_send![responder,respondsToSelector:sel!(delegate)];
                if has_delegate {
                    let editor: *mut AnyObject = msg_send![responder, delegate];
                    for id in [201, 202, 203] {
                        if self
                            .control(id)
                            .is_some_and(|field| field as *const _ as *mut AnyObject == editor)
                        {
                            focused = if id == 203 { 202 } else { id };
                        }
                    }
                }
            }
        }
        let changed = appearance::update(appearance::Canvas {
            page: self.page as u8,
            home_extra: self.home_extra,
            info_extra: self.info_extra,
            tone: view.tone,
            info_tone: summary.tone,
            action: view.action,
            busy,
            spin: if busy {
                (self.tick % 4) as f64 * 90.
            } else {
                0.
            },
            focus_field: focused,
            invalid_field: self.invalid_field,
        });
        if changed {
            if let Some(document) = &self.document {
                let _: () = msg_send![document,setNeedsDisplay:true];
            }
            for (_, control) in &self.controls {
                let _: () = msg_send![control,setNeedsDisplay:true];
            }
        }
        if self.window.is_some() {
            self.layout();
        }
        let context = tray_model::Context {
            configured: !password.is_empty(),
            paused: state.reconnect_paused,
            restricted: state.paused_until > portal::unix_now(),
            no_internet: state.no_internet,
            guardian: state.guardian,
            busy,
            link: tray::link(),
        };
        let key = format!(
            "{}:{:?}:{}:{}:{}:{}",
            tray_model::status(&context),
            context.link,
            context.busy,
            context.paused,
            context.configured,
            context.guardian
        );
        if key != self.menu_key {
            self.menu_key = key;
            let menu: Object = msg_send![class!(NSMenu), new];
            let _: () = msg_send![&menu, setAutoenablesItems: false];
            for entry in tray_model::menu(&context) {
                let item: Object = match entry {
                    tray_model::Entry::Separator => msg_send![class!(NSMenuItem), separatorItem],
                    tray_model::Entry::Status(text) => {
                        let item = self.menu_item(text, 0);
                        let _: () = msg_send![&item, setEnabled: false];
                        item
                    }
                    tray_model::Entry::Action {
                        command,
                        label,
                        enabled,
                        checked,
                    } => {
                        let tag = match command {
                            tray_model::Command::Open => 0,
                            tray_model::Command::Settings => 1,
                            tray_model::Command::Automatic => 11,
                            tray_model::Command::Disconnect => 21,
                            tray_model::Command::Retry => 20,
                            tray_model::Command::Quit => 19,
                            tray_model::Command::Guardian => 41,
                            tray_model::Command::Devices => 42,
                        };
                        let item = self.menu_item(label, tag);
                        let _: () = msg_send![&item, setEnabled: enabled];
                        let _: () =
                            msg_send![&item, setState: if checked { 1isize } else { 0isize }];
                        item
                    }
                };
                let _: () = msg_send![&menu, addItem: &*item];
            }
            let _: () = msg_send![&self.status_item, setMenu: &*menu];
            let button: *mut AnyObject = msg_send![&self.status_item, button];
            let _: () =
                msg_send![button, setToolTip: &*NSString::from_str(tray_model::status(&context))];
        }
    }
    unsafe fn menu_item(&self, title: &str, tag: isize) -> Object {
        let item: Allocated<AnyObject> = msg_send![class!(NSMenuItem), alloc];
        let item: Object = msg_send![item, initWithTitle: &*NSString::from_str(title), action: sel!(perform:), keyEquivalent: &*NSString::new()];
        let _: () = msg_send![&item, setTag: tag];
        let _: () = msg_send![&item, setTarget: &*self.controller];
        item
    }
    unsafe fn action(&mut self, tag: isize) {
        if tag == 41 {
            if config::try_update_state(|s| {
                s.guardian = !s.guardian;
                s.guardian_failures = 0;
                s.net_bad = 0;
                s.no_internet = false;
            })
            .is_err()
            {
                tray::failed("无法保存守卫者设置");
            }
            return;
        }
        if tag == 42 {
            let port = config::load_state().port;
            if portal::PORTS.contains(&port) && portal::host() == portal::HOST {
                let url = format!("http://{}:{port}/snnuportal/userstatus.jsp", portal::HOST);
                if std::process::Command::new("/usr/bin/open").arg(url).spawn().is_err() {
                    tray::failed("无法打开在线设备管理页面");
                }
            } else {
                tray::failed("尚未连接到支持设备管理的校园网门户");
            }
            return;
        }
        if runtime::BUSY.load(Ordering::SeqCst)
            && matches!(tag, 10..=14 | 16 | 18 | 20 | 21 | 30..=32)
        {
            return;
        }
        match tag {
            0..=5 => self.navigate(match tag {
                1 => Page::Settings,
                2 => Page::Account,
                3 => Page::Network,
                4 => Page::Features,
                5 => Page::Help,
                _ => Page::Home,
            }),
            10 => {
                let (cfg, pwd) = config::load_config();
                let state = config::load_state();
                let view = ui_model::present(
                    &runtime::status(),
                    !pwd.is_empty(),
                    agent::running(),
                    state.reconnect_paused,
                    state.no_internet,
                    &state.last_error,
                    &cfg.account,
                )
                .check_operator(
                    &runtime::status(),
                    &cfg.account,
                    &cfg.operator,
                    state.reconnect_paused,
                );
                match view.action {
                    ui_model::Action::Account => self.navigate(Page::Account),
                    ui_model::Action::Connect => job(connect),
                    ui_model::Action::Disconnect => job(disconnect),
                }
            }
            11 => {
                let paused = !config::load_state().reconnect_paused;
                job(move || {
                    agent::pause_automatic(paused)?;
                    Ok(if paused {
                        "自动登录已关闭，当前连接保留"
                    } else {
                        "自动登录已开启"
                    }
                    .into())
                });
            }
            12 => {
                let result = autostart::set_enabled(!autostart::registered());
                self.note_error = result.is_err();
                self.note_until = result
                    .is_ok()
                    .then(|| Instant::now() + Duration::from_secs(4));
                self.note = match result {
                    Ok(()) => "登录启动设置已更新".into(),
                    Err(e) => e,
                };
            }
            13 => {
                if runtime::BUSY.load(Ordering::SeqCst) {
                    return;
                }
                let account = string(self.control(201).unwrap()).trim().to_string();
                let password = string(
                    self.control(if self.password_visible { 203 } else { 202 })
                        .unwrap(),
                );
                let index = self.operator_index;
                let operator = match index {
                    1 => "mobile",
                    2 => "unicom",
                    _ => "",
                };
                let (old, old_password) = config::load_config();
                if !old_password.is_empty()
                    && !ui_model::account_changed(
                        &old.account,
                        &old_password,
                        &old.operator,
                        &account,
                        &password,
                        operator,
                    )
                {
                    self.account_draft = None;
                    self.open(Page::Home);
                    return;
                }
                if account.len() < 4
                    || account.len() > 20
                    || !account.chars().all(|c| c.is_ascii_alphanumeric())
                {
                    self.invalid_field = 201;
                    self.note_error = true;
                    self.note = "学号需为 4–20 位字母或数字".into();
                    self.refresh();
                    self.focus_field(201);
                    return;
                }
                if password.is_empty() || password.contains(';') {
                    self.invalid_field = 202;
                    self.note_error = true;
                    self.note = if password.is_empty() {
                        "请输入校园网密码"
                    } else {
                        "门户不支持包含分号的密码"
                    }
                    .into();
                    self.refresh();
                    self.focus_field(if self.password_visible { 203 } else { 202 });
                    return;
                }
                let online = matches!(runtime::status(), portal::Status::Online(_));
                if online {
                    let alert: Object = msg_send![class!(NSAlert), new];
                    let _: () =
                        msg_send![&alert, setMessageText: &*NSString::from_str("保存并重新连接？")];
                    let _: () = msg_send![&alert, setInformativeText: &*NSString::from_str("保存后会短暂断网，并使用新设置重新连接。")];
                    let _: *mut AnyObject =
                        msg_send![&alert, addButtonWithTitle: &*NSString::from_str("取消")];
                    let _: *mut AnyObject =
                        msg_send![&alert, addButtonWithTitle: &*NSString::from_str("重新连接")];
                    let choice: isize = msg_send![&alert, runModal];
                    if choice != 1001 {
                        return;
                    }
                }
                if self.smoke.is_none() && self.measure.is_none() {
                    request_notifications();
                }
                self.after_job = Some(Page::Home);
                job(move || save_and_connect(&account, &password, operator, online));
            }
            14 => {
                let alert: Object = msg_send![class!(NSAlert), new];
                let _: () = msg_send![&alert, setMessageText: &*NSString::from_str("删除本机保存的账号和密码？")];
                let _: () = msg_send![&alert, setInformativeText: &*NSString::from_str("会关闭自动登录，并删除钥匙串中本应用保存的密码。当前网络连接保留。")];
                let _: *mut AnyObject =
                    msg_send![&alert, addButtonWithTitle: &*NSString::from_str("取消")];
                let _: *mut AnyObject =
                    msg_send![&alert, addButtonWithTitle: &*NSString::from_str("删除")];
                let choice: isize = msg_send![&alert, runModal];
                if choice == 1001 {
                    self.after_job = Some(Page::Account);
                    job(|| {
                        storage::delete_account().map_err(|e| e.to_string())?;
                        Ok("保存的账号和密码已删除".into())
                    });
                }
            }
            15 => self.export(),
            16 => {
                self.password_visible = !self.password_visible;
                let (source, target) = if self.password_visible {
                    (202, 203)
                } else {
                    (203, 202)
                };
                set_text(
                    self.control(target).unwrap(),
                    &string(self.control(source).unwrap()),
                );
                let _: () = msg_send![self.control(source).unwrap(), setHidden: true];
                let _: () = msg_send![self.control(target).unwrap(), setHidden: false];
                if !self.password_visible {
                    set_text(self.control(203).unwrap(), "");
                }
            }
            17 => {
                if self.location.is_none() {
                    let manager: Object = msg_send![class!(CLLocationManager), new];
                    self.location = Some(manager);
                }
                let _: () = msg_send![
                    self.location.as_ref().unwrap(),
                    requestWhenInUseAuthorization
                ];
                self.note = "授权后重新打开网络信息即可显示 Wi-Fi 名称".into();
            }
            18 => {
                storage::retry_keychain();
                job(|| {
                    let (_, pwd) = config::load_config();
                    if pwd.is_empty() {
                        Err(storage::last_error()
                            .unwrap_or_else(|| "没有读取到保存的密码，请重新填写并保存".into()))
                    } else {
                        Ok("钥匙串密码已读取，可重新打开账号设置查看".into())
                    }
                });
            }
            19 => {
                runtime::stop();
                let _: () = msg_send![&self.app, terminate: std::ptr::null::<AnyObject>()];
            }
            20 => job(connect),
            21 => job(disconnect),
            40 => {
                let target = if self.page == Page::Settings || config::load_config().1.is_empty() {
                    Page::Home
                } else {
                    Page::Settings
                };
                self.navigate(target);
            }
            30..=32 => {
                self.operator_index = tag - 30;
            }
            22 => {
                if !self.confirm_discard() {
                    return;
                }
                self.account_draft = None;
                if let Some(window) = self.window.take() {
                    self.controls.clear();
                    self.document.take();
                    self.scroller.take();
                    let _: () = msg_send![&window, close];
                }
            }
            _ => {}
        }
        self.refresh();
    }
    unsafe fn export(&mut self) {
        let panel: Object = msg_send![class!(NSSavePanel), savePanel];
        let _: () = msg_send![&panel, setNameFieldStringValue: &*NSString::from_str(&format!("SNNU诊断报告-{}.txt", config::stamp_compact()))];
        let response: isize = msg_send![&panel, runModal];
        if response != 1 {
            return;
        }
        let url: *mut AnyObject = msg_send![&panel, URL];
        let path: Retained<NSString> = msg_send![url, path];
        let path = PathBuf::from(path.to_string());
        job(move || {
            std::fs::write(&path, diagnostic()).map_err(|e| e.to_string())?;
            Ok(format!("诊断已导出到 {}", path.display()))
        });
    }
    unsafe fn screenshot(&self, path: &Path) {
        let window = self.window.as_ref().unwrap();
        let _ = window;
        let view = &**self.document.as_ref().unwrap();
        let bounds: NSRect = msg_send![view, bounds];
        let bitmap: Object = msg_send![view, bitmapImageRepForCachingDisplayInRect: bounds];
        let _: () = msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: &*bitmap];
        assert_eq!(
            bounds.size.width, WIDTH,
            "桌面界面保持 Windows 版的 380 逻辑宽度"
        );
        if self.page == Page::Home {
            let button = self.control(10).unwrap();
            let frame: NSRect = msg_send![button, frame];
            assert_eq!(
                (
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height
                ),
                (122., 72., 136., 136.)
            );
            let parent: *mut AnyObject = msg_send![view, superview];
            let point: NSPoint = msg_send![view,convertPoint:NSPoint::new(190.,140.),toView:parent];
            let center: *mut AnyObject = msg_send![view,hitTest:point];
            assert_eq!(
                center, button as *const _ as *mut AnyObject,
                "圆形连接按钮中心可以点击"
            );
            let point: NSPoint = msg_send![view,convertPoint:NSPoint::new(123.,73.),toView:parent];
            let corner: *mut AnyObject = msg_send![view,hitTest:point];
            assert_ne!(
                corner, button as *const _ as *mut AnyObject,
                "圆形按钮外侧不能触发连接"
            );
        }
        let corner: Object = msg_send![&bitmap, colorAtX: 0isize, y: 0isize];
        let alpha: f64 = msg_send![&corner, alphaComponent];
        assert!(alpha > 0.99, "界面截图必须包含不透明的系统背景");
        let dict: Object = msg_send![class!(NSDictionary), new];
        let srgb: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
        let portable: Object =
            msg_send![&bitmap,bitmapImageRepByConvertingToColorSpace:srgb,renderingIntent:0usize];
        let png: Object = msg_send![&portable, representationUsingType: 4usize, properties: &*dict];
        let written: bool = msg_send![&png, writeToFile: &*NSString::from_str(&path.to_string_lossy()), atomically: true];
        assert!(written, "界面截图写入失败");
        let mut controls = Vec::new();
        for (id, control) in &self.controls {
            let frame: NSRect = msg_send![control, frame];
            controls.push(serde_json::json!({"id":id,"x":frame.origin.x,"y":frame.origin.y,"width":frame.size.width,"height":frame.size.height}));
        }
        let geometry = serde_json::json!({"width":bounds.size.width,"height":bounds.size.height,"controls":controls,"windows_palette":true});
        std::fs::write(
            path.with_extension("layout.json"),
            serde_json::to_vec_pretty(&geometry).unwrap(),
        )
        .unwrap();
    }
    unsafe fn check_account_controls(&mut self, dir: &Path) {
        let (cfg, password) = config::load_config();
        assert_eq!(string(self.control(201).unwrap()), cfg.account);
        assert_eq!(string(self.control(202).unwrap()), password);
        let title: Retained<NSString> = msg_send![self.control(13).unwrap(), title];
        assert_eq!(title.to_string(), "完成");
        self.action(16);
        assert!(self.password_visible);
        assert_eq!(string(self.control(203).unwrap()), password);
        set_text(self.control(203).unwrap(), "draft-password");
        self.action(16);
        assert!(!self.password_visible);
        assert_eq!(string(self.control(202).unwrap()), "draft-password");
        assert!(string(self.control(203).unwrap()).is_empty());
        set_text(self.control(201).unwrap(), "draft123");
        self.action(32);
        self.action(40);
        assert!(self.page == Page::Settings);
        self.action(2);
        assert_eq!(string(self.control(201).unwrap()), "draft123");
        assert_eq!(string(self.control(202).unwrap()), "draft-password");
        assert_eq!(self.operator_index, 2);
        set_text(self.control(201).unwrap(), "!");
        self.action(13);
        assert_eq!(self.invalid_field, 201);
        assert!(self.note_error);
        assert_eq!(self.note, "学号需为 4–20 位字母或数字");
        self.screenshot(&dir.join("account-invalid.png"));
        set_text(self.control(201).unwrap(), &cfg.account);
        set_text(self.control(202).unwrap(), ";");
        self.action(13);
        assert_eq!(self.invalid_field, 202);
        assert_eq!(self.note, "门户不支持包含分号的密码");
        self.account_draft = None;
        self.note.clear();
        self.note_error = false;
        self.open(Page::Account);
        self.action(13); // Unchanged credentials finish without reconnecting.
        assert!(self.page == Page::Home);
        assert!(!runtime::BUSY.load(Ordering::SeqCst));
    }
    unsafe fn tick(&mut self) {
        self.tick += 1;
        // Defer releasing the final window owner until AppKit's close callback
        // has returned. Reopening before this tick cancels the deferred cleanup.
        if self.window_closed {
            self.window.take();
            self.controls.clear();
            self.document.take();
            self.scroller.take();
            self.window_closed = false;
        }
        if runtime::OPEN_REQUESTED.swap(false, Ordering::SeqCst) {
            self.navigate(Page::Home);
        }
        if runtime::quitting() {
            let _: () = msg_send![&self.app, terminate: std::ptr::null::<AnyObject>()];
            return;
        }
        let result = RESULT.lock().unwrap().take();
        if let Some(result) = result {
            let next = self.after_job.take();
            if result.is_ok() && next.is_some() {
                self.account_draft = None;
                if self.window.is_some() {
                    self.open(next.unwrap());
                }
            }
            self.note_error = result.is_err();
            self.note_until = result
                .is_ok()
                .then(|| Instant::now() + Duration::from_secs(4));
            self.note = result.unwrap_or_else(|error| error);
            if self.note_error
                && self.page == Page::Account
                && (self.note.contains("密码") || self.note.contains("验证码"))
            {
                self.invalid_field = 202;
                self.focus_field(if self.password_visible { 203 } else { 202 });
            }
        }
        if !runtime::BUSY.load(Ordering::SeqCst)
            && self.note_until.is_some_and(|until| Instant::now() >= until)
        {
            self.note.clear();
            self.note_until = None;
        }
        if tray::expire_notice() {
            clear_notification();
        }
        if let Some(notice) = tray::take_notice() {
            self.note = format!("{}：{}", notice.title, notice.text);
            if self.smoke.is_none() && self.measure.is_none() {
                send_notification(&notice.title, &notice.text);
            }
        }
        self.refresh();
        if self.tick == 3
            && self
                .measure
                .as_ref()
                .is_some_and(|(scenario, _, _)| scenario != "online-window")
        {
            if let Some(window) = self.window.take() {
                self.controls.clear();
                self.document.take();
                self.scroller.take();
                let _: () = msg_send![&window, close];
            }
        }
        if let Some(dir) = self.smoke.clone() {
            match self.tick {
                1..=6 => {
                    let (page, name) = match self.tick {
                        1 => (Page::Home, "home"),
                        2 => (Page::Settings, "settings"),
                        3 => (Page::Account, "account"),
                        4 => (Page::Network, "network"),
                        5 => (Page::Features, "features"),
                        _ => (Page::Help, "help"),
                    };
                    self.open(page);
                    self.screenshot(&dir.join(format!("{name}.png")));
                    if self.tick == 3 {
                        self.check_account_controls(&dir);
                    }
                }
                7 => {
                    self.action(11);
                    // A pending save must not reopen a window closed by the user.
                    self.after_job = Some(Page::Home);
                    let menu: *mut AnyObject = msg_send![&self.status_item, menu];
                    let count: isize = msg_send![menu, numberOfItems];
                    assert!(count >= 8);
                }
                8 => {
                    assert!(self.window.is_none());
                    assert!(agent::running());
                    assert!(config::load_state().reconnect_paused);
                    self.open(Page::Home);
                    self.action(11);
                }
                9 => {
                    assert!(self.window.is_some());
                    assert!(!config::load_state().reconnect_paused);
                    let report = serde_json::json!({"native_window":true,"menu_bar":true,"automatic_toggle":true,"close_keeps_worker":true,"reopen":true,"password_visibility":true,"draft_preserved_on_navigation":true,"invalid_input_feedback":true,"unchanged_save_does_not_reconnect":true,"screenshots":7,"architecture":std::env::consts::ARCH});
                    std::fs::write(
                        dir.join("smoke.json"),
                        serde_json::to_vec_pretty(&report).unwrap(),
                    )
                    .unwrap();
                    self.action(19);
                }
                _ => {}
            }
        }
        if let Some((scenario, path, seconds)) = self.measure.clone() {
            // Give framework initialization and portal discovery time to settle.
            if self.tick > 10 && self.tick % 2 == 0 {
                self.samples.push(resource_sample());
            }
            if self.started.elapsed().as_secs() >= seconds + 10 {
                let report = serde_json::json!({"scenario":scenario,"architecture":std::env::consts::ARCH,
                    "sample_interval_seconds":2,"samples":self.samples,"simulated_portal":true});
                std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
                self.action(19);
            }
        }
    }
}
fn job(task: impl FnOnce() -> Result<String, String> + Send + 'static) {
    if runtime::BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    runtime::EPOCH.advance();
    std::thread::spawn(move || {
        autoreleasepool(|_| {
            let result = task();
            let (status, _) = portal::status(Duration::from_secs(4));
            runtime::publish(status);
            *RESULT.lock().unwrap() = Some(result);
            runtime::EPOCH.advance();
            runtime::BUSY.store(false, Ordering::SeqCst);
            runtime::wake();
        })
    });
}
fn connect() -> Result<String, String> {
    agent::connect_now().map(|outcome| match outcome {
        agent::ConnectOutcome::Online => "校园网已连接".into(),
        agent::ConnectOutcome::Waiting => "自动登录已开启，等待连接学校网络".into(),
    })
}
pub(super) fn save_and_connect(
    account: &str,
    password: &str,
    operator: &str,
    confirmed_online: bool,
) -> Result<String, String> {
    let was_paused = config::load_state().reconnect_paused;
    agent::pause_automatic(true)?;
    let result = (|| {
        let _connection = portal::connection_lock()?;
        let (status, port) = portal::status(Duration::from_secs(4));
        match status {
            portal::Status::Unreachable => return Err("请先连接学校网络，再保存账号".into()),
            portal::Status::Online(_) => {
                if !confirmed_online {
                    return Err("当前已恢复连接，请再次保存以确认重新连接".into());
                }
                if !portal::logout(port) {
                    return Err("当前连接未能断开，原账号设置已保留".into());
                }
            }
            portal::Status::Offline => {}
        }
        portal::login(account, password, operator, port)
            .map_err(|e| ui_model::clean_copy(&portal::friendly_login_error(&e)))?;
        let (old, old_password) = config::load_config();
        let cfg = config::Config {
            account: account.into(),
            operator: operator.into(),
            keepalive: if old_password.is_empty() {
                true
            } else {
                old.keepalive
            },
            keepalive_min: if old.keepalive_min == 0 {
                15
            } else {
                old.keepalive_min.max(5)
            },
            password_enc: String::new(),
        };
        config::save_config(&cfg, password).map_err(|e| format!("账号保存失败：{e}"))?;
        config::try_update_state(|s| {
            s.reconnect_paused = false;
            s.paused_until = 0.;
            s.last_error.clear();
        })
        .map_err(|e| e.to_string())?;
        runtime::wake();
        if old_password.is_empty() && std::env::var_os("SNNU_DATA_DIR").is_none() {
            if let Err(error) = autostart::set_enabled(true) {
                return Ok(format!("已连接；登录启动尚未开启：{error}"));
            }
        }
        Ok("账号已保存，校园网已连接".into())
    })();
    if result.is_err() {
        config::update_state(|s| s.reconnect_paused = was_paused);
        runtime::wake();
    }
    result
}
fn disconnect() -> Result<String, String> {
    if agent::logout_now() {
        Ok("校园网认证已断开，自动登录已关闭".into())
    } else {
        Err("自动登录已暂停，未能确认断开结果；请检查网络".into())
    }
}
fn network_text() -> String {
    let status = runtime::status();
    let account = config::load_config().0.account;
    match status {
        portal::Status::Online(session) => format!(
            "网络：{}\n\n账号：{}\n\n上网方式：{}\n\nIP：{}\n\n在线时长：{}",
            network::current_name(),
            portal::mask(&session.account),
            operator_label(&portal::split_account(&session.account).1),
            session.ip,
            portal::online_minutes(&session.login_time)
                .map(portal::fmt_duration)
                .unwrap_or_else(|| "暂不可读".into())
        ),
        _ => format!(
            "网络：{}\n\n保存账号：{}\n\n状态：{}\n\n认证地址：{}",
            network::current_name(),
            portal::mask(&account),
            if status == portal::Status::Offline {
                "等待认证"
            } else {
                "等待校园网或网络授权"
            },
            portal::host()
        ),
    }
}
fn operator_code(index: isize) -> &'static str {
    match index {
        1 => "mobile",
        2 => "unicom",
        _ => "",
    }
}
fn network_values() -> [String; 5] {
    let status = runtime::status();
    let cfg = config::load_config().0;
    let (account, operator, ip, duration) = match status {
        portal::Status::Online(session) => {
            let (account, operator) = portal::split_account(&session.account);
            (
                account,
                operator,
                if session.ip.is_empty() {
                    "暂不可读".into()
                } else {
                    session.ip
                },
                portal::online_minutes(&session.login_time)
                    .map(portal::fmt_duration)
                    .unwrap_or_else(|| "暂不可读".into()),
            )
        }
        _ => (cfg.account, cfg.operator, "—".into(), "—".into()),
    };
    [
        network::current_name(),
        if account.is_empty() {
            "尚未设置".into()
        } else {
            portal::mask(&account)
        },
        if account.is_empty() {
            "—".into()
        } else {
            operator_label(&operator).into()
        },
        ip,
        duration,
    ]
}
fn operator_label(operator: &str) -> &'static str {
    match operator {
        "mobile" => "移动",
        "unicom" => "联通",
        _ => "校园网",
    }
}
pub fn diagnostic() -> String {
    let (cfg, _) = config::load_config();
    let state = config::load_state();
    let mut report = format!("SNNU-AutoLogin v{} macOS {}\n生成时间：{}\n{}\n自动登录：{}\n登录启动：{}\n最近错误：{}\n\n最近日志：\n", crate::VERSION, std::env::consts::ARCH,
        config::now_stamp(), network_text(), !state.reconnect_paused, autostart::enabled(), portal::scrub(&state.last_error, &cfg.account));
    if let Ok(log) = std::fs::read_to_string(config::log_path()) {
        let tail: Vec<_> = log.lines().rev().take(40).collect();
        for line in tail.into_iter().rev() {
            report.push_str(&portal::scrub(line, &cfg.account));
            report.push('\n');
        }
    }
    report
}
fn resource_sample() -> serde_json::Value {
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        let code = libc::proc_pid_rusage(
            libc::getpid(),
            2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        );
        assert_eq!(code, 0, "进程资源采样失败");
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        let mut task: libc::proc_taskinfo = std::mem::zeroed();
        let size = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&mut task as *mut libc::proc_taskinfo).cast(),
            std::mem::size_of::<libc::proc_taskinfo>() as i32,
        );
        assert_eq!(size as usize, std::mem::size_of::<libc::proc_taskinfo>());
        serde_json::json!({"time":portal::unix_now(),"rss_bytes":info.ri_resident_size,"physical_footprint_bytes":info.ri_phys_footprint,
            "cpu_seconds": usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1e6 + usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1e6,
            "idle_wakeups":info.ri_pkg_idle_wkups,"interrupt_wakeups":info.ri_interrupt_wkups,"thread_count":task.pti_threadnum})
    }
}
unsafe extern "C-unwind" fn fields_changed(_: &AnyObject, selector: Sel, _: &AnyObject) {
    with_ui(|ui| {
        if selector == sel!(controlTextDidChange:) && !runtime::BUSY.load(Ordering::SeqCst) {
            ui.invalid_field = 0;
            if ui.note_error {
                ui.note.clear();
                ui.note_error = false;
                ui.note_until = None;
            }
        }
        ui.refresh();
    });
}
unsafe extern "C-unwind" fn perform(_: &AnyObject, _: Sel, sender: &AnyObject) {
    let tag: isize = msg_send![sender, tag];
    autoreleasepool(|_| with_ui(|ui| ui.action(tag)));
}
unsafe extern "C-unwind" fn edit_command(
    _: &AnyObject,
    _: Sel,
    _: &AnyObject,
    _: &AnyObject,
    command: Sel,
) -> Bool {
    if command == sel!(insertNewline:) {
        with_ui(|ui| ui.action(13));
        Bool::YES
    } else {
        Bool::NO
    }
}
unsafe extern "C-unwind" fn tick(_: &AnyObject, _: Sel, _: &AnyObject) {
    autoreleasepool(|_| {
        with_ui(|ui| ui.tick());
        // Close through the actual AppKit delegate outside the UI state borrow.
        // This validates the same lifetime path as clicking the window's close button.
        let window = UI.with(|cell| {
            cell.borrow().as_ref().and_then(|ui| {
                if ui.smoke.is_some() && ui.tick == 7 {
                    ui.window.clone()
                } else {
                    None
                }
            })
        });
        if let Some(window) = window {
            let _: () = msg_send![&window, performClose: std::ptr::null::<AnyObject>()];
        }
    });
}
unsafe extern "C-unwind" fn closed(_: &AnyObject, _: Sel, _: &AnyObject) {
    with_ui(|ui| {
        ui.window_closed = true;
        ui.account_draft = None;
    });
}
unsafe extern "C-unwind" fn should_close(_: &AnyObject, _: Sel, _: &AnyObject) -> Bool {
    let mut allow = true;
    with_ui(|ui| allow = ui.confirm_discard());
    Bool::new(allow)
}
unsafe extern "C-unwind" fn resume(_: &AnyObject, _: Sel, _: &AnyObject) {
    runtime::wake();
}
unsafe extern "C-unwind" fn reopen(_: &AnyObject, _: Sel, _: &AnyObject, _: Bool) -> Bool {
    with_ui(|ui| ui.navigate(Page::Home));
    Bool::YES
}
fn with_ui(f: impl FnOnce(&mut Ui)) {
    UI.with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            if let Some(ui) = state.as_mut() {
                f(ui);
            }
        }
    });
}
unsafe fn controller_class() -> &'static AnyClass {
    appearance::register_classes();
    let mut class = ClassBuilder::new(c"SNNUController", class!(NSObject)).unwrap();
    // Some AppKit releases don't publish these informal delegate protocols in
    // the runtime registry. Delegate dispatch uses respondsToSelector regardless.
    for name in [c"NSApplicationDelegate", c"NSWindowDelegate"] {
        if let Some(protocol) = AnyProtocol::get(name) {
            class.add_protocol(protocol);
        }
    }
    class.add_method(
        sel!(perform:),
        perform as unsafe extern "C-unwind" fn(_, _, _),
    );
    class.add_method(sel!(tick:), tick as unsafe extern "C-unwind" fn(_, _, _));
    class.add_method(
        sel!(control:textView:doCommandBySelector:),
        edit_command as unsafe extern "C-unwind" fn(_, _, _, _, _) -> _,
    );
    for selector in [
        sel!(controlTextDidChange:),
        sel!(controlTextDidBeginEditing:),
        sel!(controlTextDidEndEditing:),
    ] {
        class.add_method(
            selector,
            fields_changed as unsafe extern "C-unwind" fn(_, _, _),
        );
    }
    class.add_method(
        sel!(windowWillClose:),
        closed as unsafe extern "C-unwind" fn(_, _, _),
    );
    class.add_method(
        sel!(windowShouldClose:),
        should_close as unsafe extern "C-unwind" fn(_, _, _) -> _,
    );
    class.add_method(
        sel!(didWake:),
        resume as unsafe extern "C-unwind" fn(_, _, _),
    );
    class.add_method(
        sel!(applicationShouldHandleReopen:hasVisibleWindows:),
        reopen as unsafe extern "C-unwind" fn(_, _, _, _) -> _,
    );
    class.register()
}
pub fn run(args: &[String]) {
    autoreleasepool(|_| unsafe {
        let instance = match crate::process_lock::ProcessLock::acquire(
            &config::app_dir().join("application"),
            Duration::ZERO,
        ) {
            Ok(lock) => lock,
            Err(_) => {
                use std::io::Write;
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
                        &std::net::SocketAddr::from(([127, 0, 0, 1], agent::ctl_port())),
                        Duration::from_millis(200),
                    ) {
                        let _ = stream.write_all(b"O");
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                return;
            }
        };
        let app: Object = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![&app, setActivationPolicy: 1isize];
        let light: Object = msg_send![class!(NSAppearance),appearanceNamed:&*NSString::from_str("NSAppearanceNameAqua")];
        let _: () = msg_send![&app,setAppearance:&*light];
        let controller: Object = msg_send![controller_class(), new];
        let _: () = msg_send![&app, setDelegate: &*controller];
        let bar: *mut AnyObject = msg_send![class!(NSStatusBar), systemStatusBar];
        let status_item: Object = msg_send![bar, statusItemWithLength: -1.0f64];
        let button: *mut AnyObject = msg_send![&status_item, button];
        let image: *mut AnyObject = msg_send![class!(NSImage), imageWithSystemSymbolName: &*NSString::from_str("network"), accessibilityDescription: &*NSString::from_str("校园网自动登录")];
        let _: () = msg_send![image, setTemplate: true];
        let _: () = msg_send![button, setImage: image];
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let center: *mut AnyObject = msg_send![workspace, notificationCenter];
        let _: () = msg_send![center, addObserver: &*controller, selector: sel!(didWake:), name: &*NSString::from_str("NSWorkspaceDidWakeNotification"), object: std::ptr::null::<AnyObject>()];
        let smoke = args
            .iter()
            .position(|arg| arg == "--smoke-test")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        let measure = args.iter().position(|arg| arg == "--measure").map(|i| {
            (
                args[i + 1].clone(),
                PathBuf::from(&args[i + 2]),
                args.get(i + 3).and_then(|s| s.parse().ok()).unwrap_or(60),
            )
        });
        if smoke.is_some() || measure.is_some() {
            assert!(
                std::env::var_os("SNNU_DATA_DIR").is_some(),
                "测试必须使用隔离数据目录"
            );
        }
        if let Some(dir) = &smoke {
            std::fs::create_dir_all(dir).unwrap();
        }
        let hidden = args.iter().any(|arg| arg == "--daemon") || launched_at_login();
        if smoke.is_none() && measure.is_none() && !config::load_config().0.account.is_empty() {
            request_notifications();
        }
        UI.with(|cell| {
            *cell.borrow_mut() = Some(Ui {
                app: app.clone(),
                controller: controller.clone(),
                status_item,
                window: None,
                window_closed: false,
                document: None,
                scroller: None,
                home_extra: 0.,
                info_extra: 0.,
                account_saved: false,
                operator_index: 0,
                password_visible: false,
                after_job: None,
                account_draft: None,
                layout_key: String::new(),
                invalid_field: 0,
                controls: Vec::new(),
                page: Page::Home,
                menu_key: String::new(),
                note: String::new(),
                note_error: false,
                note_until: None,
                location: None,
                tick: 0,
                smoke,
                started: Instant::now(),
                measure,
                samples: Vec::new(),
            })
        });
        network::watch();
        runtime::start();
        let main_menu: Object = msg_send![class!(NSMenu), new];
        let root: Object = msg_send![class!(NSMenuItem), new];
        let app_menu: Object = msg_send![class!(NSMenu), new];
        with_ui(|ui| {
            for (title, tag) in [("使用帮助", 5), ("重新读取钥匙串", 18)] {
                let item = ui.menu_item(title, tag);
                let _: () = msg_send![&app_menu,addItem:&*item];
            }
            let close = ui.menu_item("关闭窗口", 22);
            let _: () = msg_send![&close, setKeyEquivalent: &*NSString::from_str("w")];
            let _: () = msg_send![&app_menu, addItem: &*close];
            let quit = ui.menu_item("退出校园网自动登录", 19);
            let _: () = msg_send![&quit, setKeyEquivalent: &*NSString::from_str("q")];
            let _: () = msg_send![&app_menu, addItem: &*quit];
        });
        let _: () = msg_send![&root, setSubmenu: &*app_menu];
        let _: () = msg_send![&main_menu, addItem: &*root];
        let _: () = msg_send![&app, setMainMenu: &*main_menu];
        let _: () = msg_send![&app, finishLaunching];
        with_ui(|ui| {
            if !hidden {
                ui.open(Page::Home);
            } else {
                ui.refresh();
            }
        });
        if let Some(i) = args.iter().position(|arg| arg == "--ready-file") {
            if std::env::var_os("SNNU_DATA_DIR").is_some() {
                std::fs::write(&args[i + 1], b"ready").unwrap();
            }
        }
        let timer: Object = msg_send![class!(NSTimer), scheduledTimerWithTimeInterval: 1.0f64, target: &*controller, selector: sel!(tick:), userInfo: std::ptr::null::<AnyObject>(), repeats: true];
        let _: () = msg_send![&app, run];
        let _: () = msg_send![&timer, invalidate];
        runtime::stop();
        drop(instance);
    });
}
unsafe fn request_notifications() {
    let center: *mut AnyObject =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
    let completion = block2::RcBlock::new(|granted: Bool, _: *mut AnyObject| {
        NOTIFICATIONS_ALLOWED.store(granted.as_bool(), Ordering::SeqCst);
    });
    let _: () =
        msg_send![center, requestAuthorizationWithOptions: 6usize, completionHandler: &*completion];
}
unsafe fn send_notification(title: &str, text: &str) {
    if !NOTIFICATIONS_ALLOWED.load(Ordering::SeqCst) {
        return;
    }
    let content: Object = msg_send![class!(UNMutableNotificationContent), new];
    let _: () = msg_send![&content, setTitle: &*NSString::from_str(title)];
    let _: () = msg_send![&content, setBody: &*NSString::from_str(text)];
    let request: Object = msg_send![class!(UNNotificationRequest), requestWithIdentifier: &*NSString::from_str("connection"), content: &*content, trigger: std::ptr::null::<AnyObject>()];
    let center: *mut AnyObject =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
    let _: () = msg_send![center, addNotificationRequest: &*request, withCompletionHandler: std::ptr::null::<AnyObject>()];
}
unsafe fn clear_notification() {
    let identifier = NSString::from_str("connection");
    let ids: Object = msg_send![class!(NSArray), arrayWithObject: &*identifier];
    let center: *mut AnyObject =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
    let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: &*ids];
}
unsafe fn launched_at_login() -> bool {
    let manager: *mut AnyObject = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
    let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
    if event.is_null() {
        return false;
    }
    let property: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: 0x70726f70u32];
    if property.is_null() {
        return false;
    }
    let value: u32 = msg_send![property, enumCodeValue];
    value == 0x6c676974
}
