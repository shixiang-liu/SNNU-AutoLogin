// Linux Wi-Fi 自愈：与 Windows 端口同一套三条件门控，只有
// “本身没网、门户也不可达、又不在校园网里”三条同时成立，才把 Wi-Fi 连回学校网络。
//
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
//
// 三条都由调用方注入探测函数，本模块只做决策与命令调用，测试全部打桩。
// 只读命令（/sys 链路快照、nmcli list）真实执行；连接动作默认打桩，
// 只有显式设置 SNNU_ALLOW_WIFI_CONNECT=1 才会真的调 nmcli 切换无线。
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 学校 Wi-Fi 的默认名称
pub const DEFAULT_SSID: &str = "SNNU";
/// 有线接口名关键字（Linux 命名：eth0 / enp3s0 / eno1 / ens18 / enx…）
const WIRED_KEYS: [&str; 6] = ["eth", "enp", "eno", "ens", "enx", "en"];
/// 虚拟与无线接口关键字：这些接口“已连接”不代表有线在线
const VIRTUAL_KEYS: [&str; 14] = [
    "lo", "veth", "virbr", "docker", "br-", "vmnet", "vboxnet", "bridge", "bond", "dummy", "tun",
    "tap", "wl", "wg",
];
/// 两次连接动作之间的最小间隔，避免每轮巡检都去连接
const CONNECT_COOLDOWN: Duration = Duration::from_secs(60);
/// 发起连接后等待门户可达的轮数（每轮 1 秒间隔加一次门户探测）
const RECHECK_ROUNDS: u32 = 6;
/// 允许真实切换 Wi-Fi 的开关
const ALLOW_CONNECT_ENV: &str = "SNNU_ALLOW_WIFI_CONNECT";

/// 命令入口：测试注入打桩实现，生产用 SystemCmd
pub trait Cmd {
    /// nmcli 是否可用：缺失时整个自愈降级跳过
    fn available(&self) -> bool;
    /// 只读链路快照（/sys/class/net），每行 "名称 状态 carrier"
    fn links(&self) -> Option<String>;
    /// 执行一条 nmcli 子命令；退出码为 0 时返回输出文本，否则返回 None
    fn nmcli(&self, args: &[&str]) -> Option<String>;
    /// 连接系统里已保存的 Wi-Fi；默认打桩，见模块头注释
    fn connect(&self, ssid: &str) -> bool;
}

/// 真实连接命令：只连系统已保存的配置，不扫描、不新建配置、不改系统设置
fn connect_args(ssid: &str) -> [&str; 4] {
    ["dev", "wifi", "connect", ssid]
}

/// 真实执行 nmcli 与只读查询
pub struct SystemCmd;

impl SystemCmd {
    /// 真正切换 Wi-Fi 的开关：默认关闭，避免日常巡检里发生真实连接动作。
    fn connect_allowed() -> bool {
        std::env::var(ALLOW_CONNECT_ENV).as_deref() == Ok("1")
    }
}

impl Cmd for SystemCmd {
    fn available(&self) -> bool {
        std::process::Command::new("nmcli")
            .arg("-v")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok()
    }

    fn links(&self) -> Option<String> {
        Some(crate::network::link_snapshot())
    }

    fn nmcli(&self, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("nmcli")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn connect(&self, ssid: &str) -> bool {
        let command = format!("nmcli {}", connect_args(ssid).join(" "));
        if !Self::connect_allowed() {
            crate::config::logf(&format!(
                "Wi-Fi 自愈打桩：本应执行 {command}（设置 {ALLOW_CONNECT_ENV}=1 才会真正切换无线）"
            ));
            return true;
        }
        self.nmcli(&connect_args(ssid)).is_some()
    }
}

/// 有线是否在线：只读查询，不改变任何系统状态
pub fn wired_connected(cmd: &dyn Cmd) -> bool {
    parse_wired_connected(&cmd.links().unwrap_or_default())
}

/// 当前已连接 Wi-Fi 的 SSID；没连无线或读不到时返回 None
pub fn current_wifi_ssid(cmd: &dyn Cmd) -> Option<String> {
    let out = cmd.nmcli(&["-t", "-f", "active,ssid", "dev", "wifi", "list"])?;
    parse_current_ssid(&out)
}

/// 解析链路快照：只有名称像以太网、且链路真的起来（carrier=1 或 operstate=up）才算有线在线
fn parse_wired_connected(snapshot: &str) -> bool {
    snapshot.lines().any(|line| {
        let mut columns = line.split_whitespace();
        let (Some(name), Some(state), carrier) = (columns.next(), columns.next(), columns.next())
        else {
            return false;
        };
        let name = name.to_lowercase();
        link_is_up(state, carrier)
            && WIRED_KEYS.iter().any(|key| name.contains(key))
            && !VIRTUAL_KEYS.iter().any(|key| name.contains(key))
    })
}

/// 链路是否真的起来了：carrier 是权威信号，
/// carrier=1 但网卡被管理性关闭（operstate 为 down 等）也不算在线。
fn link_is_up(state: &str, carrier: Option<&str>) -> bool {
    match carrier {
        Some("1") => !matches!(
            state.to_ascii_lowercase().as_str(),
            "down" | "lowerlayerdown" | "dormant" | "notpresent"
        ),
        // carrier 明确为 0：没有链路，operstate 是 up 也不算（网线拔了但网卡管理态还开着）
        Some("0") => false,
        // carrier 读不到：只能退回 operstate
        _ => state.eq_ignore_ascii_case("up"),
    }
}

/// 解析 nmcli -t -f active,ssid dev wifi list：只有 active=yes 的那条是当前连接
fn parse_current_ssid(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (active, ssid) = split_terse(line)?;
        (active == "yes" && !ssid.is_empty()).then_some(ssid)
    })
}

/// 解析 nmcli -t -f NAME,TYPE connection show：名称整值相等，且必须是无线配置
fn parse_profile_has(text: &str, ssid: &str) -> bool {
    text.lines().any(|line| {
        split_terse(line)
            .is_some_and(|(name, kind)| name == ssid && matches!(kind.as_str(), "802-11-wireless" | "wifi"))
    })
}

/// 切出 nmcli -t 输出的第一个未转义冒号，并还原反斜杠转义
fn split_terse(line: &str) -> Option<(String, String)> {
    let mut name = String::new();
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    name.push(next);
                }
            }
            ':' => {
                let mut rest = String::new();
                let mut escaped = false;
                for ch in chars {
                    if escaped {
                        rest.push(ch);
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else {
                        rest.push(ch);
                    }
                }
                return Some((name, rest));
            }
            _ => name.push(ch),
        }
    }
    None
}

/// 系统里是否保存过该 Wi-Fi 配置
fn profile_saved(cmd: &dyn Cmd, ssid: &str) -> bool {
    let out = cmd
        .nmcli(&["-t", "-f", "NAME,TYPE", "connection", "show"])
        .unwrap_or_default();
    parse_profile_has(&out, ssid)
}

/// 连接系统里已保存的 Wi-Fi；没有保存过这个配置就放弃（不新建配置、不改系统设置）
fn connect_saved(cmd: &dyn Cmd, ssid: &str) -> bool {
    if !profile_saved(cmd, ssid) {
        return false;
    }
    cmd.connect(ssid)
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

/// 不动作的原因
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Skip {
    /// 自愈开关关闭
    Disabled,
    /// 系统没有 nmcli：能力缺失，整块跳过
    NoNmcli,
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
enum Decision {
    Skip(Skip),
    Connect,
}

/// 一轮巡检实际做了什么
#[derive(Clone, PartialEq, Eq, Debug)]
enum Outcome {
    /// 按决策不动作
    Idle(Skip),
    /// 距上次连接动作不足冷却时间
    Cooldown,
    /// 系统没有保存目标 Wi-Fi 配置，放弃
    NoProfile(String),
    /// 系统没有连上，连接请求被拒
    ConnectFailed(String),
    /// 已请求连接，portal_ok 是随后的门户复检结果
    Connected { ssid: String, portal_ok: bool },
}

/// 纯决策：三个条件都成立才允许连接。事实用闭包传入，用不到的时候一个探测都不做。
fn decide(
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
        Outcome::Idle(Skip::NoNmcli) => "系统没有 nmcli，跳过 Wi-Fi 自愈".to_string(),
        Outcome::NoProfile(ssid) => format!("系统未保存 {ssid} 配置，放弃自动连接 Wi-Fi"),
        Outcome::ConnectFailed(ssid) => format!("连接 {ssid} 未成功，稍后重试"),
        Outcome::Connected { ssid, portal_ok } => format!(
            "本身无网、门户不可达且没连 {ssid}，已请求连接；门户复检{}",
            if *portal_ok { "已可达" } else { "仍不可达" }
        ),
        Outcome::Idle(Skip::Disabled) | Outcome::Idle(Skip::PortalOk) | Outcome::Cooldown => return,
    };
    crate::config::logf(&msg);
}

/// 门户不可达时巡检一次。探测与等待全部由调用方注入：
/// internet_ok 是条件 1 的外部网络探测，portal_ok 是条件 2 的结论（挂点就在不可达分支里），
/// recheck 是连接之后的门户复检，wait 是复检之间的等待。
fn heal(
    cfg: &crate::config::Config,
    cmd: &dyn Cmd,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    // 能力缺失先判：连 nmcli 都没有，后面的事实一个都不用去查
    if !cmd.available() {
        let outcome = if cfg.wifi_self_heal {
            Outcome::Idle(Skip::NoNmcli)
        } else {
            Outcome::Idle(Skip::Disabled)
        };
        record(state, &outcome);
        return outcome;
    }
    let target = target_ssid(cfg);
    let decision = decide(
        cfg.wifi_self_heal,
        || internet_ok(),
        || portal_ok,
        || wired_connected(cmd),
        || current_wifi_ssid(cmd).as_deref() == Some(target.as_str()),
    );
    let outcome = match decision {
        Decision::Skip(skip) => Outcome::Idle(skip),
        Decision::Connect => attempt(cmd, &target, state, wait, recheck),
    };
    record(state, &outcome);
    outcome
}

/// 连接动作：没有保存目标 Wi-Fi 就放弃，有保存就连过去并复检门户
fn attempt(
    cmd: &dyn Cmd,
    target: &str,
    state: &mut HealState,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    if state.cooling(Instant::now()) {
        return Outcome::Cooldown;
    }
    state.last_attempt = Some(Instant::now());
    if connect_saved(cmd, target) {
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
    } else if profile_saved(cmd, target) {
        Outcome::ConnectFailed(target.to_string())
    } else {
        Outcome::NoProfile(target.to_string())
    }
}

/// 门户不可达分支的挂点：三条条件同时成立才把 Wi-Fi 连回学校网络
pub fn heal_on_unreachable(
    cfg: &crate::config::Config,
    state: &mut HealState,
    quit: &Arc<AtomicBool>,
    wake: &Arc<AtomicBool>,
) {
    let cmd = SystemCmd;
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
        &cmd,
        // 这个挂点本身就在“门户不可达”分支里，条件 2 已经由调度循环确认过
        false,
        state,
        &mut internet_ok,
        &mut wait,
        &mut recheck,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const WIRED_UP: &str = "eth0 up 1\nlo unknown 1\n";
    const WIRED_DOWN: &str = "eth0 down 0\nlo unknown 1\nwlan0 up 1\n";
    const WIFI_OTHER: &str = "yes:OtherWiFi\nno:SNNU\n";
    const WIFI_SNNU: &str = "yes:SNNU\nno:OtherWiFi\n";
    const PROFILES_SNNU: &str = "SNNU:802-11-wireless\nWired connection 1:802-3-ethernet\n";
    const PROFILES_OTHER: &str = "OtherWiFi:802-11-wireless\n";
    const LIST: &str = "-t -f active,ssid dev wifi list";
    const PROFILES: &str = "-t -f NAME,TYPE connection show";

    /// 打桩 nmcli 与只读查询：按子命令给预置输出，并记录被调用过的子命令
    struct Stub {
        available: bool,
        links: Option<String>,
        output: HashMap<String, String>,
        connect_ok: bool,
        calls: RefCell<Vec<String>>,
    }

    impl Stub {
        fn new() -> Self {
            Stub {
                available: true,
                links: None,
                output: HashMap::new(),
                connect_ok: true,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, args: &str, out: &str) -> Self {
            self.output.insert(args.to_string(), out.to_string());
            self
        }

        fn links(mut self, snapshot: &str) -> Self {
            self.links = Some(snapshot.to_string());
            self
        }

        fn without_nmcli(mut self) -> Self {
            self.available = false;
            self
        }

        fn called(&self, args: &str) -> bool {
            self.calls.borrow().iter().any(|call| call == args)
        }

        fn all(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Cmd for Stub {
        fn available(&self) -> bool {
            self.available
        }

        fn links(&self) -> Option<String> {
            self.calls.borrow_mut().push("links".to_string());
            self.links.clone()
        }

        fn nmcli(&self, args: &[&str]) -> Option<String> {
            let key = args.join(" ");
            self.calls.borrow_mut().push(key.clone());
            self.output.get(&key).cloned()
        }

        fn connect(&self, ssid: &str) -> bool {
            self.calls
                .borrow_mut()
                .push(connect_args(ssid).join(" ").to_string());
            self.connect_ok
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

    /// 打桩巡检：固定外网与门户结论，等待和复检用调用方给的回调
    fn heal_stub(
        cfg: &crate::config::Config,
        cmd: &dyn Cmd,
        portal_ok: bool,
        state: &mut HealState,
        internet_ok: bool,
        wait: &mut dyn FnMut(Duration),
        recheck: &mut dyn FnMut() -> bool,
    ) -> Outcome {
        let mut internet = || internet_ok;
        heal(cfg, cmd, portal_ok, state, &mut internet, wait, recheck)
    }

    #[test]
    fn 有线判定只认链路真的起来的以太网口() {
        assert!(parse_wired_connected(WIRED_UP));
        assert!(parse_wired_connected("enp3s0 up 1\n"));
        assert!(parse_wired_connected("ens18 unknown 1\n"));
        assert!(!parse_wired_connected(WIRED_DOWN));
        // carrier=0：网线没链路，operstate 是 up 也不算在线
        assert!(!parse_wired_connected("eth0 up 0\n"));
        // carrier 读不到时退回 operstate
        assert!(parse_wired_connected("eth0 up -\n"));
        assert!(!parse_wired_connected("eth0 down -\n"));
        // carrier=1 但网卡被管理性关闭
        assert!(!parse_wired_connected("eth0 down 1\n"));
        assert!(!parse_wired_connected("eth0 lowerlayerdown 1\n"));
        // carrier 列意外缺失时按 operstate 判定；连状态列都没有才算读不出来
        assert!(parse_wired_connected("eth0 up\n"));
        assert!(!parse_wired_connected("eth0\n"));
        assert!(!parse_wired_connected(""));
    }

    #[test]
    fn 有线判定排除虚拟口和无线口() {
        for line in [
            "veth1a2b3c up 1",
            "docker0 up 1",
            "virbr0 up 1",
            "br-9f8e7d up 1",
            "vmnet8 up 1",
            "vboxnet0 up 1",
            "tun0 up 1",
            "tap0 up 1",
            "wg0 up 1",
            "wlan0 up 1",
            "wlp2s0 up 1",
            "lo unknown 1",
            "bond0 up 1",
            "dummy0 up 1",
        ] {
            assert!(!parse_wired_connected(line), "{line}");
        }
    }

    #[test]
    fn 有线判定在有线虚拟混在一起时仍认得出有线() {
        let mixed = "veth1a2b3c up 1\neth1 down 0\nwlan0 up 1\n";
        assert!(!parse_wired_connected(mixed));
        let mixed = "docker0 up 1\nenp0s31f6 up 1\n";
        assert!(parse_wired_connected(mixed));
    }

    #[test]
    fn 当前无线名称只认active为yes的那条() {
        assert_eq!(parse_current_ssid(WIFI_OTHER).as_deref(), Some("OtherWiFi"));
        assert_eq!(parse_current_ssid(WIFI_SNNU).as_deref(), Some("SNNU"));
        // nmcli -t 把字段内的冒号转义成 反斜杠+冒号
        assert_eq!(
            parse_current_ssid("yes:SNNU\\:Guest\n").as_deref(),
            Some("SNNU:Guest")
        );
        assert_eq!(parse_current_ssid("no:SNNU\n"), None);
        assert_eq!(parse_current_ssid("yes:\n"), None);
        assert_eq!(parse_current_ssid("SNNU\n"), None);
        assert_eq!(parse_current_ssid(""), None);
    }

    #[test]
    fn 已保存配置只按名称整值匹配且必须是无线配置() {
        assert!(parse_profile_has(PROFILES_SNNU, "SNNU"));
        assert!(!parse_profile_has(PROFILES_SNNU, "SNNU-Guest"));
        assert!(!parse_profile_has(PROFILES_SNNU, "Wired connection 1"));
        assert!(!parse_profile_has(PROFILES_OTHER, "SNNU"));
        // 同名但是有线配置：dev wifi connect 用不上它
        assert!(!parse_profile_has("SNNU:802-3-ethernet\n", "SNNU"));
        assert!(!parse_profile_has("", "SNNU"));
    }

    /// 三个条件的八种组合：只有“外网不通 + 门户不可达 + 不在校园网里”这一种才允许连接
    #[test]
    fn 三个条件的八种组合只有全不成立才允许连接() {
        for internet_ok in [false, true] {
            for portal_ok in [false, true] {
                for on_school_net in [false, true] {
                    let decision =
                        decide(true, || internet_ok, || portal_ok, || on_school_net, || false);
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
    fn 外网可达时零动作且一条命令都不发() {
        let stub = Stub::new();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            true,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::InternetUp));
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn 门户可达时零动作且一条命令都不发() {
        let stub = Stub::new();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            true,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::PortalOk));
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn 有线在线时零动作且不碰无线() {
        let stub = Stub::new().links(WIRED_UP);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::Wired));
        assert_eq!(stub.all(), vec!["links".to_string()]);
        assert!(
            stub.all().iter().all(|call| !call.starts_with("dev wifi")),
            "有线在线时不应该碰无线：{:?}",
            stub.all()
        );
    }

    #[test]
    fn 已经在学校无线上也零动作() {
        let stub = Stub::new().links(WIRED_DOWN).with(LIST, WIFI_SNNU);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::OnTarget));
        assert!(!stub.called(PROFILES), "{:?}", stub.all());
    }

    #[test]
    fn 没网没门户没连学校网络时连接并复检门户() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(PROFILES, PROFILES_SNNU);
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
        let mut probes = 0;
        let mut recheck = || {
            probes += 1;
            probes >= 2
        };
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
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
        // 生产命令就是 nmcli dev wifi connect SNNU
        assert!(stub.called("dev wifi connect SNNU"), "{:?}", stub.all());
        assert_eq!(waits, 2);
        assert_eq!(probes, 2);
    }

    #[test]
    fn 没有保存配置就放弃并且不发连接命令() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(PROFILES, PROFILES_OTHER);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::NoProfile("SNNU".to_string()));
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("dev wifi connect")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 开关关闭时零动作且一条命令都不发() {
        let stub = Stub::new();
        let mut state = HealState::default();
        let mut probed = false;
        let mut internet = || {
            probed = true;
            true
        };
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal(
            &cfg(false, "SNNU"),
            &stub,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::Disabled));
        assert!(!probed);
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn nmcli缺失时自动降级跳过且零探测() {
        let stub = Stub::new().without_nmcli();
        let mut state = HealState::default();
        let mut probed = false;
        let mut internet = || {
            probed = true;
            false
        };
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let outcome = heal(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(outcome, Outcome::Idle(Skip::NoNmcli));
        assert!(!probed);
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn 连接动作有冷却不会每轮都连() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(PROFILES, PROFILES_SNNU);
        let mut state = HealState::default();
        let mut wait = |_: Duration| {};
        let mut recheck = || true;
        let first = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert!(matches!(
            first,
            Outcome::Connected {
                portal_ok: true,
                ..
            }
        ));
        let second = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(second, Outcome::Cooldown);
        assert_eq!(
            stub.all()
                .iter()
                .filter(|call| call.starts_with("dev wifi connect"))
                .count(),
            1,
            "{:?}",
            stub.all()
        );
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

    #[test]
    fn 目标名称留空时回到默认学校网络() {
        assert_eq!(target_ssid(&cfg(true, "  ")), DEFAULT_SSID);
        assert_eq!(target_ssid(&cfg(true, "SNNU-Guest")), "SNNU-Guest");
    }
}
