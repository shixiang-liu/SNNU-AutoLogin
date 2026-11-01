// macOS 侧 Wi-Fi 自愈适配：networksetup / ifconfig / airport 只读探测，
// networksetup 连接动作。三条件决策、连接冷却与日志都在 crate::wifi_heal_core。
//
// 与 Windows 版的语义对照（三条件一致，探测手段不同）：
// - 外网/门户可达：复用 portal.rs 的跨平台原语，语义完全相同；
// - 有线在线：networksetup -listallhardwareports 找“以太网类硬件端口”，再看该设备
//   ifconfig 的 status: active（macOS 没有 /sys/class/net 的 carrier 文件）；
// - 当前 SSID：优先 airport -I 解析 ' SSID:'，读不到再回退 networksetup -getairportnetwork；
// - 目标可见：airport -s 扫描结果里是否有这个名称；
// - 已保存配置：networksetup -listpreferredwirelessnetworks，读不到时按“不支持”处理，
//   绝不新建配置；
// - 连接：networksetup -connectairport <设备> <SSID>，开放网络免密。
//
// 有意的行为差异（Windows 没有对应情况）：
// - airport 自 macOS 14.4 起被移除：此时 SSID 走 networksetup 回退，扫描不再可用，
//   可见性检查返回“可见”，靠 60 秒冷却把尝试频率限住，自愈不会因此彻底失效；
// - 部分 macOS 版本没有 networksetup -connectairport 子命令（命令本身存在，但打印用法后
//   非零退出）：这种情况归 UNSUPPORTED 并写日志，不退回 -setairportnetwork——那会新建
//   系统里的网络配置，违反“只连已保存配置”。
//
// 全部外部命令都经 Cmd 注入，测试打桩，绝不在测试里真连 Wi-Fi。

use crate::wifi_heal_core::{ConnectReq, ProfileState, WifiAdapter};
#[cfg(test)]
use crate::wifi_heal_core::{HealState, Outcome};
use std::process::Stdio;

/// 单次外部命令上限：超时按探测失败处理，一次卡住的调用不能拖住整个巡检
const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// airport 的完整路径：它不在 PATH 里，只能按路径调用
const AIRPORT: &str = "/System/Library/PrivateFrameworks/Apple80211.framework/Resources/airport";

/// 外部命令的执行失败原因
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmdError {
    /// 命令或子命令不存在（airport 找不到，或 networksetup 打印用法后非零退出）
    Missing,
    /// 其他失败：超时、非零退出、无法启动
    Failed,
}

/// 外部命令入口：测试注入打桩实现，生产用 SystemCmd
pub trait Cmd {
    /// 执行一条命令；退出码为 0 时返回输出文本
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CmdError>;
}

/// 真实执行外部命令
pub struct SystemCmd;

impl Cmd for SystemCmd {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CmdError> {
        use std::io::Read;
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => CmdError::Missing,
                _ => CmdError::Failed,
            })?;
        if !crate::wifi_heal_core::wait_bounded(&mut child, CMD_TIMEOUT) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CmdError::Failed);
        }
        let status = child.wait().map_err(|_| CmdError::Failed)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_end(&mut stdout);
        }
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_end(&mut stderr);
        }
        if !status.success() {
            // networksetup 对未知子命令打印用法后非零退出：按“这个子命令不可用”处理。
            // 只读输出允许宽松匹配，宁可归 UNSUPPORTED（不动作），也不要误当成连接失败去重试。
            let text = String::from_utf8_lossy(&stderr).to_lowercase();
            let unavailable = ["unrecognized", "invalid option", "usage", "not found"]
                .iter()
                .any(|hint| text.contains(hint));
            return Err(if unavailable {
                CmdError::Missing
            } else {
                CmdError::Failed
            });
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }
}

/// 把注入的命令入口接到共享决策核心
struct Probe<'a>(&'a dyn Cmd);

impl WifiAdapter for Probe<'_> {
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
        wifi_device(self.0)
            .map(|device| profile_state(self.0, &device, ssid))
            .unwrap_or(ProfileState::Unknown)
    }
    fn connect_saved(&self, ssid: &str) -> ConnectReq {
        connect_saved(self.0, ssid)
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

/// 解析 networksetup -listallhardwareports：成对的“硬件端口 → 设备名”
pub fn parse_hardware_ports(text: &str) -> Vec<(String, String)> {
    let mut ports = Vec::new();
    let mut port: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Hardware Port:") {
            port = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Device:") {
            // 只有紧跟“Hardware Port”的“Device”才配对，缺一项就丢弃，避免错位
            let device = value.trim();
            if let (Some(port), false) = (port.take(), device.is_empty()) {
                ports.push((port, device.to_string()));
            }
        }
    }
    ports
}

/// 硬件端口是不是以太网类（虚拟/桥接/手机共享都不算有线）
fn wired_port(name: &str) -> bool {
    let name = name.trim().to_lowercase();
    let wired = name.contains("ethernet")
        || name.contains("以太网")
        || name == "lan"
        || name.ends_with(" lan");
    let virtual_like = [
        "bridge",
        "virtual",
        "vmware",
        "iphone",
        "ipad",
        "bluetooth",
        "tap",
        "vpn",
        "modem",
    ]
    .iter()
    .any(|key| name.contains(key));
    wired && !virtual_like
}

/// 硬件端口是不是 Wi-Fi
fn wifi_port(name: &str) -> bool {
    let name = name.trim().to_lowercase();
    name.contains("wi-fi") || name == "airport"
}

/// 解析 ifconfig 输出：链路是否真的建立了 carrier
pub fn parse_carrier_active(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim().to_lowercase();
        if let Some(state) = line.strip_prefix("status:") {
            return state.trim() == "active";
        }
        // 部分网卡不打印 status：用协商到的线路速率兜底，media 为 (none) 就是没插线
        if let Some(media) = line.strip_prefix("media:") {
            let media = media.trim();
            return !media.is_empty() && media.contains("base") && !media.contains("(none)");
        }
        false
    })
}

/// 解析 airport -I：只认整键 SSID（BSSID 一类同行不算）
pub fn parse_airport_ssid(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = crate::wifi_heal_core::split_key_value(line)?;
        if !key.eq_ignore_ascii_case("SSID") || value.is_empty() {
            return None;
        }
        Some(value.to_string())
    })
}

/// 解析 networksetup -getairportnetwork：没关联时输出没有冒号，自然读成 None
pub fn parse_current_network(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = crate::wifi_heal_core::split_key_value(line)?;
        if !key.eq_ignore_ascii_case("Current Wi-Fi Network") || value.is_empty() {
            return None;
        }
        Some(value.to_string())
    })
}

/// 解析 airport -s：SSID 是第一列，整名相等或整名后接空格再接 BSSID
pub fn parse_scan_visible(text: &str, ssid: &str) -> bool {
    if ssid.is_empty() {
        return false;
    }
    let followed = format!("{ssid} ");
    text.lines().any(|line| {
        let line = line.trim();
        line == ssid || line.starts_with(&followed)
    })
}

/// 解析 networksetup -listpreferredwirelessnetworks：每行一个整名
pub fn parse_preferred(text: &str, ssid: &str) -> bool {
    !ssid.is_empty() && text.lines().any(|line| line.trim() == ssid)
}

/// 硬件端口清单；读不到就是空清单
fn hardware_ports(cmd: &dyn Cmd) -> Vec<(String, String)> {
    cmd.run("networksetup", &["-listallhardwareports"])
        .map(|out| parse_hardware_ports(&out))
        .unwrap_or_default()
}

/// Wi-Fi 设备名（M 系列机器通常是 en0）
fn wifi_device(cmd: &dyn Cmd) -> Option<String> {
    hardware_ports(cmd)
        .into_iter()
        .find(|(port, _)| wifi_port(port))
        .map(|(_, device)| device)
}

/// 有线是否在线：只读查询，不改变任何系统状态
pub fn wired_connected(cmd: &dyn Cmd) -> bool {
    hardware_ports(cmd)
        .iter()
        .filter(|(port, _)| wired_port(port))
        .any(|(_, device)| {
            cmd.run("ifconfig", &[device])
                .is_ok_and(|out| parse_carrier_active(&out))
        })
}

/// 当前已连接 Wi-Fi 的 SSID；没连无线或读不到时返回 None
pub fn current_wifi_ssid(cmd: &dyn Cmd) -> Option<String> {
    // airport 在 macOS 14.4 起可能已不存在，或不再返回 SSID：读不到就回退 networksetup
    if let Ok(out) = cmd.run(AIRPORT, &["-I"]) {
        if let Some(ssid) = parse_airport_ssid(&out) {
            return Some(ssid);
        }
    }
    let device = wifi_device(cmd)?;
    cmd.run("networksetup", &["-getairportnetwork", &device])
        .ok()
        .and_then(|out| parse_current_network(&out))
}

/// 目标 Wi-Fi 当前是不是可见（在范围内）
/// 命令缺失或执行失败（airport 已被移除）时按“可见”放行，限频交给共享核心的冷却；
/// 命令成功但输出为空则按“看不到目标网络”处理，与 Windows 版一致
pub fn target_visible(cmd: &dyn Cmd, ssid: &str) -> bool {
    // 扫描不到时按“可见”放行：airport 缺失（macOS 14.4+）不该让自愈彻底失效，
    // 真正限频的是共享核心里的 60 秒冷却
    cmd.run(AIRPORT, &["-s"])
        .map(|out| parse_scan_visible(&out, ssid))
        .unwrap_or(true)
}

/// 系统里是否保存过该网络配置
fn profile_state(cmd: &dyn Cmd, device: &str, ssid: &str) -> ProfileState {
    match cmd.run("networksetup", &["-listpreferredwirelessnetworks", device]) {
        Ok(out) => {
            if parse_preferred(&out, ssid) {
                ProfileState::Saved
            } else {
                ProfileState::Missing
            }
        }
        Err(_) => ProfileState::Unknown,
    }
}

/// 连接系统里已保存的网络：没有保存过就放弃（不新建配置、不改系统设置）
fn connect_saved(cmd: &dyn Cmd, ssid: &str) -> ConnectReq {
    let Some(device) = wifi_device(cmd) else {
        return ConnectReq::Unsupported("找不到 Wi-Fi 设备");
    };
    match profile_state(cmd, &device, ssid) {
        ProfileState::Saved => {}
        // 与 Windows 一致：没保存过配置就不连，交给核心记“未保存配置”
        ProfileState::Missing => return ConnectReq::Failed,
        ProfileState::Unknown => {
            return ConnectReq::Unsupported("networksetup -listpreferredwirelessnetworks 不可用")
        }
    }
    // 开放网络免密；-connectairport 在部分 macOS 版本不存在，缺失时归 UNSUPPORTED
    match cmd.run("networksetup", &["-connectairport", &device, ssid]) {
        Ok(_) => ConnectReq::Requested,
        Err(CmdError::Missing) => ConnectReq::Unsupported("networksetup -connectairport 不可用"),
        Err(CmdError::Failed) => ConnectReq::Failed,
    }
}

/// 打桩巡检入口：把注入的命令入口接到共享决策核心上（生产路径走核心的挂点）
#[cfg(test)]
pub fn heal(
    cfg: &crate::config::Config,
    cmd: &dyn Cmd,
    portal_ok: bool,
    state: &mut HealState,
    internet_ok: &mut dyn FnMut() -> bool,
    wait: &mut dyn FnMut(std::time::Duration),
    recheck: &mut dyn FnMut() -> bool,
) -> Outcome {
    crate::wifi_heal_core::heal(
        cfg,
        &Probe(cmd),
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
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    const PORTS: &str = "Hardware Port: Wi-Fi\nDevice: en0\n\nHardware Port: Thunderbolt 1\nDevice: en1\n\nHardware Port: Ethernet\nDevice: en5\n";
    const PORTS_WIFI_ONLY: &str = "Hardware Port: Wi-Fi\nDevice: en0\n";
    const PORTS_BRIDGE: &str = "Hardware Port: Wi-Fi\nDevice: en0\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\n\nHardware Port: iPhone USB\nDevice: en6\n";
    const IF_ACTIVE: &str = "en5: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tstatus: active\n";
    const IF_INACTIVE: &str =
        "en5: flags=8863<UP,BROADCAST,SMART,SIMPLEX,MULTICAST> mtu 1500\n\tstatus: inactive\n";
    const AIRPORT_OTHER: &str =
        "     agrCtlRSSI: -52\n     SSID: OtherWiFi\n     BSSID: ac:de:48:00:11:22\n";
    const AIRPORT_NONE: &str = "     agrCtlRSSI: 0\n     state: init\n     op mode: station\n";
    const NETWORK_OTHER: &str = "Current Wi-Fi Network: OtherWiFi\n";
    const NETWORK_NOT_ASSOCIATED: &str = "You are not associated with an AirPort network.\n";
    const SCAN_WITH_SNNU: &str = "                            SSID BSSID             RSSI CHANNEL HT CC SECURITY (auth/unicast/group)\n                        OtherWiFi aa:bb:cc:dd:ee:ff -70  1       Y  CN WPA2(PSK/AES/AES)\n                             SNNU ac:de:48:00:11:22 -45  6       Y  CN WPA2(PSK/AES/AES)\n";
    const SCAN_OTHER: &str = "                            SSID BSSID             RSSI CHANNEL HT CC SECURITY (auth/unicast/group)\n                        OtherWiFi aa:bb:cc:dd:ee:ff -70  1       Y  CN WPA2(PSK/AES/AES)\n";
    const PREFERRED_SNNU: &str = "Preferred networks on en0:\n\tSNNU\n\tOtherWiFi\n";
    const PREFERRED_OTHER: &str = "Preferred networks on en0:\n\tOtherWiFi\n";

    /// 打桩命令入口：按“程序 参数”给预置结果，并记录被调用过的命令
    struct Stub {
        output: HashMap<String, Result<String, CmdError>>,
        calls: RefCell<Vec<String>>,
    }

    impl Stub {
        fn new() -> Self {
            Stub {
                output: HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, command: &str, out: &str) -> Self {
            self.output.insert(command.to_string(), Ok(out.to_string()));
            self
        }

        fn missing(mut self, command: &str) -> Self {
            self.output
                .insert(command.to_string(), Err(CmdError::Missing));
            self
        }

        fn called(&self, command: &str) -> bool {
            self.calls.borrow().iter().any(|call| call == command)
        }

        fn all(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Cmd for Stub {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CmdError> {
            let key = command_line(program, args);
            self.calls.borrow_mut().push(key.clone());
            self.output
                .get(&key)
                .cloned()
                .unwrap_or(Err(CmdError::Failed))
        }
    }

    fn command_line(program: &str, args: &[&str]) -> String {
        if args.is_empty() {
            program.to_string()
        } else {
            format!("{} {}", program, args.join(" "))
        }
    }

    fn airport(args: &str) -> String {
        let args: Vec<&str> = args.split(' ').collect();
        command_line(AIRPORT, &args)
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

    #[test]
    fn 硬件端口按名称与设备成对解析() {
        let ports = parse_hardware_ports(PORTS);
        assert_eq!(
            ports,
            vec![
                ("Wi-Fi".to_string(), "en0".to_string()),
                ("Thunderbolt 1".to_string(), "en1".to_string()),
                ("Ethernet".to_string(), "en5".to_string()),
            ]
        );
        assert!(parse_hardware_ports("").is_empty());
        // 只有 Device 没有 Hardware Port 时不成对
        assert!(parse_hardware_ports("Device: en9\n").is_empty());
        assert!(parse_hardware_ports("Hardware Port: Ethernet\nDevice: \n").is_empty());
    }

    #[test]
    fn 有线端口只认以太网并排除桥接与手机共享() {
        assert!(wired_port("Ethernet"));
        assert!(wired_port("Thunderbolt Ethernet"));
        assert!(wired_port("USB 10/100/1000 LAN"));
        assert!(wired_port("以太网"));
        assert!(!wired_port("Wi-Fi"));
        assert!(!wired_port("Thunderbolt Bridge"));
        assert!(!wired_port("iPhone USB"));
        assert!(!wired_port("VMware Virtual Ethernet"));
        assert!(!wired_port("Bluetooth PAN"));
    }

    #[test]
    fn 无线端口认得出两种命名() {
        assert!(wifi_port("Wi-Fi"));
        assert!(wifi_port("AirPort"));
        assert!(!wifi_port("Ethernet"));
    }

    #[test]
    fn 有线在线只看链路状态() {
        assert!(parse_carrier_active(IF_ACTIVE));
        assert!(!parse_carrier_active(IF_INACTIVE));
        assert!(!parse_carrier_active(""));
        // 没有 status 行时用协商速率兜底，(none) 表示没插线
        assert!(parse_carrier_active(
            "en5: flags=8863<UP,BROADCAST,SMART,RUNNING> mtu 1500\n\tmedia: autoselect (1000baseT <full-duplex>)\n"
        ));
        assert!(!parse_carrier_active(
            "en5: flags=8863<UP,BROADCAST,SMART> mtu 1500\n\tmedia: autoselect (none)\n"
        ));
    }

    #[test]
    fn 当前无线名称优先airport并排除bssid() {
        assert_eq!(
            parse_airport_ssid(AIRPORT_OTHER).as_deref(),
            Some("OtherWiFi")
        );
        assert_eq!(
            parse_airport_ssid("     BSSID: ac:de:48:00:11:22\n     SSID: SNNU\n").as_deref(),
            Some("SNNU")
        );
        assert_eq!(parse_airport_ssid("     SSID: \n"), None);
        assert_eq!(parse_airport_ssid(AIRPORT_NONE), None);
        assert_eq!(parse_airport_ssid(""), None);
    }

    #[test]
    fn networksetup当前网络解析() {
        assert_eq!(
            parse_current_network(NETWORK_OTHER).as_deref(),
            Some("OtherWiFi")
        );
        assert_eq!(parse_current_network(NETWORK_NOT_ASSOCIATED), None);
        assert_eq!(parse_current_network(""), None);
    }

    #[test]
    fn 扫描结果只认第一列的名称() {
        assert!(parse_scan_visible(SCAN_WITH_SNNU, "SNNU"));
        assert!(parse_scan_visible(SCAN_WITH_SNNU, "OtherWiFi"));
        assert!(!parse_scan_visible(SCAN_WITH_SNNU, "SNNU-Guest"));
        assert!(!parse_scan_visible(SCAN_OTHER, "SNNU"));
        assert!(!parse_scan_visible(SCAN_WITH_SNNU, ""));
        assert!(!parse_scan_visible("", "SNNU"));
    }

    #[test]
    fn 首选网络列表按整名匹配() {
        assert!(parse_preferred(PREFERRED_SNNU, "SNNU"));
        assert!(!parse_preferred(PREFERRED_SNNU, "SNNU-Guest"));
        assert!(!parse_preferred(PREFERRED_OTHER, "SNNU"));
        assert!(!parse_preferred("", "SNNU"));
        assert!(!parse_preferred(PREFERRED_SNNU, ""));
    }

    /// 三条件都成立：请求连接并复检门户
    #[test]
    fn 没网没门户没连学校网络时连接并复检门户() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_WITH_SNNU)
            .with(
                "networksetup -listpreferredwirelessnetworks en0",
                PREFERRED_SNNU,
            )
            .with("networksetup -connectairport en0 SNNU", "");
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
        assert!(
            stub.called("networksetup -connectairport en0 SNNU"),
            "{:?}",
            stub.all()
        );
        assert_eq!(waits, 2);
        assert_eq!(probes, 2);
        // 只用 -connectairport：-setairportnetwork 会新建系统网络配置
        assert!(
            stub.all()
                .iter()
                .all(|call| !call.contains("-setairportnetwork")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 有线在线时只查ifconfig不碰无线() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_ACTIVE);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
        assert_eq!(outcome, Outcome::Idle(crate::wifi_heal_core::Skip::Wired));
        assert_eq!(
            stub.all(),
            vec![
                "networksetup -listallhardwareports".to_string(),
                "ifconfig en5".to_string()
            ]
        );
    }

    #[test]
    fn 桥接与手机共享不算有线() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS_BRIDGE)
            .with("ifconfig bridge0", IF_ACTIVE)
            .with("ifconfig en6", IF_ACTIVE)
            .with(&airport("-I"), "     SSID: SNNU\n");
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
            Outcome::Idle(crate::wifi_heal_core::Skip::OnTarget)
        );
        assert!(!stub.called("ifconfig bridge0"), "{:?}", stub.all());
        assert!(!stub.called("ifconfig en6"), "{:?}", stub.all());
    }

    #[test]
    fn airport缺失时回退networksetup读当前网络() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .missing(&airport("-I"))
            .with(
                "networksetup -getairportnetwork en0",
                "Current Wi-Fi Network: SNNU\n",
            );
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
            Outcome::Idle(crate::wifi_heal_core::Skip::OnTarget)
        );
        assert!(
            stub.called("networksetup -getairportnetwork en0"),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 附近看不到目标网络时跳过连接() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_OTHER);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
        assert_eq!(outcome, Outcome::NotVisible("SNNU".to_string()));
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("networksetup -connectairport")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 没有保存配置就放弃并且不发连接命令() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_WITH_SNNU)
            .with(
                "networksetup -listpreferredwirelessnetworks en0",
                PREFERRED_OTHER,
            );
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
        assert_eq!(outcome, Outcome::NoProfile("SNNU".to_string()));
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("networksetup -connectairport")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn connectairport不可用时归不支持而不是反复重试() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_WITH_SNNU)
            .with(
                "networksetup -listpreferredwirelessnetworks en0",
                PREFERRED_SNNU,
            )
            .missing("networksetup -connectairport en0 SNNU");
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
            Outcome::Unsupported {
                ssid: "SNNU".to_string(),
                reason: "networksetup -connectairport 不可用"
            }
        );
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.contains("-setairportnetwork")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 没有无线端口时不连接并归为不支持() {
        let stub = Stub::new()
            .with(
                "networksetup -listallhardwareports",
                "Hardware Port: Ethernet\nDevice: en5\n",
            )
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_WITH_SNNU);
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
            Outcome::Unsupported {
                ssid: "SNNU".to_string(),
                reason: "找不到 Wi-Fi 设备"
            }
        );
        assert!(
            !stub
                .all()
                .iter()
                .any(|call| call.starts_with("networksetup -connectairport")),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 读不到已保存配置时归不支持() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS)
            .with("ifconfig en5", IF_INACTIVE)
            .with(&airport("-I"), AIRPORT_OTHER)
            .with(&airport("-s"), SCAN_WITH_SNNU)
            .missing("networksetup -listpreferredwirelessnetworks en0");
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
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
            Outcome::Unsupported {
                ssid: "SNNU".to_string(),
                reason: "networksetup -listpreferredwirelessnetworks 不可用"
            }
        );
    }

    #[test]
    fn airport缺失时可见性放行而不是放弃自愈() {
        let stub = Stub::new()
            .with("networksetup -listallhardwareports", PORTS_WIFI_ONLY)
            .missing(&airport("-I"))
            .with("networksetup -getairportnetwork en0", NETWORK_OTHER)
            .missing(&airport("-s"))
            .with(
                "networksetup -listpreferredwirelessnetworks en0",
                PREFERRED_SNNU,
            )
            .with("networksetup -connectairport en0 SNNU", "");
        let mut state = HealState::default();
        let mut wait = |_: Duration| {};
        let mut recheck = || true;
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
        assert!(matches!(
            outcome,
            Outcome::Connected {
                portal_ok: true,
                ..
            }
        ));
        assert!(
            stub.called("networksetup -connectairport en0 SNNU"),
            "{:?}",
            stub.all()
        );
    }

    #[test]
    fn 开关关闭时零动作且一条命令都不发() {
        let stub = Stub::new();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || unreachable();
        let outcome = heal(
            &cfg(false, "SNNU"),
            &stub,
            false,
            &mut state,
            &mut internet,
            &mut wait,
            &mut recheck,
        );
        assert_eq!(
            outcome,
            Outcome::Idle(crate::wifi_heal_core::Skip::Disabled)
        );
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }

    #[test]
    fn 外网可达时零动作且一条命令都不发() {
        let stub = Stub::new();
        let mut state = HealState::default();
        let mut wait = |_: Duration| unreachable();
        let mut recheck = || unreachable();
        let mut internet = || true;
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
            Outcome::Idle(crate::wifi_heal_core::Skip::InternetUp)
        );
        assert!(stub.all().is_empty(), "{:?}", stub.all());
    }
}
