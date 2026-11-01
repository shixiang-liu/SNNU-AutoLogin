// Windows Wi-Fi 自愈：只有“本身没网、门户也不可达、又不在校园网里”三条同时成立，
// 才把 Wi-Fi 连回学校网络。只连系统里已经保存的配置，不新建配置、不改系统设置。
//
// 条件 1：外部互联网不可用（只是门户不可达不算，门户自身故障时绝不动 Wi-Fi）
// 条件 2：门户探测失败
// 条件 3：没有有线在线，且当前 Wi-Fi 不是目标学校网络
// 三条都成立后还要确认目标网络真的在范围内，免得在校外每轮都发一次注定失败的连接。
// 三条条件都由调用方注入探测函数，本模块只做决策与 netsh 调用，测试全部打桩。

use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 学校 Wi-Fi 的默认名称
pub const DEFAULT_SSID: &str = "SNNU";
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
/// 两次连接动作之间的最小间隔，避免每轮巡检都去连接
const CONNECT_COOLDOWN: Duration = Duration::from_secs(60);
/// 发起连接后等待门户可达的轮数（每轮 1 秒间隔加一次门户探测）
const RECHECK_ROUNDS: u32 = 6;
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

/// 有界等待子进程结束：超时返回 false，由调用方负责收尾
fn wait_bounded(child: &mut std::process::Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => return false,
        }
    }
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

/// 按首个冒号切出“键 : 值”（中文输出也可能是全角冒号），值已去空白
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let at = line.find([':', '：'])?;
    let sep = line[at..].chars().next()?;
    Some((line[..at].trim(), line[at + sep.len_utf8()..].trim()))
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
    /// 附近看不到目标 Wi-Fi，本轮不连接
    NotVisible(String),
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
    // 校外没有目标网络时连接只会白试：跳过也占用冷却，扫描最多每分钟一次
    if !target_visible(cmd, target) {
        return Outcome::NotVisible(target.to_string());
    }
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

    /// 打桩 netsh：按子命令给预置输出，并记录被调用过的子命令
    struct Stub {
        output: HashMap<String, String>,
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
            self.output.insert(args.to_string(), out.to_string());
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
            self.output.get(&key).cloned()
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
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
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
    fn 连接动作有冷却不会每轮都连() {
        let stub = Stub::new()
            .with("interface show interface", CN_WIRED_DOWN)
            .with("wlan show interfaces", WLAN_OTHER)
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
        assert_eq!(second, Outcome::Cooldown);
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
