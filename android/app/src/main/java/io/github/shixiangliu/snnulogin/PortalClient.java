package io.github.shixiangliu.snnulogin;

import java.io.IOException;
import java.io.InterruptedIOException;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;
import static io.github.shixiangliu.snnulogin.PortalProtocol.*;

/** All HTTP goes through an injected transport; the Android transport is bound to one Network. */
public final class PortalClient {
    public interface Transport {
        Response request(String method, String url, Map<String, String> headers, byte[] body, int timeout) throws IOException;
    }
    public static final class Response {
        public final int code;
        public final Map<String, List<String>> headers;
        public final byte[] body;
        public Response(int code, Map<String, List<String>> headers, byte[] body) {
            this.code = code; this.headers = headers; this.body = body;
        }
        public String header(String name) {
            for (Map.Entry<String, List<String>> entry : headers.entrySet()) {
                if (name.equalsIgnoreCase(entry.getKey()) && !entry.getValue().isEmpty()) return entry.getValue().get(0);
            }
            return "";
        }
    }
    public static final class Status {
        public final Kind kind;
        public final Session session;
        public final Endpoint endpoint;
        public Status(Kind kind, Session session, Endpoint endpoint) {
            this.kind = kind; this.session = session; this.endpoint = endpoint;
        }
        public static Status unreachable() { return new Status(Kind.UNREACHABLE, null, null); }
    }
    private final Transport transport;
    private final BooleanSupplier cancelled;
    private final String approvedHost;
    public boolean authenticatedHere;
    public PortalClient(Transport transport, BooleanSupplier cancelled, String approvedHost) {
        this.transport = transport; this.cancelled = cancelled; this.approvedHost = approvedHost;
    }
    private void check() throws InterruptedIOException {
        if (cancelled.getAsBoolean()) throw new InterruptedIOException("操作已取消");
    }
    private Response request(String method, String url, String cookie, byte[] body, int timeout) throws IOException {
        check();
        Map<String, String> headers = new LinkedHashMap<>();
        headers.put("User-Agent", USER_AGENT);
        headers.put("Accept-Language", "zh-CN,zh;q=0.9");
        headers.put("Cache-Control", "no-cache");
        if (!cookie.isEmpty()) headers.put("Cookie", cookie);
        if (body != null) {
            headers.put("Content-Type", "application/x-www-form-urlencoded");
            headers.put("Referer", url.substring(0, url.lastIndexOf('/') + 1) + "login.jsp");
        }
        Response response = transport.request(method, url, headers, body, timeout);
        check();
        return response;
    }
    private Response page(Endpoint endpoint, String path, String cookie, int timeout) throws IOException {
        String url = endpoint.base() + path;
        long deadline = System.nanoTime() + timeout * 1_000_000L;
        for (int i = 0; i < 4; i++) {
            int left = (int) ((deadline - System.nanoTime()) / 1_000_000L);
            if (left <= 0) throw new IOException("认证服务器响应超时");
            Response response = request("GET", url, cookie, null, left);
            if (response.code >= 300 && response.code < 400) {
                String location = response.header("Location");
                if (location.isEmpty()) throw new IOException("门户跳转缺少地址");
                String next;
                try { next = URI.create(url).resolve(location).toString(); }
                catch (IllegalArgumentException ex) { throw new IOException("门户跳转地址无效"); }
                if (!sameOrigin(endpoint, next)) throw new IOException("已阻止跨地址门户跳转");
                cookie = mergeCookies(cookie, response);
                url = next;
            } else {
                if (response.code != 200) throw new IOException("门户返回 HTTP " + response.code);
                // Cookies from intermediate redirects must survive to the login POST.
                String merged = mergeCookies(cookie, response);
                Map<String, List<String>> headers = new LinkedHashMap<>(response.headers);
                headers.put("X-SNNU-Session-Cookie", java.util.Collections.singletonList(merged));
                return new Response(response.code, headers, response.body);
            }
        }
        throw new IOException("门户跳转过多");
    }
    public Status statusAt(Endpoint endpoint, int timeout) throws IOException {
        Response response = page(endpoint, "userstatus.jsp", "", timeout);
        String html = decode(response.body);
        // New hosts need both the portal structure and its campus identity before being surfaced.
        if (!trustedHost(endpoint.host) && !endpoint.host.equalsIgnoreCase(approvedHost)
            && !html.contains("陕西师范大学")) return Status.unreachable();
        Session session = session(html);
        if (session != null) return new Status(Kind.ONLINE, session, endpoint);
        return loginForm(html) ? new Status(Kind.OFFLINE, null, endpoint) : Status.unreachable();
    }
    /** Shared pool for the portal port scan; daemon threads never outlive the process. */
    private static final ExecutorService SCANNER = Executors.newFixedThreadPool(
        Math.min(4, PORTS.length), job -> {
            Thread thread = new Thread(job, "portal-scan"); thread.setDaemon(true); return thread;
        });
    public Status status(Endpoint cached) throws IOException {
        Status offline = null;
        if (cached != null) {
            try {
                Status result = statusAt(cached, 1200);
                if (result.kind != Kind.UNREACHABLE) return result;
            } catch (IOException ex) { check(); }
        }
        // Discover the actual Wi-Fi portal before scanning, including a changed IP or port.
        for (String probe : PROBES) {
            check();
            try {
                Response response = request("GET", probe, "", null, 1000);
                Endpoint found = endpoint(response.header("Location"));
                if (found != null) {
                    Status result = statusAt(found, 1500);
                    if (result.kind == Kind.ONLINE) return result;
                    if (result.kind == Kind.OFFLINE) offline = result;
                }
            } catch (IOException ex) { check(); }
        }
        // Serial probing of 11 ports can hold the single worker for seconds when the
        // portal is unreachable; scan them concurrently under one deadline instead.
        String host = cached == null ? HOST : cached.host;
        List<Future<Status>> futures = new ArrayList<>();
        for (int port : PORTS) {
            final int candidate = port;
            futures.add(SCANNER.submit(() -> {
                try { return statusAt(new Endpoint(host, candidate), 450); }
                catch (IOException ex) { check(); return null; }
            }));
        }
        long deadlineNanos = System.nanoTime() + 1_500_000_000L;
        for (Future<Status> future : futures) {
            check();
            long leftMillis = (deadlineNanos - System.nanoTime()) / 1_000_000L;
            Status result;
            try {
                result = leftMillis > 0 ? future.get(leftMillis, TimeUnit.MILLISECONDS) : null;
            } catch (Exception ex) { result = null; }
            if (result == null) continue;
            if (result.kind == Kind.ONLINE) return result;
            if (result.kind == Kind.OFFLINE && offline == null) offline = result;
        }
        return offline == null ? Status.unreachable() : offline;
    }
    public boolean approved(Endpoint endpoint) {
        return trustedHost(endpoint.host) || endpoint.host.equalsIgnoreCase(approvedHost);
    }
    public Status login(Endpoint endpoint, String account, String password, String operator) throws IOException {
        // Recheck immediately before submission; cached page/host alone never authorizes login.
        Status current = statusAt(endpoint, 4000);
        if (current.kind == Kind.ONLINE) return current;
        if (current.kind != Kind.OFFLINE) throw new IOException("连不上认证服务器");
        if (!approved(endpoint)) throw new IOException("认证地址已变化，请在网络信息中核对并确认新地址");
        for (int attempt = 0; attempt < 2; attempt++) {
            Response page = page(endpoint, "login.jsp?_=" + System.currentTimeMillis(), "", 5000);
            String cookie = page.header("X-SNNU-Session-Cookie");
            check();
            Response response = request("POST", endpoint.base() + "login", cookie,
                form(account, password, operator).getBytes(StandardCharsets.UTF_8), 10000);
            String html;
            if (response.code >= 300 && response.code < 400) {
                String location = response.header("Location");
                String target;
                try { target = URI.create(endpoint.base() + "login").resolve(location).toString(); }
                catch (IllegalArgumentException ex) { throw new IOException("登录跳转地址无效"); }
                if (location.isEmpty() || !sameOrigin(endpoint, target)) throw new IOException("已阻止登录后的跨地址跳转");
                String path = target.substring(endpoint.base().length());
                html = decode(page(endpoint, path, mergeCookies(cookie, response), 4000).body);
            } else {
                if (response.code != 200) throw new IOException("认证服务器返回 HTTP " + response.code);
                html = decode(response.body);
            }
            Session logged = session(html);
            if (logged != null) {
                if (!logged.matches(account, operator)) throw new IOException("当前使用其他账号或上网方式，保留当前连接");
                authenticatedHere = true;
                return new Status(Kind.ONLINE, logged, endpoint);
            }
            String error = error(html);
            if (failure(error) == Failure.CAPTCHA && attempt == 0) continue;
            if (failure(error) != Failure.CAPTCHA) {
                try {
                    Status after = statusAt(endpoint, 4000);
                    if (after.kind == Kind.ONLINE && after.session.matches(account, operator)) {
                        authenticatedHere = true;
                        return after;
                    }
                } catch (IOException unavailable) {
                    check(); // Preserve the explicit credential/captcha error when the fallback probe fails.
                }
            }
            throw new IOException(error);
        }
        throw new IOException("请输入验证码");
    }
    public Status logout(Endpoint endpoint, String account, String operator) throws IOException {
        Status current = statusAt(endpoint, 4000);
        if (current.kind == Kind.OFFLINE) return current;
        if (current.kind != Kind.ONLINE) throw new IOException("连不上认证服务器");
        if (!current.session.matches(account, operator)) throw new IOException("当前使用其他账号或上网方式，保留当前连接");
        if (!approved(endpoint)) throw new IOException("请先确认校园网认证地址");
        page(endpoint, "logoff", "", 6000);
        for (int i = 0; i < 3; i++) {
            Status after = statusAt(endpoint, 3000);
            if (after.kind == Kind.OFFLINE) return after;
        }
        throw new IOException("下线尚未确认，自动登录已暂停");
    }
    public boolean internetOk() throws IOException {
        for (int i = 0; i < PROBES.length; i++) {
            check();
            try {
                Response response = request("GET", PROBES[i], "", null, 3000);
                if (i == 0 && response.code == 204) return true;
                if (i == 1 && response.code == 200 && decode(response.body).trim().equals("Microsoft Connect Test")) return true;
            } catch (IOException ex) { check(); }
        }
        return false;
    }
    static String mergeCookies(String previous, Response response) {
        Map<String, String> pairs = new LinkedHashMap<>();
        for (String part : previous.split(";")) putCookie(pairs, part.trim());
        for (Map.Entry<String, List<String>> entry : response.headers.entrySet()) {
            if ("Set-Cookie".equalsIgnoreCase(entry.getKey())) {
                for (String cookie : entry.getValue()) putCookie(pairs, cookie.split(";", 2)[0].trim());
            }
        }
        return String.join("; ", pairs.values());
    }
    private static void putCookie(Map<String, String> pairs, String value) {
        int separator = value.indexOf('=');
        if (separator > 0 && !value.contains("\r") && !value.contains("\n")) pairs.put(value.substring(0, separator), value);
    }
}
