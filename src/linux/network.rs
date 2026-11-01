//! Linux 只读网络查询与门户连接：只读 /proc 和 /sys，不改动任何网络设置。
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 门户连接：无头端口直连。
/// Windows/macOS 端口为绕开代理才把连接绑到物理网卡，Linux 端口没有这个需求（也避免引入特权操作）。
pub fn connect_portal(remote: SocketAddr, timeout: Option<Duration>) -> io::Result<TcpStream> {
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&remote, timeout),
        None => TcpStream::connect(remote),
    }
}

/// 只读：默认路由的接口名（状态展示用）。
pub fn current_name() -> String {
    let route = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    default_route_iface(&route).unwrap_or_else(|| "网络未连接".to_string())
}

/// 解析 /proc/net/route：目标是 0.0.0.0 且带 RTF_UP 的那条就是默认路由。
fn default_route_iface(route: &str) -> Option<String> {
    route.lines().skip(1).find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let (name, destination, flags) = (cols.first()?, cols.get(1)?, cols.get(3)?);
        let up = u16::from_str_radix(flags, 16).ok()? & 0x2 != 0;
        (destination == &"00000000" && up).then(|| (*name).to_string())
    })
}

/// 只读链路快照：/sys/class/net 下每个接口一行 "名称 状态 carrier"，
/// carrier 读不到（部分虚拟设备）时写 -，与明确为 0 的“没链路”区分开。
/// 这就是“有线是否在线”判断的唯一数据源，不调用任何可写命令。
pub fn link_snapshot() -> String {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let state = read_field(&entry.path().join("operstate")).unwrap_or_else(|| "unknown".into());
        let carrier = read_field(&entry.path().join("carrier")).unwrap_or_else(|| "-".into());
        lines.push(format!("{name} {state} {carrier}"));
    }
    lines.sort();
    lines.join("\n")
}

fn read_field(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// 网络变化监听：每 2 秒比对一次只读快照，变了就唤醒巡检。
/// 用轮询而不是 netlink，是为了不引入额外依赖；最坏情况也只是晚一个巡检周期。
pub fn spawn_net_watch(wake: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut last = link_snapshot();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = link_snapshot();
            if now != last {
                last = now;
                wake.store(true, Ordering::SeqCst);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
eth0\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\t\t0\t0\t0\n\
wlan0\t0000A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t\t0\t0\t0\n";

    #[test]
    fn 默认路由只认目标为零且带up标志的那条() {
        assert_eq!(default_route_iface(ROUTE).as_deref(), Some("eth0"));
        // 没有默认路由（只有同网段路由）时不算连上网络
        let no_default = "Iface\tDestination\tGateway\nwlan0\t0000A8C0\t00000000\t0001\n";
        assert_eq!(default_route_iface(no_default), None);
        // Flags 里没有 RTF_UP 的默认路由属于残留条目
        let down = "Iface\tDestination\tGateway\neth0\t00000000\t0100A8C0\t0000\n";
        assert_eq!(default_route_iface(down), None);
        assert_eq!(default_route_iface(""), None);
    }

    #[test]
    fn 链路快照每行都是名称状态carrier三列() {
        let snapshot = link_snapshot();
        for line in snapshot.lines() {
            assert_eq!(line.split_whitespace().count(), 3, "{line}");
        }
        // 有 /sys/class/net 的环境至少要能读出 loopback
        if !snapshot.is_empty() {
            assert!(
                snapshot.lines().any(|line| line.starts_with("lo ")),
                "{snapshot}"
            );
        }
    }
}
