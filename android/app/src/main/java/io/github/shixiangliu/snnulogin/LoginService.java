package io.github.shixiangliu.snnulogin;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkRequest;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import java.io.IOException;
import java.security.GeneralSecurityException;
import java.util.Objects;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import static io.github.shixiangliu.snnulogin.PortalProtocol.*;

/** A single process, foreground service and network worker. UI never performs HTTP. */
public final class LoginService extends Service {
    public static final String CONNECT = "connect", DISCONNECT = "disconnect", PAUSE = "pause",
        RESUME = "resume", REFRESH = "refresh", INSPECT = "inspect", STOP = "stop", START = "start", UPDATE_ACCOUNT = "update-account";
    static final String CHANNEL = "campus-connection";
    private static final int NOTICE = 1;
    private static final String POLL = "poll";
    private static volatile LoginService instance;
    private static final AtomicLong updateIds = new AtomicLong();
    private static final AtomicReference<AccountDraft> accountDraft = new AtomicReference<>();
    private static final class AccountDraft {
        final long id = updateIds.incrementAndGet();
        final String account, password, operator;
        final PortalClient.Status consent;
        AccountDraft(String account, String password, String operator, PortalClient.Status consent) {
            this.account = account; this.password = password; this.operator = operator; this.consent = consent;
        }
    }
    private final Handler main = new Handler(Looper.getMainLooper());
    private final RunGate gate = new RunGate();
    private final ScheduledThreadPoolExecutor worker = ConnectionWorker.get();
    private ScheduledFuture<?> pending;
    private volatile WifiTransport activeTransport;
    private volatile Network network;
    private LinkProperties links;
    private Store store;
    private ConnectivityManager connectivity;
    private boolean registered, foreground, destroyed, screenRegistered, lastValidated = true;
    /** Set by the activity: a visible screen justifies a much shorter poll than a background one. */
    static volatile boolean visibleNow;
    private String lastNoticeKey = "";
    // Worker-only connection history, invalidated on network changes through an epoch.
    private Network historyNetwork;
    private int failures, badInternet;
    private boolean selfLogin;
    private long lastInternetCheck;

    public static boolean running() { return instance != null; }
    static void setVisible(boolean visible) {
        visibleNow = visible;
        LoginService service = instance;
        if (service != null) service.main.post(service::scheduleRefresh);
    }
    static void inspectWhenIdle() {
        LoginService service = instance;
        if (service != null && service.foreground && !service.store.snapshot.busy && accountDraft.get() == null
                && service.activeTransport == null && (service.pending == null || service.pending.getDelay(TimeUnit.MILLISECONDS) > 0)) {
            service.invalidate(); service.queue(0, INSPECT);
        }
    }
    public static void cancelCurrent() {
        LoginService service = instance;
        if (service != null) service.invalidate();
    }
    public static boolean send(Context context, String action) {
        accountDraft.set(null);
        return start(context, new Intent(context, LoginService.class).setAction(action));
    }
    public static boolean updateAccount(Context context, String account, String password, String operator, PortalClient.Status consent) {
        AccountDraft draft = new AccountDraft(account, password, operator, consent);
        accountDraft.set(draft);
        // Only a request ID crosses Binder. Password drafts remain in-process and are consumed once.
        boolean started = start(context, new Intent(context, LoginService.class).setAction(UPDATE_ACCOUNT).putExtra("update-id", draft.id));
        if (!started) accountDraft.compareAndSet(draft, null);
        return started;
    }
    private static boolean start(Context context, Intent intent) {
        StatusInspector.cancelActive();
        try {
            context.startForegroundService(intent);
            return true;
        } catch (RuntimeException ex) {
            Store store = Store.get(context);
            store.snapshot = new Store.Snapshot(PortalClient.Status.unreachable(), "后台启动受限",
                "请打开应用后点击连接，并检查系统后台运行设置", "Wi-Fi", false, running(), false);
            store.log("系统暂不允许启动前台服务");
            return false;
        }
    }
    /** Waking the phone is a strong hint that the user wants the state to be current. */
    private final BroadcastReceiver screenOn = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) { scheduleRefresh(); }
    };
    @Override public void onCreate() {
        super.onCreate();
        getSystemService(NotificationManager.class).createNotificationChannel(
            new NotificationChannel(CHANNEL, "校园网连接", NotificationManager.IMPORTANCE_LOW));
        // startForegroundService() gives us five seconds; enter the foreground before any disk or Keystore work.
        enterForeground(null);
        StatusInspector.cancelActive();
        store = Store.get(this);
        connectivity = getSystemService(ConnectivityManager.class);
        worker.setRemoveOnCancelPolicy(true);
        worker.setExecuteExistingDelayedTasksAfterShutdownPolicy(false);
        instance = this;
        chooseNetwork(null);
        links = network == null ? null : connectivity.getLinkProperties(network);
        try {
            connectivity.registerNetworkCallback(new NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build(), callback);
            registered = true;
        } catch (RuntimeException ex) { store.log("网络事件监听不可用，将按周期检查"); }
        try {
            IntentFilter filter = new IntentFilter(Intent.ACTION_SCREEN_ON);
            if (Build.VERSION.SDK_INT >= 33) registerReceiver(screenOn, filter, Context.RECEIVER_NOT_EXPORTED);
            else registerReceiver(screenOn, filter);
            screenRegistered = true;
        } catch (RuntimeException ex) { store.log("亮屏事件监听不可用"); }
        store.log("后台已启动");
        refreshNotification();
    }
    private final ConnectivityManager.NetworkCallback callback = new ConnectivityManager.NetworkCallback() {
        @Override public void onAvailable(Network available) { main.post(() -> networkChanged(available)); }
        @Override public void onLost(Network lost) {
            main.post(() -> {
                if (lost.equals(network)) {
                    invalidate();
                    network = null;
                    links = null;
                    publish(gate.current(), PortalClient.Status.unreachable(), "等待校园网", "请先连接学校 Wi-Fi", false, false);
                    queue(1, POLL);
                }
            });
        }
        @Override public void onLinkPropertiesChanged(Network changed, LinkProperties properties) {
            main.post(() -> {
                if (changed.equals(network) && !Objects.equals(links, properties)) {
                    links = properties;
                    scheduleRefresh();
                }
            });
        }
        @Override public void onCapabilitiesChanged(Network changed, NetworkCapabilities capabilities) {
            main.post(() -> {
                if (network == null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) { networkChanged(changed); return; }
                // Losing or gaining VALIDATED is how Android reports a captive portal appearing or clearing.
                if (changed.equals(network) && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                    boolean validated = capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED);
                    if (validated != lastValidated) { lastValidated = validated; scheduleRefresh(); }
                }
            });
        }
    };
    private void chooseNetwork(Network preferred) {
        if (wifi(preferred)) { network = preferred; return; }
        if (wifi(network)) return;
        network = null;
        for (Network candidate : connectivity.getAllNetworks()) if (wifi(candidate)) { network = candidate; break; }
    }
    private boolean wifi(Network candidate) {
        if (candidate == null) return false;
        NetworkCapabilities capabilities = connectivity.getNetworkCapabilities(candidate);
        return capabilities != null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
            && !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN);
    }
    private void networkChanged(Network preferred) {
        if (destroyed) return;
        Network before = network;
        if (preferred == null) network = null;
        chooseNetwork(preferred);
        if (!Objects.equals(before, network)) {
            links = network == null ? null : connectivity.getLinkProperties(network);
            invalidate();
            publish(gate.current(), PortalClient.Status.unreachable(), network == null ? "等待校园网" : "正在检查连接",
                network == null ? "请先连接学校 Wi-Fi" : "正在识别当前网络", network != null, false);
            queue(1, POLL);
        }
    }
    private void scheduleRefresh() {
        if (!destroyed && foreground) { invalidate(); queue(1, POLL); }
    }
    private void invalidate() {
        gate.invalidate();
        if (pending != null) pending.cancel(false);
        WifiTransport transport = activeTransport;
        if (transport != null) transport.cancel();
    }
    /** Minimal notification first, real content later; never let the five-second deadline expire. */
    private void enterForeground(Notification notice) {
        Notification value = notice;
        if (value == null) {
            value = new Notification.Builder(this, CHANNEL).setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("校园网连接").setContentText("正在启动后台连接").setOngoing(true)
                .setOnlyAlertOnce(true).setCategory(Notification.CATEGORY_SERVICE)
                .setVisibility(Notification.VISIBILITY_PRIVATE).build();
        }
        try {
            if (Build.VERSION.SDK_INT >= 34) startForeground(NOTICE, value, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
            else startForeground(NOTICE, value);
            foreground = true;
        } catch (RuntimeException ex) {
            if (store != null) store.log("系统不允许此时进入前台运行，请打开应用后重试");
        }
    }
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null || intent.getAction() == null ? START : intent.getAction();
        if (!foreground) {
            enterForeground(notification(store.snapshot));
            if (!foreground) { stopSelf(); return START_NOT_STICKY; }
        }
        AccountDraft draft = null;
        if (UPDATE_ACCOUNT.equals(action)) {
            AccountDraft latest = accountDraft.get();
            if (latest == null || latest.id != intent.getLongExtra("update-id", -1) || !accountDraft.compareAndSet(latest, null)) {
                // An interrupted process or a newer command invalidates the old draft.
                if (pending == null) queue(0, REFRESH);
                return START_STICKY;
            }
            draft = latest;
        }
        invalidate();
        try {
            if (CONNECT.equals(action) || RESUME.equals(action)) {
                store.automatic(true);
                store.pausedUntil(0);
            } else if (DISCONNECT.equals(action) || PAUSE.equals(action) || STOP.equals(action)) {
                // Persist before enqueueing or cancelling: queued logins cannot override a user pause.
                store.automatic(false);
            }
            if (PAUSE.equals(action)) {
                // Turning automatic login off is the user's way of stopping the background: nothing is left to watch.
                publish(gate.current(), store.snapshot.status, "自动连接已暂停", "当前连接保持不变", false, store.snapshot.limited);
                leaveForeground();
                return START_NOT_STICKY;
            }
            if (STOP.equals(action)) {
                store.snapshot = Store.Snapshot.stopped();
                leaveForeground();
                return START_NOT_STICKY;
            }
            if (draft != null) {
                final AccountDraft update = draft;
                long token = gate.current();
                pending = worker.schedule(() -> updateAccount(token, update), 0, TimeUnit.SECONDS);
            } else queue(0, action);
        } catch (IOException ex) {
            store.snapshot = new Store.Snapshot(store.snapshot.status, "设置保存失败", "请检查存储空间后重试", "Wi-Fi", false, true, false);
            refreshNotification();
        }
        return START_STICKY;
    }
    private void queue(long seconds, String action) {
        if (destroyed || worker.isShutdown()) return;
        if (pending != null) pending.cancel(false);
        long token = gate.current();
        pending = worker.schedule(() -> tick(token, action), seconds, TimeUnit.SECONDS);
    }
    private void later(long token, long seconds) {
        main.post(() -> { if (!destroyed && gate.valid(token)) queue(seconds, POLL); });
    }
    private void publish(long token, PortalClient.Status status, String title, String detail, boolean busy, boolean limited) {
        gate.runIfCurrent(token, () -> {
            store.snapshot = new Store.Snapshot(status, title, detail, "Wi-Fi", busy, true, limited);
            main.post(() -> { if (gate.valid(token)) refreshNotification(); });
        });
    }
    private void tick(long token, String action) {
        if (!gate.valid(token)) return;
        long delay = visibleNow ? 5 : 30;
        String password = "";
        PortalClient.Status observed = PortalClient.Status.unreachable();
        try {
            // Callback registration may fail on a device; periodic enumeration is the fallback.
            Network chosen = network;
            if (!wifi(chosen)) {
                historyNetwork = null; selfLogin = false; badInternet = 0;
                main.post(() -> { if (gate.valid(token)) networkChanged(null); });
                publish(token, PortalClient.Status.unreachable(), "等待校园网", "请先连接学校 Wi-Fi", false, false);
                later(token, 60);
                return;
            }
            if (!chosen.equals(historyNetwork)) {
                historyNetwork = chosen; failures = 0; badInternet = 0; selfLogin = false; lastInternetCheck = 0;
            }
            final Network selected = chosen;
            WifiTransport transport = new WifiTransport(chosen, () -> !gate.valid(token) || !selected.equals(network));
            activeTransport = transport;
            if (!gate.valid(token)) { transport.cancel(); return; }
            PortalClient client = new PortalClient(transport, () -> !gate.valid(token), store.approvedHost());
            Store.Account account = store.account();
            boolean requested = !POLL.equals(action) && !START.equals(action) && !PAUSE.equals(action);
            boolean stable = store.snapshot.status.kind == Kind.ONLINE || store.snapshot.status.kind == Kind.OFFLINE;
            if (UiModel.showProbeProgress(requested, stable) && !POLL.equals(action) && !PAUSE.equals(action)) {
                publish(token, store.snapshot.status, DISCONNECT.equals(action) ? "正在断开校园网" : "正在检查连接", "状态会自动更新", true, false);
            }
            PortalClient.Status status = client.status(store.endpoint());
            observed = status;
            if (!gate.valid(token)) return;
            if (status.endpoint != null) {
                final PortalClient.Status discovered = status;
                if (!gate.runIfCurrent(token, () -> {
                    try { store.endpoint(discovered.endpoint); } catch (IOException ex) { store.log("门户地址缓存保存失败"); }
                })) return;
            }
            if (INSPECT.equals(action)) {
                String title = status.kind == Kind.ONLINE ? "已连接" : status.kind == Kind.OFFLINE ? "未连接" : "等待校园网";
                publish(token, status, title, "", false, false);
            } else if (status.kind == Kind.UNREACHABLE) {
                // Retry soon: an unreachable portal is usually a captive-portal hiccup, not a reason to wait a minute.
                publish(token, status, "等待校园网", "暂未检测到学校认证门户，正在自动重试", false, false);
                delay = 20;
            } else if (DISCONNECT.equals(action)) {
                status = client.logout(status.endpoint, account.name, account.operator);
                observed = status;
                selfLogin = false;
                publish(token, status, "已断开", "自动登录已关闭，Wi-Fi 保持连接", false, false);
                safeLog(token, "已主动断开校园网认证");
                main.post(() -> {
                    if (!gate.valid(token)) return;
                    leaveForeground();
                });
            } else if (status.kind == Kind.ONLINE) {
                failures = 0;
                if (!status.session.matches(account.name, account.operator)) {
                    selfLogin = false;
                    publish(token, status, "当前使用其他账号或上网方式", "保留当前连接，请核对账号设置", false, false);
                } else {
                    if (store.pausedUntil() > 0) gate.runIfCurrent(token, () -> {
                        try { store.pausedUntil(0); } catch (IOException ignored) { }
                    });
                    long now = System.currentTimeMillis();
                    // Connection maintenance belongs to automatic login. A user pause stops outside probes.
                    boolean checkInternet = ConnectionPolicy.shouldProbeInternet(store.automatic(), now, lastInternetCheck);
                    if (checkInternet) {
                        boolean ok = client.internetOk();
                        if (!gate.valid(token)) return;
                        lastInternetCheck = now;
                        badInternet = ok ? 0 : Math.min(badInternet + 1, 10);
                    }
                    if (ConnectionPolicy.mayRepair(store.automatic(), store.pausedUntil(), now, store.lastRepair(),
                            badInternet, selfLogin, status.session, account.name, account.operator)) {
                        PortalClient.Status confirmed = client.statusAt(status.endpoint, 4000);
                        if (confirmed.kind == Kind.ONLINE && confirmed.session.matches(account.name, account.operator)) {
                            final boolean[] saved = {false};
                            gate.runIfCurrent(token, () -> {
                                try { store.lastRepair(now); saved[0] = true; } catch (IOException ignored) { }
                            });
                            if (saved[0]) {
                                publish(token, status, "正在修复连接", "校园网已认证，但外网暂不可用", true, true);
                                password = store.password();
                                observed = client.logout(status.endpoint, account.name, account.operator);
                                status = client.login(status.endpoint, account.name, password, account.operator);
                                if (!gate.valid(token)) return;
                                selfLogin = client.authenticatedHere;
                                badInternet = 0;
                                safeLog(token, "校园网已重新认证");
                            }
                        }
                    }
                    publish(token, status, badInternet >= 2 ? "已连接，但暂时无法上网" : "已连接",
                        !store.automatic() ? "自动登录已关闭，当前连接保持不变" : "自动登录已开启",
                        false, badInternet >= 2);
                }
            } else if (!account.configured) {
                publish(token, status, "未连接", "请先设置校园网账号", false, false);
            } else if (!store.automatic()) {
                publish(token, status, "未连接", "自动登录已关闭", false, false);
            } else if (!ConnectionPolicy.mayLogin(true, store.pausedUntil(), System.currentTimeMillis())) {
                String reason = UiModel.isProblem(store.snapshot.title) ? store.snapshot.detail : "请先处理账号问题，再重新连接";
                publish(token, status, "自动重试暂缓", reason, false, false);
            } else if (!client.approved(status.endpoint)) {
                publish(token, status, "发现新的认证地址", "请在网络信息中核对并确认新地址", false, false);
                delay = 60;
            } else {
                publish(token, status, "正在连接", "正在提交校园网认证", true, false);
                password = store.password();
                status = client.login(status.endpoint, account.name, password, account.operator);
                if (!gate.valid(token)) return;
                selfLogin = client.authenticatedHere;
                failures = 0; badInternet = 0; lastInternetCheck = 0;
                gate.runIfCurrent(token, () -> { try { store.pausedUntil(0); } catch (IOException ignored) { } });
                if (status.session != null && status.session.matches(account.name, account.operator)) {
                    publish(token, status, "已连接", "自动登录已开启", false, false);
                    safeLog(token, "校园网认证成功");
                } else {
                    publish(token, status, "当前使用其他账号或上网方式", "保留当前连接，请核对账号设置", false, false);
                }
            }
        } catch (GeneralSecurityException ex) {
            gate.runIfCurrent(token, () -> { try { store.pauseFor(Failure.CREDENTIAL); } catch (IOException ignored) { } });
            publish(token, observed, "请重新输入密码", "保存的密码无法解密，请打开账号设置", false, false);
            safeLog(token, "密码密钥不可用，等待重新设置账号");
            delay = 60;
        } catch (IOException | RuntimeException ex) {
            if (!gate.valid(token)) return;
            String message = ex.getMessage() == null ? "网络请求失败，请稍后重试" : ex.getMessage();
            // Socket exceptions can contain addresses, but no submitted passwords or raw forms are logged.
            message = redact(message, store.account().name, password);
            if (message.length() > 180) message = "网络请求失败，请稍后重试";
            failures++;
            long retry = nextDelaySeconds(failure(message), failures);
            if (retry < 0) {
                final Failure cause = failure(message);
                gate.runIfCurrent(token, () -> { try { store.pauseFor(cause); } catch (IOException ignored) { } });
                delay = 60;
            } else delay = retry;
            publish(token, observed, "连接需要处理", message, false, false);
            safeLog(token, "连接未完成：" + failure(message).name());
        } finally {
            activeTransport = null;
        }
        later(token, delay);
    }
    private void safeLog(long token, String message) { gate.runIfCurrent(token, () -> store.log(message)); }
    private void updateAccount(long token, AccountDraft draft) {
        String password = "";
        final PortalClient.Status[] observed = {PortalClient.Status.unreachable()};
        try {
            if (!gate.valid(token)) return;
            store.validateAccount(draft.account, draft.password, draft.operator);
            password = draft.password.isEmpty() ? store.password() : draft.password;
            publish(token, store.snapshot.status, "正在保存并连接", "新设置验证成功后才替换原配置", true, false);
            Network selected = network;
            PortalClient client = null;
            if (wifi(selected)) {
                WifiTransport transport = new WifiTransport(selected, () -> !gate.valid(token) || !selected.equals(network));
                activeTransport = transport;
                client = new PortalClient(transport, () -> !gate.valid(token), store.approvedHost());
                observed[0] = client.status(store.endpoint());
            }
            final String secret = password;
            PortalClient.Status result = AccountUpdate.apply(client, observed[0], draft.consent,
                draft.account, password, draft.operator, () -> !gate.valid(token), () -> {
                    synchronized (gate) {
                        if (!gate.valid(token)) throw new java.io.InterruptedIOException("操作已取消");
                        store.saveAndEnable(draft.account, secret, draft.operator);
                    }
                }, status -> observed[0] = status);
            if (!gate.valid(token)) return;
            selfLogin = client != null && client.authenticatedHere;
            historyNetwork = selected; failures = 0; badInternet = 0; lastInternetCheck = 0;
            if (result.endpoint != null) gate.runIfCurrent(token, () -> {
                try { store.endpoint(result.endpoint); } catch (IOException ignored) { store.log("门户地址缓存保存失败"); }
            });
            publish(token, result, result.kind == Kind.ONLINE ? "已连接" : "账号已保存",
                result.kind == Kind.ONLINE ? "新设置已保存，自动登录已开启" : "等待学校 Wi-Fi，连接后会自动认证", false, false);
            safeLog(token, "账号设置已更新");
        } catch (GeneralSecurityException ex) {
            publish(token, observed[0], "账号更新未完成", "密码密钥不可用，请重新输入密码", false, false);
        } catch (IOException | RuntimeException ex) {
            if (!gate.valid(token)) return;
            String message = ex.getMessage() == null ? "账号更新未完成，请重试" : ex.getMessage();
            message = redact(redact(message, draft.account, password), store.account().name, "");
            publish(token, observed[0], "账号更新未完成", message, false, false);
            safeLog(token, "账号更新未完成；等待用户处理");
        } finally { activeTransport = null; }
        later(token, 30);
    }
    private PendingIntent operation(String action, int code) {
        return PendingIntent.getForegroundService(this, code, new Intent(this, LoginService.class).setAction(action),
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }
    private Notification notification(Store.Snapshot snapshot) {
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class)
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        UiModel.View view = store.presentation(wifi(network));
        PendingIntent primary;
        switch (view.action) {
            case CONNECT: primary = operation(CONNECT, 1); break;
            case DISCONNECT: primary = operation(DISCONNECT, 1); break;
            case CANCEL: primary = operation(PAUSE, 1); break;
              case CHECK: primary = operation(INSPECT, 1); break;
            default:
                String page = view.action == UiModel.Action.SETUP || view.action == UiModel.Action.ACCOUNT ? "account"
                    : view.action == UiModel.Action.DETAILS ? "network" : view.action == UiModel.Action.HELP
                        ? (snapshot.title.equals("后台启动受限") ? "background" : "help") : "home";
                primary = PendingIntent.getActivity(this, 3, new Intent(this, MainActivity.class).putExtra("page", page)
                    .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP),
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        }
        Notification.Builder notice = new Notification.Builder(this, CHANNEL).setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(view.title).setContentText(view.detail).setContentIntent(open)
            .setOngoing(true).setOnlyAlertOnce(true).setCategory(Notification.CATEGORY_SERVICE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .addAction(new Notification.Action.Builder(null, view.button, primary).build());
        if (store.account().configured && view.action != UiModel.Action.CANCEL) {
            notice.addAction(new Notification.Action.Builder(null, store.automatic() ? "暂停自动连接" : "恢复自动连接",
                operation(store.automatic() ? PAUSE : RESUME, 2)).build());
        }
        return notice.addAction(new Notification.Action.Builder(null, "打开", open).build()).build();
    }
    /** Leave the foreground, drop the notification and stop; a queued refresh must not re-post it. */
    private void leaveForeground() {
        foreground = false;
        stopForeground(STOP_FOREGROUND_REMOVE);
        getSystemService(NotificationManager.class).cancel(NOTICE);
        stopSelf();
    }
    private void refreshNotification() {
        if (destroyed || !foreground) return;
        UiModel.View view = store.presentation(wifi(network));
        String key = view.title + '\n' + view.detail + '\n' + view.button + '\n' + store.automatic();
        if (key.equals(lastNoticeKey)) return;
        lastNoticeKey = key;
        getSystemService(NotificationManager.class).notify(NOTICE, notification(store.snapshot));
    }
    @Override public void onDestroy() {
        destroyed = true;
        invalidate();
        if (registered) {
            try { connectivity.unregisterNetworkCallback(callback); } catch (RuntimeException ignored) { }
        }
        if (screenRegistered) {
            try { unregisterReceiver(screenOn); } catch (RuntimeException ignored) { }
        }
        accountDraft.set(null);
        getSystemService(NotificationManager.class).cancel(NOTICE);
        main.removeCallbacksAndMessages(null);
        if (instance == this) instance = null;
        store.snapshot = Store.Snapshot.stopped();
        store.log("后台已停止，保留当前网络认证");
        super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }
}
