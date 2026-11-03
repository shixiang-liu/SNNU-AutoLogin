//! Opt-in Wi-Fi recovery. Connection throttling never throttles portal detection.
//! Only saved, visible SNNU profiles are used; no profiles, passwords or routes are changed.
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::*;

const SSID: &[u8] = b"SNNU";
const CONNECT_INTERVAL: Duration = Duration::from_secs(60);
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(15);
const PENDING_WINDOW: Duration = Duration::from_secs(45);
const EVENT_CHANGED: u32 = 1;
const EVENT_CONNECTED: u32 = 2;
const EVENT_DISCONNECTED: u32 = 4;

struct EventContext {
    wake: Arc<AtomicBool>,
    events: AtomicU32,
}

/// ACM notifications do not require subscribing to location-sensitive MSM events.
/// Keep the context alive until unregister and handle close have drained callbacks.
pub(crate) struct Watch {
    native: Native,
    context: Box<EventContext>,
}
impl Watch {
    pub(crate) fn start(wake: Arc<AtomicBool>) -> io::Result<Self> {
        let watch = Self {
            native: Native::open()?,
            context: Box::new(EventContext {
                wake,
                events: AtomicU32::new(0),
            }),
        };
        unsafe {
            check(WlanRegisterNotification(
                watch.native.0,
                WLAN_NOTIFICATION_SOURCE_ACM,
                false,
                Some(wlan_event),
                Some((&*watch.context as *const EventContext).cast()),
                None,
                None,
            ))?;
        }
        Ok(watch)
    }
    pub(crate) fn drain(&self, recovery: &mut Recovery) {
        recovery.wlan_events(self.context.events.swap(0, Ordering::SeqCst));
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = WlanRegisterNotification(
                self.native.0,
                WLAN_NOTIFICATION_SOURCE_NONE,
                false,
                None,
                None,
                None,
                None,
            );
        }
    }
}
fn event_flags(source: WLAN_NOTIFICATION_SOURCES, code: u32) -> u32 {
    if source != WLAN_NOTIFICATION_SOURCE_ACM {
        return 0;
    }
    match WLAN_NOTIFICATION_ACM(code as i32) {
        value if value == wlan_notification_acm_connection_complete => {
            EVENT_CHANGED | EVENT_CONNECTED
        }
        value if value == wlan_notification_acm_disconnected => EVENT_CHANGED | EVENT_DISCONNECTED,
        value
            if [
                wlan_notification_acm_scan_complete,
                wlan_notification_acm_scan_list_refresh,
                wlan_notification_acm_network_available,
                wlan_notification_acm_interface_arrival,
                wlan_notification_acm_connection_attempt_fail,
            ]
            .contains(&value) =>
        {
            EVENT_CHANGED
        }
        _ => 0,
    }
}
unsafe extern "system" fn wlan_event(
    data: *mut L2_NOTIFICATION_DATA,
    context: *mut std::ffi::c_void,
) {
    if data.is_null() || context.is_null() {
        return;
    }
    let mut flags = event_flags((*data).NotificationSource, (*data).NotificationCode);
    // A connection_complete can also report failure. Only a confirmed success
    // ends the previous request's cooldown; scan/failure events must not do so.
    if flags & EVENT_CONNECTED != 0
        && ((*data).pData.is_null()
            || ((*data).dwDataSize as usize)
                < std::mem::size_of::<WLAN_CONNECTION_NOTIFICATION_DATA>()
            || (*((*data).pData as *const WLAN_CONNECTION_NOTIFICATION_DATA)).wlanReasonCode != 0)
    {
        flags &= !EVENT_CONNECTED;
    }
    if flags != 0 {
        let context = &*(context as *const EventContext);
        context.events.fetch_or(flags, Ordering::SeqCst);
        context.wake.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Disabled,
    ConnectedOrBusy,
    InternetAvailable,
    Pending,
    Cooldown,
    NotVisibleOrSaved,
    Requested,
    Cancelled,
}

trait Adapter {
    type Candidate;
    fn detached(&mut self) -> io::Result<bool>;
    fn candidate(&mut self) -> io::Result<Option<Self::Candidate>>;
    fn connect(
        &mut self,
        candidate: Self::Candidate,
        allowed: &mut dyn FnMut() -> bool,
    ) -> io::Result<bool>;
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbePlan {
    WaitForAddress,
    Probe(Duration),
}

#[derive(Default)]
pub(crate) struct Recovery {
    last_connect: Option<Instant>,
    next_discovery: Option<Instant>,
    pending_until: Option<Instant>,
    last_notice: String,
}

impl Recovery {
    pub(crate) fn waiting(&self) -> bool {
        self.waiting_at(Instant::now())
    }
    pub(crate) fn probe_plan(&self, address_ready: bool) -> ProbePlan {
        self.probe_plan_at(Instant::now(), address_ready)
    }
    fn probe_plan_at(&self, now: Instant, address_ready: bool) -> ProbePlan {
        if self.waiting_at(now) {
            if address_ready {
                ProbePlan::Probe(Duration::from_secs(1))
            } else {
                ProbePlan::WaitForAddress
            }
        } else {
            ProbePlan::Probe(Duration::from_secs(4))
        }
    }
    fn waiting_at(&self, now: Instant) -> bool {
        self.pending_until.is_some_and(|until| now < until)
    }
    /// Cancels follow-up work, not the user's current Wi-Fi or authenticated session.
    /// Keep connection cooldown so pause/resume cannot create a connection storm.
    pub(crate) fn cancel(&mut self) {
        self.pending_until = None;
        self.next_discovery = None;
    }
    pub(crate) fn address_ready(&mut self) {
        if self.waiting() {
            self.notice("网络地址已就绪，开始检测校园网认证服务器");
        }
    }
    pub(crate) fn portal_reached(&mut self) {
        let pending = self.waiting();
        self.cancel();
        if pending {
            self.notice("认证服务器已可达，结束 Wi-Fi 连接等待");
        }
    }
    fn notice(&mut self, text: &str) {
        if self.last_notice != text {
            crate::config::logf(text);
            self.last_notice = text.into();
        }
    }
    pub(crate) fn unreadable_state(&mut self) {
        self.cancel();
        let notice = "自动登录状态无法读取，已暂停 Wi-Fi 自愈；请检查诊断后重新连接";
        if self.last_notice != notice {
            crate::config::logf(notice);
            self.last_notice = notice.into();
        }
    }
    pub(crate) fn network_changed(&mut self) {
        self.next_discovery = None;
    }
    fn wlan_events(&mut self, events: u32) {
        if events & EVENT_CONNECTED != 0 {
            // Association succeeded: a later disconnect is a new recovery cycle,
            // not a retry of the previous request, even if DHCP/portal is slow.
            self.last_connect = None;
            self.cancel();
            self.notice("Wi-Fi 已连接，上一轮连接请求的冷却已结束");
        }
        if events & EVENT_DISCONNECTED != 0 {
            self.cancel();
            self.notice("Wi-Fi 已断开，重新等待已保存的 SNNU");
        }
        if events & EVENT_CHANGED != 0 {
            self.network_changed();
        }
    }

    fn tick<A: Adapter>(
        &mut self,
        now: Instant,
        adapter: &mut A,
        allowed: &mut dyn FnMut() -> bool,
        internet_ok: &mut dyn FnMut() -> bool,
    ) -> io::Result<Outcome> {
        if !allowed() {
            self.cancel();
            return Ok(Outcome::Disabled);
        }
        if self.waiting_at(now) {
            return Ok(Outcome::Pending);
        }
        // These timers gate only new connection/discovery work. The agent probes
        // the portal before reaching this method on every iteration.
        if self
            .last_connect
            .is_some_and(|at| now.saturating_duration_since(at) < CONNECT_INTERVAL)
        {
            return Ok(Outcome::Cooldown);
        }
        if self.next_discovery.is_some_and(|until| now < until) {
            return Ok(Outcome::Cooldown);
        }
        self.next_discovery = Some(now + DISCOVERY_INTERVAL);
        if !adapter.detached()? {
            return Ok(Outcome::ConnectedOrBusy);
        }
        if internet_ok() {
            return Ok(Outcome::InternetAvailable);
        }
        if !allowed() {
            self.cancel();
            return Ok(Outcome::Cancelled);
        }
        let Some(candidate) = adapter.candidate()? else {
            return Ok(Outcome::NotVisibleOrSaved);
        };
        // Recheck link state after discovery and the possibly slow Internet probe.
        if !allowed() || !adapter.detached()? {
            self.cancel();
            return Ok(Outcome::Cancelled);
        }
        // Count failed submissions as attempts too. Otherwise an API failure can
        // repeatedly interrupt association or flood a stopped WLAN service.
        let request = adapter.connect(candidate, allowed);
        let submitted_at = Instant::now().max(now);
        match request {
            Ok(false) => {
                self.cancel();
                return Ok(Outcome::Cancelled);
            }
            Err(error) => {
                self.last_connect = Some(submitted_at);
                return Err(error);
            }
            Ok(true) => self.last_connect = Some(submitted_at),
        }
        self.pending_until = Some(submitted_at + PENDING_WINDOW);
        Ok(Outcome::Requested)
    }

    pub(crate) fn on_unreachable(&mut self, quit: &AtomicBool) {
        let mut allowed = || {
            !quit.load(Ordering::SeqCst)
                && crate::config::load_config().0.wifi_self_heal
                && crate::config::load_state_checked().is_ok_and(|state| {
                    crate::portal::automatic_login_allowed(&state, crate::portal::unix_now())
                })
        };
        // Do not even open WLAN or probe the public Internet while disabled or
        // while our previous association request is still pending.
        if !allowed() {
            self.cancel();
            return;
        }
        if self.waiting() {
            return;
        }
        let now = Instant::now();
        if self
            .last_connect
            .is_some_and(|at| now.saturating_duration_since(at) < CONNECT_INTERVAL)
            || self.next_discovery.is_some_and(|until| now < until)
        {
            return;
        }
        let result = Native::open().and_then(|mut adapter| {
            self.tick(
                now,
                &mut adapter,
                &mut allowed,
                // With no physical IPv4 and no active Wi-Fi/wired link (checked
                // by tick), avoid spending two HTTP timeouts before association.
                &mut || crate::network::physical_ipv4().is_some() && crate::portal::internet_ok(),
            )
        });
        let notice = match result {
            Ok(Outcome::Requested) => {
                // Each request is a timing event, including retries after cooldown.
                crate::config::logf("已请求连接已保存的 SNNU，网络就绪后自动认证");
                self.last_notice.clear();
                return;
            }
            Ok(Outcome::NotVisibleOrSaved) => {
                "未发现可用且已保存的 SNNU，请先在系统中手动连接一次".to_string()
            }
            Ok(Outcome::ConnectedOrBusy) => "已有网络连接或系统正在连接，保留当前网络".to_string(),
            Ok(Outcome::InternetAvailable) => "当前可以上网，不自动连接学校 Wi-Fi".to_string(),
            Err(error) => {
                self.next_discovery = Some(now + CONNECT_INTERVAL);
                format!("自动连接学校 Wi-Fi 暂不可用：{error}")
            }
            _ => return,
        };
        if self.last_notice != notice {
            crate::config::logf(&notice);
            self.last_notice = notice;
        }
    }
}

struct WlanMemory<T>(*mut T);
impl<T> Drop for WlanMemory<T> {
    fn drop(&mut self) {
        unsafe {
            WlanFreeMemory(self.0.cast());
        }
    }
}
struct Native(HANDLE);
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            let _ = WlanCloseHandle(self.0, None);
        }
    }
}
struct Candidate {
    interface: GUID,
    profile: [u16; 256],
    ssid: DOT11_SSID,
    bss: DOT11_BSS_TYPE,
}
fn check(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
impl Native {
    fn open() -> io::Result<Self> {
        let mut handle = HANDLE::default();
        let mut version = 0;
        unsafe {
            check(WlanOpenHandle(2, None, &mut version, &mut handle))?;
        }
        Ok(Self(handle))
    }
    fn interfaces(&self) -> io::Result<Vec<WLAN_INTERFACE_INFO>> {
        unsafe {
            let mut raw = std::ptr::null_mut();
            check(WlanEnumInterfaces(self.0, None, &mut raw))?;
            if raw.is_null() {
                return Err(io::Error::other("无法读取无线网卡"));
            }
            let memory = WlanMemory(raw);
            let list = &*memory.0;
            Ok(std::slice::from_raw_parts(
                list.InterfaceInfo.as_ptr(),
                list.dwNumberOfItems as usize,
            )
            .to_vec())
        }
    }
}
impl Adapter for Native {
    type Candidate = Candidate;
    fn detached(&mut self) -> io::Result<bool> {
        if crate::network::wired_link_up()? {
            return Ok(false);
        }
        let interfaces = self.interfaces()?;
        // Also preserve other Wi-Fi networks. Association without DHCP is still
        // connected; repeatedly submitting WlanConnect here would reset progress.
        Ok(!interfaces.is_empty()
            && interfaces.iter().all(|interface| {
                interface.isState == wlan_interface_state_disconnected
                    || interface.isState == wlan_interface_state_not_ready
            })
            && interfaces
                .iter()
                .any(|interface| interface.isState == wlan_interface_state_disconnected))
    }
    fn candidate(&mut self) -> io::Result<Option<Candidate>> {
        for interface in self.interfaces()? {
            if interface.isState != wlan_interface_state_disconnected {
                continue;
            }
            unsafe {
                let mut raw = std::ptr::null_mut();
                check(WlanGetAvailableNetworkList(
                    self.0,
                    &interface.InterfaceGuid,
                    0,
                    None,
                    &mut raw,
                ))?;
                if raw.is_null() {
                    return Err(io::Error::other("无法读取附近网络"));
                }
                let memory = WlanMemory(raw);
                let list = &*memory.0;
                let networks = std::slice::from_raw_parts(
                    list.Network.as_ptr(),
                    list.dwNumberOfItems as usize,
                );
                for network in networks {
                    if eligible(network) {
                        return Ok(Some(Candidate {
                            interface: interface.InterfaceGuid,
                            profile: network.strProfileName,
                            ssid: network.dot11Ssid,
                            bss: network.dot11BssType,
                        }));
                    }
                }
            }
        }
        Ok(None)
    }
    fn connect(
        &mut self,
        mut candidate: Candidate,
        allowed: &mut dyn FnMut() -> bool,
    ) -> io::Result<bool> {
        // Serialize with manual disconnect/account changes, but do not hold the
        // connection lock during discovery, Internet probes, DHCP or waiting.
        let _guard = crate::portal::connection_lock().map_err(io::Error::other)?;
        if !allowed() || !self.detached()? {
            return Ok(false);
        }
        let parameters = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(candidate.profile.as_ptr()),
            pDot11Ssid: &mut candidate.ssid,
            dot11BssType: candidate.bss,
            ..Default::default()
        };
        // ERROR_SUCCESS means request accepted, not associated or authenticated.
        unsafe {
            check(WlanConnect(self.0, &candidate.interface, &parameters, None))?;
        }
        Ok(true)
    }
}
fn eligible(network: &WLAN_AVAILABLE_NETWORK) -> bool {
    network.dot11Ssid.uSSIDLength as usize == SSID.len()
        && network.dot11Ssid.ucSSID[..SSID.len()] == *SSID
        && network.dot11BssType == dot11_BSS_type_infrastructure
        && network.bNetworkConnectable.as_bool()
        && network.uNumberOfBssids > 0
        && network.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0
        && network.strProfileName[0] != 0
        && network.strProfileName.contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_association_then_radio_cycle_does_not_wait_for_old_cooldown() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        assert_eq!(
            state
                .tick(start, &mut adapter, &mut || true, &mut || false)
                .unwrap(),
            Outcome::Requested
        );
        state.wlan_events(EVENT_CHANGED | EVENT_CONNECTED);
        state.wlan_events(EVENT_CHANGED | EVENT_DISCONNECTED);
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(10),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Requested
        );
        assert_eq!(adapter.requests, 2);
    }
    #[test]
    fn scan_and_disconnect_events_do_not_bypass_failed_request_cooldown() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        adapter.fail = true;
        assert!(state
            .tick(start, &mut adapter, &mut || true, &mut || false)
            .is_err());
        state.wlan_events(EVENT_CHANGED | EVENT_DISCONNECTED);
        state.wlan_events(EVENT_CHANGED);
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(5),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        assert_eq!(adapter.requests, 1);
    }
    #[test]
    fn wlan_callback_accepts_success_only_with_valid_success_payload() {
        let mut context = EventContext {
            wake: Arc::new(AtomicBool::new(false)),
            events: AtomicU32::new(0),
        };
        let mut payload = WLAN_CONNECTION_NOTIFICATION_DATA::default();
        let mut data = L2_NOTIFICATION_DATA {
            NotificationSource: WLAN_NOTIFICATION_SOURCE_ACM,
            NotificationCode: wlan_notification_acm_connection_complete.0 as u32,
            ..Default::default()
        };
        let context_ptr = (&mut context as *mut EventContext).cast();
        unsafe {
            wlan_event(&mut data, context_ptr);
        }
        assert_eq!(context.events.swap(0, Ordering::SeqCst), EVENT_CHANGED);
        data.dwDataSize = std::mem::size_of_val(&payload) as u32;
        payload.wlanReasonCode = 1;
        data.pData = (&mut payload as *mut WLAN_CONNECTION_NOTIFICATION_DATA).cast();
        unsafe {
            wlan_event(&mut data, context_ptr);
        }
        assert_eq!(context.events.swap(0, Ordering::SeqCst), EVENT_CHANGED);
        payload.wlanReasonCode = 0;
        data.pData = (&mut payload as *mut WLAN_CONNECTION_NOTIFICATION_DATA).cast();
        unsafe {
            wlan_event(&mut data, context_ptr);
        }
        assert_eq!(
            context.events.swap(0, Ordering::SeqCst),
            EVENT_CHANGED | EVENT_CONNECTED
        );
        assert!(context.wake.load(Ordering::SeqCst));
        assert_eq!(
            event_flags(
                WLAN_NOTIFICATION_SOURCE_MSM,
                wlan_notification_acm_connection_complete.0 as u32
            ),
            0
        );
    }
    #[test]
    #[ignore = "Registers/unregisters read-only WLAN notifications; never connects"]
    fn native_notification_registration_smoke() {
        let watch = Watch::start(Arc::new(AtomicBool::new(false)))
            .expect("WLAN notification registration failed");
        watch.drain(&mut Recovery::default());
        drop(watch);
    }
    #[derive(Default)]
    struct Fake {
        detached: bool,
        candidate: bool,
        requests: usize,
        inspections: usize,
        discovery: usize,
        fail: bool,
    }
    impl Adapter for Fake {
        type Candidate = ();
        fn detached(&mut self) -> io::Result<bool> {
            self.inspections += 1;
            Ok(self.detached)
        }
        fn candidate(&mut self) -> io::Result<Option<()>> {
            self.discovery += 1;
            Ok(self.candidate.then_some(()))
        }
        fn connect(&mut self, _: (), allowed: &mut dyn FnMut() -> bool) -> io::Result<bool> {
            if !allowed() {
                return Ok(false);
            }
            self.requests += 1;
            if self.fail {
                Err(io::Error::other("WLAN unavailable"))
            } else {
                Ok(true)
            }
        }
    }
    fn available() -> Fake {
        Fake {
            detached: true,
            candidate: true,
            ..Default::default()
        }
    }
    #[test]
    fn dhcp_ready_at_twenty_five_seconds_probes_before_the_sixty_second_cooldown() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        state
            .tick(start, &mut adapter, &mut || true, &mut || false)
            .unwrap();
        for seconds in [1, 10, 20] {
            assert_eq!(
                state.probe_plan_at(start + Duration::from_secs(seconds), false),
                ProbePlan::WaitForAddress
            );
        }
        assert_eq!(
            state.probe_plan_at(start + Duration::from_secs(25), true),
            ProbePlan::Probe(Duration::from_secs(1))
        );
        // A reachable login page cancels pending association; normal login can run
        // immediately, while a second association request remains prohibited.
        state.cancel();
        assert_eq!(
            state.probe_plan_at(start + Duration::from_secs(26), true),
            ProbePlan::Probe(Duration::from_secs(4))
        );
        assert_eq!(adapter.requests, 1);
    }
    #[test]
    fn unknown_link_state_never_issues_a_connection_request() {
        struct Denied;
        impl Adapter for Denied {
            type Candidate = ();
            fn detached(&mut self) -> io::Result<bool> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
            fn candidate(&mut self) -> io::Result<Option<()>> {
                panic!("must not scan after a failed link query")
            }
            fn connect(&mut self, _: (), _: &mut dyn FnMut() -> bool) -> io::Result<bool> {
                panic!("must not connect")
            }
        }
        assert!(Recovery::default()
            .tick(Instant::now(), &mut Denied, &mut || true, &mut || false)
            .is_err());
    }
    #[test]
    #[ignore = "Reads local WLAN APIs only; never connects or changes profiles"]
    fn native_wlan_read_only_smoke() {
        let mut adapter = Native::open().expect("WLAN service must be available");
        let detached = adapter.detached().expect("adapter link query failed");
        println!("WLAN readable; safely detached: {detached}");
        if detached {
            println!(
                "Saved visible school profile: {}",
                adapter
                    .candidate()
                    .expect("network list unavailable")
                    .is_some()
            );
        }
    }
    #[test]
    fn request_cooldown_does_not_block_ready_portal_or_repeat_association() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        assert_eq!(
            state
                .tick(start, &mut adapter, &mut || true, &mut || false)
                .unwrap(),
            Outcome::Requested
        );
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(20),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Pending
        );
        // Portal readiness is handled by the agent, before the recovery gate.
        state.cancel();
        assert!(!state.waiting_at(start + Duration::from_secs(21)));
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(30),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        assert_eq!(adapter.requests, 1);
    }
    #[test]
    fn association_timeout_does_not_allow_reconnection_before_sixty_seconds() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        state
            .tick(start, &mut adapter, &mut || true, &mut || false)
            .unwrap();
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(50),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(61),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Requested
        );
        assert_eq!(adapter.requests, 2);
    }
    #[test]
    fn pause_or_disable_cancels_pending_work_without_resetting_cooldown() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        state
            .tick(start, &mut adapter, &mut || true, &mut || false)
            .unwrap();
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(3),
                    &mut adapter,
                    &mut || false,
                    &mut || false
                )
                .unwrap(),
            Outcome::Disabled
        );
        assert!(!state.waiting_at(start + Duration::from_secs(4)));
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(5),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        assert_eq!(adapter.requests, 1);
    }
    #[test]
    fn connected_or_associating_network_is_never_replaced() {
        let mut state = Recovery::default();
        let mut adapter = available();
        adapter.detached = false;
        assert_eq!(
            state
                .tick(Instant::now(), &mut adapter, &mut || true, &mut || panic!(
                    "must not probe"
                ))
                .unwrap(),
            Outcome::ConnectedOrBusy
        );
        assert_eq!(adapter.requests, 0);
        assert_eq!(adapter.discovery, 0);
    }
    #[test]
    fn working_internet_prevents_connection_and_discovery() {
        let mut state = Recovery::default();
        let mut adapter = available();
        assert_eq!(
            state
                .tick(Instant::now(), &mut adapter, &mut || true, &mut || true)
                .unwrap(),
            Outcome::InternetAvailable
        );
        assert_eq!(adapter.requests, 0);
        assert_eq!(adapter.discovery, 0);
    }
    #[test]
    fn pause_after_internet_probe_prevents_submission() {
        let mut state = Recovery::default();
        let mut adapter = available();
        let mut checks = 0;
        assert_eq!(
            state
                .tick(
                    Instant::now(),
                    &mut adapter,
                    &mut || {
                        checks += 1;
                        checks == 1
                    },
                    &mut || false
                )
                .unwrap(),
            Outcome::Cancelled
        );
        assert_eq!(adapter.requests, 0);
    }
    #[test]
    fn network_change_after_discovery_prevents_submission() {
        struct Changed(usize);
        impl Adapter for Changed {
            type Candidate = ();
            fn detached(&mut self) -> io::Result<bool> {
                self.0 += 1;
                Ok(self.0 == 1)
            }
            fn candidate(&mut self) -> io::Result<Option<()>> {
                Ok(Some(()))
            }
            fn connect(&mut self, _: (), _: &mut dyn FnMut() -> bool) -> io::Result<bool> {
                panic!("new network must be retained")
            }
        }
        let mut state = Recovery::default();
        assert_eq!(
            state
                .tick(Instant::now(), &mut Changed(0), &mut || true, &mut || false)
                .unwrap(),
            Outcome::Cancelled
        );
        assert!(!state.waiting());
    }
    #[test]
    fn pause_while_waiting_for_connection_lock_does_not_create_a_phantom_cooldown() {
        let mut adapter = available();
        let mut state = Recovery::default();
        let mut checks = 0;
        let start = Instant::now();
        assert_eq!(
            state
                .tick(
                    start,
                    &mut adapter,
                    &mut || {
                        checks += 1;
                        checks < 4
                    },
                    &mut || false
                )
                .unwrap(),
            Outcome::Cancelled
        );
        assert_eq!(adapter.requests, 0);
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(1),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Requested
        );
    }
    #[test]
    fn failed_connection_is_still_throttled() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        adapter.fail = true;
        assert!(state
            .tick(start, &mut adapter, &mut || true, &mut || false)
            .is_err());
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(16),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        assert_eq!(adapter.requests, 1);
    }
    #[test]
    fn unavailable_target_is_not_connected_and_network_events_refresh_discovery() {
        let start = Instant::now();
        let mut state = Recovery::default();
        let mut adapter = available();
        adapter.candidate = false;
        assert_eq!(
            state
                .tick(start, &mut adapter, &mut || true, &mut || false)
                .unwrap(),
            Outcome::NotVisibleOrSaved
        );
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(5),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Cooldown
        );
        adapter.candidate = true;
        state.network_changed();
        assert_eq!(
            state
                .tick(
                    start + Duration::from_secs(6),
                    &mut adapter,
                    &mut || true,
                    &mut || false
                )
                .unwrap(),
            Outcome::Requested
        );
    }
    #[test]
    fn only_visible_saved_exact_infrastructure_ssid_is_eligible() {
        let mut network = WLAN_AVAILABLE_NETWORK::default();
        network.dot11Ssid.uSSIDLength = 4;
        network.dot11Ssid.ucSSID[..4].copy_from_slice(SSID);
        network.dot11BssType = dot11_BSS_type_infrastructure;
        network.bNetworkConnectable = true.into();
        network.uNumberOfBssids = 1;
        network.dwFlags = WLAN_AVAILABLE_NETWORK_HAS_PROFILE;
        network.strProfileName[0] = b'p' as u16;
        assert!(eligible(&network));
        network.dwFlags = 0;
        assert!(!eligible(&network));
        network.dwFlags = WLAN_AVAILABLE_NETWORK_HAS_PROFILE;
        network.uNumberOfBssids = 0;
        assert!(!eligible(&network));
        network.uNumberOfBssids = 1;
        network.dot11Ssid.ucSSID[3] = b'X';
        assert!(!eligible(&network));
    }
}
