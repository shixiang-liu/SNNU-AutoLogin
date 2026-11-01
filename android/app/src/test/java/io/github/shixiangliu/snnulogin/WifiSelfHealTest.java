package io.github.shixiangliu.snnulogin;

import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BooleanSupplier;
import org.junit.Test;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/**
 * 与 Windows 版 src/wifi_heal_core.rs 的 decide() / internet_gate() 测试逐条对照，外加手机特有的
 * 「另一个出口（蜂窝）可用」这一维。全部事实都是打桩闭包，不触碰任何真实 Wi-Fi 或网络：
 * 这个测试类只验证纯决策逻辑。
 */
public class WifiSelfHealTest {
    private static final String SSID = WifiSelfHeal.DEFAULT_SSID;
    private static final BooleanSupplier NO = () -> false;
    private static final BooleanSupplier YES = () -> true;

    @Test public void allEightConditionCombinationsOnlyConnectWhenEveryConditionFails() {
        for (int mask = 0; mask < 8; mask++) {
            final boolean internetUp = (mask & 4) != 0, portalOk = (mask & 2) != 0, onTarget = (mask & 1) != 0;
            WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES, () -> internetUp, () -> portalOk, () -> onTarget, NO);
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
        assertEquals(WifiSelfHeal.Skip.INTERNET_UP, WifiSelfHeal.decide(true, YES, YES, NO, NO, NO).skip);
        assertEquals(WifiSelfHeal.Skip.PORTAL_OK, WifiSelfHeal.decide(true, YES, NO, YES, NO, NO).skip);
        assertEquals(WifiSelfHeal.Skip.ON_TARGET_SSID, WifiSelfHeal.decide(true, YES, NO, NO, YES, NO).skip);
        assertEquals(WifiSelfHeal.Skip.DISABLED, WifiSelfHeal.decide(false, YES, NO, NO, NO, NO).skip);
        assertEquals(WifiSelfHeal.Skip.MOBILE_NETWORK, WifiSelfHeal.decide(true, NO, NO, NO, NO, YES).skip);
        // 总开关优先于所有条件：关掉自动登录后，任何组合都不动作。
        assertEquals(WifiSelfHeal.Skip.DISABLED, WifiSelfHeal.decide(false, YES, YES, YES, YES, YES).skip);
        assertEquals(WifiSelfHeal.Skip.DISABLED, WifiSelfHeal.decide(false, NO, NO, NO, NO, YES).skip);
        // 手机没有有线链路可判定，因此比 Windows 版少一个 Wired，多一个 MOBILE_NETWORK。
        assertEquals(5, WifiSelfHeal.Skip.values().length);
    }

    @Test public void internetUpSkipsWithoutProbingTheRest() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES, YES,
            () -> { probes.incrementAndGet(); return false; },
            () -> { probes.incrementAndGet(); return false; },
            () -> { probes.incrementAndGet(); return false; });
        assertEquals(WifiSelfHeal.Skip.INTERNET_UP, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void portalReachableSkipsWithoutReadingTheWifiName() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES, NO, YES,
            () -> { probes.incrementAndGet(); return false; },
            () -> { probes.incrementAndGet(); return false; });
        assertEquals(WifiSelfHeal.Skip.PORTAL_OK, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void alreadyOnTheCampusWifiIsNotHealedAgain() {
        AtomicInteger reads = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES, NO, NO,
            () -> { reads.incrementAndGet(); return true; }, NO);
        assertEquals(WifiSelfHeal.Skip.ON_TARGET_SSID, plan.skip);
        assertEquals(1, reads.get());
    }

    @Test public void disabledSwitchSkipsWithoutAnyProbe() {
        AtomicInteger probes = new AtomicInteger();
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(false, YES, YES,
            () -> { probes.incrementAndGet(); return true; },
            () -> { probes.incrementAndGet(); return true; },
            () -> { probes.incrementAndGet(); return true; });
        assertEquals(WifiSelfHeal.Skip.DISABLED, plan.skip);
        assertEquals(0, probes.get());
    }

    @Test public void noInternetAndNoPortalAndAnotherNetworkConnects() {
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, YES, NO, NO, NO, NO);
        assertTrue(plan.connects());
        assertEquals(WifiSelfHeal.Decision.CONNECT, plan.decision);
    }

    // ---- 没有任何无线关联时（onWifi=false）：手机自己的出口说了算 ----

    @Test public void noWifiAssociationButMobileDataIsUpIsLeftAlone() {
        // 用户正靠流量上网：绝不为了连 SNNU 去动 Wi-Fi。
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, NO, NO, NO, NO, YES);
        assertFalse(plan.connects());
        assertEquals(WifiSelfHeal.Skip.MOBILE_NETWORK, plan.skip);
    }

    @Test public void noWifiAssociationAndNoOtherEgressConnectsBack() {
        // 既没有无线关联、也没有蜂窝：连接的网络确实不可用，必须尝试连回学校网络。
        WifiSelfHeal.Plan plan = WifiSelfHeal.decide(true, NO, NO, NO, NO, NO);
        assertTrue(plan.connects());
        assertEquals(WifiSelfHeal.Decision.CONNECT, plan.decision);
    }

    @Test public void withoutWifiAssociationTheWifiSideFactsAreNeverProbed() {
        // 没有无线出口就没有可探测的「这张 Wi-Fi 的外网/门户」，两条供应商一个都不该被调用。
        AtomicInteger probes = new AtomicInteger();
        BooleanSupplier counted = () -> { probes.incrementAndGet(); return true; };
        WifiSelfHeal.Plan connects = WifiSelfHeal.decide(true, NO, counted, counted, counted, NO);
        assertTrue(connects.connects());
        assertEquals(0, probes.get());
        WifiSelfHeal.Plan mobile = WifiSelfHeal.decide(true, NO, counted, counted, counted, YES);
        assertEquals(WifiSelfHeal.Skip.MOBILE_NETWORK, mobile.skip);
        assertEquals(0, probes.get());
    }

    @Test public void otherEgressOnlyMattersWhenThereIsNoWifiAssociation() {
        // 连着 Wi-Fi 时，蜂窝是否可用不改变结论：判断只看这张 Wi-Fi 自己的探测结果。
        for (int mask = 0; mask < 8; mask++) {
            final boolean internetUp = (mask & 4) != 0, portalOk = (mask & 2) != 0, onTarget = (mask & 1) != 0;
            WifiSelfHeal.Plan withMobile = WifiSelfHeal.decide(true, YES, () -> internetUp, () -> portalOk, () -> onTarget, YES);
            WifiSelfHeal.Plan withoutMobile = WifiSelfHeal.decide(true, YES, () -> internetUp, () -> portalOk, () -> onTarget, NO);
            assertEquals(withoutMobile.decision, withMobile.decision);
            assertEquals(withoutMobile.skip, withMobile.skip);
        }
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
        assertTrue(WifiSelfHeal.decide(true, YES, NO, NO,
            () -> WifiSelfHeal.matchesSsid(WifiSelfHeal.UNKNOWN_SSID, SSID), NO).connects());
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
