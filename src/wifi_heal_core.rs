// Wi-Fi 自愈的共享决策核心：只有“本身没网、门户也不可达、又不在校园网里”三条同时成立，
// 才把 Wi-Fi 连回学校网络。只连系统里已经保存的配置，不新建配置、不改系统设置。
//
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
// 三条都成立后还要确认目标网络真的在范围内，免得在校外每轮都发一次注定失败的连接。
//
// 平台差异全部由 WifiAdapter 提供：Windows 用 netsh，macOS 用 networksetup / ifconfig /
// airport。本模块只做决策、冷却与日志，测试全部打桩。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 学校 Wi-Fi 的默认名称
pub const DEFAULT_SSID: &str = "SNNU";
/// 两次连接动作之间的最小间隔，避免每轮巡检都去连接
const CONNECT_COOLDOWN: Duration = Duration::from_secs(60);
/// 发起连接后等待门户可达的轮数（每轮 1 秒间隔加一次门户探测）
const RECHECK_ROUNDS: u32 = 6;

/// 系统里是否保存过目标网络的配置
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileState {
    /// 保存过
    Saved,
    /// 明确没有保存过
    Missing,
    /// 读不到（平台命令缺失一类），无法判断；netsh 没有这种情况，只有 macOS 侧会产生
    #[allow(dead_code)]
    Unknown,
}

/// 连接请求的结果
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectReq {
    /// 已经发出连接请求
    Requested,
    /// 请求被拒绝或执行失败
    Failed,
    /// 平台不支持自动连接，reason 用于日志；netsh 没有这种情况，只有 macOS 侧会产生
    #[allow(dead_code)]
    Unsupported(&'static str),
}

/// 平台探测与动作：只读查询加一个连接动作，不改任何系统设置
pub trait WifiAdapter {
    /// 有线是否在线
    fn wired_connected(&self) -> bool;
    /// 当前已连接 Wi-Fi 的 SSID
    fn current_ssid(&self) -> Option<String>;
    /// 目标 Wi-Fi 当前是否可见（在范围内）
    fn target_visible(&self, ssid: &str) -> bool;
    /// 系统里是否保存过该网络配置
    fn profile_saved(&self, ssid: &str) -> ProfileState;
    /// 连接系统里已保存的网络
    fn connect_saved(&self, ssid: &str) -> ConnectReq;
}

/// 不动作的原因
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skip {
    /// 自愈开关关闭
    Disabled,
    /// 外网还能上：这是门户自己的问题，只等门户恢复，不动 Wi-Fi
    InternetUp,
    /// 门户可达，不需要自愈
    PortalOk,
    /// 有线在线，不干预 Wi-Fi
    Wired,
    /// 已经连在目标 Wi-Fi 上，重复连接没有意义
    OnTarget,
}

/// 纯决策结论
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Skip(Skip),
    Connect,
}

/// 一轮巡检实际做了什么
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// 按决策不动作
    Idle(Skip),
    /// 距上次连接动作不足冷却时间
    Cooldown,
    /// 附近看不到目标 Wi-Fi，本轮不连接
    NotVisible(String),
    /// 系统没有保存目标 Wi-Fi 配置，放弃
    NoProfile(String),
    /// 系统没有连上，连接请求被拒
    ConnectFailed(String),
    /// 平台不支持自动连接（命令缺失一类），本轮不动作
    Unsupported { ssid: String, reason: &'static str },
    /// 已请求连接，portal_ok 是随后的门户复检结果
    Connected { ssid: String, portal_ok: bool },
}

/// 纯决策：三个条件都成立才允许连接。事实用闭包传入，用不到的时候一个探测都不做。
pub fn decide(
    enabled: bool,
    internet_ok: impl FnOnce() -> bool,
    portal_ok: impl FnOnce() -> bool,
    wired: impl FnOnce() -> bool,
    on_target_wifi: impl FnOnce() -> bool,
) -> Decision {
    if !enabled {
        return Decision::Skip(Skip::Disabled);
    }
    // 条件 1：外网还能上就不是“本身没网”，门户故障不该动 Wi-Fi
    if internet_ok() {
        return Decision::Skip(Skip::InternetUp);
    }
    if portal_ok() {
        return Decision::Skip(Skip::PortalOk);
    }
    // 条件 3：有线在线或已经连在学校 Wi-Fi 上都算在校园网里
    if wired() {
        return Decision::Skip(Skip::Wired);
    }
    if on_target_wifi() {
        return Decision::Skip(Skip::OnTarget);
    }
    Decision::Connect
}

/// 常驻循环持有的巡检状态：连接动作冷却，加同一结论只记一次日志
#[derive(Default)]
pub struct HealState {
    last_attempt: Option<Instant>,
    last: Option<Outcome>,
}

impl HealState {
    /// 距离上次连接动作是否还在冷却期内
    fn cooling(&self, now: Instant) -> bool {
        self.last_attempt
            .is_some_and(|at| now.duration_since(at) < CONNECT_COOLDOWN)
    }
}

/// 自愈目标 SSID：配置留空时按默认学校 Wi-Fi
pub fn target_ssid(cfg: &crate::config::Config) -> String {
    let ssid = cfg.ssid.trim();
    if ssid.is_empty() {
        DEFAULT_SSID.to_string()
    } else {
        ssid.to_string()
    }
}

/// 门户不可达时巡检一次。探测与等待全部由调用方注入：
/// internet_ok 是条件 1 的外部网络探测，portal_ok 是条件 2 的结论（挂点就在不可达分支里），
/// recheck 是连接之后的门户复检，wait 是复检之间的等待。
pub fn heal<A: WifiAdapter>(
    cfg: &crate::config::Config,
    adapter: &A,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    let target = target_ssid(cfg);
    let decision = decide(
        cfg.wifi_self_heal,
        || internet_ok(),
        || portal_ok,
        || adapter.wired_connected(),
        || adapter.current_ssid().as_deref() == Some(target.as_str()),
    );
    let outcome = match decision {
        Decision::Skip(skip) => Outcome::Idle(skip),
        Decision::Connect => attempt(adapter, &target, state, wait, recheck),
    };
    record(state, &outcome);
    outcome
}

/// 连接动作：没有保存目标网络就放弃，有保存就连过去并复检门户
fn attempt<A: WifiAdapter>(
    adapter: &A,
    target: &str,
    state: &mut HealState,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    if state.cooling(Instant::now()) {
        return Outcome::Cooldown;
    }
    state.last_attempt = Some(Instant::now());
    // 校外没有目标网络时连接只会白试：跳过也占用冷却，扫描最多每分钟一次
    if !adapter.target_visible(target) {
        return Outcome::NotVisible(target.to_string());
    }
    match adapter.connect_saved(target) {
        ConnectReq::Requested => {
            let mut portal_ok = false;
            for _ in 0..RECHECK_ROUNDS {
                wait(Duration::from_secs(1));
                if recheck() {
                    portal_ok = true;
                    break;
                }
            }
            Outcome::Connected {
                ssid: target.to_string(),
                portal_ok,
            }
        }
        ConnectReq::Unsupported(reason) => Outcome::Unsupported {
            ssid: target.to_string(),
            reason,
        },
        ConnectReq::Failed => match adapter.profile_saved(target) {
            ProfileState::Saved => Outcome::ConnectFailed(target.to_string()),
            // 读不到配置状态时按不支持处理：宁可不动，也不新建或改动系统配置
            ProfileState::Unknown => Outcome::Unsupported {
                ssid: target.to_string(),
                reason: "无法读取已保存的网络配置",
            },
            ProfileState::Missing => Outcome::NoProfile(target.to_string()),
        },
    }
}

/// 同一结论只记一次日志，避免掉线期间每轮巡检都刷屏
fn record(state: &mut HealState, outcome: &Outcome) {
    if state.last.as_ref() == Some(outcome) {
        return;
    }
    state.last = Some(outcome.clone());
    let msg = match outcome {
        Outcome::Idle(Skip::InternetUp) => "外网可通、门户不可达，不切换 Wi-Fi".to_string(),
        Outcome::Idle(Skip::Wired) => "有线在线，不干预 Wi-Fi".to_string(),
        Outcome::Idle(Skip::OnTarget) => "已经连在目标 Wi-Fi 上，无需自愈".to_string(),
        Outcome::NotVisible(ssid) => format!("附近看不到 {} 网络，本轮不连接", ssid),
        Outcome::NoProfile(ssid) => format!("系统未保存 {} 配置，放弃自动连接 Wi-Fi", ssid),
        Outcome::ConnectFailed(ssid) => format!("连接 {} 未成功，稍后重试", ssid),
        Outcome::Unsupported { ssid, reason } => {
            format!("无法自动连接 {}（{}），本轮不动作", ssid, reason)
        }
        Outcome::Connected { ssid, portal_ok } => format!(
            "本身无网、门户不可达且没连 {}，已请求连接；门户复检{}",
            ssid,
            if *portal_ok {
                "已可达"
            } else {
                "仍不可达"
            }
        ),
        Outcome::Idle(Skip::Disabled) | Outcome::Idle(Skip::PortalOk) | Outcome::Cooldown => {
            return;
        }
    };
    crate::config::logf(&msg);
}

/// 门户不可达分支的挂点：三条条件同时成立才把 Wi-Fi 连回学校网络
#[cfg(any(windows, target_os = "macos"))]
pub fn heal_on_unreachable(
    cfg: &crate::config::Config,
    state: &mut HealState,
    quit: &Arc<AtomicBool>,
    wake: &Arc<AtomicBool>,
) {
    #[cfg(windows)]
    let probe = crate::windows_wifi::SystemCmd;
    #[cfg(target_os = "macos")]
    let probe = crate::macos::wifi_heal::SystemCmd;
    let mut internet_ok = crate::portal::internet_ok;
    let mut wait = |gap: Duration| crate::agent::sleep(gap.as_secs().max(1), quit, wake);
    let mut recheck = || {
        !matches!(
            crate::portal::status(Duration::from_secs(3)).0,
            crate::portal::Status::Unreachable
        )
    };
    let _ = heal(
        cfg,
        &probe,
        // 这个挂点本身就在“门户不可达”分支里，条件 2 已经由调度循环确认过
        false,
        state,
        &mut internet_ok,
        &mut wait,
        &mut recheck,
    );
}

// ---- 跨平台小工具（两个平台的探测解析共用，避免逐字复制） ----

/// 按首个冒号切出“键 : 值”（中文输出也可能是全角冒号），值已去空白
pub fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let at = line.find([':', '：'])?;
    let sep = line[at..].chars().next()?;
    Some((line[..at].trim(), line[at + sep.len_utf8()..].trim()))
}

/// 有界等待子进程结束：超时返回 false，由调用方负责收尾
pub fn wait_bounded(child: &mut std::process::Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 打桩适配器：只记录被问过哪些事实
    #[derive(Default)]
    struct Stub {
        wired: bool,
        ssid: Option<String>,
        visible: bool,
        profile: Option<bool>,
        asked: RefCell<Vec<&'static str>>,
    }

    impl WifiAdapter for Stub {
        fn wired_connected(&self) -> bool {
            self.asked.borrow_mut().push("wired");
            self.wired
        }
        fn current_ssid(&self) -> Option<String> {
            self.asked.borrow_mut().push("ssid");
            self.ssid.clone()
        }
        fn target_visible(&self, _: &str) -> bool {
            self.asked.borrow_mut().push("visible");
            self.visible
        }
        fn profile_saved(&self, _: &str) -> ProfileState {
            self.asked.borrow_mut().push("profile");
            match self.profile {
                Some(true) => ProfileState::Saved,
                Some(false) => ProfileState::Missing,
                None => ProfileState::Unknown,
            }
        }
        fn connect_saved(&self, _: &str) -> ConnectReq {
            self.asked.borrow_mut().push("connect");
            // 与两个平台的适配器一致：系统没有保存过这个配置就返回失败
            match self.profile {
                Some(false) => ConnectReq::Failed,
                _ => ConnectReq::Requested,
            }
        }
    }

    fn cfg(enabled: bool, ssid: &str) -> crate::config::Config {
        crate::config::Config {
            wifi_self_heal: enabled,
            ssid: ssid.to_string(),
            ..Default::default()
        }
    }

    fn unreachable<T>() -> T {
        panic!("这一步不应该被问到")
    }

    /// 三个条件的八种组合：只有“外网不通 + 门户不可达 + 不在校园网里”这一种才允许连接
    #[test]
    fn 三个条件的八种组合只有全不成立才允许连接() {
        for internet_ok in [false, true] {
            for portal_ok in [false, true] {
                for on_school_net in [false, true] {
                    let decision = decide(
                        true,
                        || internet_ok,
                        || portal_ok,
                        || on_school_net,
                        || false,
                    );
                    assert_eq!(
                        decision == Decision::Connect,
                        !internet_ok && !portal_ok && !on_school_net,
                        "外网通={internet_ok} 门户可达={portal_ok} 在校园网={on_school_net}"
                    );
                }
            }
        }
    }

    #[test]
    fn 门户可达时有线在线和已连目标都不动作() {
        // 门户可达或已在校园网时，后面的事实一个都不该去查
        assert_eq!(
            decide(true, || false, || true, unreachable, unreachable),
            Decision::Skip(Skip::PortalOk)
        );
        assert_eq!(
            decide(true, || false, || false, || true, unreachable),
            Decision::Skip(Skip::Wired)
        );
        assert_eq!(
            decide(true, || false, || false, || false, || true),
            Decision::Skip(Skip::OnTarget)
        );
    }

    #[test]
    fn 开关关闭和外网可达都不查任何事实() {
        let stub = Stub::default();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(false, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Idle(Skip::Disabled)
        );
        let mut internet = || true;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Idle(Skip::InternetUp)
        );
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                true,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Idle(Skip::PortalOk)
        );
        assert!(stub.asked.borrow().is_empty(), "{:?}", stub.asked.borrow());
    }

    #[test]
    fn 有线在线或已连目标时不查无线也不连接() {
        let stub = Stub {
            wired: true,
            ..Default::default()
        };
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Idle(Skip::Wired)
        );
        assert_eq!(*stub.asked.borrow(), vec!["wired"]);

        let stub = Stub {
            ssid: Some("SNNU".to_string()),
            ..Default::default()
        };
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Idle(Skip::OnTarget)
        );
        assert_eq!(*stub.asked.borrow(), vec!["wired", "ssid"]);
    }

    #[test]
    fn 没有保存配置时按决策放弃不发连接命令() {
        let stub = Stub {
            visible: true,
            profile: Some(false),
            ..Default::default()
        };
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::NoProfile("SNNU".to_string())
        );
        // 连接被拒后再确认一次配置状态，据此区分“未保存配置”和“连接失败”
        assert_eq!(
            *stub.asked.borrow(),
            vec!["wired", "ssid", "visible", "connect", "profile"]
        );
    }

    #[test]
    fn 看不到目标网络时跳过连接并且跳过也占用冷却() {
        let stub = Stub::default();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::NotVisible("SNNU".to_string())
        );
        assert_eq!(*stub.asked.borrow(), vec!["wired", "ssid", "visible"]);
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &stub,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Cooldown
        );
        // 冷却在“该不该连接”这一步才生效：决策要问的两个事实照旧，扫描与连接不再重复
        assert_eq!(
            *stub.asked.borrow(),
            vec!["wired", "ssid", "visible", "wired", "ssid"],
            "冷却期内不再扫描"
        );
        assert_eq!(
            *stub.asked.borrow(),
            vec!["wired", "ssid", "visible", "wired", "ssid"]
        );
        assert_eq!(
            stub.asked
                .borrow()
                .iter()
                .filter(|ask| **ask == "visible")
                .count(),
            1,
            "{:?}",
            stub.asked.borrow()
        );
    }

    #[test]
    fn 平台不支持自动连接时归为不支持而不是失败() {
        struct Unsupported;
        impl WifiAdapter for Unsupported {
            fn wired_connected(&self) -> bool {
                false
            }
            fn current_ssid(&self) -> Option<String> {
                None
            }
            fn target_visible(&self, _: &str) -> bool {
                true
            }
            fn profile_saved(&self, _: &str) -> ProfileState {
                ProfileState::Saved
            }
            fn connect_saved(&self, _: &str) -> ConnectReq {
                ConnectReq::Unsupported("测试环境缺命令")
            }
        }
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || false;
        assert_eq!(
            heal(
                &cfg(true, "SNNU"),
                &Unsupported,
                false,
                &mut state,
                &mut internet,
                &mut wait,
                &mut recheck
            ),
            Outcome::Unsupported {
                ssid: "SNNU".to_string(),
                reason: "测试环境缺命令"
            }
        );
    }

    #[test]
    fn 连接成功后复检门户并占用冷却() {
        let stub = Stub {
            visible: true,
            profile: Some(true),
            ..Default::default()
        };
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut probes = 0;
        let mut recheck = || {
            probes += 1;
            probes >= 2
        };
        let mut internet = || false;
        let outcome = heal(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(
            outcome,
            Outcome::Connected {
                ssid: "SNNU".to_string(),
                portal_ok: true
            }
        );
        assert!(stub.asked.borrow().contains(&"connect"), "{:?}", stub.asked);
        assert_eq!(waits, 2);
        assert_eq!(probes, 2);
    }

    #[test]
    fn 目标名称留空时回到默认学校网络() {
        assert_eq!(target_ssid(&cfg(true, "  ")), DEFAULT_SSID);
        assert_eq!(target_ssid(&cfg(true, "SNNU-Guest")), "SNNU-Guest");
    }

    #[test]
    fn 配置默认开启自愈且默认连学校网络() {
        let cfg: crate::config::Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.wifi_self_heal);
        assert_eq!(cfg.ssid, DEFAULT_SSID);
        let cfg: crate::config::Config =
            serde_json::from_str(r#"{"wifi_self_heal":false,"ssid":"SNNU-Guest"}"#).unwrap();
        assert!(!cfg.wifi_self_heal);
        assert_eq!(cfg.ssid, "SNNU-Guest");
    }
}
