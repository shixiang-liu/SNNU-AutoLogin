package io.github.shixiangliu.snnulogin;

import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BooleanSupplier;
import org.junit.Test;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/**
 * 与 Windows 版 src/windows_wifi.rs 的 decide() 测试逐条对照。全部事实都是打桩闭包，
 * 不触碰任何真实 Wi-Fi 或网络：这个测试类只验证纯决策逻辑。
 */
public class WifiSelfHealTest {
    private static final String SSID = WifiSelfHeal.DEFAULT_SSID;
    private static final BooleanSupplier NO = () -> false;
    private static final BooleanSupplier YES = () -> true;

    @Test public void allEightConditionCombinationsOnlyConnectWhenEveryConditionFails() {
        for (int mask = 0; mask < 8; mask++) {
            final boolean internetUp = (mask & 4) != 0, portalOk = (mask & 2) != 0, onTarget = (mask & 1) != 0;
            WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, () -> internetUp, () -> portalOk, () -> onTarget);
            String label = "internet=" + internetUp + " portal=" + portalOk + " onTarget=" + onTarget;
            if (internetUp || portalOk || onTarget) {
                assertFalse(label, plan.connects());
                WifiSelfHeal.Skip expected = internetUp ? WifiSelfHeal.Skip.INTERNET_UP
                    : portalOk ? WifiSelfHeal.Skip.PORTAL_OK : WifiSelfHeal.Skip.ON_TARGET_SSID;
                assertEquals(label, expected, plan.skip);
            } else {
                assertTrue(label, plan.connects());
                assertNull(label, plan.skip);
            }
        }
    }

    @Test public void everyBlockedConditionHasItsOwnSkipReason() {
        assertEquals(WifiSelfHeal.Skip.INTERNET_UP, WifiSelfHeal.decide(true, YES, NO, NO).skip);
        assertEquals(WifiSelfHeal.Skip.PORTAL_OK, WifiSelfHeal.decide(true, NO, YES, NO).skip);
        assertEquals(WifiSelfHeal.Skip.ON_TARGET_SSID, WifiSelfHeal.decide(true, NO, NO, YES).skip);
        assertEquals(WifiSelfHeal.Skip.DISABLED, WifiSelfHeal.decide(false, NO, NO, NO).skip);
        // 总开关优先于三条条件：关掉自动登录后，三个条件同时成立也不动作。
        assertEquals(WifiSelfHeal.Skip.DISABLED, WifiSelfHeal.decide(false, YES, YES, YES).skip);
        // 手机没有有线链路可判定，因此比 Windows 版少一个 Wired。
        assertEquals(4, WifiSelfHeal.Skip.values().length);
    }

    @Test public void internetUpSkipsWithoutProbingTheRest() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES,
            () -> { probes.incrementAndGet(); return false; },
            () -> { probes.incrementAndGet(); return false; });
        assertEquals(WifiSelfHeal.Skip.INTERNET_UP, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void portalReachableSkipsWithoutReadingTheWifiName() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, NO, YES,
            () -> { probes.incrementAndGet(); return false; });
        assertEquals(WifiSelfHeal.Skip.PORTAL_OK, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void alreadyOnTheCampusWifiIsNotHealedAgain() {
        AtomicInteger reads = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, NO, NO,
            () -> { reads.incrementAndGet(); return true; });
        assertEquals(WifiSelfHeal.Skip.ON_TARGET_SSID, plan.skip);
        assertEquals(1, reads.get());
    }

    @Test public void disabledSwitchSkipsWithoutAnyProbe() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(false, YES, YES,
            () -> { probes.incrementAndGet(); return true; });
        assertEquals(WifiSelfHeal.Skip.DISABLED, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void noInternetAndNoPortalAndAnotherNetworkConnects() {
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, NO, NO, NO);
        assertTrue(plan.connects());
        assertEquals(WifiSelfHeal.Decision.CONNECT, plan.decision);
    }

    @Test public void campusSsidComparisonStripsSystemQuotesOnly() {
        assertTrue(WifiSelfHeal.matchesSsid("\"SNNU\"", SSID));
        assertTrue(WifiSelfHeal.matchesSsid("SNNU", SSID));
        assertTrue(WifiSelfHeal.matchesSsid("  \"SNNU\"  ", SSID));
        assertTrue(WifiSelfHeal.matchesSsid(SSID, SSID));
        // 精确匹配：大小写不同、带后缀或前缀的都不是这一张网。
        assertFalse(WifiSelfHeal.matchesSsid("snnu", SSID));
        assertFalse(WifiSelfHeal.matchesSsid("SNNU-2", SSID));
        assertFalse(WifiSelfHeal.matchesSsid("SNNU-Student", SSID));
        assertFalse(WifiSelfHeal.matchesSsid("eduroam", SSID));
        assertFalse(WifiSelfHeal.matchesSsid("[SNNU]", SSID));
    }

    @Test public void unknownSsidCountsAsNotOnTheCampusWifi() {
        assertFalse(WifiSelfHeal.matchesSsid(null, SSID));
        assertFalse(WifiSelfHeal.matchesSsid("", SSID));
        assertFalse(WifiSelfHeal.matchesSsid("   ", SSID));
        assertFalse(WifiSelfHeal.matchesSsid(WifiSelfHeal.UNKNOWN_SSID, SSID));
        assertFalse(WifiSelfHeal.matchesSsid("\"<unknown ssid>\"", SSID));
        // 没有定位权限就读不到名称：此时仍允许尝试连接，与 Windows 版读不到 SSID 时一致。
        assertTrue(WifiSelfHeal.decide(true, NO, NO,
            () -> WifiSelfHeal.matchesSsid(WifiSelfHeal.UNKNOWN_SSID, SSID)).connects());
    }

    @Test public void connectRequestsAreRateLimitedByTheCooldown() {
        assertFalse(WifiSelfHeal.cooling(10_000L, 0L));
        assertTrue(WifiSelfHeal.cooling(10_000L, 10_000L));
        assertTrue(WifiSelfHeal.cooling(10_000L + WifiSelfHeal.CONNECT_COOLDOWN_MS - 1, 10_000L));
        assertFalse(WifiSelfHeal.cooling(10_000L + WifiSelfHeal.CONNECT_COOLDOWN_MS, 10_000L));
    }

    @Test public void theToolServesOneFixedCampusNetwork() {
        assertEquals("SNNU", WifiSelfHeal.DEFAULT_SSID);
        assertEquals("<unknown ssid>", WifiSelfHeal.UNKNOWN_SSID);
    }
}
