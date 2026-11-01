// Linux Wi-Fi 自愈适配：nmcli 只读探测 + /sys 链路快照，连接动作走 nmcli。
// 三条件决策、连接冷却与日志都在 crate::wifi_heal_core，本模块只负责“问 nmcli”。
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
// 三条都成立后还要确认目标网络真的在范围内，免得在校外每轮都发一次注定失败的连接。
//
// 与 Windows 的 NetshProbe、macOS 的 Probe 同形：只读命令真实执行，连接动作默认打桩，
// 只有显式设置 SNNU_ALLOW_WIFI_CONNECT=1 才会真的调 nmcli 切换无线。
//
// nmcli 缺失时（降级路径）探针只给保守答案：没连无线、没有有线在线、读不到已保存配置，
// 连接动作归 UNSUPPORTED 并写日志，绝不新建或改动 NetworkManager 里的任何配置。
use crate::wifi_heal_core::{ConnectReq, ProfileState, WifiAdapter};
pub use crate::wifi_heal_core::{target_ssid, DEFAULT_SSID};
use std::process::Stdio;

/// 有线接口名关键字（Linux 命名：eth0 / enp3s0 / eno1 / ens18 / enx…）
const WIRED_KEYS: [&str; 6] = ["eth", "enp", "eno", "ens", "enx", "en"];
/// 虚拟与无线接口关键字：这些接口“已连接”不代表有线在线
const VIRTUAL_KEYS: [&str; 14] = [
    "lo", "veth", "virbr", "docker", "br-", "vmnet", "vboxnet", "bridge", "bond", "dummy", "tun",
    "tap", "wl", "wg",
];
/// 允许真实切换 Wi-Fi 的开关
const ALLOW_CONNECT_ENV: &str = "SNNU_ALLOW_WIFI_CONNECT";

/// 命令入口：测试注入打桩实现，生产用 SystemCmd
pub trait Cmd {
    /// nmcli 是否可用：缺失时整个自愈降级
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

/// 把注入的命令入口接到共享决策核心：与 windows_wifi::NetshProbe、macos::wifi_heal::Probe 同形
struct Probe<'a>(&'a dyn Cmd);

impl WifiAdapter for Probe<'_> {
    fn wired_connected(&self) -> bool {
        // nmcli 缺失时只读事实一个都不查，直接答“没有有线在线”
        self.0.available() && wired_connected(self.0)
    }
    fn current_ssid(&self) -> Option<String> {
        if !self.0.available() {
            return None;
        }
        current_wifi_ssid(self.0)
    }
    fn target_visible(&self, ssid: &str) -> bool {
        // nmcli 缺失时没有扫描能力，按“可见”放行：随后的 connect_saved 必定归 UNSUPPORTED，
        // 一条连接命令都不会发出，限频交给共享核心的 60 秒冷却。
        // 与 macOS 侧 airport 被移除时的处理一致（见 src/macos/wifi_heal.rs 模块头注释）。
        !self.0.available() || target_visible(self.0, ssid)
    }
    fn profile_saved(&self, ssid: &str) -> ProfileState {
        if !self.0.available() {
            return ProfileState::Unknown;
        }
        if profile_saved(self.0, ssid) {
            ProfileState::Saved
        } else {
            ProfileState::Missing
        }
    }
    fn connect_saved(&self, ssid: &str) -> ConnectReq {
        if !self.0.available() {
            return ConnectReq::Unsupported("系统没有 nmcli");
        }
        // 只连系统已保存的配置：没有保存过就不发连接命令（nmcli dev wifi connect 会新建配置）
        if !profile_saved(self.0, ssid) {
            return ConnectReq::Failed;
        }
        if self.0.connect(ssid) {
            ConnectReq::Requested
        } else {
            ConnectReq::Failed
        }
    }
}

impl WifiAdapter for SystemCmd {
    fn wired_connected(&self) -> bool {
        Probe(self as &dyn Cmd).wired_connected()
    }
    fn current_ssid(&self) -> Option<String> {
        Probe(self as &dyn Cmd).current_ssid()
    }
    fn target_visible(&self, ssid: &str) -> bool {
        Probe(self as &dyn Cmd).target_visible(ssid)
    }
    fn profile_saved(&self, ssid: &str) -> ProfileState {
        Probe(self as &dyn Cmd).profile_saved(ssid)
    }
    fn connect_saved(&self, ssid: &str) -> ConnectReq {
        Probe(self as &dyn Cmd).connect_saved(ssid)
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

/// 目标 Wi-Fi 当前是不是可见（在范围内）；读不到扫描结果按“看不到”处理
fn target_visible(cmd: &dyn Cmd, ssid: &str) -> bool {
    let out = cmd
        .nmcli(&["-t", "-f", "SSID", "dev", "wifi", "list"])
        .unwrap_or_default();
    parse_scan_visible(&out, ssid)
}

/// 解析 nmcli -t -f SSID dev wifi list：每行一个网络名，整值相等才算可见
fn parse_scan_visible(text: &str, ssid: &str) -> bool {
    text.lines().any(|line| unescape_terse(line) == ssid)
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

/// 切出 nmcli -t 输出的第一个未转义冒号，并还原两侧的反斜杠转义
fn split_terse(line: &str) -> Option<(String, String)> {
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            ':' => {
                return Some((
                    unescape_terse(&line[..idx]),
                    unescape_terse(&line[idx + ch.len_utf8()..]),
                ))
            }
            _ => {}
        }
    }
    None
}

/// 还原 nmcli -t 的反斜杠转义（字段内的冒号写成 反斜杠+冒号）
fn unescape_terse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut escaped = false;
    for ch in text.chars() {
        match (escaped, ch) {
            (true, ch) => {
                out.push(ch);
                escaped = false;
            }
            (false, '\\') => escaped = true,
            (false, ch) => out.push(ch),
        }
    }
    out
}

/// 系统里是否保存过该 Wi-Fi 配置
fn profile_saved(cmd: &dyn Cmd, ssid: &str) -> bool {
    let out = cmd
        .nmcli(&["-t", "-f", "NAME,TYPE", "connection", "show"])
        .unwrap_or_default();
    parse_profile_has(&out, ssid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_heal_core::{self as core, Decision, HealState, Outcome, Skip};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    const WIRED_UP: &str = "eth0 up 1\nlo unknown 1\n";
    const WIRED_DOWN: &str = "eth0 down 0\nlo unknown 1\nwlan0 up 1\n";
    const WIFI_OTHER: &str = "yes:OtherWiFi\nno:SNNU\n";
    const WIFI_SNNU: &str = "yes:SNNU\nno:OtherWiFi\n";
    const PROFILES_SNNU: &str = "SNNU:802-11-wireless\nWired connection 1:802-3-ethernet\n";
    const PROFILES_OTHER: &str = "OtherWiFi:802-11-wireless\n";
    const SCAN_SNNU: &str = "SNNU\nSNNU-Guest\nOtherWiFi\n";
    const SCAN_OTHER: &str = "OtherWiFi\nCoffeeShop\n";
    const LIST: &str = "-t -f active,ssid dev wifi list";
    const SCAN: &str = "-t -f SSID dev wifi list";
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

    /// 打桩巡检：固定外网与门户结论，等待和复检用调用方给的回调。
    /// 决策本身全部来自共享核心 wifi_heal_core，这里只把打桩命令入口接上去。
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
        core::heal(
            cfg,
            &Probe(cmd),
            portal_ok,
            state,
            &mut internet,
            wait,
            recheck,
        )
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
    fn 扫描结果只认整值相等的网络名() {
        assert!(parse_scan_visible(SCAN_SNNU, "SNNU"));
        assert!(parse_scan_visible(SCAN_SNNU, "SNNU-Guest"));
        assert!(!parse_scan_visible(SCAN_SNNU, "SNNU-Guest2"));
        assert!(!parse_scan_visible(SCAN_SNNU, "OtherWiFi2"));
        assert!(!parse_scan_visible(SCAN_OTHER, "SNNU"));
        // nmcli -t 把字段内的冒号转义成 反斜杠+冒号
        assert!(parse_scan_visible("SNNU\\:Guest\n", "SNNU:Guest"));
        assert!(!parse_scan_visible("", "SNNU"));
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

    /// 三个条件的八种组合：只有“外网不通 + 门户不可达 + 不在校园网里”这一种才允许连接。
    /// 真值表断言的对象是共享决策核心 wifi_heal_core::decide，Linux 不再自带一份决策。
    #[test]
    fn 三个条件的八种组合只有全不成立才允许连接() {
        for internet_ok in [false, true] {
            for portal_ok in [false, true] {
                for on_school_net in [false, true] {
                    let decision = core::decide(
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
            core::decide(true, || false, || true, unreachable, unreachable),
            Decision::Skip(Skip::PortalOk)
        );
        assert_eq!(
            core::decide(true, || false, || false, || true, unreachable),
            Decision::Skip(Skip::Wired)
        );
        assert_eq!(
            core::decide(true, || false, || false, || false, || true),
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
            .with(SCAN, SCAN_SNNU)
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
    fn 附近看不到目标网络时跳过连接并且跳过也占用冷却() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(SCAN, SCAN_OTHER);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let first = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(first, Outcome::NotVisible("SNNU".to_string()));
        // 看不到目标网络就不查已保存配置，更不发连接命令
        assert!(!stub.called(PROFILES), "{:?}", stub.all());
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("dev wifi connect")),
            "{:?}",
            stub.all()
        );
        // 跳过也占用冷却：扫描最多每分钟一次，校外不会每轮都扫
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
    }

    #[test]
    fn 没有保存配置就放弃并且不发连接命令() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(SCAN, SCAN_SNNU)
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
        let outcome = core::heal(
            &cfg(false, "SNNU"),
            &Probe(&stub),
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
    fn nmcli缺失时不发任何命令并按不支持处理() {
        let stub = Stub::new().without_nmcli();
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
        // 能力缺失不再是独立的一档决策：探针给保守答案，连接动作归 UNSUPPORTED 并写日志
        assert_eq!(
            outcome,
            Outcome::Unsupported {
                ssid: "SNNU".to_string(),
                reason: "系统没有 nmcli"
            }
        );
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn 连接动作有冷却不会每轮都连() {
        let stub = Stub::new()
            .links(WIRED_DOWN)
            .with(LIST, WIFI_OTHER)
            .with(SCAN, SCAN_SNNU)
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
