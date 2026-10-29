// 门户交互：状态查询、登录、下线、失败归类

use std::time::Duration;

use crate::config::{load_state, update_state};

pub const HOST: &str = "202.117.144.205";
pub const PORTS: [u16; 11] = [
    8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600,
];
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const MARK: &str = "当前登录账号";

pub fn host() -> String {
    if let Ok(h) = std::env::var("SNNU_PORTAL_HOST") {
        return h;
    }
    let cached = load_state().portal_host;
    if !cached.is_empty() {
        return cached;
    }
    HOST.to_string()
}

// All campus requests share the same physical route, including login and logout.
fn request_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .tcp_connector(crate::network::connect_portal)
}

#[cfg(all(test, windows))]
#[test]
#[ignore = "Requires a real authenticated campus Wi-Fi; GET only, never submits credentials"]
fn campus_read_only_status_through_physical_adapter() {
    let observed = status_at(HOST, 8609, Duration::from_secs(5));
    let classification = match &observed {
        Some(Status::Online(_)) => "online",
        Some(Status::Offline) => "offline",
        _ => "unreachable",
    };
    println!("Read-only physical-network portal classification: {classification}");
    assert!(
        matches!(observed, Some(Status::Online(_))),
        "Expected the already authenticated session; no login/logout performed"
    );
}

fn redirect_target(url: &str, timeout: Duration) -> Option<(String, u16)> {
    let resp = request_builder()
        .redirects(0)
        .build()
        .get(url)
        .set("User-Agent", UA)
        .timeout(timeout)
        .call();
    let loc = match resp {
        Ok(r) => r.header("location").map(|s| s.to_string()),
        Err(ureq::Error::Status(code, r)) if (300..400).contains(&code) => {
            r.header("location").map(|s| s.to_string())
        }
        _ => None,
    };
    // 其他网络的认证跳转不能当作学校门户，更不能往那里发送校园网密码。
    loc.filter(|l| {
        l.split_once("://")
            .and_then(|(_, r)| r.split_once('/'))
            .map(|(_, path)| path.starts_with("snnuportal/"))
            .unwrap_or(false)
    })
    .and_then(|l| parse_host_port(&l))
}

fn parse_host_port(loc: &str) -> Option<(String, u16)> {
    let rest = loc.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let mut it = authority.split(':');
    let host = it.next()?.to_string();
    let port = match it.next() {
        Some(p) => p.parse::<u16>().ok()?,
        None => 80,
    };
    if host.is_empty()
        || port == 0
        || it.next().is_some()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    Some((host, port))
}

#[derive(Debug, PartialEq, Clone)]
pub enum Status {
    Online(Session),
    Offline,
    Unreachable,
}

/// 从门户状态页解析的在线会话。
#[derive(Debug, PartialEq, Clone)]
pub struct Session {
    pub account: String,
    pub ip: String,
    /// 门户给的登录时间，形如 "2026-10-25 14:06:29"（本机时区）
    pub login_time: String,
}

fn decode(bytes: &[u8]) -> String {
    // 门户有的页面是 UTF-8，有的是 GBK。先按 UTF-8 严格解码，
    // 失败再回退 GB18030，避免把 UTF-8 中文当成 GBK 变成乱码。
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.trim_start_matches('\u{feff}').to_string();
    }
    let (txt, _, _) = encoding_rs::GB18030.decode(bytes);
    txt.to_string()
}

fn fetch(url: &str, secs: u64) -> Result<String, String> {
    let resp = request_builder()
        .build()
        .get(url)
        .set("User-Agent", UA)
        .set("Accept-Language", "zh-CN,zh;q=0.9")
        .timeout(Duration::from_secs(secs))
        .call();
    match resp {
        Ok(r) => {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut r.into_reader(), &mut buf)
                .map_err(|e| e.to_string())?;
            Ok(decode(&buf))
        }
        Err(e) => Err(format!("{}", e)),
    }
}

fn take_account(body: &str) -> Option<String> {
    let idx = body.find(MARK)?;
    let rest = &body[idx + MARK.len()..];
    let cleaned: String = {
        let mut out = String::new();
        let mut in_tag = false;
        for ch in rest.chars() {
            match ch {
                '<' => {
                    in_tag = true;
                    out.push(' ');
                }
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out.replace("&nbsp;", " ").replace(['：', ':'], " ")
    };
    let value = cleaned.split_whitespace().next()?;
    if value.len() < 4
        || value.len() > 128
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-".contains(c))
    {
        return None;
    }
    Some(value.to_string())
}

/// 取状态页里某个标签后面的值：跳过 HTML 标签，取到行尾为止
/// （"当前IP：10.1.2.3"、"登录时间：2026-10-25 14:06:29" 都靠它）
fn take_value(body: &str, key: &str) -> Option<String> {
    let idx = body.find(key)?;
    let rest = &body[idx + key.len()..];
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in rest.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                let tag = tag.trim().to_ascii_lowercase();
                if (tag.starts_with("br") || matches!(tag.as_str(), "/td" | "/tr" | "/p" | "/div"))
                    && !out.trim_matches(['：', ':', ' ', '\u{3000}']).is_empty()
                {
                    break;
                }
                in_tag = false;
                out.push(' ');
            }
            '\n' | '\r' => break,
            _ if in_tag => tag.push(ch),
            _ if !in_tag && out.chars().count() < 64 => out.push(ch),
            _ => {}
        }
    }
    // 只掐首尾的键值分隔符（"：" / ":"）和空白，值内部的冒号要留着（登录时间 "14:06:29"）
    let out = out.replace("&nbsp;", " ");
    let v = out.trim_matches(['：', ':', ' ', '\u{3000}']).to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn take_ip(body: &str) -> String {
    take_value(body, "当前IP")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn take_login_time(body: &str) -> String {
    take_value(body, "登录时间").unwrap_or_default()
}

/// 一次 fetch 到的状态页 → 在线会话；取不到账号就认为没在线
fn session_of(body: &str) -> Option<Session> {
    let account = take_account(body)?;
    Some(Session {
        ip: take_ip(body),
        login_time: take_login_time(body),
        account,
    })
}

/// 返回 (状态, 端口)
pub fn status(timeout: Duration) -> (Status, u16) {
    use std::time::Instant;
    let timeout = timeout.max(Duration::from_millis(500));
    let deadline = Instant::now() + timeout;
    let cached = load_state();
    let h = host();
    // 正常在线时仍只请求已知门户；缓存失效才并行查找，整个过程共用一个截止时间。
    if cached.port > 0 {
        if let Some(status) = status_at(&h, cached.port, timeout.min(Duration::from_millis(500))) {
            return (status, cached.port);
        }
    }
    let mut ports = PORTS.to_vec();
    if cached.port > 0 {
        ports.retain(|p| *p != cached.port);
        ports.insert(0, cached.port);
    }
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return (Status::Unreachable, 0);
    };
    let targets: Vec<_> = ports.iter().map(|p| (h.clone(), *p)).collect();
    if let Some((status, new_host, port)) = probe_candidates(targets, remaining, true) {
        if h != new_host || cached.port != port {
            cache_portal(&new_host, port);
        }
        (status, port)
    } else {
        (Status::Unreachable, 0)
    }
}

fn probe_candidates(
    targets: Vec<(String, u16)>,
    timeout: Duration,
    discover: bool,
) -> Option<(Status, String, u16)> {
    #[cfg(target_os = "macos")]
    let discover = discover
        && !(std::env::var_os("SNNU_DATA_DIR").is_some()
            && std::env::var("SNNU_PORTAL_HOST").as_deref() == Ok("127.0.0.1"));
    use std::{sync::mpsc, time::Instant};
    let deadline = Instant::now() + timeout;
    let (tx, rx) = mpsc::channel();
    for (rank, (h, p)) in targets.iter().cloned().enumerate() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = deadline
                .checked_duration_since(Instant::now())
                .and_then(|left| status_at(&h, p, left));
            let _ = tx.send((rank, h, p, result));
        });
    }
    if discover {
        // 发现新主机与旧地址扫描同时进行，旧 IP 超时不能饿死地址漂移恢复。
        let cached = load_state();
        let urls = [
            format!(
                "http://{}:{}/snnuportal/userstatus.jsp",
                host(),
                if cached.port > 0 {
                    cached.port
                } else {
                    PORTS[0]
                }
            ),
            "http://connect.rom.miui.com/generate_204".into(),
            "http://www.msftconnecttest.com/connecttest.txt".into(),
        ];
        for (i, url) in urls.into_iter().enumerate() {
            let tx = tx.clone();
            let rank = targets.len() + i;
            std::thread::spawn(move || {
                let result = (|| {
                    let left = deadline.checked_duration_since(Instant::now())?;
                    let (h, p) = redirect_target(&url, left)?;
                    let left = deadline.checked_duration_since(Instant::now())?;
                    status_at(&h, p, left).map(|s| (h, p, s))
                })();
                let (h, p, status) = match result {
                    Some((h, p, s)) => (h, p, Some(s)),
                    None => (String::new(), 0, None),
                };
                let _ = tx.send((rank, h, p, status));
            });
        }
    }
    drop(tx);
    select_probe(rx, deadline)
}

fn select_probe(
    rx: std::sync::mpsc::Receiver<(usize, String, u16, Option<Status>)>,
    deadline: std::time::Instant,
) -> Option<(Status, String, u16)> {
    use std::time::Instant;
    let mut offline: Option<(usize, Status, String, u16)> = None;
    let mut offline_deadline = None;
    loop {
        let end = offline_deadline.unwrap_or(deadline).min(deadline);
        let Some(left) = end.checked_duration_since(Instant::now()) else {
            break;
        };
        match rx.recv_timeout(left) {
            Ok((_, h, p, Some(s @ Status::Online(_)))) => return Some((s, h, p)),
            Ok((rank, h, p, Some(Status::Offline))) => {
                if offline.as_ref().is_none_or(|(old, ..)| rank < *old) {
                    offline = Some((rank, Status::Offline, h, p));
                }
                // 给已经在返回途中的在线会话一个短暂优先窗口，避免先收到登录表单就顶号。
                offline_deadline.get_or_insert(Instant::now() + Duration::from_millis(100));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    offline.map(|(_, s, h, p)| (s, h, p))
}

/// 操作完成后的快速确认只查已知门户，不在界面反馈前重新扫描全部端口。
pub fn status_quick(timeout: Duration) -> Option<Status> {
    let port = load_state().port;
    if port == 0 {
        return None;
    }
    status_at(&host(), port, timeout)
}

fn status_at(h: &str, port: u16, timeout: Duration) -> Option<Status> {
    let body = fetch_status_page(h, port, timeout)?;
    if let Some(session) = session_of(&body) {
        return Some(Status::Online(session));
    }
    let low = body.to_ascii_lowercase();
    let account_field = [
        "name=\"account\"",
        "name='account'",
        "id=\"account\"",
        "id='account'",
    ];
    let password_field = [
        "name=\"password\"",
        "name='password'",
        "id=\"password\"",
        "id='password'",
    ];
    if account_field.iter().any(|s| low.contains(s))
        && password_field.iter().any(|s| low.contains(s))
    {
        Some(Status::Offline)
    } else {
        None
    }
}

fn fetch_status_page(h: &str, port: u16, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    let agent = request_builder().redirects(0).build();
    let base = format!("http://{}:{}", h, port);
    let mut url = format!("{}/snnuportal/userstatus.jsp", base);
    for _ in 0..4 {
        let left = deadline.checked_duration_since(std::time::Instant::now())?;
        let response = agent
            .get(&url)
            .set("User-Agent", UA)
            .timeout(left)
            .call()
            .ok()?;
        if (300..400).contains(&response.status()) {
            let location = response.header("location")?;
            url = if location.starts_with("http://") {
                location.to_string()
            } else if location.starts_with('/') && !location.starts_with("//") {
                format!("{}{}", base, location)
            } else if !location.contains("://") && !location.starts_with("//") {
                format!("{}/snnuportal/{}", base, location)
            } else {
                return None;
            };
            let (redirect_host, redirect_port) = parse_host_port(&url)?;
            // 换主机/端口交给发现流程验证并一起缓存，不能跟随后仍返回旧地址。
            if !redirect_host.eq_ignore_ascii_case(h) || redirect_port != port {
                return None;
            }
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
        return Some(decode(&bytes));
    }
    None
}

fn cache_portal(h: &str, port: u16) {
    update_state(|s| {
        s.portal_host = h.to_string();
        s.port = port;
    });
    crate::config::logf(&format!("已验证门户地址 {}:{}", h, port));
}

fn strip_block(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}", tag);
    let mut out = String::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find(&open) {
        let start = pos + rel;
        out.push_str(&html[pos..start]);
        match lower[start..].find(&close) {
            Some(end_rel) => {
                let end = start + end_rel;
                pos = lower[end..]
                    .find('>')
                    .map(|gt| end + gt + 1)
                    .unwrap_or(html.len());
            }
            None => pos = html.len(),
        }
    }
    out.push_str(&html[pos..]);
    out
}

/// 把 HTML 转成纯文本：去掉脚本、样式、注释和标签，规整空白
pub fn clean_text(html: &str) -> String {
    let mut text = strip_block(html, "script");
    text = strip_block(&text, "style");
    text = strip_block(&text, "noscript");

    let mut out = String::new();
    let mut in_tag = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let rest: String = chars.clone().take(3).collect();
            if rest.starts_with("!--") {
                for _ in 0..3 {
                    chars.next();
                }
                let mut tail = String::new();
                for c in chars.by_ref() {
                    tail.push(c);
                    if tail.ends_with("-->") {
                        break;
                    }
                }
                continue;
            }
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(ch);
        }
    }

    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 去掉门户登录表单的字段噪声：号/密码/校园网/代拨/验证码等。
fn cut_form_noise(txt: &str) -> String {
    let norm_chars: Vec<(char, usize)> = txt
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(i, c)| (if c == '：' { ':' } else { c }, i))
        .collect();
    let norm: String = norm_chars.iter().map(|(c, _)| *c).collect();
    let patterns = [
        "账号:密码:校园网:校园网代拨:",
        "账号:密码:",
        "代拨:联通",
        "验证码:记住密码",
        "号:密码:",
    ];
    if let Some(pos) = patterns.iter().filter_map(|p| norm.find(p)).min() {
        let char_pos = norm[..pos].chars().count();
        if let Some(&(_, idx)) = norm_chars.get(char_pos) {
            return txt[..idx].trim().to_string();
        }
    }
    txt.trim().to_string()
}

/// 提取门户明确给出的错误句；没有明确错误时返回空。
fn extract_message(txt: &str) -> String {
    let mut cut = cut_form_noise(txt);
    for prefix in ["陕西师范大学网络Portal", "陕西师范大学网络Portal "] {
        if let Some(rest) = cut.strip_prefix(prefix) {
            cut = rest.trim().to_string();
            break;
        }
    }
    if let Some(pos) = cut.find("登录失败") {
        return cut[pos..].trim().to_string();
    }
    cut.trim().to_string()
}

/// (是否验证码问题, 提示文本)
pub fn classify(body: &str) -> (bool, String) {
    let txt = extract_message(&clean_text(body));
    let low = txt.to_lowercase();
    if txt.is_empty() {
        return (false, "账号或密码错误，请检查后重试".into());
    }

    // 凭据错误优先于验证码：门户经常在密码错误的页面上顺便显示验证码，
    // 如果先判验证码，就会把“密码错了”误报成“验证码错了”。
    let credential_errors = [
        "密码错误",
        "密码输入错误",
        "密码不正确",
        "密码有误",
        "密码无效",
        "密码不匹配",
        "密码验证失败",
        "账号密码有误",
        "账号或密码",
        "用户名或密码",
        "账号不存在",
        "用户不存在",
        "未开通",
        "已停机",
        "停机",
        "欠费",
        "余额不足",
        "绑定失败",
    ];
    if let Some(seg) = txt
        .split(['。', '!', '！', ';', '；'])
        .map(str::trim)
        .find(|seg| credential_errors.iter().any(|w| seg.contains(w)))
    {
        return (false, take_chars(seg, 120));
    }

    let captcha_errors = [
        "验证码错误",
        "验证码不正确",
        "验证码无效",
        "验证码已过期",
        "请输入验证码",
        "验证码不能为空",
    ];
    // 门户明确写了什么，就显示什么；只统一清理 HTML / 表单噪声。
    if let Some(seg) = txt
        .split(['。', '!', '！', ';', '；'])
        .map(str::trim)
        .find(|seg| captcha_errors.iter().any(|w| seg.contains(w)))
    {
        return (true, take_chars(seg, 120));
    }
    if low.contains("captcha") {
        return (true, "请输入验证码".into());
    }

    let low_body = body.to_ascii_lowercase();
    let compact: String = txt
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '：' { ':' } else { c })
        .collect();
    if page_needs_captcha(body)
        || low_body.contains("<form")
        || low_body.contains("name=\"account\"")
        || low_body.contains("name='account'")
        || low_body.contains("id=\"account\"")
        || low_body.contains("id='account'")
        || low_body.contains("name=\"password\"")
        || low_body.contains("name='password'")
        || low_body.contains("id=\"password\"")
        || low_body.contains("id='password'")
        || compact.contains("号:密码:")
        || compact.contains("代拨:联通")
    {
        // 只是登录表单回显，没有任何错误语句时，再给通用提示。
        return (false, "账号或密码错误，请检查后重试".into());
    }

    let words = ["失败", "错误", "无效", "已过期", "认证", "在线数"];
    for seg in txt.split(['。', '!', '！', ';', '；']) {
        let seg = seg.trim();
        if !seg.is_empty() && words.iter().any(|w| seg.contains(w)) {
            return (false, take_chars(seg, 120));
        }
    }
    let msg = take_chars(&txt, 120);
    if msg.is_empty() {
        (false, "认证没有通过".into())
    } else {
        crate::config::logf(&format!("错误页未识别: {}", msg));
        (false, msg)
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 清洗历史状态里可能残留的 HTML / CSS / 乱码；无法识别时给可操作提示
pub fn display_error(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let msg = extract_message(&clean_text(raw));
    if msg.is_empty() || msg.contains('<') || msg.contains('>') {
        return "认证未通过，请重试一次".into();
    }
    let keywords = [
        "密码",
        "账号",
        "用户",
        "运营商",
        "余额",
        "欠费",
        "停机",
        "绑定",
        "在线数",
        "失败",
        "错误",
        "无效",
        "未开通",
        "已过期",
        "认证",
        "验证码",
    ];
    if !keywords.iter().any(|w| msg.contains(w)) {
        return "认证未通过，请重试一次".into();
    }
    take_chars(&msg, 120)
}

/// 把门户的账号密码错误收敛成一句可操作提示；其他明确错误保留原文。
pub fn friendly_login_error(msg: &str) -> String {
    let compact: String = msg
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '：' { ':' } else { c })
        .collect();
    let credential = [
        "账号密码有误",
        "密码有误",
        "密码错误",
        "密码不正确",
        "密码无效",
        "账号或密码",
        "用户名或密码",
    ];
    if credential.iter().any(|w| compact.contains(w)) {
        return "您的账号密码有误".into();
    }
    msg.trim().to_string()
}

pub fn connection_lock() -> Result<crate::process_lock::ProcessLock, String> {
    crate::process_lock::ProcessLock::acquire(
        &crate::config::app_dir().join("connection"),
        Duration::from_secs(90),
    )
    .map_err(|e| format!("等待网络操作失败：{}", e))
}

pub fn login(account: &str, password: &str, operator: &str, port: u16) -> Result<(), String> {
    let _lock = connection_lock()?;
    login_unlocked(account, password, operator, port)
}

/// 自动任务在获得连接锁后再次检查暂停/熔断，避免排队期间用户已手动断开。
pub fn login_automatic(
    account: &str,
    password: &str,
    operator: &str,
    port: u16,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let _lock = connection_lock()?;
    let state = load_state();
    if cancelled() || !automatic_login_allowed(&state, unix_now()) {
        return Err("自动登录已暂停".into());
    }
    // 其他任务可能已完成登录，不再重复提交密码。
    let (current, current_port) = status(Duration::from_secs(4));
    if cancelled() || !automatic_login_allowed(&load_state(), unix_now()) {
        return Err("自动登录已暂停".into());
    }
    match current {
        Status::Online(_) => Ok(()),
        Status::Unreachable => Err("连不上认证服务器".into()),
        Status::Offline => {
            let result = login_unlocked(
                account,
                password,
                operator,
                if current_port == 0 {
                    port
                } else {
                    current_port
                },
            );
            if let Err(msg) = &result {
                // 熔断也必须在连接锁内落盘，阻止其他排队任务再次提交错误密码。
                if next_delay(fail_kind(msg), 1).is_none() {
                    update_state(|s| s.paused_until = unix_now() + 1800.0);
                }
            }
            result
        }
    }
}

pub fn automatic_login_allowed(state: &crate::config::State, now: f64) -> bool {
    !state.reconnect_paused && now >= state.paused_until
}

fn login_unlocked(account: &str, password: &str, operator: &str, port: u16) -> Result<(), String> {
    let base = format!("http://{}:{}/snnuportal", host(), port);
    // 每次提交创建独立认证会话，最多尝试两次。
    let attempts = 2;
    let mut final_msg = String::new();

    for attempt in 0..attempts {
        let (session_cookie, _body) = open_login_page(port, "");
        std::thread::sleep(Duration::from_millis(120));
        let mut req = request_builder()
            .build()
            .post(&format!("{}/login", base))
            .set("User-Agent", UA)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .set("Referer", &format!("{}/login.jsp", base))
            .timeout(Duration::from_secs(10));
        if !session_cookie.is_empty() {
            req = req.set("Cookie", &session_cookie);
        }
        let form = vec![
            ("sourceurl", "null"),
            ("account", account),
            ("password", password),
            ("yys", operator),
            ("issave", ""),
        ];
        let resp = req.send_form(&form);

        let body = match resp {
            Ok(r) => {
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut r.into_reader(), &mut buf);
                decode(&buf)
            }
            Err(e) => return Err(format!("连不上认证服务器（{}）", e)),
        };

        if body.contains(MARK) {
            mark_login_ok();
            return Ok(());
        }

        let (captcha, msg) = classify(&body);
        if captcha {
            final_msg = msg;
            if attempt + 1 < attempts {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            // 门户页面写了什么就报什么，不再替它改写文案。
            let out = friendly_login_error(&final_msg);
            update_state(|s| {
                s.last_error = out.clone();
            });
            crate::config::logf(&format!("登录失败: {}", scrub(&out, account)));
            return Err(out);
        }

        // 响应不是验证码：用在线状态兜底确认是否其实已登录。
        // 只复查当前提交使用的端口，避免在持有连接锁时重新全端口扫描。
        if let Some(Status::Online(s)) = status_at(&host(), port, Duration::from_secs(2)) {
            let (online_user, _) = split_account(&s.account);
            if online_user == account {
                mark_login_ok();
                return Ok(());
            }
        }
        let out = friendly_login_error(&msg);
        update_state(|s| {
            s.last_error = out.clone();
        });
        crate::config::logf(&format!("登录失败: {}", scrub(&out, account)));
        return Err(out);
    }

    Err(if final_msg.is_empty() {
        "认证没有通过".to_string()
    } else {
        final_msg
    })
}

fn mark_login_ok() {
    update_state(|s| {
        s.self_login_ok = true;
        s.paused_until = 0.0;
        s.last_error.clear();
        s.no_internet = false;
        s.net_bad = 0;
        s.last_ok_ts = unix_now();
    });
}

/// 重新请求登录页。门户的验证码要求会随刷新消失，
/// 所以碰到验证码时先刷新页面，再自动重试登录。
fn cookie_from(resp: &ureq::Response) -> Option<String> {
    // 一次响应可能有多个 Set-Cookie（JSESSIONID 之外还有负载均衡 Cookie），
    // 全部带回浏览器式会话，验证码才能和登录对上。
    let pairs: Vec<&str> = resp
        .all("set-cookie")
        .iter()
        .filter_map(|v| v.split(';').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join("; "))
    }
}

/// 打开发登录页；cookie 为空时相当于新开一个无历史的验证页面。
fn open_login_page(port: u16, cookie: &str) -> (String, String) {
    let url = format!(
        "http://{}:{}/snnuportal/login.jsp?_={:.3}",
        host(),
        port,
        unix_now()
    );
    let mut req = request_builder()
        .build()
        .get(&url)
        .set("User-Agent", UA)
        .set("Accept", "text/html,application/xhtml+xml")
        .set("Accept-Language", "zh-CN,zh;q=0.9")
        .set("Cache-Control", "no-cache")
        .set("Pragma", "no-cache")
        .timeout(Duration::from_secs(5));
    if !cookie.is_empty() {
        req = req.set("Cookie", cookie);
    }
    if let Ok(r) = req.call() {
        let next = cookie_from(&r).unwrap_or_else(|| cookie.to_string());
        let body = r.into_string().unwrap_or_default();
        return (next, body);
    }
    (cookie.to_string(), String::new())
}

/// 登录页里是否仍在要求验证码
fn page_needs_captcha(html: &str) -> bool {
    let low = html.to_ascii_lowercase();
    low.contains("name=\"checkcode\"")
        || low.contains("name='checkcode'")
        || low.contains("name=checkcode")
        || low.contains("id=\"checkcode\"")
        || low.contains("id='checkcode'")
}

pub fn logout(port: u16) -> bool {
    let Ok(_lock) = connection_lock() else {
        return false;
    };
    let h = host();
    if fetch(&format!("http://{}:{}/snnuportal/logoff", h, port), 6).is_err() {
        return false;
    }
    for _ in 0..3 {
        if matches!(
            status_at(&h, port, Duration::from_secs(3)),
            Some(Status::Offline)
        ) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

/// 保活同时验证联网，避免把认证重定向误记为成功。
pub fn internet_ping() -> bool {
    internet_ok()
}

pub fn internet_ok() -> bool {
    #[cfg(target_os = "macos")]
    if std::env::var_os("SNNU_DATA_DIR").is_some()
        && std::env::var("SNNU_PORTAL_HOST").as_deref() == Ok("127.0.0.1")
    {
        return internet_probe("http://127.0.0.1:8601/generate_204", 204, "");
    }
    let probes = [
        ("http://connect.rom.miui.com/generate_204", 204, ""),
        (
            "http://www.msftconnecttest.com/connecttest.txt",
            200,
            "Microsoft Connect Test",
        ),
    ];
    probes
        .iter()
        .any(|(url, code, body)| internet_probe(url, *code, body))
}

fn internet_probe(url: &str, expected_code: u16, expected_body: &str) -> bool {
    use std::io::Read;
    // Keep public DNS/proxy routing together: a TUN DNS may return synthetic IPs
    // which cannot be reached by a socket bound to a physical adapter.
    let Ok(response) = ureq::AgentBuilder::new()
        .redirects(0)
        .build()
        .get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(3))
        .call()
    else {
        return false;
    };
    if response.status() != expected_code {
        return false;
    }
    if expected_code == 204 {
        return true;
    }
    let mut body = String::new();
    response
        .into_reader()
        .take(256)
        .read_to_string(&mut body)
        .is_ok()
        && body.trim() == expected_body
}

pub fn unix_now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn split_account(acct: &str) -> (String, String) {
    match acct.find('@') {
        Some(i) => (
            acct[..i].trim().to_string(),
            acct[i + 1..].trim().to_string(),
        ),
        None => (acct.trim().to_string(), String::new()),
    }
}

pub fn mask(acct: &str) -> String {
    let chars: Vec<char> = acct.trim().chars().collect();
    if chars.len() <= 4 {
        return acct.to_string();
    }
    format!(
        "{}***{}",
        chars[..3].iter().collect::<String>(),
        chars[chars.len() - 2..].iter().collect::<String>()
    )
}

/// 日志脱敏：门户错误页有可能回显账号，落盘前先把学号掩掉
pub fn scrub(text: &str, account: &str) -> String {
    let account = account.trim();
    if account.is_empty() {
        return text.to_string();
    }
    let masked = mask(account);
    if masked == account {
        return text.to_string();
    }
    text.replace(account, &masked)
}

/// "2026-10-25 14:06:29" → Unix 秒（门户给的是本机时区，直接当日历时间算）
pub fn parse_portal_time(s: &str) -> Option<f64> {
    let nums: Vec<i64> = s
        .trim()
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<i64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if nums.len() < 6 {
        return None;
    }
    let (y, mo, d, h, mi, se) = (nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) as f64 * 86400.0 + (h * 3600 + mi * 60 + se) as f64)
}

/// 1970-01-01 起算的天数（Howard Hinnant 的 days_from_civil）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 已在线多少分钟；时间解析不了或门户时钟超前就返回 None
/// （门户给的是本机时区的日历时间，所以两边都按本地日历时间比，
/// 不引入 Unix 时间，避免 UTC+8 把短会话算成负数）
pub fn online_minutes(login_time: &str) -> Option<u64> {
    let t = parse_portal_time(login_time)?;
    let now = local_naive_now();
    if now < t {
        return None;
    }
    Some(((now - t) / 60.0) as u64)
}

/// 本机当前时间的"日历秒"：和 parse_portal_time 同一把尺子
fn local_naive_now() -> f64 {
    #[cfg(windows)]
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    #[cfg(target_os = "macos")]
    let t = crate::macos::local_time();
    days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64) as f64 * 86400.0
        + (t.wHour as i64 * 3600 + t.wMinute as i64 * 60 + t.wSecond as i64) as f64
}

/// 分钟数 → "42 分钟" / "1 小时 23 分" / "2 天 3 小时"
pub fn fmt_duration(min: u64) -> String {
    if min < 60 {
        format!("{} 分钟", min)
    } else if min < 60 * 24 {
        format!("{} 小时 {} 分", min / 60, min % 60)
    } else {
        format!("{} 天 {} 小时", min / (60 * 24), (min / 60) % 24)
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum FailKind {
    /// 账号或密码错误：重试没有意义，应熔断等用户改配置
    Credential,
    /// 需要验证码：重试只会加重风控
    Captcha,
    /// 余额、套餐、锁定等账号状态，需要处理账号而非修改密码。
    AccountState,
    /// 门户不可达 / 本机没网：固定间隔重探
    Unreachable,
    Other,
}

pub fn fail_kind(msg: &str) -> FailKind {
    if msg.contains("验证码") {
        return FailKind::Captcha;
    }
    if [
        "欠费",
        "余额不足",
        "流量用完",
        "流量已用尽",
        "账号锁定",
        "账号被锁",
        "帐号被锁",
        "已锁定",
        "已被锁",
        "已停机",
        "已停用",
        "已过期",
        "未开通",
        "账号禁用",
        "账号已禁用",
        "用户已在线",
        "在线数",
    ]
    .iter()
    .any(|s| msg.contains(s))
    {
        return FailKind::AccountState;
    }
    if msg.contains("密码")
        || [
            "账号不存在",
            "帐号不存在",
            "用户不存在",
            "用户名不存在",
            "无效用户",
            "账号无效",
            "帐号无效",
        ]
        .iter()
        .any(|s| msg.contains(s))
    {
        return FailKind::Credential;
    }
    if msg.contains("连不上") || msg.contains("超时") || msg.contains("未认证") {
        return FailKind::Unreachable;
    }
    FailKind::Other
}

/// 按错误类型给下一次重试间隔；None 表示熔断（不再自动重试）
pub fn next_delay(kind: FailKind, attempt: i32) -> Option<Duration> {
    match kind {
        FailKind::Credential | FailKind::Captcha | FailKind::AccountState => None,
        FailKind::Unreachable => Some(Duration::from_secs(30)),
        FailKind::Other => {
            let table = [5u64, 10, 20, 60];
            let i = (attempt.max(0) as usize).min(table.len() - 1);
            Some(Duration::from_secs(table[i]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有 HTTP 回归用例只连接回环地址，不访问学校或真实外网。
    fn serve(response: String) -> (String, std::thread::JoinHandle<()>) {
        serve_many(vec![response])
    }

    fn serve_many(responses: Vec<String>) -> (String, std::thread::JoinHandle<()>) {
        serve_sequence(responses.into_iter().map(|r| (Duration::ZERO, r)).collect())
    }

    fn serve_sequence(responses: Vec<(Duration, String)>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            for (delay, response) in responses {
                loop {
                    if let Ok((mut stream, _)) = listener.accept() {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream.set_nonblocking(false).unwrap();
                        let mut request = Vec::new();
                        let mut buf = [0u8; 1024];
                        while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                            let n = stream.read(&mut buf).unwrap();
                            if n == 0 {
                                break;
                            }
                            request.extend_from_slice(&buf[..n]);
                        }
                        std::thread::sleep(delay);
                        let _ = stream.write_all(response.as_bytes());
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "mock server did not receive a request"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
        (url, worker)
    }

    fn response(code: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn discovery_preserves_redirect_without_following_it() {
        let (url, worker) = serve("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:12345/snnuportal/login.jsp\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        assert_eq!(
            redirect_target(&url, Duration::from_secs(2)),
            Some(("127.0.0.1".into(), 12345))
        );
        worker.join().unwrap();
        let (url, worker) = serve("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:12345/hotel/login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        assert_eq!(redirect_target(&url, Duration::from_secs(2)), None);
        worker.join().unwrap();
    }

    #[test]
    fn internet_check_rejects_captive_portal_and_redirects() {
        for reply in [
            response("200 OK", "<html>please login</html>"),
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:12345/snnuportal/login.jsp\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        ] {
            let (url, worker) = serve(reply);
            assert!(!internet_probe(&url, 204, ""));
            worker.join().unwrap();
        }
        let (url, worker) = serve(response("200 OK", "<html>please login</html>"));
        assert!(!internet_probe(&url, 200, "Microsoft Connect Test"));
        worker.join().unwrap();
        let (url, worker) = serve(response("204 No Content", ""));
        assert!(internet_probe(&url, 204, ""));
        worker.join().unwrap();
        let (url, worker) = serve(response("200 OK", "Microsoft Connect Test"));
        assert!(internet_probe(&url, 200, "Microsoft Connect Test"));
        worker.join().unwrap();
    }

    #[test]
    fn discovered_endpoint_must_serve_a_real_portal_page() {
        for (body, expected) in [
            ("<html>hotel wifi</html>", None),
            (
                "<form><input name=\"account\"><input name=\"password\"></form>",
                Some(Status::Offline),
            ),
        ] {
            let (url, worker) = serve(response("200 OK", body));
            let (host, port) = parse_host_port(&url).unwrap();
            assert_eq!(status_at(&host, port, Duration::from_secs(2)), expected);
            worker.join().unwrap();
        }
    }

    #[test]
    fn status_redirect_to_login_page_keeps_working() {
        let (url, worker) = serve_many(vec![
            "HTTP/1.1 302 Found\r\nLocation: /snnuportal/login.jsp\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            response("200 OK", "<form><input name=\"account\"><input name=\"password\"></form>"),
        ]);
        let (h, p) = parse_host_port(&url).unwrap();
        assert_eq!(
            status_at(&h, p, Duration::from_secs(2)),
            Some(Status::Offline)
        );
        worker.join().unwrap();
    }

    #[test]
    fn moved_status_endpoint_is_not_reported_with_old_address() {
        let (url, worker) = serve("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/snnuportal/login.jsp\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        let (h, p) = parse_host_port(&url).unwrap();
        assert_eq!(status_at(&h, p, Duration::from_secs(2)), None);
        worker.join().unwrap();
    }

    #[test]
    fn automatic_login_is_blocked_while_editing_or_disconnected() {
        let mut s = crate::config::State::default();
        assert!(automatic_login_allowed(&s, 100.0));
        s.reconnect_paused = true;
        assert!(!automatic_login_allowed(&s, 100.0));
        s.reconnect_paused = false;
        s.paused_until = 200.0;
        assert!(!automatic_login_allowed(&s, 100.0));
        assert!(automatic_login_allowed(&s, 200.0));
    }

    #[test]
    fn single_line_status_html_keeps_account_and_fields_separate() {
        let html="<table><tr><td>当前登录账号：</td><td><span>20241234@mobile</span></td></tr><tr><td>登录时间：</td><td>2026-10-25 14:06:29</td></tr><tr><td>当前IP：</td><td>10.1.2.3</td></tr></table>";
        assert_eq!(take_account(html).as_deref(), Some("20241234@mobile"));
        assert_eq!(take_login_time(html), "2026-10-25 14:06:29");
        assert_eq!(take_ip(html), "10.1.2.3");
        assert!(session_of("当前登录账号：<span>未登录</span>").is_none());
    }

    #[test]
    fn unavailable_ports_share_one_timeout_budget() {
        let (url, worker) = serve_sequence(vec![(
            Duration::from_millis(700),
            response("200 OK", "nothing"),
        )]);
        let target = parse_host_port(&url).unwrap();
        let started = std::time::Instant::now();
        assert!(probe_candidates(vec![target], Duration::from_millis(150), false).is_none());
        assert!(started.elapsed() < Duration::from_millis(600));
        worker.join().unwrap();
    }

    #[test]
    fn an_online_session_wins_over_a_fast_login_form() {
        // Queue replies in the exact order under test. A sleeping HTTP server
        // cannot guarantee it runs inside the 100 ms priority window on macOS.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send((0, "offline".into(), 8601, Some(Status::Offline)))
            .unwrap();
        let online = session_of("当前登录账号：<span>20241234@mobile</span>").unwrap();
        tx.send((1, "online".into(), 8602, Some(Status::Online(online))))
            .unwrap();
        drop(tx);
        let result = select_probe(rx, std::time::Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(matches!(result.0, Status::Online(_)));
        assert_eq!((result.1.as_str(), result.2), ("online", 8602));
    }

    #[test]
    fn completed_login_forms_keep_candidate_preference() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send((2, "fallback".into(), 8603, Some(Status::Offline)))
            .unwrap();
        tx.send((0, "preferred".into(), 8601, Some(Status::Offline)))
            .unwrap();
        drop(tx);
        let result = select_probe(rx, std::time::Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(
            (result.0, result.1.as_str(), result.2),
            (Status::Offline, "preferred", 8601)
        );
    }

    #[test]
    fn parallel_probe_reads_a_real_online_http_response() {
        let (url, worker) = serve(response(
            "200 OK",
            "当前登录账号：<span>20241234@mobile</span>",
        ));
        let result = probe_candidates(
            vec![parse_host_port(&url).unwrap()],
            Duration::from_secs(2),
            false,
        )
        .unwrap();
        assert!(matches!(result.0, Status::Online(_)));
        worker.join().unwrap();
    }

    #[test]
    fn balance_and_account_restrictions_are_not_password_errors() {
        assert_eq!(fail_kind("您的账号已欠费"), FailKind::AccountState);
        assert_eq!(fail_kind("用户已在线，请先下线"), FailKind::AccountState);
        assert_eq!(next_delay(FailKind::AccountState, 1), None);
        assert_eq!(fail_kind("您的账号密码有误"), FailKind::Credential);
    }

    #[test]
    fn queued_login_child() {
        let Some(ready) = std::env::var_os("SNNU_TEST_LOGIN_READY") else {
            return;
        };
        std::fs::write(ready, "ready").unwrap();
        let result = login_automatic("fixture", "unused", "", 1, || false);
        assert_eq!(result.unwrap_err(), "自动登录已暂停");
    }

    #[cfg(windows)]
    #[test]
    fn queued_login_rechecks_pause_after_acquiring_connection_lock() {
        use std::process::{Command, Stdio};
        let dir =
            std::env::temp_dir().join(format!("snnu-queued-login-test-{}", std::process::id()));
        let app = dir.join(crate::config::APP_NAME);
        std::fs::create_dir_all(&app).unwrap();
        let ready = app.join("ready");
        let state_path = app.join("state.json");
        std::fs::write(
            &state_path,
            serde_json::to_string(&crate::config::State::default()).unwrap(),
        )
        .unwrap();
        let lock = crate::process_lock::ProcessLock::acquire(
            &app.join("connection"),
            Duration::from_secs(2),
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "portal::tests::queued_login_child"])
            .env("APPDATA", &dir)
            .env("SNNU_PORTAL_HOST", "127.0.0.1")
            .env("SNNU_TEST_LOGIN_READY", &ready)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // 模拟用户在自动任务排队时点了“断开”，连接锁释放后仍不得登录。
        let paused = crate::config::State {
            reconnect_paused: true,
            ..Default::default()
        };
        std::fs::write(&state_path, serde_json::to_string(&paused).unwrap()).unwrap();
        drop(lock);
        assert!(child.wait().unwrap().success());
        std::fs::remove_file(state_path).unwrap();
        std::fs::remove_file(ready).unwrap();
        std::fs::remove_dir(app).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn 账号拆分() {
        assert_eq!(
            split_account("20241234@mobile"),
            ("20241234".to_string(), "mobile".to_string())
        );
        assert_eq!(
            split_account("20241234"),
            ("20241234".to_string(), String::new())
        );
        assert_eq!(
            split_account(" 20241234@unicom "),
            ("20241234".to_string(), "unicom".to_string())
        );
    }

    #[test]
    fn 失败归类() {
        assert_eq!(fail_kind("认证失败：密码错误"), FailKind::Credential);
        assert_eq!(fail_kind("账号不存在"), FailKind::Credential);
        assert_eq!(fail_kind("请输入验证码"), FailKind::Captcha);
        assert_eq!(fail_kind("连不上认证服务器"), FailKind::Unreachable);
        assert_eq!(fail_kind("别的什么"), FailKind::Other);
    }

    #[test]
    fn 退避策略() {
        // 密码错和验证码不该重试
        assert_eq!(next_delay(FailKind::Credential, 1), None);
        assert_eq!(next_delay(FailKind::Captcha, 1), None);
        // 门户不可达固定 30 秒
        assert_eq!(
            next_delay(FailKind::Unreachable, 1),
            Some(Duration::from_secs(30))
        );
        // 其它错误 5/10/20/60，封顶 60
        assert_eq!(next_delay(FailKind::Other, 0), Some(Duration::from_secs(5)));
        assert_eq!(
            next_delay(FailKind::Other, 1),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            next_delay(FailKind::Other, 2),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            next_delay(FailKind::Other, 3),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            next_delay(FailKind::Other, 99),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn 门户地址解析() {
        assert_eq!(
            parse_host_port("http://202.117.144.205:8600/snnuportal/login.jsp"),
            Some(("202.117.144.205".to_string(), 8600))
        );
        assert_eq!(
            parse_host_port("http://portal.snnu.edu.cn/login"),
            Some(("portal.snnu.edu.cn".to_string(), 80))
        );
        assert_eq!(parse_host_port("不是网址"), None);
    }

    #[test]
    fn 状态页取账号() {
        let html = r#"<td>当前登录账号：<span class="zhengwen1">20241234@mobile</span></td>"#;
        assert_eq!(take_account(html), Some("20241234@mobile".to_string()));
        assert_eq!(take_account("<html>什么也没有</html>"), None);
    }

    #[test]
    fn 错误信息摘取() {
        let html = "<html><body><div>认证失败：密码错误，请重新输入</div></body></html>";
        let (captcha, msg) = classify(html);
        assert!(!captcha);
        assert!(msg.contains("密码错误"));

        let html2 = "<html><body>请输入验证码后重试</body></html>";
        let (captcha2, msg2) = classify(html2);
        assert!(captcha2);
        assert_eq!(msg2, "请输入验证码后重试");

        // 密码错误页即使附带验证码输入框，也必须优先报密码错误
        let html3 = r#"
            <form>
              <div>您输入的密码错误，请重新输入</div>
              <input type="text" name="checkcode">
            </form>
        "#;
        let (captcha3, msg3) = classify(html3);
        assert!(!captcha3);
        assert!(msg3.contains("密码错误"));

        // 只有验证码字段、没有错误语句时是表单回显，给通用登录失败提示
        let html4 = r#"<form><div>请输入验证码</div><input name="checkcode"></form>"#;
        let (captcha4, msg4) = classify(html4);
        assert!(captcha4);
        assert_eq!(msg4, "请输入验证码");

        let html5 =
            r#"<form><input name="account"><input name="password"><input name="checkcode"></form>"#;
        let (captcha5, msg5) = classify(html5);
        assert!(!captcha5);
        assert_eq!(msg5, "账号或密码错误，请检查后重试");

        let html6 = "<div>号： 密码： 校园网： 校园网 代拨： 联通 移动 电信验证码： i</div>";
        let (captcha6, msg6) = classify(html6);
        assert!(!captcha6);
        assert_eq!(msg6, "账号或密码错误，请检查后重试");

        let html7 = "陕西师范大学网络Portal 登录失败:您的账号密码有误 账 号： 密 码： 校园网： 校园网 代 拨： 联 通 移 动 电 信 验证码： 记住密码 自助服务主页 | 当前IP:10.138.40.230 Copyright （C）2012";
        let cut7 = cut_form_noise(html7);
        assert!(cut7.contains("登录失败"), "cut7={}", cut7);
        let msg7x = extract_message(html7);
        assert_eq!(msg7x, "登录失败:您的账号密码有误", "msg7x={}", msg7x);
        let (captcha7, msg7) = classify(html7);
        assert!(!captcha7);
        assert_eq!(msg7, "登录失败:您的账号密码有误");
        assert_eq!(display_error(html7), "登录失败:您的账号密码有误");

        assert_eq!(
            friendly_login_error("登录失败:您的账号密码有误 账 号： 密 码："),
            "您的账号密码有误"
        );
        assert_eq!(friendly_login_error("您的账号已欠费"), "您的账号已欠费");
    }

    #[test]
    fn 编码识别() {
        let utf8 = "认证失败：密码错误".as_bytes();
        assert_eq!(decode(utf8), "认证失败：密码错误");
        let (gbk, _, _) = encoding_rs::GB18030.encode("认证失败：密码错误");
        assert_eq!(decode(&gbk), "认证失败：密码错误");
    }

    #[test]
    fn 错误页清理样式与脚本() {
        let html = r#"
            <html><head>
              <style>#mb { display: none; color: red; }</style>
              <script>var ret = "认证失败";</script>
            </head><body>
              <!-- 旧模板 -->
              <div class="err">登录失败：账号或密码错误</div>
            </body></html>
        "#;
        assert_eq!(clean_text(html), "登录失败：账号或密码错误");
        let (_, msg) = classify(html);
        assert!(msg.contains("账号或密码错误"));
        assert!(!msg.contains("display"));
        assert!(!msg.contains('#'));

        // 无法识别的乱码和 CSS 应转换为可操作提示。
        let stale = "闟疊タ甯塨寖瀀y 缃戬粿Portal #mb{ display: none; font-family: arial }";
        assert_eq!(display_error(stale), "认证未通过，请重试一次");
        assert_eq!(
            display_error("登录失败：账号或密码错误"),
            "登录失败：账号或密码错误"
        );
    }

    // ---- 状态页字段解析 ----

    const STATUS_PAGE: &str = r#"<html><body>
        <span>当前登录账号： 20241234@mobile</span><br>
        登录时间：2026-10-25 14:06:29 <br>
        当前IP：10.1.2.3 <br>
        </body></html>"#;

    #[test]
    fn 状态页取_ip与登录时间() {
        assert_eq!(take_ip(STATUS_PAGE), "10.1.2.3");
        assert_eq!(take_login_time(STATUS_PAGE), "2026-10-25 14:06:29");
        assert_eq!(take_account(STATUS_PAGE).unwrap(), "20241234@mobile");
        let s = session_of(STATUS_PAGE).unwrap();
        assert_eq!(s.account, "20241234@mobile");
        assert_eq!(s.ip, "10.1.2.3");
        assert_eq!(s.login_time, "2026-10-25 14:06:29");
    }

    #[test]
    fn 状态页缺字段时降级不炸() {
        let only_acct = "当前登录账号：20241234@mobile";
        let s = session_of(only_acct).unwrap();
        assert_eq!(s.account, "20241234@mobile");
        assert!(s.ip.is_empty());
        assert!(s.login_time.is_empty());
        assert!(session_of("什么都没有").is_none());
    }

    #[test]
    fn 门户时间解析() {
        // 2026-10-25 14:06:29 按日历时间算的秒数（不涉及时区）
        let t = parse_portal_time("2026-10-25 14:06:29").unwrap();
        assert_eq!(t, 1792937189.0);
        assert!(parse_portal_time("").is_none());
        assert!(parse_portal_time("not-a-time").is_none());
        assert!(parse_portal_time("2026-13-40 99:99:99").is_none());
        assert!(parse_portal_time("2026-10-25").is_none()); // 缺时分秒
    }

    #[test]
    fn 在线时长() {
        // 未来时间：门户时钟异常，不显示时长也不显示负数
        assert_eq!(online_minutes("2999-01-01 00:00:00"), None);
        assert_eq!(online_minutes("垃圾时间"), None);
        // 很久以前：正常返回一个大数
        let old = online_minutes("2020-01-01 00:00:00").unwrap();
        assert!(old > 60 * 24 * 365 * 5);
    }

    #[test]
    fn 时长格式() {
        assert_eq!(fmt_duration(0), "0 分钟");
        assert_eq!(fmt_duration(42), "42 分钟");
        assert_eq!(fmt_duration(83), "1 小时 23 分");
        assert_eq!(fmt_duration(60 * 26 + 5), "1 天 2 小时");
    }

    #[test]
    fn 日志脱敏() {
        assert_eq!(
            scrub("登录失败: 20241234 密码错误", "20241234"),
            "登录失败: 202***34 密码错误"
        );
        // 空账号、短账号原样返回
        assert_eq!(scrub("无账号", ""), "无账号");
        assert_eq!(scrub("ab 出错", "ab"), "ab 出错");
    }

    #[test]
    fn 掩码边界() {
        assert_eq!(mask("20241234"), "202***34");
        assert_eq!(mask("ab"), "ab");
        assert_eq!(mask(""), "");
    }
}