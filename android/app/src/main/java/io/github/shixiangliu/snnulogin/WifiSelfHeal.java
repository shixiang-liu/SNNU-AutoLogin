package io.github.shixiangliu.snnulogin;

import java.util.function.BooleanSupplier;

/**
 * Dropped-connection self-heal, decision only. The rule is the one the Windows client implements in
 * src/wifi_heal_core.rs: an unreachable portal alone is never a reason to touch Wi-Fi -- the network
 * the device is actually on must also be unusable, and the device must not already be on the campus
 * Wi-Fi. A device that is online through some other working network is left alone.
 *
 * Android differs from the desktop in one place: a phone usually has a second egress (cellular).
 * The desktop can only ask "is there a Wi-Fi association or a cable"; when neither exists the
 * internet verdict can only come from a TUN/virtual adapter, so it keeps trying to reconnect. On a
 * phone, no Wi-Fi association plus a validated mobile network is a real, user-chosen egress, so the
 * answer is the opposite: leave it alone. Both branches are expressed here so the platform
 * difference is visible in one place.
 *
 * The facts arrive as lazily evaluated suppliers, so a decision never probes for a fact it does not
 * need. No Android type is referenced here, which keeps the rule verifiable offline.
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
     * 因此 Windows 版的 Wired（有线在线，不干预无线）在这一版没有对应项；
     * 取而代之的是 MOBILE_NETWORK：手机自己的出口可用（蜂窝等），用户在用流量，不去抢。
     */
    public enum Skip { DISABLED, INTERNET_UP, PORTAL_OK, ON_TARGET_SSID, MOBILE_NETWORK }

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
     * 决策入口。两条分支对应「设备此刻挂在哪种网络上」：
     *
     * 连着某个 Wi-Fi 时（onWifi=true）——与 Windows 版同序：经这张 Wi-Fi 的外网探测还通，
     * 说明网络本身可用，门户一时的故障不该动 Wi-Fi；门户可达同理；已经连在 SNNU 上则无需重复连接。
     *
     * 没有任何 Wi-Fi 关联时（onWifi=false）——没有无线出口可探测，于是问手机自己的出口
     * （otherNetworkUp，即蜂窝等默认网络）。可用就说明用户正靠流量上网，绝不去抢；
     * 一点出口都没有才是「连接的网络不可用」，此时应当尝试连回学校网络。
     *
     * enabled 是总开关，在 Android 上就是「自动登录」。判断顺序与短路行为同 Windows 版，
     * 因此被跳过的分支一个探测都不会做。
     */
    public static Plan decide(boolean enabled, BooleanSupplier onWifi, BooleanSupplier internetUp,
            BooleanSupplier portalReachable, BooleanSupplier onTargetWifi, BooleanSupplier otherNetworkUp) {
        if (!enabled) return skip(Skip.DISABLED);
        if (!onWifi.getAsBoolean()) {
            // 没有无线出口：外网与门户都无从探测（供应商也不会被调用），只看手机自己的出口。
            return otherNetworkUp.getAsBoolean() ? skip(Skip.MOBILE_NETWORK) : CONNECT;
        }
        // 条件 1：这张 Wi-Fi 的外网还能上就不是「连接的网络不可用」，门户一时的故障不该动 Wi-Fi
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
