// Wi-Fi 自愈的共享决策核心：只有“本身没网、门户也不可达、又不在校园网里”三条同时成立，
// 才把 Wi-Fi 连回学校网络。只连系统里已经保存的配置，不新建配置、不改系统设置。
//
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
// 三条都成立后还要确认目标网络真的在范围内，免得在校外每轮都发一次注定失败的连接。
//
// 平台差异全部由 WifiAdapter 提供：Windows 用 netsh，macOS 用 networksetup / ifconfig /
// airport，Linux 用 nmcli 加 /sys 链路快照。本模块只做决策、冷却与日志，测试全部打桩。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 学校 Wi-Fi 的默认名称
pub const DEFAULT_SSID: &str = "SNNU";
/// 两次连接动作之间的最小间隔，避免每轮巡检都去连接
const CONNECT_COOLDOWN: Duration = Duration::from_secs(60);
/// 关联之后等待门户可达的轮数（每轮 1 秒间隔加一次门户探测）
const RECHECK_ROUNDS: u32 = 6;
/// 发起连接后等 Wi-Fi 关联的总预算：实测关联加 DHCP 要 15~30 秒，
/// 复检必须排在关联之后，否则窗口全烧在还没连上的空等里
pub const ASSOC_TIMEOUT: Duration = Duration::from_secs(20);
/// 关联探测的间隔：每 2 秒问一次，10 次正好用满 ASSOC_TIMEOUT
pub const ASSOC_POLL: Duration = Duration::from_secs(2);

/// 系统里是否保存过目标网络的配置
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileState {
    /// 保存过
    Saved,
    /// 明确没有保存过
    Missing,
    /// 读不到（平台命令缺失一类），无法判断；netsh 没有这种情况，只有 macOS / Linux 侧会产生
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
    /// 平台不支持自动连接，reason 用于日志；netsh 没有这种情况，只有 macOS / Linux 侧会产生
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
    /// 是否已经关联到目标网络。关联完成才值得复检门户；看不出来的平台返回 Unknown，
    /// 保持原有“连上就直接复检”的流程。
    fn association(&self, _ssid: &str) -> AssocState {
        AssocState::Unknown
    }
}

/// 与目标网络的关联状态
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssocState {
    /// 已关联，可以复检门户
    Connected,
    /// 还没关联上（正在关联、等 DHCP）
    Pending,
    /// 平台看不出来，按原流程直接复检
    Unknown,
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
    /// 上一轮已经把连接请求发出去、门户却还没通：关联随时可能完成，
    /// 冷却不该继续挡住随后的登录（实测里那 60 秒空转就出在这里）。
    pending_association: bool,
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
///
/// 生产挂点走 heal_interruptible（退出与网络变化要能打断等待），这里是不关心打断的入口。
#[allow(dead_code)]
pub fn heal<A: WifiAdapter>(
    cfg: &crate::config::Config,
    adapter: &A,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    heal_interruptible(
        cfg,
        adapter,
        portal_ok,
        state,
        internet_ok,
        wait,
        recheck,
        &mut || false,
    )
}

/// 同 heal，但等待过程可被 interrupted 打断（退出、网络变化唤醒一类）。
pub fn heal_interruptible<A: WifiAdapter>(
    cfg: &crate::config::Config,
    adapter: &A,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
    interrupted: &mut dyn FnMut() -> bool,
) -> Outcome {
    let target = target_ssid(cfg);
    // 上一轮已发出连接但门户没通：关联一旦完成就立刻复检，不再等冷却空转。
    // 这个解锁机会只给一次，门户仍不通就交回常规决策（此时已经连在目标网络上）。
    if cfg.wifi_self_heal
        && state.pending_association
        && adapter.association(&target) == AssocState::Connected
    {
        state.pending_association = false;
        // 这一轮就是要把登录送出去，复检不再中途收手：被打断等于白等一轮
        let portal_ok = portal_recheck(wait, recheck, &mut || false);
        crate::config::logf(&format!(
            "{} 已关联完成，门户复检{}（未等连接冷却）",
            target,
            if portal_ok {
                "已可达"
            } else {
                "仍不可达"
            }
        ));
        let outcome = Outcome::Connected {
            ssid: target.clone(),
            portal_ok,
        };
        state.last = Some(outcome.clone());
        return outcome;
    }
    let decision = decide(
        cfg.wifi_self_heal,
        || internet_ok(),
        || portal_ok,
        || adapter.wired_connected(),
        || adapter.current_ssid().as_deref() == Some(target.as_str()),
    );
    let outcome = match decision {
        Decision::Skip(skip) => Outcome::Idle(skip),
        Decision::Connect => attempt(adapter, &target, state, wait, recheck, interrupted),
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
    interrupted: &mut dyn FnMut() -> bool,
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
            // 先等关联：没关联上就跑门户复检只会把窗口烧在还没连上的空等里。
            // 关联没在预算内完成也算“请求已发出”，下一轮靠关联信号直接解锁。
            if !wait_for_association(adapter, target, wait, interrupted) {
                state.pending_association = true;
                return Outcome::ConnectFailed(target.to_string());
            }
            let portal_ok = portal_recheck(wait, recheck, interrupted);
            state.pending_association = !portal_ok;
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

/// 等 Wi-Fi 关联完成：每 ASSOC_POLL 问一次，最多 ASSOC_TIMEOUT。
/// 返回 false 表示预算内没关联上（或等待被打断），此时再跑门户复检没有意义。
fn wait_for_association<A: WifiAdapter>(
    adapter: &A,
    target: &str,
    wait: &mut dyn FnMut(Duration),
    interrupted: &mut dyn FnMut() -> bool,
) -> bool {
    let polls = (ASSOC_TIMEOUT.as_secs() / ASSOC_POLL.as_secs()) as u32;
    for _ in 0..polls {
        match adapter.association(target) {
            // 已经关联上、或平台看不出来：不再多花一次等待，直接进复检
            AssocState::Connected | AssocState::Unknown => return true,
            AssocState::Pending => {}
        }
        wait(ASSOC_POLL);
        if interrupted() {
            return false;
        }
    }
    false
}

/// 关联完成后的门户复检：每 1 秒一轮，最多 RECHECK_ROUNDS 轮
fn portal_recheck(
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
    interrupted: &mut dyn FnMut() -> bool,
) -> bool {
    for _ in 0..RECHECK_ROUNDS {
        wait(Duration::from_secs(1));
        if interrupted() {
            return false;
        }
        if recheck() {
            return true;
        }
    }
    false
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
        // 开关关闭与门户可达是常态，不值得写日志；冷却说明上一轮刚动作过，也不写。
        Outcome::Idle(Skip::Disabled) | Outcome::Idle(Skip::PortalOk) | Outcome::Cooldown => {
            return;
        }
    };
    crate::config::logf(&msg);
}

/// 「外网可通」能不能当作不动 Wi-Fi 的理由。
///
/// 只有已经落在某条网络上时才算数：外网通 + 有线在线，或外网通 + 连着某个 Wi-Fi，
/// 那才是"在校园网里门户自己故障"，等门户恢复即可。
/// 完全没有任何关联（没连 Wi-Fi、没插网线）时，外网只能来自 TUN / 虚拟网卡一类，
/// 说明机器根本没连回学校网络，此时干等是错的，应该继续尝试连回。
pub fn internet_gate(internet_ok: bool, has_wifi: bool, wired: bool) -> bool {
    internet_ok && (has_wifi || wired)
}

/// 门户不可达分支的挂点：三条条件同时成立才把 Wi-Fi 连回学校网络
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
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
    #[cfg(target_os = "linux")]
    let probe = crate::linux::wifi::SystemCmd;
    // 中途掉线且没有任何关联时，别的出口（TUN、虚拟网卡）仍可能让外网探测通过；
    // 这种"外网可通"不能当成不动 Wi-Fi 的理由，否则界面会一直停在「等待校园网」。
    let mut internet_ok = || {
        // 先问外网：不通就没有后面两次 netsh，常态下不多花一次调用。
        if !crate::portal::internet_ok() {
            return false;
        }
        internet_gate(
            true,
            probe.current_ssid().is_some(),
            probe.wired_connected(),
        )
    };
    let mut wait = |gap: Duration| crate::agent::sleep(gap.as_secs().max(1), quit, wake);
    // 退出或网络变化（DHCP 落地、换网）都算打断：等关联时一旦系统报地址变化就提前收手，
    // 交给下一轮用「已关联」这个信号直接复检，不必把 20 秒预算耗满
    let mut interrupted = || {
        use std::sync::atomic::Ordering;
        quit.load(Ordering::SeqCst) || wake.load(Ordering::SeqCst)
    };
    let mut recheck = || {
        !matches!(
            crate::portal::status(Duration::from_secs(3)).0,
            crate::portal::Status::Unreachable
        )
    };
    let _ = heal_interruptible(
        cfg,
        &probe,
        // 这个挂点本身就在“门户不可达”分支里，条件 2 已经由调度循环确认过
        false,
        state,
        &mut internet_ok,
        &mut wait,
        &mut recheck,
        &mut interrupted,
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
        /// 关联状态；None 表示平台看不出来（默认流程：不等关联直接复检）
        assoc: Option<AssocState>,
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
        fn association(&self, _: &str) -> AssocState {
            self.asked.borrow_mut().push("assoc");
            self.assoc.unwrap_or(AssocState::Unknown)
        }
    }

    /// 按顺序回答关联状态的适配器：第 n 次问到给出第 n 个答案，用完后重复最后一个
    struct AssocSeq {
        answers: Vec<AssocState>,
        seen: RefCell<usize>,
        connects: RefCell<usize>,
    }

    impl AssocSeq {
        fn new(answers: &[AssocState]) -> Self {
            AssocSeq {
                answers: answers.to_vec(),
                seen: RefCell::new(0),
                connects: RefCell::new(0),
            }
        }

        fn polls(&self) -> usize {
            *self.seen.borrow()
        }

        fn connects(&self) -> usize {
            *self.connects.borrow()
        }
    }

    impl WifiAdapter for AssocSeq {
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
            *self.connects.borrow_mut() += 1;
            ConnectReq::Requested
        }
        fn association(&self, _: &str) -> AssocState {
            let mut seen = self.seen.borrow_mut();
            let answer = *self
                .answers
                .get(*seen)
                .unwrap_or_else(|| self.answers.last().unwrap());
            *seen += 1;
            answer
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

    /// 中途掉线、机器上既没 Wi-Fi 也没网线时，别的出口让外网探测通过也不能当成"在校园网里"。
    #[test]
    fn 完全没有任何关联时外网可通不算在校园网里() {
        // 有 Wi-Fi 关联或网线在线：外网可通就是在校园网里，等门户恢复即可。
        assert!(internet_gate(true, true, false));
        assert!(internet_gate(true, false, true));
        // 什么都没有：外网只可能来自 TUN/虚拟网卡，必须继续尝试连回。
        assert!(!internet_gate(true, false, false));
        // 外网本来就不通：与有没有关联无关，照常进入自愈判断。
        for has_wifi in [false, true] {
            for wired in [false, true] {
                assert!(!internet_gate(false, has_wifi, wired));
            }
        }
    }

    #[test]
    fn 目标名称留空时回到默认学校网络() {
        assert_eq!(target_ssid(&cfg(true, "  ")), DEFAULT_SSID);
        assert_eq!(target_ssid(&cfg(true, "SNNU-Guest")), "SNNU-Guest");
    }

    // ---- 关联等待与冷却解锁 ----

    #[test]
    fn 关联按两秒一次轮询并且提前返回() {
        // 第 3 次问到才关联上：2 次等待共 4 秒，不必等满 20 秒
        let net = AssocSeq::new(&[
            AssocState::Pending,
            AssocState::Pending,
            AssocState::Connected,
        ]);
        let mut waited = Vec::new();
        let mut wait = |d: Duration| waited.push(d);
        let mut interrupted = || false;
        assert!(wait_for_association(
            &net,
            "SNNU",
            &mut wait,
            &mut interrupted
        ));
        assert_eq!(waited, vec![ASSOC_POLL; 2]);
        assert_eq!(net.polls(), 3);
    }

    #[test]
    fn 关联二十秒内没完成就放弃() {
        let net = AssocSeq::new(&[AssocState::Pending]);
        let mut waited = Vec::new();
        let mut wait = |d: Duration| waited.push(d);
        let mut interrupted = || false;
        assert!(!wait_for_association(
            &net,
            "SNNU",
            &mut wait,
            &mut interrupted
        ));
        // 10 次 × 2 秒正好是 20 秒预算
        assert_eq!(waited.len(), 10);
        assert_eq!(waited.iter().sum::<Duration>(), ASSOC_TIMEOUT);
        assert!(waited.iter().all(|d| *d == ASSOC_POLL));
    }

    #[test]
    fn 关联等待可被打断() {
        let net = AssocSeq::new(&[AssocState::Pending]);
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut asked = 0;
        let mut interrupted = || {
            asked += 1;
            asked >= 3
        };
        assert!(!wait_for_association(
            &net,
            "SNNU",
            &mut wait,
            &mut interrupted
        ));
        assert_eq!(waits, 3, "第 3 次等待之后就该收手");
        assert_eq!(net.polls(), 3);
    }

    #[test]
    fn 关联成功之后才复检门户() {
        let net = AssocSeq::new(&[AssocState::Pending, AssocState::Connected]);
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut probes = 0;
        let mut recheck = || {
            probes += 1;
            true
        };
        let mut internet = || false;
        let mut interrupted = || false;
        let outcome = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert_eq!(
            outcome,
            Outcome::Connected {
                ssid: "SNNU".to_string(),
                portal_ok: true
            }
        );
        // 关联 1 次等待 + 复检 1 次等待
        assert_eq!(waits, 2);
        assert_eq!(probes, 1);
        assert_eq!(net.connects(), 1);
    }

    #[test]
    fn 关联没完成就提前返回且一次复检都不跑() {
        let net = AssocSeq::new(&[AssocState::Pending]);
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut recheck = || unreachable();
        let mut internet = || false;
        let mut interrupted = || false;
        let outcome = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert_eq!(outcome, Outcome::ConnectFailed("SNNU".to_string()));
        assert_eq!(waits, 10, "只花关联预算，不再叠加 6 轮复检");
    }

    #[test]
    fn 冷却期内关联完成就立即复检登录() {
        let net = AssocSeq::new(&[AssocState::Connected]);
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut internet = || false;
        let mut interrupted = || false;
        // 第 1 轮：连接已发出、关联也完成了，但门户还没通
        let mut recheck = || false;
        let first = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert_eq!(
            first,
            Outcome::Connected {
                ssid: "SNNU".to_string(),
                portal_ok: false
            }
        );

        // 第 2 轮仍在冷却期内：关联已就绪就直接复检，既不发连接也不等冷却
        let mut recheck = || true;
        let second = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert_eq!(
            second,
            Outcome::Connected {
                ssid: "SNNU".to_string(),
                portal_ok: true
            }
        );
        assert_eq!(net.connects(), 1, "解锁路径不该再发一次连接");
    }

    #[test]
    fn 解锁机会只用一次门户仍不通就回到常规决策() {
        let net = AssocSeq::new(&[AssocState::Connected]);
        let mut state = HealState::default();
        let mut wait = |_: Duration| {};
        let mut internet = || false;
        let mut interrupted = || false;
        let mut recheck = || false;
        let first = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert!(matches!(
            first,
            Outcome::Connected {
                portal_ok: false,
                ..
            }
        ));
        let second = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert!(matches!(
            second,
            Outcome::Connected {
                portal_ok: false,
                ..
            }
        ));
        // 第三轮已经用掉解锁机会：交回常规决策，仍在冷却期内就继续冷却，不重复发连接
        let third = heal_interruptible(
            &cfg(true, "SNNU"),
            &net,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
            &mut interrupted,
        );
        assert_eq!(third, Outcome::Cooldown);
        assert_eq!(net.connects(), 1, "解锁失败不该引起第二轮连接");
    }

    // ---- 时延对比：旧路径的冷却空转 vs 新路径的关联即解锁 ----

    /// 实测口径：Wi-Fi 关联加 DHCP 要 15~30 秒
    const SIM_ASSOC_SECS: u64 = 25;
    /// 一次门户探测最坏耗时
    const SIM_PROBE: Duration = Duration::from_secs(3);
    /// 掉线期间守护循环的巡检间隔
    const SIM_POLL: Duration = Duration::from_secs(8);

    /// 虚拟时钟：所有等待与探测耗时都累加到这里
    #[derive(Default)]
    struct VClock {
        ms: u64,
    }

    impl VClock {
        fn advance(&mut self, d: Duration) {
            self.ms += d.as_millis() as u64;
        }

        fn secs(&self) -> u64 {
            self.ms / 1000
        }
    }

    /// 打桩网络：关联在第 SIM_ASSOC_SECS 秒完成
    struct SimNet {
        clock: RefCell<VClock>,
    }

    impl SimNet {
        fn elapsed(&self) -> u64 {
            self.clock.borrow().secs()
        }
    }

    impl WifiAdapter for SimNet {
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
            ConnectReq::Requested
        }
        fn association(&self, _: &str) -> AssocState {
            if self.elapsed() >= SIM_ASSOC_SECS {
                AssocState::Connected
            } else {
                AssocState::Pending
            }
        }
    }

    /// 旧实现（本次改动前）的时延模型：连接后立刻 6 轮复检（1 秒间隔 + 3 秒探测），
    /// 全失败就进 60 秒冷却；冷却期内不感知“关联已完成”，只能等冷却走完再连一次。
    /// 对应现场 login.log 的 16:55:23 → 16:56:29（66 秒）→ 16:56:34 登录。
    fn old_path_secs() -> u64 {
        let mut clock = VClock::default();
        // 第 1 次连接：复检窗口只有 6×(1s+3s)=24 秒，撑不到第 25 秒的关联完成
        for _ in 0..RECHECK_ROUNDS {
            clock.advance(Duration::from_secs(1));
            clock.advance(SIM_PROBE);
        }
        assert!(
            clock.secs() < SIM_ASSOC_SECS,
            "复检窗口本来就撑不到关联完成"
        );
        // 冷却 60 秒，期间每 8 秒醒一次也不动作
        while clock.secs() < 60 {
            clock.advance(SIM_POLL);
        }
        // 冷却过后再连一次：关联早好了，一次探测就登录
        clock.advance(SIM_PROBE);
        clock.secs()
    }

    /// 新实现：真的跑 heal_interruptible，按守护循环的节奏反复巡检直到门户可达
    fn new_path_secs() -> u64 {
        let net = SimNet {
            clock: RefCell::new(VClock::default()),
        };
        let mut state = HealState::default();
        let mut internet = || false;
        let mut interrupted = || false;
        loop {
            let outcome = {
                let clock = &net.clock;
                let mut wait = |d: Duration| clock.borrow_mut().advance(d);
                // 门户探测本身也要花时间，探测时关联可能刚好完成
                let mut recheck = || {
                    clock.borrow_mut().advance(SIM_PROBE);
                    clock.borrow().secs() >= SIM_ASSOC_SECS
                };
                heal_interruptible(
                    &cfg(true, "SNNU"),
                    &net,
                    false,
                    &mut state,
                    &mut internet,
                    &mut wait,
                    &mut recheck,
                    &mut interrupted,
                )
            };
            if matches!(
                outcome,
                Outcome::Connected {
                    portal_ok: true,
                    ..
                }
            ) {
                return net.elapsed();
            }
            // 一轮巡检结束按调度循环的间隔再睡一会儿（生产里还会被地址变化提前叫醒）
            net.clock.borrow_mut().advance(SIM_POLL);
            assert!(net.elapsed() < 300, "新路径 5 分钟内没恢复：{outcome:?}");
        }
    }

    #[test]
    fn 新路径把冷却空转整个去掉() {
        let old = old_path_secs();
        let new = new_path_secs();
        println!("旧路径恢复时延 ≈ {old} 秒（关联 {SIM_ASSOC_SECS}s + 冷却空转 + 复检）");
        println!("新路径恢复时延 ≈ {new} 秒（关联 {SIM_ASSOC_SECS}s + 复检 3s）");
        assert!(new < old, "新路径 {new} 秒应该明显快于旧路径 {old} 秒");
        // 省下的正是冷却空转：旧路径必须等满 60 秒冷却才可能再连
        assert!(old - new >= 30, "新路径至少省下 30 秒，实际 {old} - {new}");
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
