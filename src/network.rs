//! 只读取 Windows 当前连接，不扫描热点、不改变网卡或系统权限。
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Bind only this connection; leave the user's proxy and system routes alone.
/// Re-enumerate for every connection so a network change cannot retain an old IP.
pub fn connect_portal(remote: SocketAddr, timeout: Option<Duration>) -> io::Result<TcpStream> {
    if remote.is_ipv4() && !remote.ip().is_loopback() {
        if let Some(source) = physical_ipv4() {
            return connect_from(source, remote, timeout);
        }
    }
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&remote, timeout),
        None => TcpStream::connect(remote),
    }
}

fn connect_from(
    source: Ipv4Addr,
    remote: SocketAddr,
    timeout: Option<Duration>,
) -> io::Result<TcpStream> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.bind(&SocketAddr::from((source, 0)).into())?;
    match timeout {
        Some(timeout) => socket.connect_timeout(&remote.into(), timeout)?,
        None => socket.connect(&remote.into())?,
    }
    Ok(socket.into())
}

fn adapter_priority(kind: u32, name: &str, gateway: bool) -> Option<u8> {
    if !gateway {
        return None;
    }
    if kind == IF_TYPE_IEEE80211 {
        return Some(0);
    }
    if kind != IF_TYPE_ETHERNET_CSMACD {
        return None;
    }
    let name = name.to_ascii_lowercase();
    if [
        "virtual",
        "vmware",
        "hyper-v",
        "tap-",
        "tun",
        "clash",
        "wintun",
        "wireguard",
        "remote ndis",
        "rndis",
        "internet sharing",
    ]
    .iter()
    .any(|marker| name.contains(marker))
    {
        return None;
    }
    Some(1)
}

pub(crate) fn physical_ipv4() -> Option<Ipv4Addr> {
    unsafe {
        let mut size = 16_384u32;
        for _ in 0..3 {
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let first = buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let result = GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(first),
                &mut size,
            );
            if result == 111 {
                continue;
            }
            if result != 0 {
                return None;
            }
            let mut best = None;
            let mut entry = first;
            while !entry.is_null() {
                let adapter = &*entry;
                if adapter.OperStatus == IfOperStatusUp {
                    let description = adapter.Description.to_string().unwrap_or_default();
                    let name = adapter.FriendlyName.to_string().unwrap_or_default();
                    if let Some(priority) = adapter_priority(
                        adapter.IfType,
                        &format!("{name} {description}"),
                        !adapter.FirstGatewayAddress.is_null(),
                    ) {
                        let mut address = adapter.FirstUnicastAddress;
                        while !address.is_null() {
                            let sockaddr = (*address).Address.lpSockaddr;
                            if !sockaddr.is_null()
                                && (*sockaddr).sa_family == AF_INET
                                && (*address).Address.iSockaddrLength as usize
                                    >= std::mem::size_of::<SOCKADDR_IN>()
                            {
                                let ip = Ipv4Addr::from(
                                    (*(sockaddr as *const SOCKADDR_IN))
                                        .sin_addr
                                        .S_un
                                        .S_addr
                                        .to_ne_bytes(),
                                );
                                if !ip.is_unspecified()
                                    && !ip.is_loopback()
                                    && !ip.is_link_local()
                                    && best.is_none_or(|(rank, _)| priority < rank)
                                {
                                    best = Some((priority, ip));
                                }
                            }
                            address = (*address).Next;
                        }
                    }
                }
                entry = adapter.Next;
            }
            return best.map(|(_, ip)| ip);
        }
        None
    }
}

/// A physical Ethernet link prevents automatic Wi-Fi intervention, even before DHCP.
pub(crate) fn wired_link_up() -> io::Result<bool> {
    unsafe {
        let mut size = 16_384u32;
        for _ in 0..3 {
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let first = buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let code = GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(first),
                &mut size,
            );
            if code == 111 {
                continue;
            }
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let mut entry = first;
            while !entry.is_null() {
                let adapter = &*entry;
                if adapter.OperStatus == IfOperStatusUp && adapter.IfType == IF_TYPE_ETHERNET_CSMACD
                {
                    let name = format!(
                        "{} {}",
                        adapter.FriendlyName.to_string().unwrap_or_default(),
                        adapter.Description.to_string().unwrap_or_default()
                    );
                    if adapter_priority(adapter.IfType, &name, true).is_some() {
                        return Ok(true);
                    }
                }
                entry = adapter.Next;
            }
            return Ok(false);
        }
        Err(io::Error::other("网卡信息持续变化，暂缓自动连接 Wi-Fi"))
    }
}

pub fn current_name() -> String {
    unsafe {
        let mut size = 16_384u32;
        for _ in 0..3 {
            // IP_ADAPTER_ADDRESSES 需要指针对齐，不能直接把 Vec<u8> 强转。
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let first = buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let result =
                GetAdaptersAddresses(0, GAA_FLAG_INCLUDE_GATEWAYS, None, Some(first), &mut size);
            if result == 111 {
                continue;
            } // ERROR_BUFFER_OVERFLOW
            if result != 0 {
                break;
            }
            let mut best = 0;
            let _ = GetBestInterface(u32::from_ne_bytes([202, 117, 144, 205]), &mut best);
            let mut entry = first;
            let mut wired = false;
            let mut wifi = false;
            while !entry.is_null() {
                let adapter = &*entry;
                if adapter.OperStatus == IfOperStatusUp && !adapter.FirstUnicastAddress.is_null() {
                    if adapter.IfType == IF_TYPE_ETHERNET_CSMACD
                        && !adapter.FirstGatewayAddress.is_null()
                    {
                        wired = true;
                        if adapter.Anonymous1.Anonymous.IfIndex == best {
                            return "有线网络".into();
                        }
                    }
                    if adapter.IfType == IF_TYPE_IEEE80211 {
                        wifi = true;
                    }
                }
                entry = adapter.Next;
            }
            if let Some(ssid) = current_ssid() {
                return ssid;
            }
            if wifi {
                return "Wi-Fi · 名称暂不可读".into();
            }
            if wired {
                return "有线网络".into();
            }
            return "尚未连接网络".into();
        }
        current_ssid().unwrap_or_else(|| "当前网络".into())
    }
}

unsafe fn current_ssid() -> Option<String> {
    let mut version = 0;
    let mut handle = HANDLE::default();
    if WlanOpenHandle(2, None, &mut version, &mut handle) != 0 {
        return None;
    }
    let mut list = std::ptr::null_mut();
    let mut name = None;
    if WlanEnumInterfaces(handle, None, &mut list) == 0 && !list.is_null() {
        let interfaces = std::slice::from_raw_parts(
            (*list).InterfaceInfo.as_ptr(),
            (*list).dwNumberOfItems as usize,
        );
        for interface in interfaces {
            if interface.isState != wlan_interface_state_connected {
                continue;
            }
            let mut size = 0;
            let mut data = std::ptr::null_mut();
            if WlanQueryInterface(
                handle,
                &interface.InterfaceGuid,
                wlan_intf_opcode_current_connection,
                None,
                &mut size,
                &mut data,
                None,
            ) == 0
                && !data.is_null()
            {
                if size as usize >= std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
                    let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
                    name = decode_ssid(&ssid.ucSSID, ssid.uSSIDLength as usize);
                }
                WlanFreeMemory(data);
            }
            if name.is_some() {
                break;
            }
        }
        WlanFreeMemory(list as _);
    }
    let _ = WlanCloseHandle(handle, None);
    name
}

fn decode_ssid(bytes: &[u8], len: usize) -> Option<String> {
    let bytes = bytes.get(..len)?;
    if bytes.is_empty() {
        return None;
    }
    let decoded = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.to_string(),
    };
    let name: String = decoded
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn campus_route_excludes_vpn_and_phone_tethering() {
        assert_eq!(
            adapter_priority(IF_TYPE_IEEE80211, "MediaTek Wi-Fi", true),
            Some(0)
        );
        assert_eq!(
            adapter_priority(IF_TYPE_ETHERNET_CSMACD, "Intel Ethernet", true),
            Some(1)
        );
        for name in [
            "FlClash",
            "Remote NDIS based Internet Sharing Device",
            "VMware Virtual Ethernet Adapter",
            "WireGuard Tunnel",
            "TAP-Windows Adapter",
        ] {
            assert_eq!(adapter_priority(IF_TYPE_ETHERNET_CSMACD, name, true), None);
        }
        assert_eq!(
            adapter_priority(IF_TYPE_ETHERNET_CSMACD, "Intel Ethernet", false),
            None
        );
        assert_eq!(
            adapter_priority(IF_TYPE_IEEE80211, "Wi-Fi Direct", false),
            None
        );
    }

    #[test]
    fn bound_socket_uses_requested_source_address() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = connect_from(
            Ipv4Addr::LOCALHOST,
            listener.local_addr().unwrap(),
            Some(Duration::from_secs(1)),
        )
        .unwrap();
        let (_, peer) = listener.accept().unwrap();
        assert_eq!(client.local_addr().unwrap(), peer);
        assert_eq!(peer.ip(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn loopback_http_fixtures_bypass_physical_network_selection() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client =
            connect_portal(listener.local_addr().unwrap(), Some(Duration::from_secs(1))).unwrap();
        assert!(client.local_addr().unwrap().ip().is_loopback());
    }
    #[test]
    fn ssid_uses_actual_length_and_preserves_unicode() {
        assert_eq!(
            decode_ssid(b"SNNU-Student\0\0", 12).as_deref(),
            Some("SNNU-Student")
        );
        let name = "师大无线";
        assert_eq!(
            decode_ssid(name.as_bytes(), name.len()).as_deref(),
            Some(name)
        );
        assert_eq!(decode_ssid(&[1, 2], 33), None);
        assert_eq!(decode_ssid(&[], 0), None);
    }
}
