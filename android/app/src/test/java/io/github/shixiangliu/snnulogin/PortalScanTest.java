package io.github.shixiangliu.snnulogin;

import org.junit.Test;
import java.io.IOException;
import java.io.InterruptedIOException;
import java.nio.charset.StandardCharsets;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import static org.junit.Assert.*;
import static io.github.shixiangliu.snnulogin.PortalProtocol.*;

public class PortalScanTest {
    private static final class BlockingTransport implements PortalClient.Transport {
        final CountDownLatch firstStarted = new CountDownLatch(1), released = new CountDownLatch(1);
        final CountDownLatch firstFinished = new CountDownLatch(1);
        final AtomicInteger cancellations = new AtomicInteger();
        final boolean fastOnline;
        BlockingTransport(boolean fastOnline) { this.fastOnline = fastOnline; }
        @Override public PortalClient.Response request(String method, String url, Map<String, String> headers,
                byte[] body, int timeout) throws IOException {
            if (!url.contains("userstatus.jsp")) throw new IOException("No discovery redirect");
            boolean first = url.contains(":8601/");
            if (first) firstStarted.countDown();
            try {
                if (fastOnline && url.contains(":8602/")) {
                    if (!firstStarted.await(3, TimeUnit.SECONDS)) throw new IOException("First scan never started");
                    return new PortalClient.Response(200, Collections.emptyMap(),
                        "当前登录账号：20241234@mobile".getBytes(StandardCharsets.UTF_8));
                }
                if (!released.await(5, TimeUnit.SECONDS)) throw new IOException("Test request timed out");
                throw new IOException("Request closed");
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                throw new InterruptedIOException("Request interrupted");
            } finally {
                if (first) firstFinished.countDown();
            }
        }
        @Override public void cancelRequests() {
            cancellations.incrementAndGet();
            released.countDown();
        }
    }

    @Test public void completedOnlinePortWinsOverBlockedFirstPortAndCleansUp() throws Exception {
        BlockingTransport transport = new BlockingTransport(true);
        try {
            PortalClient.Status status = new PortalClient(transport, () -> false, "").status(null);
            assertEquals(Kind.ONLINE, status.kind);
            assertEquals(8602, status.endpoint.port);
            assertEquals(1, transport.cancellations.get());
            assertTrue("old scan must stop", transport.firstFinished.await(2, TimeUnit.SECONDS));
        } finally { transport.cancelRequests(); }
    }

    @Test public void userCancellationClosesScanRequestsAndDoesNotPublishOldStatus() throws Exception {
        BlockingTransport transport = new BlockingTransport(false);
        AtomicBoolean cancelled = new AtomicBoolean();
        AtomicReference<Throwable> outcome = new AtomicReference<>();
        Thread caller = new Thread(() -> {
            try { new PortalClient(transport, cancelled::get, "").status(null); }
            catch (Throwable ex) { outcome.set(ex); }
        });
        try {
            caller.start();
            assertTrue(transport.firstStarted.await(3, TimeUnit.SECONDS));
            cancelled.set(true);
            caller.join(3000);
            assertFalse("cancelled scan must return", caller.isAlive());
            assertTrue(outcome.get() instanceof InterruptedIOException);
            assertEquals(1, transport.cancellations.get());
            assertTrue(transport.firstFinished.await(2, TimeUnit.SECONDS));
        } finally {
            transport.cancelRequests(); caller.interrupt(); caller.join(3000);
        }
    }

    @Test public void scanDeadlineClosesOutstandingRequests() throws Exception {
        BlockingTransport transport = new BlockingTransport(false);
        try {
            assertEquals(Kind.UNREACHABLE, new PortalClient(transport, () -> false, "").status(null).kind);
            assertEquals(1, transport.cancellations.get());
            assertTrue(transport.firstFinished.await(2, TimeUnit.SECONDS));
        } finally { transport.cancelRequests(); }
    }
}
