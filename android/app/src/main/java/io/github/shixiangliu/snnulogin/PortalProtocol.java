package io.github.shixiangliu.snnulogin;

import java.io.IOException;
import java.net.URI;
import java.net.URLEncoder;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.Charset;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Android-free protocol parsing: shared behavior with the Windows portal module. */
public final class PortalProtocol {
    private PortalProtocol() {}
    public static final String HOST = "202.117.144.205";
    public static final int[] PORTS = {8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600};
    public static final String[] PROBES = {
        "http://connect.rom.miui.com/generate_204",
        "http://www.msftconnecttest.com/connecttest.txt"
    };
    public static final String USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    public enum Kind { ONLINE, OFFLINE, UNREACHABLE }
    public enum Failure { CREDENTIAL, CAPTCHA, ACCOUNT_STATE, UNREACHABLE, OTHER }

    public static final class Session {
        public final String account, ip, loginTime;
        public Session(String account, String ip, String loginTime) {
            this.account = account; this.ip = ip; this.loginTime = loginTime;
        }
        public boolean matches(String user, String operator) {
            String[] parts = account.split("@", 2);
            return !user.isEmpty() && parts[0].trim().equals(user.trim())
                && (parts.length == 2 ? parts[1].trim() : "").equals(operator);
        }
    }

    public static final class Endpoint {
        public final String host;
        public final int port;
        public Endpoint(String host, int port) { this.host = host; this.port = port; }
        public String base() { return "http://" + host + ":" + port + "/snnuportal/"; }
        @Override public boolean equals(Object other) {
            return other instanceof Endpoint && port == ((Endpoint) other).port
                && host.equalsIgnoreCase(((Endpoint) other).host);
        }
        @Override public int hashCode() { return host.toLowerCase(Locale.ROOT).hashCode() * 31 + port; }
    }

    public static String decode(byte[] bytes) {
        String value;
        try {
            value = StandardCharsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(bytes)).toString();
        } catch (CharacterCodingException ex) {
            value = new String(bytes, Charset.forName("GB18030"));
        }
        return value.startsWith("\ufeff") ? value.substring(1) : value;
    }

    public static String cleanText(String html) {
        return html.replaceAll("(?is)<(script|style|noscript)\\b[^>]*>.*?</\\1\\s*>", " ")
            .replaceAll("(?s)<!--.*?-->", " ").replaceAll("(?s)<[^>]*>", " ")
            .replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">")
            .replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
            .replaceAll("[\\s\u3000]+", " ").trim();
    }

    public static Session session(String html) {
        int start = html.indexOf("当前登录账号");
        if (start < 0) return null;
        String value = cleanText(html.substring(start + "当前登录账号".length()));
        Matcher match = Pattern.compile("^[\\s：:]*([A-Za-z0-9@._-]{4,128})(?:\\s|$)").matcher(value);
        if (!match.find()) return null;
        return new Session(match.group(1), field(html, "当前IP"), field(html, "登录时间"));
    }

    static String field(String html, String label) {
        int i = html.indexOf(label);
        if (i < 0) return "";
        String tail = html.substring(i + label.length());
        tail = tail.replaceAll("(?i)<br\\s*/?>|</(?:td|tr|p|div)\\s*>", "\n");
        for (String line : tail.split("[\\r\\n]")) {
            String value = cleanText(line).replaceFirst("^[\\s：:]+", "");
            if (!value.isEmpty()) return value.substring(0, Math.min(value.length(), 64));
        }
        return "";
    }

    public static boolean loginForm(String html) {
        String low = html.toLowerCase(Locale.ROOT);
        return hasField(low, "account") && hasField(low, "password");
    }
    private static boolean hasField(String html, String name) {
        return Pattern.compile("(?:name|id)\\s*=\\s*(?:\"" + name + "\"|'" + name + "'|" + name + "(?=[\\s>]))").matcher(html).find();
    }

    /** Discovery never accepts user-info, arbitrary paths, schemes or port numbers. */
    public static Endpoint endpoint(String location) {
        try {
            URI uri = URI.create(location);
            int port = uri.getPort() < 0 ? 80 : uri.getPort();
            if (!"http".equalsIgnoreCase(uri.getScheme()) || uri.getHost() == null
                || uri.getUserInfo() != null || !uri.getPath().startsWith("/snnuportal/")
                || !allowedPort(port) || !uri.getHost().matches("[A-Za-z0-9.-]+")) return null;
            return new Endpoint(uri.getHost(), port);
        } catch (IllegalArgumentException ex) { return null; }
    }
    private static boolean allowedPort(int port) {
        if (port == 80) return true;
        for (int candidate : PORTS) if (candidate == port) return true;
        return false;
    }
    public static boolean trustedHost(String host) {
        String low = host.toLowerCase(Locale.ROOT);
        return low.equals(HOST) || low.equals("snnu.edu.cn") || low.endsWith(".snnu.edu.cn");
    }
    public static boolean sameOrigin(Endpoint endpoint, String url) {
        Endpoint parsed = endpoint(url);
        return endpoint.equals(parsed);
    }

    public static String form(String account, String password, String operator) throws IOException {
        return "sourceurl=null&account=" + URLEncoder.encode(account, "UTF-8")
            + "&password=" + URLEncoder.encode(password, "UTF-8")
            + "&yys=" + URLEncoder.encode(operator, "UTF-8") + "&issave=";
    }

    public static String error(String html) {
        String text = cleanText(html);
        text = text.replaceFirst("^陕西师范大学网络Portal\\s*", "");
        text = text.replaceAll("(?s)(?:账\\s*号|号)\\s*[：:]\\s*密\\s*码\\s*[：:].*", "").trim();
        text = text.replaceAll("(?s)代\\s*拨\\s*[：:]\\s*联\\s*通.*", "").trim();
        String[] credentials = {"密码错误", "密码输入错误", "密码不正确", "密码有误", "密码无效", "密码不匹配", "密码验证失败", "账号密码有误", "账号或密码", "用户名或密码", "账号不存在", "用户不存在", "未开通", "停机", "欠费", "余额不足", "绑定失败"};
        for (String sentence : text.split("[。!！;；]")) {
            if (containsAny(sentence, credentials)) return limited(sentence.trim());
        }
        for (String sentence : text.split("[。!！;；]")) {
            if (sentence.contains("验证码")) return limited(sentence.trim());
        }
        if (text.toLowerCase(Locale.ROOT).contains("captcha")) return "请输入验证码";
        for (String sentence : text.split("[。!！;；]")) {
            if (containsAny(sentence, new String[]{"失败", "错误", "无效", "已过期", "在线数", "欠费", "锁定", "停用"})) return limited(sentence.trim());
        }
        return loginForm(html) || text.isEmpty() ? "账号或密码错误，请检查后重试" : limit(text);
    }
    /** 去掉表单噪声后可能为空（例如句子本身只是“账号：密码：”），空提示会误导用户，回退到通用文案。 */
    private static String limited(String sentence) {
        String compact = limit(sentence);
        return compact.isEmpty() ? "账号或密码错误，请检查后重试" : compact;
    }
    private static String limit(String text) {
        String compact = text.replaceAll("(?s)(账号|号)\\s*[：:]\\s*密码\\s*[：:].*", "").trim();
        return compact.substring(0, Math.min(compact.length(), 120));
    }
    private static boolean containsAny(String text, String[] words) {
        for (String word : words) if (text.contains(word)) return true;
        return false;
    }
    public static Failure failure(String message) {
        if (message.contains("验证码")) return Failure.CAPTCHA;
        if (containsAny(message, new String[]{"欠费", "余额不足", "流量用完", "流量已用尽", "锁定", "停机", "停用", "已过期", "未开通", "禁用", "用户已在线", "在线数"})) return Failure.ACCOUNT_STATE;
        if (message.contains("密码") || containsAny(message, new String[]{"账号不存在", "帐号不存在", "用户不存在", "用户名不存在", "无效用户", "账号无效", "帐号无效"})) return Failure.CREDENTIAL;
        if (containsAny(message, new String[]{"连不上", "超时", "未认证"})) return Failure.UNREACHABLE;
        return Failure.OTHER;
    }
    /** -1 means the persisted 30-minute circuit breaker, rather than another login. */
    public static long nextDelaySeconds(Failure failure, int attempt) {
        switch (failure) {
            case CREDENTIAL: case CAPTCHA: case ACCOUNT_STATE: return -1;
            case UNREACHABLE: return 30;
            default: return new long[]{5, 10, 20, 60}[Math.min(Math.max(attempt, 0), 3)];
        }
    }
    public static String redact(String text, String account, String password) {
        String out = text;
        if (!password.isEmpty()) {
            out = out.replace(password, "[密码已隐藏]");
            try { out = out.replace(URLEncoder.encode(password, "UTF-8"), "[密码已隐藏]"); }
            catch (java.io.UnsupportedEncodingException impossible) { throw new AssertionError(impossible); }
        }
        if (!account.isEmpty()) out = out.replace(account, mask(account));
        return out.replaceAll("(?<![A-Za-z0-9])\\d{6,}(?:@(mobile|unicom))?", "[账号已隐藏]");
    }
    public static String mask(String account) {
        int at = account.indexOf('@');
        String user = at < 0 ? account : account.substring(0, at);
        return user.length() <= 4 ? "***" : user.substring(0, 3) + "***" + user.substring(user.length() - 2);
    }
    public static String onlineDuration(String loginTime, java.time.LocalDateTime now) {
        try {
            java.time.LocalDateTime start = java.time.LocalDateTime.parse(loginTime,
                java.time.format.DateTimeFormatter.ofPattern("uuuu-MM-dd HH:mm:ss").withResolverStyle(java.time.format.ResolverStyle.STRICT));
            long minutes = java.time.Duration.between(start, now).toMinutes();
            if (start.isAfter(now)) return "";
            if (minutes < 60) return minutes + " 分钟";
            if (minutes < 1440) return minutes / 60 + " 小时 " + minutes % 60 + " 分";
            return minutes / 1440 + " 天 " + minutes % 1440 / 60 + " 小时";
        } catch (java.time.DateTimeException ex) { return ""; }
    }
}
