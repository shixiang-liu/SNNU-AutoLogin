//! 将网络和自动连接状态整理成用户能理解的一种状态和一个主要操作。
use crate::portal::{self, Status};
use std::sync::atomic::{AtomicU64, Ordering};

/// 操作前后各推进一次版本，丢弃在操作前或操作中开始的旧探测。
pub struct RefreshEpoch(AtomicU64);
impl RefreshEpoch {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    pub fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn accepts(&self, stamp: u64, busy: bool) -> bool {
        !busy && stamp == self.current()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Disconnect,
    Connect,
    Account,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone {
    Good,
    Neutral,
    Warning,
    Error,
}
pub struct View {
    pub title: &'static str,
    pub subtitle: String,
    pub action: Action,
    pub button: &'static str,
    pub footer: &'static str,
    pub tone: Tone,
    pub network_check: bool,
}
impl View {
    #[cfg(windows)]
    pub fn with_wifi_recovery(mut self, enabled: bool, status: &Status, allowed: bool) -> Self {
        if enabled && matches!(status, Status::Unreachable) && self.action != Action::Account {
            self.footer = "只连接已保存的 SNNU，保留其他网络";
            if allowed {
                self.subtitle = "已开启自动连接，等待可用的 SNNU".into();
            }
        }
        self
    }

    pub fn check_operator(
        mut self,
        status: &Status,
        account: &str,
        operator: &str,
        paused: bool,
    ) -> Self {
        if !paused && self.tone == Tone::Good {
            if let Status::Online(session) = status {
                let (user, code) = portal::split_account(&session.account);
                if user == account && code != operator {
                    self.title = "上网方式与设置不同";
                    self.subtitle = "当前连接保留，可在账号设置中核对上网方式".into();
                    self.action = Action::Account;
                    self.button = "查看账号设置";
                    self.tone = Tone::Warning;
                    self.footer = "确认设置与网页登录时的选择一致";
                }
            }
        }
        self
    }
}

pub fn present(
    status: &Status,
    configured: bool,
    running: bool,
    paused: bool,
    no_internet: bool,
    error: &str,
    account: &str,
) -> View {
    let make = |title, sub: &str, action, button, footer, tone| View {
        title,
        subtitle: sub.into(),
        action,
        button,
        footer,
        tone,
        network_check: false,
    };
    if !configured {
        return make(
            if matches!(status, Status::Online(_)) {
                "已连接"
            } else {
                "设置自动登录"
            },
            "设置一次账号，以后自动登录",
            Action::Account,
            "设置账号",
            "账号密码只用于校园网认证",
            Tone::Neutral,
        );
    }
    match status {
        Status::Online(session) => {
            if portal::split_account(&session.account).0 != account {
                return make(
                    "当前使用其他账号",
                    "保留当前连接，不会自动切换账号",
                    Action::Account,
                    "查看账号",
                    "共用路由器时请留意当前在线账号",
                    Tone::Warning,
                );
            }
            if no_internet {
                return View {
                    network_check: true,
                    ..make(
                        "网络暂不可用",
                        "校园网已认证，暂时无法访问外网",
                        Action::Disconnect,
                        "点击断开",
                        "断开会关闭自动登录，Wi-Fi 保持连接",
                        Tone::Warning,
                    )
                };
            }
            make(
                "已连接",
                if paused {
                    "自动登录已关闭，当前连接保持不变"
                } else if !running {
                    "自动登录正在启动"
                } else {
                    "自动登录已开启，掉线后自动恢复"
                },
                Action::Disconnect,
                "点击断开",
                "断开会关闭自动登录，Wi-Fi 保持连接",
                Tone::Good,
            )
        }
        Status::Unreachable => make(
            "等待校园网",
            if paused {
                "自动登录已关闭，可点击开启连接"
            } else {
                "连接学校 Wi-Fi 或网线后会自动登录"
            },
            Action::Connect,
            if paused {
                "点击连接"
            } else {
                "重新检测"
            },
            "这里控制校园网登录，不会切换 Wi-Fi",
            Tone::Neutral,
        ),
        Status::Offline if paused => make(
            "未连接",
            "自动登录已关闭",
            Action::Connect,
            "点击连接",
            "点击连接后，会恢复自动登录和断线重连",
            Tone::Neutral,
        ),
        Status::Offline => match portal::fail_kind(error) {
            portal::FailKind::AccountState => View {
                subtitle: clean_copy(&portal::friendly_login_error(&portal::display_error(error))),
                ..make(
                    "账号需要处理",
                    "",
                    Action::Connect,
                    "处理后重试",
                    "请在校园网自助服务中检查账号状态",
                    Tone::Warning,
                )
            },
            portal::FailKind::Credential => View {
                subtitle: clean_copy(&portal::friendly_login_error(&portal::display_error(error))),
                ..make(
                    "连接未成功",
                    "",
                    Action::Account,
                    "检查账号密码",
                    "修改并保存后会重新验证",
                    Tone::Error,
                )
            },
            portal::FailKind::Captcha => make(
                "需要重新验证",
                "认证页面要求重新验证，可以再次尝试连接",
                Action::Connect,
                "重新验证",
                "反复出现时可稍后再试，或查看连接诊断",
                Tone::Warning,
            ),
            _ => make(
                "未连接",
                if error.is_empty() {
                    "自动登录已开启，也可以立即连接"
                } else {
                    "连接暂未成功，可以再次尝试"
                },
                Action::Connect,
                "点击连接",
                "已保存账号，无需重复填写",
                Tone::Neutral,
            ),
        },
    }
}

pub fn clean_copy(s: &str) -> String {
    s.trim().trim_end_matches(['。', '.']).to_string()
}

pub struct NetworkSummary {
    pub title: &'static str,
    pub description: String,
    pub tone: Tone,
}

pub fn network_summary(
    status: &Status,
    configured: bool,
    paused: bool,
    no_internet: bool,
    error: &str,
    account: &str,
    loading: bool,
) -> NetworkSummary {
    let make = |title, description: &str, tone| NetworkSummary {
        title,
        description: description.into(),
        tone,
    };
    if loading {
        return make("正在检查连接", "状态会自动更新", Tone::Neutral);
    }
    match status {
        Status::Online(session)
            if configured && portal::split_account(&session.account).0 != account =>
        {
            make("当前使用其他账号", "当前连接保持不变", Tone::Warning)
        }
        Status::Online(_) if no_internet => make(
            "已连接，但暂时无法上网",
            "校园网登录仍有效，网络访问暂未恢复",
            Tone::Warning,
        ),
        Status::Online(_) => make(
            "已连接",
            if !configured {
                "尚未设置自动登录账号"
            } else if paused {
                "自动登录已关闭，当前连接保持不变"
            } else {
                "自动登录已开启"
            },
            Tone::Good,
        ),
        Status::Unreachable => make(
            "等待校园网",
            if paused {
                "自动登录已关闭"
            } else {
                "暂未检测到学校认证网络"
            },
            Tone::Neutral,
        ),
        Status::Offline if !configured => make("未连接", "尚未设置自动登录账号", Tone::Neutral),
        Status::Offline if paused => make("未连接", "自动登录已关闭", Tone::Neutral),
        Status::Offline if !error.is_empty() => NetworkSummary {
            title: "未连接",
            description: clean_copy(&portal::friendly_login_error(&portal::display_error(error))),
            tone: if portal::fail_kind(error) == portal::FailKind::Credential {
                Tone::Error
            } else {
                Tone::Warning
            },
        },
        Status::Offline => make(
            "未连接",
            if configured {
                "自动登录已开启，等待连接"
            } else {
                "尚未设置自动登录账号"
            },
            Tone::Neutral,
        ),
    }
}

pub fn account_changed(
    old_account: &str,
    old_password: &str,
    old_operator: &str,
    account: &str,
    password: &str,
    operator: &str,
) -> bool {
    old_account.trim() != account.trim() || old_password != password || old_operator != operator
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn wifi_recovery_hint_respects_missing_account_and_user_pause() {
        let waiting = super::present(
            &super::Status::Unreachable,
            true,
            true,
            false,
            false,
            "",
            "test",
        )
        .with_wifi_recovery(true, &super::Status::Unreachable, true);
        assert!(waiting.subtitle.contains("已开启自动连接"));
        let paused = super::present(
            &super::Status::Unreachable,
            true,
            true,
            true,
            false,
            "",
            "test",
        )
        .with_wifi_recovery(true, &super::Status::Unreachable, false);
        assert!(!paused.subtitle.contains("已开启自动连接"));
        let setup = super::present(
            &super::Status::Unreachable,
            false,
            false,
            false,
            false,
            "",
            "",
        )
        .with_wifi_recovery(true, &super::Status::Unreachable, true);
        assert_eq!(setup.action, super::Action::Account);
        assert!(!setup.subtitle.contains("已开启自动连接"));
    }

    use super::*;
    #[test]
    fn network_information_preserves_online_state_with_automatic_login_off() {
        let summary = network_summary(&online(), true, true, false, "", "20241234", false);
        assert_eq!(summary.title, "已连接");
        assert!(summary.description.contains("自动登录已关闭"));
        assert!(!summary.description.contains("可以上网"));
    }
    #[test]
    fn network_information_explains_a_limited_connection_in_plain_language() {
        let summary = network_summary(&online(), true, false, true, "", "20241234", false);
        assert_eq!(summary.title, "已连接，但暂时无法上网");
        assert_eq!(summary.tone, Tone::Warning);
    }
    #[test]
    fn network_information_identifies_the_other_online_account() {
        let summary = network_summary(&online(), true, false, false, "", "other", false);
        assert_eq!(summary.title, "当前使用其他账号");
        assert!(summary.description.contains("保持不变"));
    }
    #[test]
    fn network_information_distinguishes_loading_missing_network_and_missing_account() {
        assert_eq!(
            network_summary(&Status::Unreachable, true, false, false, "", "", true).title,
            "正在检查连接"
        );
        assert_eq!(
            network_summary(&Status::Unreachable, true, false, false, "", "", false).title,
            "等待校园网"
        );
        let summary = network_summary(
            &Status::Offline,
            false,
            true,
            false,
            "旧密码错误",
            "",
            false,
        );
        assert_eq!(summary.description, "尚未设置自动登录账号");
    }
    #[test]
    fn online_power_action_follows_connection_instead_of_automatic_login_preference() {
        for running in [false, true] {
            for paused in [false, true] {
                let view = present(&online(), true, running, paused, false, "", "20241234");
                assert_eq!(view.title, "已连接");
                assert_eq!(view.action, Action::Disconnect);
                assert_eq!(view.button, "点击断开");
                if paused {
                    assert!(view.subtitle.contains("自动登录已关闭"));
                }
            }
        }
    }
    #[test]
    fn explicitly_disconnected_state_offers_connect_and_explains_resuming_automatic_login() {
        let view = present(
            &Status::Offline,
            true,
            true,
            true,
            false,
            "旧的密码错误",
            "20241234",
        );
        assert_eq!(view.title, "未连接");
        assert_eq!(view.action, Action::Connect);
        assert!(view.footer.contains("恢复自动登录"));
    }
    #[test]
    fn captive_portal_verification_does_not_ask_user_to_change_the_password() {
        let view = present(
            &Status::Offline,
            true,
            true,
            false,
            false,
            "需要验证码",
            "20241234",
        );
        assert_eq!(view.action, Action::Connect);
        assert_eq!(view.button, "重新验证");
        assert!(!view.subtitle.contains("密码"));
    }
    #[test]
    fn authenticated_but_limited_connection_keeps_disconnect_and_offers_separate_network_check() {
        let view = present(&online(), true, true, false, true, "", "20241234");
        assert_eq!(view.action, Action::Disconnect);
        assert!(view.network_check);
    }
    #[test]
    fn other_online_accounts_stay_protected_even_when_automatic_login_is_off() {
        for paused in [true, false] {
            let view = present(&online(), true, true, paused, false, "", "other");
            assert_eq!(view.action, Action::Account);
            assert!(view.subtitle.contains("保留当前连接"));
        }
    }
    #[test]
    fn waiting_for_school_network_never_relabels_a_connect_action_as_disconnect() {
        for paused in [true, false] {
            let view = present(
                &Status::Unreachable,
                true,
                true,
                paused,
                false,
                "",
                "20241234",
            );
            assert_eq!(view.title, "等待校园网");
            assert_eq!(view.action, Action::Connect);
            assert!(!view.button.contains("断开"));
        }
    }
    fn online() -> Status {
        Status::Online(portal::Session {
            account: "20241234@mobile".into(),
            ip: String::new(),
            login_time: String::new(),
        })
    }
    #[test]
    fn paused_connection_does_not_claim_network_disconnected() {
        let view = present(&online(), true, true, true, false, "", "20241234");
        assert_eq!(view.title, "已连接");
        assert_eq!(view.action, Action::Disconnect);
    }
    #[test]
    fn primary_action_matches_connection_and_error() {
        assert_eq!(
            present(&online(), true, true, false, false, "", "20241234").action,
            Action::Disconnect
        );
        assert_eq!(
            present(
                &Status::Offline,
                true,
                true,
                false,
                false,
                "密码错误",
                "20241234"
            )
            .action,
            Action::Account
        );
        assert_eq!(
            present(
                &Status::Unreachable,
                true,
                true,
                false,
                false,
                "",
                "20241234"
            )
            .action,
            Action::Connect
        );
        assert_eq!(
            present(&online(), true, true, false, false, "", "other").action,
            Action::Account
        );
    }
    #[test]
    fn editing_unchanged_account_does_not_need_reconnect() {
        assert!(!account_changed(
            "20241234",
            "pw",
            "mobile",
            " 20241234 ",
            "pw",
            "mobile"
        ));
        assert!(account_changed(
            "20241234", "pw", "mobile", "20241234", "new", "mobile"
        ));
        assert!(account_changed(
            "20241234", "pw", "mobile", "20241234", "pw", "unicom"
        ));
    }
    #[test]
    fn short_ui_copy_has_no_terminal_periods() {
        assert_eq!(clean_copy(" 连接正常。 "), "连接正常");
        for status in [online(), Status::Offline, Status::Unreachable] {
            for paused in [false, true] {
                let view = present(
                    &status,
                    true,
                    true,
                    paused,
                    false,
                    "您的账号密码有误。",
                    "20241234",
                );
                for copy in [view.title, view.subtitle.as_str(), view.button, view.footer] {
                    assert!(
                        !copy.ends_with(['。', '.']),
                        "unexpected punctuation: {copy}"
                    );
                }
            }
        }
    }

    #[test]
    fn forgotten_account_always_offers_setup_even_if_auto_login_is_off() {
        let view = present(&Status::Offline, false, false, true, false, "", "");
        assert_eq!(view.action, Action::Account);
    }

    #[test]
    fn snapshots_started_before_or_during_an_operation_cannot_replace_its_result() {
        let epoch = RefreshEpoch::new();
        let before = epoch.current();
        epoch.advance();
        let during = epoch.current();
        assert!(!epoch.accepts(before, true));
        assert!(!epoch.accepts(during, true));
        epoch.advance();
        assert!(!epoch.accepts(before, false));
        assert!(!epoch.accepts(during, false));
        assert!(epoch.accepts(epoch.current(), false));
    }

    #[test]
    fn balance_issues_do_not_send_user_to_edit_the_password() {
        let view = present(
            &Status::Offline,
            true,
            true,
            false,
            false,
            "您的账号已欠费",
            "20241234",
        );
        assert_eq!(view.action, Action::Connect);
        assert_eq!(view.button, "处理后重试");
        assert!(view.subtitle.contains("欠费"));
    }

    #[test]
    fn operator_mismatch_is_visible_without_disconnect_or_config_changes() {
        let status = online();
        let view = present(&status, true, true, false, false, "", "20241234")
            .check_operator(&status, "20241234", "unicom", false);
        assert_eq!(view.action, Action::Account);
        assert_eq!(view.tone, Tone::Warning);
        let paused = present(&status, true, true, true, false, "", "20241234")
            .check_operator(&status, "20241234", "unicom", true);
        assert_eq!(paused.action, Action::Disconnect);
    }
}
