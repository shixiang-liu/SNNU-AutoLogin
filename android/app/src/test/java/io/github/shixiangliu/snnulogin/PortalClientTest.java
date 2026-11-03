package io.github.shixiangliu.snnulogin;

import org.junit.Test;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.atomic.AtomicBoolean;
import static org.junit.Assert.*;
import static io.github.shixiangliu.snnulogin.PortalProtocol.*;

public class PortalClientTest {
    private static final Endpoint ENDPOINT = new Endpoint(HOST, 8601);
    private static final String FORM = "<input name='account'><input name='password'>";
    private static final String ONLINE = "当前登录账号：<span>20241234@mobile</span>";
    private static PortalClient.Response response(int code, String body, String... headers) {
        Map<String, List<String>> map = new LinkedHashMap<>();
        for (int i = 0; i < headers.length; i += 2) map.computeIfAbsent(headers[i], key -> new ArrayList<>()).add(headers[i + 1]);
        return new PortalClient.Response(code, map, body.getBytes(StandardCharsets.UTF_8));
    }
    private static final class Script implements PortalClient.Transport {
        final Deque<PortalClient.Response> responses = new ArrayDeque<>();
        final List<String> methods = new ArrayList<>(), urls = new ArrayList<>(), bodies = new ArrayList<>();
        final List<Map<String, String>> headers = new ArrayList<>();
        Script(PortalClient.Response... responses) { this.responses.addAll(Arrays.asList(responses)); }
        @Override public synchronized PortalClient.Response request(String method, String url, Map<String, String> headers, byte[] body, int timeout) throws IOException {
            methods.add(method); urls.add(url); this.headers.add(headers); bodies.add(body == null ? "" : new String(body, StandardCharsets.UTF_8));
            if (responses.isEmpty()) throw new IOException("No scripted response");
            return responses.remove();
        }
    }
    @Test public void statusIsNotPublicInternetValidation() throws Exception {
        Script script = new Script(response(200, FORM));
        assertEquals(Kind.OFFLINE, new PortalClient(script, () -> false, "").statusAt(ENDPOINT, 500).kind);
    }
    @Test public void loginForwardsAllSessionCookiesAndEncodedForm() throws Exception {
        Script script = new Script(response(200, FORM), response(200, FORM, "Set-Cookie", "JSESSIONID=one; Path=/", "Set-Cookie", "route=two; HttpOnly"), response(200, ONLINE));
        PortalClient client = new PortalClient(script, () -> false, "");
        assertEquals(Kind.ONLINE, client.login(ENDPOINT, "20241234", "p&=", "mobile").kind);
        assertTrue(client.authenticatedHere);
        assertEquals("JSESSIONID=one; route=two", script.headers.get(2).get("Cookie"));
        assertTrue(script.bodies.get(2).contains("password=p%26%3D"));
        assertTrue(script.urls.stream().allMatch(url -> url.startsWith(ENDPOINT.base())));
    }
    @Test public void statusRedirectCannotSwitchOrigin() {
        Script script = new Script(response(302, "", "Location", "http://evil.test:8601/snnuportal/login.jsp"));
        assertThrows(IOException.class, () -> new PortalClient(script, () -> false, "").statusAt(ENDPOINT, 500));
        assertEquals(1, script.urls.size()); assertEquals("", script.bodies.get(0));
    }
    @Test public void postRedirectNeverForwardsPasswordOrCookieToOtherHost() {
        Script script = new Script(response(200, FORM), response(200, FORM), response(307, "", "Location", "http://evil.test:8601/snnuportal/login"));
        assertThrows(IOException.class, () -> new PortalClient(script, () -> false, "").login(ENDPOINT, "20241234", "secret", "mobile"));
        assertEquals(3, script.urls.size()); assertFalse(script.urls.stream().anyMatch(url -> url.contains("evil.test")));
    }
    @Test public void sameOriginPostRedirectUsesGet() throws Exception {
        Script script = new Script(response(200, FORM), response(200, FORM), response(302, "", "Location", "/snnuportal/userstatus.jsp"), response(200, ONLINE));
        assertEquals(Kind.ONLINE, new PortalClient(script, () -> false, "").login(ENDPOINT, "20241234", "secret", "mobile").kind);
        assertEquals(Arrays.asList("GET", "GET", "POST", "GET"), script.methods);
        assertEquals("", script.bodies.get(3));
    }
    @Test public void redirectCookiesSurviveUntilPost() throws Exception {
        Script script = new Script(response(200, FORM), response(302, "", "Location", "login.jsp", "Set-Cookie", "route=one; Path=/"),
            response(200, FORM, "Set-Cookie", "JSESSIONID=two"), response(200, ONLINE));
        new PortalClient(script, () -> false, "").login(ENDPOINT, "20241234", "pw", "mobile");
        assertEquals("route=one; JSESSIONID=two", script.headers.get(3).get("Cookie"));
    }
    @Test public void differentOnlineAccountIsNeverSubmittedOver() throws Exception {
        Script script = new Script(response(200, "当前登录账号：20249999@unicom"));
        PortalClient client = new PortalClient(script, () -> false, "");
        assertEquals(Kind.ONLINE, client.login(ENDPOINT, "20241234", "pw", "mobile").kind);
        assertFalse(client.authenticatedHere); assertEquals(Collections.singletonList("GET"), script.methods);
    }
    @Test public void logoutProtectsOtherAccountAndOperator() {
        for (String account : new String[]{"20249999@mobile", "20241234@unicom"}) {
            Script script = new Script(response(200, "当前登录账号：" + account));
            assertThrows(IOException.class, () -> new PortalClient(script, () -> false, "").logout(ENDPOINT, "20241234", "mobile"));
            assertFalse(script.urls.stream().anyMatch(url -> url.endsWith("logoff")));
        }
    }
    @Test public void unfamiliarDiscoveryNeedsCampusIdentityAndExplicitApproval() throws Exception {
        Endpoint unknown = new Endpoint("new-portal.test", 8601);
        Script first = new Script(response(200, FORM));
        assertEquals(Kind.UNREACHABLE, new PortalClient(first, () -> false, "").statusAt(unknown, 500).kind);
        Script second = new Script(response(200, "陕西师范大学" + FORM));
        assertThrows(IOException.class, () -> new PortalClient(second, () -> false, "").login(unknown, "20241234", "pw", "mobile"));
        assertEquals(Collections.singletonList("GET"), second.methods);
        Script third = new Script(response(200, FORM), response(200, FORM), response(200, ONLINE));
        assertEquals(Kind.ONLINE, new PortalClient(third, () -> false, unknown.host).login(unknown, "20241234", "pw", "mobile").kind);
    }
    @Test public void captchaRefreshesWithNewSessionAndStopsAfterTwoSubmissions() {
        Script script = new Script(response(200, FORM), response(200, FORM, "Set-Cookie", "JSESSIONID=a"), response(200, "请输入验证码"),
            response(200, FORM, "Set-Cookie", "JSESSIONID=b"), response(200, "请输入验证码"));
        assertThrows(IOException.class, () -> new PortalClient(script, () -> false, "").login(ENDPOINT, "20241234", "pw", "mobile"));
        assertEquals(2, script.methods.stream().filter("POST"::equals).count());
        assertEquals("JSESSIONID=b", script.headers.get(4).get("Cookie"));
    }
    @Test public void networkOrUserCancellationPreventsSubmission() {
        AtomicBoolean cancelled = new AtomicBoolean();
        Script script = new Script(response(200, FORM), response(200, FORM));
        PortalClient.Transport transport = (method, url, headers, body, timeout) -> {
            PortalClient.Response response = script.request(method, url, headers, body, timeout);
            if (script.urls.size() == 2) cancelled.set(true);
            return response;
        };
        assertThrows(IOException.class, () -> new PortalClient(transport, cancelled::get, "").login(ENDPOINT, "20241234", "pw", "mobile"));
        assertFalse(script.methods.contains("POST"));
    }
    @Test public void redirectToLoginDoesNotCountAsInternetAccess() throws Exception {
        Script script = new Script(response(302, "", "Location", ENDPOINT.base()), response(200, FORM));
        assertFalse(new PortalClient(script, () -> false, "").internetOk());
    }
    @Test public void cookieRenewalReplacesPreviousValue() {
        assertEquals("route=old; JSESSIONID=new", PortalClient.mergeCookies("route=old; JSESSIONID=old", response(200, "", "Set-Cookie", "JSESSIONID=new; Path=/")));
    }
    @Test public void failedFallbackProbeDoesNotEraseCredentialErrorOrItsCircuitBreaker() {
        Script script = new Script(response(200, FORM), response(200, FORM), response(200, "您的账号密码有误"));
        IOException error = assertThrows(IOException.class, () -> new PortalClient(script, () -> false, "").login(ENDPOINT, "20241234", "pw", "mobile"));
        assertEquals(Failure.CREDENTIAL, failure(error.getMessage()));
        assertEquals(-1, nextDelaySeconds(failure(error.getMessage()), 1));
    }
}
