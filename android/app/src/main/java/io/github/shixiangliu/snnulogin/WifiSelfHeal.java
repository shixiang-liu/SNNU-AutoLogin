package io.github.shixiangliu.snnulogin;

import java.util.function.BooleanSupplier;

/**
 * Dropped-connection self-heal, decision only. Same semantics as decide() in the Windows client
 * (src/windows_wifi.rs): an unreachable portal alone is never a reason to touch Wi-Fi -- the
 * device must also have no external network and must not already be on the campus Wi-Fi.
 * The three facts arrive as lazily evaluated suppliers, so a decision never probes for a fact it
 * does not need. No Android type is referenced here, which keeps the rule verifiable offline.
 */
public final class WifiSelfHeal {
    private WifiSelfHeal() {}
    /** 学校无线网名称。本工具只服务这一张网，因此没有可配置项。 */
    public static final String DEFAULT_SSID = "SNNU";
    /** WifiInfo.getSSID() 在未连无线或调用方权限不足时返回的占位值。 */
    public static final String UNKNOWN_SSID = "<unknown ssid>";
    /** 两次连接动作之间的最小间隔，避免每一轮巡检都去请求连接。 */
    public static final long CONNECT_COOLDOWN_MS = 60_000L;

    /**
     * 不动作的原因，与 Windows 版 Skip 逐项对应。手机没有可判定的有线校园链路，
     * 因此 Windows 版的 Wired（有线在线，不干预无线）在这一版没有对应项。
     */
    public enum Skip { DISABLED, INTERNET_UP, PORTAL_OK, ON_TARGET_SSID }

    /** 纯决策结论。 */
    public enum Decision { SKIP, CONNECT }

    /** 一次决策的结果：不连接时带上原因。 */
    public static final class Plan {
        public final Decision decision;
        public final Skip skip;
        private Plan(Decision decision, Skip skip) { this.decision = decision; this.skip = skip; }
        public boolean connects() { return decision == Decision.CONNECT; }
    }

    private static final Plan CONNECT = new Plan(Decision.CONNECT, null);
    private static Plan skip(Skip reason) { return new Plan(Decision.SKIP, reason); }

    /**
     * 三条件门控：外网不可达、门户不可达、又没连在学校无线网上，三条同时成立才允许连接。
     * enabled 是总开关，在 Android 上就是「自动登录」。判断顺序与短路行为同 Windows 版，
     * 因此被跳过的分支一个探测都不会做。
     */
    public static Plan decide(boolean enabled, BooleanSupplier internetUp,
            BooleanSupplier portalReachable, BooleanSupplier onTargetWifi) {
        if (!enabled) return skip(Skip.DISABLED);
        // 条件 1：外网还能上就不是「本身没网」，门户一时的故障不该动 Wi-Fi
        if (internetUp.getAsBoolean()) return skip(Skip.INTERNET_UP);
        if (portalReachable.getAsBoolean()) return skip(Skip.PORTAL_OK);
        // 条件 3：已经连在学校无线网上，重复连接没有意义
        if (onTargetWifi.getAsBoolean()) return skip(Skip.ON_TARGET_SSID);
        return CONNECT;
    }

    /**
     * 无线名称比对。系统对可解码的名称会加双引号，读不到时给出 UNKNOWN_SSID；空值与未知值
     * 都算「没能确认在学校网络上」，于是仍然允许尝试连接——与 Windows 版读不到 SSID 时一致。
     * 名称是区分大小写的字节串，按精确匹配处理（Windows 版同为精确匹配）。
     */
    public static boolean matchesSsid(String current, String target) {
        String left = normalizeSsid(current), right = normalizeSsid(target);
        return !left.isEmpty() && left.equals(right);
    }

    /** 去掉系统加的双引号；未知值与空白归为空串。 */
    static String normalizeSsid(String ssid) {
        if (ssid == null) return "";
        String value = ssid.trim();
        if (value.length() >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
            value = value.substring(1, value.length() - 1).trim();
        }
        return value.equals(UNKNOWN_SSID) ? "" : value;
    }

    /** 连接动作是否还在冷却期；lastAttempt 为 0 表示从未请求过。 */
    public static boolean cooling(long now, long lastAttempt) {
        return lastAttempt != 0 && now - lastAttempt < CONNECT_COOLDOWN_MS;
    }
}
