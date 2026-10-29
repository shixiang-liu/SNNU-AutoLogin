//! SystemConfiguration notifications cover address, route and link changes.
use core_foundation::{array::CFArray, base::TCFType, string::CFString};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::{
    collections::{HashMap, HashSet},
    ffi::CStr,
    io,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};
#[link(name = "CoreWLAN", kind = "framework")]
extern "C" {}
#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCreate(
        allocator: *const c_void,
        name: *const c_void,
        callback: Option<extern "C" fn(*const c_void, *const c_void, *mut c_void)>,
        context: *const c_void,
    ) -> *const c_void;
    fn SCDynamicStoreSetNotificationKeys(
        store: *const c_void,
        keys: *const c_void,
        patterns: *const c_void,
    ) -> u8;
    fn SCDynamicStoreCreateRunLoopSource(
        allocator: *const c_void,
        store: *const c_void,
        order: isize,
    ) -> *const c_void;
    fn SCDynamicStoreCopyValue(store: *const c_void, key: *const c_void) -> *const c_void;
    fn SCDynamicStoreCopyKeyList(store: *const c_void, pattern: *const c_void) -> *const c_void;
    fn SCNetworkInterfaceCopyAll() -> *const c_void;
    fn SCNetworkInterfaceGetBSDName(interface: *const c_void) -> *const c_void;
    fn SCNetworkInterfaceGetInterfaceType(interface: *const c_void) -> *const c_void;
    fn SCNetworkInterfaceGetLocalizedDisplayName(interface: *const c_void) -> *const c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> *const c_void;
    fn CFRunLoopAddSource(runloop: *const c_void, source: *const c_void, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFRelease(value: *const c_void);
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    static kCFRunLoopDefaultMode: *const c_void;
}
#[derive(Debug)]
pub(crate) struct PhysicalSource {
    name: String,
    index: u32,
    ip: Ipv4Addr,
}
fn adapter_priority(name: &str, kind: &str, display: &str) -> Option<u8> {
    let display = display.to_ascii_lowercase();
    if !name.starts_with("en")
        || ["iphone", "ipad", "tether", "bluetooth", "virtual", "vmware"]
            .iter()
            .any(|word| display.contains(word))
    {
        return None;
    }
    match kind {
        "IEEE80211" => Some(0),
        "Ethernet" => Some(1),
        _ => None,
    }
}
unsafe fn cf_string(value: *const c_void) -> String {
    if value.is_null() {
        String::new()
    } else {
        CFString::wrap_under_get_rule(value.cast()).to_string()
    }
}
unsafe fn routed_interfaces() -> HashSet<String> {
    let mut names = HashSet::new();
    let pattern = CFString::new("State:/Network/Service/[^/]+/IPv4");
    let keys = SCDynamicStoreCopyKeyList(std::ptr::null(), pattern.as_concrete_TypeRef().cast());
    if !keys.is_null() {
        for i in 0..CFArrayGetCount(keys) {
            let value = SCDynamicStoreCopyValue(std::ptr::null(), CFArrayGetValueAtIndex(keys, i));
            if !value.is_null() {
                let router: Option<objc2::rc::Retained<NSString>> =
                    msg_send![value as *mut AnyObject,objectForKey:&*NSString::from_str("Router")];
                let name: Option<objc2::rc::Retained<NSString>> = msg_send![value as *mut AnyObject,objectForKey:&*NSString::from_str("InterfaceName")];
                if router.is_some_and(|router| {
                    router
                        .to_string()
                        .parse::<Ipv4Addr>()
                        .is_ok_and(|ip| !ip.is_unspecified())
                }) {
                    if let Some(name) = name {
                        names.insert(name.to_string());
                    }
                }
                CFRelease(value);
            }
        }
        CFRelease(keys);
    }
    names
}
pub(crate) fn physical_source() -> Option<PhysicalSource> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let routed = routed_interfaces();
        let interfaces = SCNetworkInterfaceCopyAll();
        if interfaces.is_null() {
            return None;
        }
        let mut ranks = HashMap::new();
        for i in 0..CFArrayGetCount(interfaces) {
            let interface = CFArrayGetValueAtIndex(interfaces, i);
            let name = cf_string(SCNetworkInterfaceGetBSDName(interface));
            let kind = cf_string(SCNetworkInterfaceGetInterfaceType(interface));
            let display = cf_string(SCNetworkInterfaceGetLocalizedDisplayName(interface));
            if routed.contains(&name) {
                if let Some(rank) = adapter_priority(&name, &kind, &display) {
                    ranks.insert(name, rank);
                }
            }
        }
        CFRelease(interfaces);
        let mut first = std::ptr::null_mut();
        if libc::getifaddrs(&mut first) != 0 {
            return None;
        }
        let mut current = first;
        let mut best: Option<(u8, PhysicalSource)> = None;
        while !current.is_null() {
            let entry = &*current;
            if !entry.ifa_addr.is_null()
                && !entry.ifa_name.is_null()
                && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET
                && entry.ifa_flags & (libc::IFF_UP | libc::IFF_RUNNING) as u32
                    == (libc::IFF_UP | libc::IFF_RUNNING) as u32
            {
                let name = CStr::from_ptr(entry.ifa_name).to_string_lossy();
                if let Some(&rank) = ranks.get(name.as_ref()) {
                    let ip = Ipv4Addr::from(
                        (*(entry.ifa_addr as *const libc::sockaddr_in))
                            .sin_addr
                            .s_addr
                            .to_ne_bytes(),
                    );
                    let index = libc::if_nametoindex(entry.ifa_name);
                    if index != 0
                        && !ip.is_unspecified()
                        && !ip.is_loopback()
                        && !ip.is_link_local()
                        && best.as_ref().is_none_or(|(old, _)| rank < *old)
                    {
                        best = Some((
                            rank,
                            PhysicalSource {
                                name: name.into_owned(),
                                index,
                                ip,
                            },
                        ));
                    }
                }
            }
            current = entry.ifa_next;
        }
        libc::freeifaddrs(first);
        best.map(|(_, source)| source)
    })
}
/// Pin this socket to a live physical interface without changing VPN settings
/// or system routes. Re-enumeration on every connection follows network changes.
pub fn connect_portal(remote: SocketAddr, timeout: Option<Duration>) -> io::Result<TcpStream> {
    if remote.is_ipv4() && !remote.ip().is_loopback() {
        if let Some(source) = physical_source() {
            return connect_from(&source, remote, timeout);
        }
    }
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&remote, timeout),
        None => TcpStream::connect(remote),
    }
}
fn connect_from(
    source: &PhysicalSource,
    remote: SocketAddr,
    timeout: Option<Duration>,
) -> io::Result<TcpStream> {
    use std::os::fd::AsRawFd;
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    // Darwin requires the interface scope as well as the local source address.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_BOUND_IF,
            (&source.index as *const u32).cast(),
            std::mem::size_of::<u32>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    socket.bind(&SocketAddr::from((source.ip, 0)).into())?;
    match timeout {
        Some(timeout) => socket.connect_timeout(&remote.into(), timeout)?,
        None => socket.connect(&remote.into())?,
    }
    Ok(socket.into())
}
extern "C" fn changed(_: *const c_void, _: *const c_void, _: *mut c_void) {
    // Coalesce notifications in the worker's wake flag.
    crate::macos::runtime::wake();
}
pub fn watch() {
    std::thread::Builder::new()
        .name("snnu-network".into())
        .spawn(|| unsafe {
            let name = CFString::new("SNNU-AutoLogin");
            let store = SCDynamicStoreCreate(
                std::ptr::null(),
                name.as_concrete_TypeRef().cast(),
                Some(changed),
                std::ptr::null(),
            );
            if store.is_null() {
                crate::config::logf("网络事件监听创建失败，继续使用定时巡检");
                return;
            }
            let patterns = CFArray::from_CFTypes(&[CFString::new("State:/Network/.*")]);
            if SCDynamicStoreSetNotificationKeys(
                store,
                std::ptr::null(),
                patterns.as_concrete_TypeRef().cast(),
            ) == 0
            {
                CFRelease(store);
                crate::config::logf("网络事件订阅失败，继续使用定时巡检");
                return;
            }
            let source = SCDynamicStoreCreateRunLoopSource(std::ptr::null(), store, 0);
            if source.is_null() {
                CFRelease(store);
                crate::config::logf("网络事件监听：创建 RunLoop 源失败，继续使用定时巡检");
                return;
            }
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
            CFRunLoopRun();
            CFRelease(source);
            CFRelease(store);
        })
        .expect("网络监听线程启动失败");
}
pub fn current_name() -> String {
    objc2::rc::autoreleasepool(|_| unsafe {
        let client: *mut AnyObject = msg_send![class!(CWWiFiClient), sharedWiFiClient];
        let wifi: *mut AnyObject = msg_send![client, interface];
        let ssid: Option<objc2::rc::Retained<NSString>> = if wifi.is_null() {
            None
        } else {
            msg_send![wifi, ssid]
        };
        let wifi_name: Option<objc2::rc::Retained<NSString>> = if wifi.is_null() {
            None
        } else {
            msg_send![wifi, interfaceName]
        };
        if let Some(source) = physical_source() {
            return if wifi_name
                .as_ref()
                .is_some_and(|name| name.to_string() == source.name)
            {
                ssid.as_ref()
                    .map(|name| name.to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Wi-Fi · 名称暂不可读".into())
            } else {
                "有线网络".into()
            };
        }
        for family in ["IPv4", "IPv6"] {
            let key = CFString::new(&format!("State:/Network/Global/{family}"));
            let value = SCDynamicStoreCopyValue(std::ptr::null(), key.as_concrete_TypeRef().cast());
            if value.is_null() {
                continue;
            }
            let primary: Option<objc2::rc::Retained<NSString>> = msg_send![value as *mut AnyObject, objectForKey: &*NSString::from_str("PrimaryInterface")];
            let result = primary.map(|primary| {
                if wifi_name
                    .as_ref()
                    .is_some_and(|name| name.to_string() == primary.to_string())
                {
                    ssid.as_ref()
                        .map(|name| name.to_string())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| "Wi-Fi · 名称暂不可读".into())
                } else {
                    "有线网络".into()
                }
            });
            CFRelease(value);
            if let Some(result) = result {
                return result;
            }
        }
        "尚未连接网络".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn campus_route_excludes_vpn_virtual_and_phone_interfaces() {
        assert_eq!(adapter_priority("en0", "IEEE80211", "Wi-Fi"), Some(0));
        assert_eq!(adapter_priority("en1", "Ethernet", "USB Ethernet"), Some(1));
        assert_eq!(adapter_priority("utun0", "PPP", "VPN"), None);
        assert_eq!(adapter_priority("en7", "Ethernet", "iPhone USB"), None);
        assert_eq!(
            adapter_priority("vmnet0", "Ethernet", "Virtual Adapter"),
            None
        );
    }
    #[test]
    fn bound_socket_uses_the_requested_interface_and_source_address() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let remote = listener.local_addr().unwrap();
        let source = PhysicalSource {
            name: "lo0".into(),
            index: unsafe { libc::if_nametoindex(c"lo0".as_ptr()) },
            ip: Ipv4Addr::LOCALHOST,
        };
        assert_ne!(source.index, 0);
        let stream = connect_from(&source, remote, Some(Duration::from_secs(2))).unwrap();
        assert_eq!(stream.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        assert!(listener.accept().unwrap().1.ip().is_loopback());
    }
    #[test]
    fn loopback_fixtures_bypass_physical_selection() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let stream =
            connect_portal(listener.local_addr().unwrap(), Some(Duration::from_secs(2))).unwrap();
        assert!(stream.local_addr().unwrap().ip().is_loopback());
        assert!(listener.accept().unwrap().1.ip().is_loopback());
    }
}
