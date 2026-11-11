package io.github.shixiangliu.snnulogin;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.util.Locale;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

final class Store {
    private static Store instance;
    static synchronized Store get(Context context) {
        if (instance == null) instance = new Store(context.getApplicationContext());
        return instance;
    }
    private static final String ALIAS = "SNNU-AutoLogin/password/v1";
    private final SharedPreferences preferences;
    private final File directory;
    volatile Snapshot snapshot = Snapshot.stopped();

    static final class Account {
        final String name, operator;
        final boolean configured;
        Account(String name, String operator, boolean configured) {
            this.name = name; this.operator = operator; this.configured = configured;
        }
    }
    static final class Snapshot {
        final PortalClient.Status status;
        final String title, detail, network;
        final boolean busy, running, limited, inspecting;
        Snapshot(PortalClient.Status status, String title, String detail, String network,
                 boolean busy, boolean running, boolean limited) {
            this(status, title, detail, network, busy, running, limited, false);
        }
        private Snapshot(PortalClient.Status status, String title, String detail, String network,
                 boolean busy, boolean running, boolean limited, boolean inspecting) {
            this.status = status; this.title = title; this.detail = detail; this.network = network;
            this.busy = busy; this.running = running; this.limited = limited; this.inspecting = inspecting;
        }
        Snapshot withInspection(boolean value) { return new Snapshot(status, title, detail, network, busy, running, limited, value); }
        static Snapshot stopped() {
            return new Snapshot(PortalClient.Status.unreachable(), "后台未运行", "点击连接以检查校园网", "Wi-Fi", false, false, false);
        }
    }
    private Store(Context context) {
        directory = context.getFilesDir();
        preferences = context.getSharedPreferences("settings", Context.MODE_PRIVATE);
    }
    Account account() {
        return new Account(preferences.getString("account", ""), preferences.getString("operator", ""),
            !preferences.getString("password", "").isEmpty());
    }
    synchronized void saveAccount(String account, String password, String operator) throws GeneralSecurityException, IOException {
        persistAccount(account, password, operator, false);
    }
    synchronized void saveAndEnable(String account, String password, String operator) throws GeneralSecurityException, IOException {
        persistAccount(account, password, operator, true);
    }
    private void persistAccount(String account, String password, String operator, boolean enable) throws GeneralSecurityException, IOException {
        validateAccount(account, password, operator);
        String encrypted = preferences.getString("password", "");
        if (!password.isEmpty()) encrypted = encrypt(password);
        SharedPreferences.Editor editor = preferences.edit().putString("account", account).putString("password", encrypted)
            .putString("operator", operator).putLong("paused_until", 0).remove("pause_reason");
        if (enable) editor.putBoolean("automatic", true);
        commit(editor);
    }
    void validateAccount(String account, String password, String operator) throws IOException {
        Account saved = account();
        AccountUpdate.Validation validation = AccountUpdate.validate(account, password, operator, saved.name, saved.configured);
        if (validation != null) throw new IOException(validation.message);
    }
    UiModel.View presentation(boolean wifi) {
        return presentation(wifi, false);
    }
    UiModel.View presentation(boolean wifi, boolean validated) {
        Snapshot current = snapshot;
        Account account = account();
        UiModel.Input input = new UiModel.Input();
        input.status = current.status; input.configured = account.configured; input.account = account.name; input.operator = account.operator;
        input.automatic = automatic(); input.running = current.running; input.wifi = wifi; input.wifiValidated = wifi && validated; input.busy = current.busy;
        input.limited = current.limited; input.stage = current.title; input.message = current.detail; input.inspecting = current.inspecting;
        input.pausedUntil = pausedUntil(); input.now = System.currentTimeMillis();
        input.blockedBy = pauseReason();
        input.portalApproved = current.status.endpoint != null && (PortalProtocol.trustedHost(current.status.endpoint.host)
            || current.status.endpoint.host.equalsIgnoreCase(approvedHost()));
        return UiModel.present(input);
    }
    synchronized void forget() throws IOException {
        commit(preferences.edit().remove("account").remove("password").remove("operator")
            .putBoolean("automatic", false).putLong("paused_until", 0).remove("pause_reason"));
        try {
            KeyStore keyStore = KeyStore.getInstance("AndroidKeyStore"); keyStore.load(null);
            keyStore.deleteEntry(ALIAS);
        } catch (GeneralSecurityException ex) { /* Ciphertext has already been removed. */ }
    }
    String password() throws GeneralSecurityException, IOException {
        String value = preferences.getString("password", "");
        if (value.isEmpty()) return "";
        try {
            String[] parts = value.split(":", 2);
            if (parts.length != 2) throw new GeneralSecurityException("密码格式无效");
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, key(false), new GCMParameterSpec(128, Base64.decode(parts[0], Base64.NO_WRAP)));
            cipher.updateAAD(ALIAS.getBytes(StandardCharsets.UTF_8));
            return new String(cipher.doFinal(Base64.decode(parts[1], Base64.NO_WRAP)), StandardCharsets.UTF_8);
        } catch (IllegalArgumentException ex) { throw new GeneralSecurityException("保存的密码无法读取，请重新输入", ex); }
    }
    private String encrypt(String plain) throws GeneralSecurityException, IOException {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding"); cipher.init(Cipher.ENCRYPT_MODE, key(true));
        cipher.updateAAD(ALIAS.getBytes(StandardCharsets.UTF_8));
        return Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP) + ":"
            + Base64.encodeToString(cipher.doFinal(plain.getBytes(StandardCharsets.UTF_8)), Base64.NO_WRAP);
    }
    private SecretKey key(boolean create) throws GeneralSecurityException, IOException {
        KeyStore keyStore = KeyStore.getInstance("AndroidKeyStore"); keyStore.load(null);
        java.security.Key existing = keyStore.getKey(ALIAS, null);
        if (existing instanceof SecretKey) return (SecretKey) existing;
        if (!create) throw new GeneralSecurityException("密码密钥不可用，请重新输入密码");
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256).setUserAuthenticationRequired(false).build());
        return generator.generateKey();
    }
    boolean automatic() { return preferences.getBoolean("automatic", false); }
    boolean guardian() { return preferences.getBoolean("guardian", false); }
    void guardian(boolean value) throws IOException { commit(preferences.edit().putBoolean("guardian", value)); }
    boolean notificationAsked() { return preferences.getBoolean("notification_asked", false); }
    void notificationAsked(boolean value) throws IOException { commit(preferences.edit().putBoolean("notification_asked", value)); }
    void automatic(boolean value) throws IOException { commit(preferences.edit().putBoolean("automatic", value)); }
    boolean boot() { return preferences.getBoolean("boot", false); }
    void boot(boolean value) throws IOException { commit(preferences.edit().putBoolean("boot", value)); }
    long pausedUntil() { return preferences.getLong("paused_until", 0); }
    void pausedUntil(long value) throws IOException {
        SharedPreferences.Editor editor = preferences.edit().putLong("paused_until", value);
        if (value == 0) editor.remove("pause_reason");
        commit(editor);
    }
    void pauseFor(PortalProtocol.Failure reason) throws IOException {
        commit(preferences.edit().putLong("paused_until", System.currentTimeMillis() + 1_800_000L).putString("pause_reason", reason.name()));
    }
    PortalProtocol.Failure pauseReason() {
        try { return PortalProtocol.Failure.valueOf(preferences.getString("pause_reason", "OTHER")); }
        catch (IllegalArgumentException ex) { return PortalProtocol.Failure.OTHER; }
    }
    long lastRepair() { return preferences.getLong("last_repair", 0); }
    void lastRepair(long value) throws IOException { commit(preferences.edit().putLong("last_repair", value)); }
    String approvedHost() { return preferences.getString("approved_host", ""); }
    void approve(String host) throws IOException {
        if (!host.matches("[A-Za-z0-9.-]+")) throw new IOException("认证地址无效");
        commit(preferences.edit().putString("approved_host", host));
    }
    PortalProtocol.Endpoint endpoint() {
        String host = preferences.getString("portal_host", "");
        int port = preferences.getInt("portal_port", 0);
        return PortalProtocol.endpoint("http://" + host + ":" + port + "/snnuportal/userstatus.jsp");
    }
    void endpoint(PortalProtocol.Endpoint endpoint) throws IOException {
        PortalProtocol.Endpoint old = endpoint();
        if (endpoint != null && !endpoint.equals(old)) commit(preferences.edit().putString("portal_host", endpoint.host).putInt("portal_port", endpoint.port));
    }
    private static void commit(SharedPreferences.Editor editor) throws IOException {
        if (!editor.commit()) throw new IOException("设置保存失败，请检查存储空间");
    }
    synchronized void log(String safeMessage) {
        // Only application-authored, redacted messages are accepted. No HTTP bodies, exceptions or URLs.
        try {
            File file = new File(directory, "login.log"), old = new File(directory, "login.log.1");
            if (file.length() > 200 * 1024) {
                Files.move(file.toPath(), old.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            }
            String line = String.format(Locale.ROOT, "%tF %<tT %s%n", System.currentTimeMillis(), safeMessage);
            try (FileOutputStream output = new FileOutputStream(file, true)) { output.write(line.getBytes(StandardCharsets.UTF_8)); }
        } catch (IOException ignored) { /* Logging must not break authentication. */ }
    }
    synchronized String diagnostic() {
        Snapshot current = snapshot;
        StringBuilder result = new StringBuilder("SNNU-AutoLogin Android ").append(BuildConfig.VERSION_NAME)
            .append("\nAndroid ").append(android.os.Build.VERSION.RELEASE).append(" / API ").append(android.os.Build.VERSION.SDK_INT)
            .append("\n机型：").append(android.os.Build.MANUFACTURER).append(' ').append(android.os.Build.MODEL)
            .append("\n后台运行：").append(current.running).append("\n自动登录：").append(automatic())
            .append("\n开机运行：").append(boot())
            .append("\n账号：").append(PortalProtocol.mask(account().name))
            .append("\n上网方式：").append(operatorLabel(account().operator))
            .append("\n状态：").append(current.title).append("\n说明：").append(current.detail);
        if (current.status.endpoint != null) result.append("\n门户：").append(current.status.endpoint.base());
        if (current.status.session != null) result.append("\n当前 IP：").append(current.status.session.ip);
        result.append("\n\n最近日志：\n");
        try {
            File file = new File(directory, "login.log");
            if (file.exists()) {
                String log = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
                result.append(log.substring(Math.max(0, log.length() - 16000)));
            }
        } catch (IOException ex) { result.append("日志暂不可读\n"); }
        String password = "";
        try { password = password(); } catch (GeneralSecurityException | IOException ignored) { }
        return PortalProtocol.redact(result.toString(), account().name, password);
    }
    static String operatorLabel(String value) { return value.equals("mobile") ? "移动" : value.equals("unicom") ? "联通" : "校园网"; }
}
