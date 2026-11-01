// Windows 侧 Wi-Fi 自愈适配：netsh 只读探测 + netsh 连接动作。
// 三条件决策、连接冷却与日志都在 crate::wifi_heal_core，本模块只负责“问 netsh”。
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
// 三条都成立后还要确认目标网络真的在范围内，免得在校外每轮都发一次注定失败的连接。
use crate::wifi_heal_core::{split_key_value, AssocState, ConnectReq, ProfileState, WifiAdapter};
pub use crate::wifi_heal_core::{target_ssid, DEFAULT_SSID};
#[cfg(test)]
use crate::wifi_heal_core::{HealState, Outcome};
use std::process::Stdio;
use std::time::Duration;

/// 有线接口名关键字。只认“以太网 / ethernet”：老命名“本地连接 / Local Area Connection”
/// 不识别是有意为之——Wi-Fi Direct 虚拟网卡也叫“本地连接* N”，与下面的虚拟网卡关键字
/// 无法区分，误判成有线会让自愈彻底失效。
const WIRED_KEYS: [&str; 2] = ["以太网", "ethernet"];
/// 虚拟网卡关键字：这些接口“已连接”不代表有线在线
const VIRTUAL_KEYS: [&str; 8] = [
    "vethernet",
    "virtual",
    "vmware",
    "hyper-v",
    "wsl",
    "loopback",
    "bluetooth",
    "tap",
];
/// netsh 单次调用上限：超时按探测失败处理，一次卡住的调用不能拖住整个巡检
const NETSH_TIMEOUT: Duration = Duration::from_secs(8);
/// netsh 执行入口：测试注入打桩实现，生产用 SystemCmd
pub trait Cmd {
    /// 执行一条 netsh 子命令；退出码为 0 时返回输出文本，否则返回 None
    fn netsh(&self, args: &[&str]) -> Option<String>;
}

/// 真实执行 netsh
pub struct SystemCmd;

impl Cmd for SystemCmd {
    fn netsh(&self, args: &[&str]) -> Option<String> {
        use std::io::Read;
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("netsh")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // stderr 不读：netsh 的报错文本用不上，也免得管道满了卡住子进程
            .stderr(Stdio::null())
            // CREATE_NO_WINDOW：界面程序里跑控制台程序不要闪出黑窗
            .creation_flags(0x08000000)
            .spawn()
            .ok()?;
        if !wait_bounded(&mut child, NETSH_TIMEOUT) {
            // 超时（或等待出错）：结束子进程，按这次探测失败处理
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        if !child.wait().ok()?.success() {
            return None;
        }
        let mut buf = Vec::new();
        child.stdout.take()?.read_to_end(&mut buf).ok()?;
        Some(decode(&buf))
    }
}

/// 把 netsh 注入入口接到共享决策核心：netsh 没有“不支持”语义，失败即失败
struct NetshProbe<'a>(&'a dyn Cmd);

impl WifiAdapter for NetshProbe<'_> {
    fn wired_connected(&self) -> bool {
        wired_connected(self.0)
    }
    fn current_ssid(&self) -> Option<String> {
        current_wifi_ssid(self.0)
    }
    fn target_visible(&self, ssid: &str) -> bool {
        target_visible(self.0, ssid)
    }
    fn profile_saved(&self, ssid: &str) -> ProfileState {
        if profile_saved(self.0, ssid) {
            ProfileState::Saved
        } else {
            ProfileState::Missing
        }
    }
    fn connect_saved(&self, ssid: &str) -> ConnectReq {
        if connect_saved(self.0, ssid) {
            ConnectReq::Requested
        } else {
            ConnectReq::Failed
        }
    }
    fn association(&self, ssid: &str) -> AssocState {
        association(self.0, ssid)
    }
}

impl WifiAdapter for SystemCmd {
    fn wired_connected(&self) -> bool {
        NetshProbe(self as &dyn Cmd).wired_connected()
    }
    fn current_ssid(&self) -> Option<String> {
        NetshProbe(self as &dyn Cmd).current_ssid()
    }
    fn target_visible(&self, ssid: &str) -> bool {
        NetshProbe(self as &dyn Cmd).target_visible(ssid)
    }
    fn profile_saved(&self, ssid: &str) -> ProfileState {
        NetshProbe(self as &dyn Cmd).profile_saved(ssid)
    }
    fn connect_saved(&self, ssid: &str) -> ConnectReq {
        NetshProbe(self as &dyn Cmd).connect_saved(ssid)
    }
    fn association(&self, ssid: &str) -> AssocState {
        NetshProbe(self as &dyn Cmd).association(ssid)
    }
}

/// 有界等待子进程结束：实现见共享核心，两个平台共用同一套收尾逻辑
fn wait_bounded(child: &mut std::process::Child, limit: Duration) -> bool {
    crate::wifi_heal_core::wait_bounded(child, limit)
}

/// netsh 按控制台代码页输出（中文系统是 GBK）：先严格按 UTF-8，失败再按 GB18030
fn decode(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.trim_start_matches('\u{feff}').to_string();
    }
    encoding_rs::GB18030.decode(bytes).0.into_owned()
}

/// 有线是否在线：只读查询，不改变任何系统状态
pub fn wired_connected(cmd: &dyn Cmd) -> bool {
    let out = cmd
        .netsh(&["interface", "show", "interface"])
        .unwrap_or_default();
    parse_wired_connected(&out)
}

/// 当前已连接 Wi-Fi 的 SSID；没连无线或读不到时返回 None
pub fn current_wifi_ssid(cmd: &dyn Cmd) -> Option<String> {
    let out = cmd.netsh(&["wlan", "show", "interfaces"])?;
    parse_current_ssid(&out)
}

/// 解析 netsh interface show interface：名称为实有线网卡的接口处于已连接即算在线
fn parse_wired_connected(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        let lower = line.to_lowercase();
        // 先排除断开：中文“已断开连接”含“已连接”，英文 Disconnected 含 Connected
        if lower.contains("断开") || lower.contains("disconnected") {
            return false;
        }
        if !(lower.contains("已连接") || lower.contains("connected")) {
            return false;
        }
        let name = interface_name(line).to_lowercase();
        WIRED_KEYS.iter().any(|key| name.contains(key))
            && !VIRTUAL_KEYS.iter().any(|key| name.contains(key))
    })
}

/// 取接口名：netsh 用两个以上空格对齐列，切不出列时退回整行判断
fn interface_name(line: &str) -> &str {
    line.split("  ")
        .map(str::trim)
        .filter(|column| !column.is_empty())
        .last()
        .unwrap_or(line)
}

/// 解析 netsh wlan show interfaces 里的 SSID（BSSID 一类同行不算）
fn parse_current_ssid(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = split_key_value(line)?;
        // “BSSID”“AP BSSID”也带 SSID 字样，必须整键相等；空值表示这条没连上
        if !key.eq_ignore_ascii_case("SSID") || value.is_empty() {
            return None;
        }
        Some(value.to_string())
    })
}

/// 解析 netsh wlan show interfaces 的「状态 / State」行：已连接为 true，断开为 false。
/// 中文「已断开连接」含「已连接」，英文 Disconnected 含 Connected，必须先排除断开。
fn parse_interface_connected(text: &str) -> Option<bool> {
    text.lines().find_map(|line| {
        let (key, value) = split_key_value(line)?;
        if !(key == "状态" || key.eq_ignore_ascii_case("state")) {
            return None;
        }
        let lower = value.to_lowercase();
        if lower.contains("断开") || lower.contains("disconnected") {
            return Some(false);
        }
        Some(lower.contains("已连接") || lower.contains("connected"))
    })
}

/// 与目标网络的关联状态：接口已连接且 SSID 是目标网络才算关联完成。
/// 读不到 netsh 输出时返回 Unknown，由调用方按原有流程直接复检门户。
pub fn association(cmd: &dyn Cmd, ssid: &str) -> AssocState {
    let Some(out) = cmd.netsh(&["wlan", "show", "interfaces"]) else {
        return AssocState::Unknown;
    };
    match parse_interface_connected(&out) {
        Some(true) if parse_current_ssid(&out).as_deref() == Some(ssid) => AssocState::Connected,
        // 已连接但连的是别的网络，或者还停在断开：都当作关联尚未完成
        Some(_) => AssocState::Pending,
        None => AssocState::Unknown,
    }
}

/// 解析 netsh wlan show profiles：已保存的配置文件里是否有这个名称
fn parse_profile_has(text: &str, ssid: &str) -> bool {
    text.lines()
        .any(|line| split_key_value(line).is_some_and(|(_, value)| value == ssid))
}

/// 解析 netsh wlan show networks：可见网络列表里有没有这个名称
/// （网络条目形如“SSID 1 : SNNU”；“BSSID 1 :”一类不算）
fn parse_network_visible(text: &str, ssid: &str) -> bool {
    text.lines().any(|line| {
        let Some((key, value)) = split_key_value(line) else {
            return false;
        };
        ssid_entry_key(key) && value == ssid
    })
}

/// 网络条目的键是“SSID 加序号”，大小写不敏感
fn ssid_entry_key(key: &str) -> bool {
    match key.to_ascii_lowercase().strip_prefix("ssid") {
        Some(rest) => {
            let rest = rest.trim();
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// 系统里是否保存过该 Wi-Fi 配置
fn profile_saved(cmd: &dyn Cmd, ssid: &str) -> bool {
    let out = cmd.netsh(&["wlan", "show", "profiles"]).unwrap_or_default();
    parse_profile_has(&out, ssid)
}

/// 目标 Wi-Fi 当前是不是可见（在范围内）
pub fn target_visible(cmd: &dyn Cmd, ssid: &str) -> bool {
    let out = cmd.netsh(&["wlan", "show", "networks"]).unwrap_or_default();
    parse_network_visible(&out, ssid)
}

/// 连接系统里已保存的 Wi-Fi；没有保存过这个配置就放弃（不新建配置、不改系统设置）
fn connect_saved(cmd: &dyn Cmd, ssid: &str) -> bool {
    if !profile_saved(cmd, ssid) {
        return false;
    }
    let arg = format!("name={}", ssid);
    cmd.netsh(&["wlan", "connect", arg.as_str()]).is_some()
}

/// 打桩巡检入口：把 netsh 探测接到共享决策核心上（生产路径走核心的挂点）
#[cfg(test)]
fn heal(
    cfg: &crate::config::Config,
    cmd: &dyn Cmd,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    crate::wifi_heal_core::heal(
        cfg,
        &NetshProbe(cmd),
        portal_ok,
        state,
        internet_ok,
        wait,
        recheck,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_heal_core::{decide, AssocState, Decision, Skip};
    use std::cell::RefCell;
    use std::collections::HashMap;

    const CN_WIRED_UP: &str = "管理员状态     状态           类型             接口名称\n\
-------------------------------------------------------------------------\n\
已启用           已连接           专用             以太网\n";
    const CN_WIRED_DOWN: &str = "管理员状态     状态           类型             接口名称\n\
-------------------------------------------------------------------------\n\
已启用           已连接           专用             WLAN 2\n\
已启用           已断开连接         专用             以太网\n";
    const EN_WIRED_UP: &str = "Admin State    State          Type             Interface Name\n\
-------------------------------------------------------------------------\n\
Enabled        Connected      Dedicated        Ethernet\n";
    const WLAN_OTHER: &str = "    名称                   : WLAN 2\n\
    状态                   : 已连接\n\
    SSID                   : OtherWiFi\n\
    AP BSSID               : ac:ce:92:58:1e:a0\n";
    const WLAN_SNNU: &str = "    名称                   : WLAN 2\n\
    状态                   : 已连接\n\
    SSID                   : SNNU\n";
    const PROFILES_SNNU: &str = "接口 WLAN 2 上的配置文件: \n\n\
用户配置文件\n\
-------------\n\
    所有用户配置文件 : SNNU\n\
    所有用户配置文件 : Redmi K60\n";
    const PROFILES_OTHER: &str = "接口 WLAN 2 上的配置文件: \n\n\
用户配置文件\n\
-------------\n\
    所有用户配置文件 : OtherWiFi\n";
    const NETWORKS_SNNU: &str = "接口 WLAN 2 上的网络: 有 2 个网络当前可见。\n\n\
SSID 1 : OtherWiFi\n\
    网络类型             : 结构\n\
    信号                   : 90%\n\n\
SSID 2 : SNNU\n\
    网络类型             : 结构\n";
    const NETWORKS_OTHER: &str = "接口 WLAN 2 上的网络: 有 1 个网络当前可见。\n\n\
SSID 1 : OtherWiFi\n\
    网络类型             : 结构\n";

    /// 打桩 netsh：按子命令给预置输出，并记录被调用过的子命令。
    /// 同一个子命令可以排多份输出（with 之后用 then 追加），按调用顺序依次消费，
    /// 用来演示“连接前还是别的网络、连接后已经关联到学校网络”。
    struct Stub {
        output: HashMap<String, Vec<String>>,
        calls: RefCell<Vec<String>>,
    }

    impl Stub {
        fn new() -> Self {
            Stub {
                output: HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, args: &str, out: &str) -> Self {
            self.output.insert(args.to_string(), vec![out.to_string()]);
            self
        }

        /// 同一个子命令的下一次输出：前面几份被消费完之前不会用到它
        fn then(mut self, args: &str, out: &str) -> Self {
            self.output
                .entry(args.to_string())
                .or_default()
                .push(out.to_string());
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
        fn netsh(&self, args: &[&str]) -> Option<String> {
            let key = args.join(" ");
            self.calls.borrow_mut().push(key.clone());
            let answers = self.output.get(&key)?;
            // 排了多份就依次消费，只剩最后一份时重复使用
            let index = self
                .calls
                .borrow()
                .iter()
                .filter(|call| **call == key)
                .count()
                - 1;
            Some(answers[index.min(answers.len() - 1)].clone())
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
    fn 有线判定认得出中文和英文的以太网() {
        assert!(parse_wired_connected(CN_WIRED_UP));
        assert!(parse_wired_connected(EN_WIRED_UP));
    }

    #[test]
    fn 有线判定先排除断开并且不把无线当有线() {
        assert!(!parse_wired_connected(CN_WIRED_DOWN));
        assert!(!parse_wired_connected(
            "Enabled        Connected      Dedicated        Wi-Fi\n\
Enabled        Disconnected   Dedicated        Ethernet\n"
        ));
        assert!(!parse_wired_connected(""));
    }

    #[test]
    fn 有线判定排除虚拟网卡() {
        for line in [
            "已启用           已连接           专用             vEthernet (Default Switch)",
            "已启用           已连接           专用             VMware Network Adapter VMnet8",
            "Enabled        Connected      Dedicated        Hyper-V Virtual Ethernet Adapter",
            "Enabled        Connected      Dedicated        Bluetooth Network Connection",
            "Enabled        Connected      Dedicated        TAP-Windows Adapter V9",
            "已启用           已连接           专用             本地连接* 10",
        ] {
            assert!(!parse_wired_connected(line), "{line}");
        }
    }

    #[test]
    fn 有线判定在有线和虚拟混在一起时仍认得出有线() {
        let mixed = "已启用           已连接           专用             vEthernet (WSL)\n\
已启用           已断开连接         专用             以太网\n\
已启用           已连接           专用             WLAN 2\n";
        assert!(!parse_wired_connected(mixed));
        let mixed =
            "已启用           已连接           专用             vEthernet (Default Switch)\n\
已启用           已连接           专用             以太网 2\n";
        assert!(parse_wired_connected(mixed));
    }

    #[test]
    fn 有线判定在列切不开时退回整行() {
        assert!(parse_wired_connected(
            "Enabled Connected Dedicated Ethernet"
        ));
        assert!(!parse_wired_connected(
            "Enabled Connected Dedicated VMware Virtual Ethernet Adapter for VMnet1"
        ));
    }

    #[test]
    fn 当前无线名称只认ssid整键() {
        assert_eq!(parse_current_ssid(WLAN_OTHER).as_deref(), Some("OtherWiFi"));
        assert_eq!(
            parse_current_ssid("    BSSID                  : ac:ce:92:58:1e:a0\n    SSID : SNNU\n")
                .as_deref(),
            Some("SNNU")
        );
        // 全角冒号与空值
        assert_eq!(
            parse_current_ssid("    SSID：SNNU\n").as_deref(),
            Some("SNNU")
        );
        assert_eq!(
            parse_current_ssid("    状态                   : 已断开连接\n"),
            None
        );
        assert_eq!(parse_current_ssid("    SSID                   : \n"), None);
        assert_eq!(parse_current_ssid(""), None);
    }

    #[test]
    fn 已保存配置只按名称整值匹配() {
        assert!(parse_profile_has(PROFILES_SNNU, "SNNU"));
        assert!(!parse_profile_has(PROFILES_SNNU, "SNNU-Guest"));
        assert!(!parse_profile_has(PROFILES_SNNU, "Redmi"));
        assert!(!parse_profile_has(PROFILES_OTHER, "SNNU"));
        assert!(!parse_profile_has("", "SNNU"));
    }

    #[test]
    fn gbk输出也能解析() {
        let (bytes, _, _) = encoding_rs::GB18030.encode("已启用 已连接 专用 以太网");
        assert!(parse_wired_connected(&decode(&bytes)));
        let (bytes, _, _) = encoding_rs::GB18030.encode("    SSID : SNNU\n");
        assert_eq!(parse_current_ssid(&decode(&bytes)).as_deref(), Some("SNNU"));
        // UTF-8 原样透传（含 BOM）
        assert_eq!(decode("    SSID : SNNU\n".as_bytes()), "    SSID : SNNU\n");
        assert_eq!(decode(&[0xef, 0xbb, 0xbf, b'a']), "a");
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
    fn 外网可达时零动作且一次netsh都不调() {
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
    fn 门户可达时零动作且一次netsh都不调() {
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
        let stub = Stub::new().with("interface show interface", CN_WIRED_UP);
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
        assert_eq!(stub.all(), vec!["interface show interface".to_string()]);
        assert!(
            stub.all().iter().all(|call| !call.starts_with("wlan")),
            "有线在线时不应该碰无线：{:?}",
            stub.all()
        );
    }

    #[test]
    fn 已经在学校无线上也零动作() {
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_SNNU);
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
        assert!(!stub.called("wlan show profiles"), "{:?}", stub.all());
    }

    #[test]
    fn 没网没门户没连学校网络时连接并复检门户() {
        // 连接前还连在别的网络上，连接后才关联到学校网络
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
            .then("wlan show interfaces", WLAN_SNNU)
            .with("wlan show profiles", PROFILES_SNNU)
            .with("wlan show networks", NETWORKS_SNNU)
            .with("wlan connect name=SNNU", "连接请求已完成。\n");
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
        assert!(stub.called("wlan connect name=SNNU"), "{:?}", stub.all());
        assert_eq!(waits, 2);
        assert_eq!(probes, 2);
    }

    #[test]
    fn 接口状态只认状态行并先排除断开() {
        assert_eq!(parse_interface_connected(WLAN_SNNU), Some(true));
        assert_eq!(parse_interface_connected(WLAN_OTHER), Some(true));
        assert_eq!(
            parse_interface_connected("    状态                   : 已断开连接\n"),
            Some(false)
        );
        assert_eq!(
            parse_interface_connected("    State                  : disconnected\n"),
            Some(false)
        );
        // 只看“状态/State”行：名称行里出现 Connected 字样不算
        assert_eq!(
            parse_interface_connected("    名称                   : WLAN 2\n"),
            None
        );
        assert_eq!(parse_interface_connected(""), None);
    }

    #[test]
    fn 关联状态要求接口已连接且ssid是目标() {
        let cmd = Stub::new().with("wlan show interfaces", WLAN_SNNU);
        assert_eq!(association(&cmd, "SNNU"), AssocState::Connected);
        // 已连但连的是别的网络：关联尚未完成
        let cmd = Stub::new().with("wlan show interfaces", WLAN_OTHER);
        assert_eq!(association(&cmd, "SNNU"), AssocState::Pending);
        // 断开着
        let cmd = Stub::new().with(
            "wlan show interfaces",
            "    名称                   : WLAN 2\n    状态                   : 已断开连接\n",
        );
        assert_eq!(association(&cmd, "SNNU"), AssocState::Pending);
        // 读不到 netsh：看不出来，按原流程直接复检
        let cmd = Stub::new();
        assert_eq!(association(&cmd, "SNNU"), AssocState::Unknown);
    }

    #[test]
    fn 关联没完成时提前失败且不跑门户复检() {
        // 连接请求发出去了，但接口始终停在别的网络
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
            .with("wlan show profiles", PROFILES_SNNU)
            .with("wlan show networks", NETWORKS_SNNU)
            .with("wlan connect name=SNNU", "连接请求已完成。\n");
        let mut state = HealState::default();
        let mut waits = 0;
        let mut wait = |_: Duration| waits += 1;
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
        assert_eq!(outcome, Outcome::ConnectFailed("SNNU".to_string()));
        // 只花关联预算（10 次 × 2 秒），一次门户复检都不跑
        assert_eq!(waits, 10);
        // 决策先问一次当前网络，之后 10 次关联探测，正好 11 次
        assert_eq!(
            stub.all()
                .iter()
                .filter(|call| **call == "wlan show interfaces")
                .count(),
            11,
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 没有保存配置就放弃并且不发连接命令() {
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
            .with("wlan show networks", NETWORKS_SNNU)
            .with("wlan show profiles", PROFILES_OTHER);
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
                .any(|call| call.starts_with("wlan connect")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 开关关闭时零动作且连外网都不探() {
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
    fn 连接成功后已经连在目标网络上不再重复连接() {
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
            .then("wlan show interfaces", WLAN_SNNU)
            .with("wlan show profiles", PROFILES_SNNU)
            .with("wlan show networks", NETWORKS_SNNU)
            .with("wlan connect name=SNNU", "连接请求已完成。\n");
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
        // 关联完成后再巡检就是“已经在学校网络上”，不会再发第二次连接
        assert_eq!(second, Outcome::Idle(Skip::OnTarget));
        assert_eq!(
            stub.all()
                .iter()
                .filter(|call| call.starts_with("wlan connect"))
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

    #[test]
    fn 可见网络按ssid条目匹配并排除bssid() {
        assert!(parse_network_visible(NETWORKS_SNNU, "SNNU"));
        assert!(!parse_network_visible(NETWORKS_SNNU, "SNNU-Guest"));
        assert!(!parse_network_visible(NETWORKS_OTHER, "SNNU"));
        assert!(!parse_network_visible("", "SNNU"));
        // mode=bssid 的“BSSID 1”与隐藏网络的空名称都不能算可见
        let detailed = "SSID 1 : OtherWiFi\n    BSSID 1                 : ac:ce:92:58:1e:a0\n\
SSID 2 : \n    网络类型             : 结构\n";
        assert!(!parse_network_visible(detailed, "SNNU"));
        assert!(!parse_network_visible(detailed, "ac:ce:92:58:1e:a0"));
    }

    #[test]
    fn 附近看不到目标网络时跳过连接() {
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
            .with("wlan show networks", NETWORKS_OTHER);
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
        assert_eq!(outcome, Outcome::NotVisible("SNNU".to_string()));
        assert!(!stub.called("wlan show profiles"), "{:?}", stub.all());
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("wlan connect")),
            "{:?}",
            stub.all()
        );
        // 跳过同样占用冷却：看不到网络时最多每分钟扫描一次
        let again = heal_stub(
            &cfg(true, "SNNU"),
            &stub,
            false,
            &mut state,
            false,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(again, Outcome::Cooldown);
        assert_eq!(
            stub.all()
                .iter()
                .filter(|call| call.starts_with("wlan show networks"))
                .count(),
            1,
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn netsh调用超时按失败处理() {
        // ping 要跑约 3 秒：给 300ms 上限必然超时，之后必须能收尾
        let mut slow = std::process::Command::new("cmd")
            .args(["/c", "ping -n 4 127.0.0.1 >nul"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(!wait_bounded(&mut slow, Duration::from_millis(300)));
        let _ = slow.kill();
        let _ = slow.wait();

        let mut quick = std::process::Command::new("cmd")
            .args(["/c", "exit 0"])
            .spawn()
            .unwrap();
        assert!(wait_bounded(&mut quick, NETSH_TIMEOUT));
        assert!(quick.wait().unwrap().success());
    }
}
