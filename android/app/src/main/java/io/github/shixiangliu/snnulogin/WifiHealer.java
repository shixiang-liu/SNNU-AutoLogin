package io.github.shixiangliu.snnulogin;

import android.content.Context;
import android.net.wifi.WifiConfiguration;
import android.net.wifi.WifiInfo;
import android.net.wifi.WifiManager;
import android.net.wifi.WifiNetworkSuggestion;
import android.os.Build;
import java.util.Collections;

/**
 * Android 组装层：读当前无线名称、按 API 级别请求一次连接。决策不在这里（见 WifiSelfHeal），
 * 这里只做系统调用与结果归类，好把「系统允许应用做什么」如实写清楚。
 *
 * 连接动作的真实边界（系统限制，应用绕不过去）：
 * - API 29 起应用只能用 WifiNetworkSuggestion「建议」网络，连不连、何时连由系统决定，
 *   用户可以在系统设置里撤销，厂商实现也可能延迟；应用无法静默强制连接。
 * - API 26–28 走已弃用的 WifiConfiguration + addNetwork/enableNetwork 路径，需要
 *   CHANGE_WIFI_STATE。该路径自 Android 10（API 29）起对普通应用失效，所以只在旧系统生效。
 * - 读当前 SSID 需要定位权限（API 26–32 是 ACCESS_FINE_LOCATION，且系统定位开关要打开）。
 *   本应用不申请定位权限，因此未授权时系统返回 UNKNOWN_SSID，自愈会跳过「已连学校网络」这一条判断。
 * - 不扫描、不改系统设置、不新建/删除系统里的其它配置：只对开放网络 SNNU 请求一次连接。
 */
final class WifiHealer {
    /** 一次连接动作的结果，只用于日志与用户可见文案。 */
    enum Result {
        /** 已向系统提交连接请求 */
        REQUESTED,
        /** 权限不足或系统策略拒绝 */
        DENIED,
        /** 当前 API 级别没有可用路径 */
        UNSUPPORTED,
        /** 系统明确拒绝了这次请求 */
        REJECTED
    }

    private final WifiManager wifi;

    WifiHealer(Context context) {
        wifi = context.getApplicationContext().getSystemService(WifiManager.class);
    }

    /** 当前已连无线的名称；未连无线、系统未授权定位或读不到时返回空串。 */
    @SuppressWarnings("deprecation")
    String currentSsid() {
        try {
            WifiInfo info = wifi == null ? null : wifi.getConnectionInfo();
            return WifiSelfHeal.normalizeSsid(info == null ? null : info.getSSID());
        } catch (RuntimeException ex) {
            // 读不到名称只影响「是否已经在学校网络上」这一条判断，不影响后面的连接尝试。
            return "";
        }
    }

    /** 请求连接开放网络：Android 10 起提交系统建议，更低的版本走旧接口。 */
    Result connect(String ssid) {
        if (ssid.isEmpty()) return Result.REJECTED;
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return suggest(ssid);
            return legacyConnect(ssid);
        } catch (SecurityException ex) {
            return Result.DENIED;
        } catch (RuntimeException ex) {
            return Result.REJECTED;
        }
    }

    /** API 29+：只提交建议，重复提交同一网络由系统去重，不会堆积配置。 */
    private Result suggest(String ssid) {
        if (wifi == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return Result.UNSUPPORTED;
        WifiNetworkSuggestion suggestion = new WifiNetworkSuggestion.Builder().setSsid(ssid).build();
        int status = wifi.addNetworkSuggestions(Collections.singletonList(suggestion));
        return status == WifiManager.STATUS_NETWORK_SUGGESTIONS_SUCCESS ? Result.REQUESTED : Result.REJECTED;
    }

    /**
     * API 26–28：补一条开放网络配置并请求连接。这条路径在 API 29 已弃用，只服务旧系统。
     * 刻意不调用 getConfiguredNetworks()：它同样需要定位权限，而本应用不申请定位。
     */
    @SuppressWarnings("deprecation")
    private Result legacyConnect(String ssid) {
        if (wifi == null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return Result.UNSUPPORTED;
        WifiConfiguration configuration = new WifiConfiguration();
        configuration.SSID = "\"" + ssid + "\"";
        configuration.allowedKeyManagement.set(WifiConfiguration.KeyMgmt.NONE);
        configuration.status = WifiConfiguration.Status.ENABLED;
        int networkId = wifi.addNetwork(configuration);
        // 系统里已有同名配置会返回 -1：不重复建配置，交给系统自己重连，不算失败。
        if (networkId < 0) return Result.REJECTED;
        if (!wifi.enableNetwork(networkId, true)) return Result.REJECTED;
        wifi.reconnect();
        return Result.REQUESTED;
    }
}
