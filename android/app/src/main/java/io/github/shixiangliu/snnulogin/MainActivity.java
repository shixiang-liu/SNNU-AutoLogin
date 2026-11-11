package io.github.shixiangliu.snnulogin;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.content.res.ColorStateList;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.Settings;
import android.text.InputType;
import android.text.Editable;
import android.text.TextWatcher;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.ArrayAdapter;
import android.widget.Switch;
import android.widget.TextView;
import android.widget.Toast;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/** Platform views only. The Activity owns no network workers and releases its refresh on pause. */
@android.annotation.SuppressLint("SetTextI18n") // This campus utility intentionally has a Chinese-only UI.
public final class MainActivity extends Activity {
    private static final int GREEN = 0xff167a4a, BLUE = 0xff2563eb, INK = 0xff182230, MUTED = 0xff667085, PAPER = 0xfff6f7f9;
    private static final int EXPORT = 10, NOTIFICATIONS = 11;
    private final Handler main = new Handler(Looper.getMainLooper());
    private Store store;
    private StatusInspector inspector;
    private LinearLayout content;
    private TextView statusTitle, statusDetail, connectionMeta, actionHint, autoHint, formError;
    private TextView netStatus, netStatusDetail, netName, netAccount, netOperator, netIp, netDuration, netPortal, netChecked;
    private PortalClient.Status lastStatus;
    private long statusAt;
    private TextView notificationStatus, notificationDetail, batteryStatus, batteryDetail;
    private TextView accountCaption, accountValue, operatorValue;
    private TextView homeNetwork;
    private Button notificationButton, batteryButton, resumeAutomatic;
    private long settingsReadAt;
    private long manualCheckAt;
    private long resumeAttemptAt;
    private boolean notificationsReady = true;
    private LinearLayout autoCard, notificationRow;
    private UiModel.View presentation;
    private Button power, connectionCheck;
    private Switch automatic;
    private EditText account, password;
    private Spinner operator;
    private String page = "home";
    private boolean updating;
    private boolean returningFromPortal;
    private String draftName, draftOperator, restoredName;
    private int restoredOperator = -1;
    private android.window.OnBackInvokedCallback backCallback;
    private final Runnable refresh = new Runnable() {
        @Override public void run() {
            if (inspector != null) inspector.refresh(false);
            if (notificationStatus != null && SystemClock.elapsedRealtime() - settingsReadAt >= 2000) renderSystemSettings();
            renderStatus(); main.postDelayed(this, 1000);
        }
    };
    @Override public void onCreate(Bundle saved) {
        super.onCreate(saved);
        getWindow().setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        store = Store.get(this);
        inspector = new StatusInspector(getApplicationContext(), main, this::renderStatus);
        page = validPage(getIntent().getStringExtra("page"));
        if (saved != null) {
            page = validPage(saved.getString("page", "home"));
            restoredName = saved.getString("account-draft"); restoredOperator = saved.getInt("operator-draft", -1);
            returningFromPortal = saved.getBoolean("returning-from-portal", false);
        }
        if (Build.VERSION.SDK_INT >= 33) {
            backCallback = this::back;
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, backCallback);
        }
        show(page);
    }
    @Override protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        out.putString("page", page);
        if (account != null) { out.putString("account-draft", account.getText().toString()); out.putInt("operator-draft", operator.getSelectedItemPosition()); }
        out.putBoolean("returning-from-portal", returningFromPortal);
        // Password input has saveEnabled=false: plaintext never goes into saved instance state.
    }
    @Override protected void onNewIntent(Intent intent) { super.onNewIntent(intent); setIntent(intent); show(validPage(intent.getStringExtra("page"))); }
    @Override protected void onResume() {
        super.onResume(); LoginService.setVisible(true); inspector.resume(); LoginService.inspectWhenIdle();
        // If automatic login is on but the system killed the service, opening the app brings it back.
        if (store != null && store.automatic() && store.account().configured && !LoginService.running()
                && SystemClock.elapsedRealtime() - resumeAttemptAt > 3000) {
            resumeAttemptAt = SystemClock.elapsedRealtime();
            command(LoginService.RESUME);
        }
        renderSystemSettings(); main.removeCallbacks(refresh); refresh.run();
        if (returningFromPortal) { returningFromPortal = false; checkConnection(); }
    }
    @Override protected void onPause() { LoginService.setVisible(false); main.removeCallbacks(refresh); inspector.pause(); super.onPause(); }
    @Override public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused && store != null) { renderSystemSettings(); renderStatus(); }
    }
    @Override public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request == NOTIFICATIONS) { renderSystemSettings(); renderStatus(); }
    }
    @Override protected void onDestroy() {
        main.removeCallbacksAndMessages(null);
        inspector.pause();
        if (password != null) password.setText("");
        if (Build.VERSION.SDK_INT >= 33 && backCallback != null) getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(backCallback);
        super.onDestroy();
    }
    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private static String validPage(String value) {
        if ("advanced".equals(value)) return "settings";
        return "settings".equals(value) || "account".equals(value) || "network".equals(value) || "help".equals(value)
            || "background".equals(value) ? value : "home";
    }
    private void back() { if (page.equals("home")) finish(); else show(page.equals("settings") || page.equals("account") ? "home" : "settings"); }
    private void show(String target) {
        // A removed account page must not retain its view tree or an unsaved plaintext password.
        if (password != null) {
            getSystemService(InputMethodManager.class).hideSoftInputFromWindow(password.getWindowToken(), 0);
            password.setText("");
        }
        account = null; password = null; operator = null;
        page = validPage(target);
        statusTitle = null; statusDetail = null; power = null; connectionCheck = null; automatic = null;
        netStatus = null; netStatusDetail = null; netName = null; netAccount = null; netOperator = null; netIp = null; netDuration = null; netPortal = null; netChecked = null;
        autoCard = null; notificationRow = null; connectionMeta = null; actionHint = null; autoHint = null; formError = null; presentation = null;
        circleColor = 0; accountCaption = null; accountValue = null; operatorValue = null; homeNetwork = null;
        notificationStatus = null; notificationDetail = null; notificationButton = null;
        batteryStatus = null; batteryDetail = null; batteryButton = null;
        resumeAutomatic = null;
        LinearLayout shell = new LinearLayout(this);
        shell.setOrientation(LinearLayout.VERTICAL); shell.setBackgroundColor(PAPER);
        shell.setPadding(dp(20), dp(16), dp(20), dp(12));
        shell.setOnApplyWindowInsetsListener((view, insets) -> {
            int left, top, right, bottom;
            if (Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
                left = bars.left; top = bars.top; right = bars.right;
                bottom = Math.max(bars.bottom, insets.getInsets(WindowInsets.Type.ime()).bottom);
            } else {
                left = insets.getSystemWindowInsetLeft(); top = insets.getSystemWindowInsetTop();
                right = insets.getSystemWindowInsetRight(); bottom = insets.getSystemWindowInsetBottom();
            }
            view.setPadding(dp(20) + left, dp(16) + top, dp(20) + right, dp(12) + bottom);
            return insets;
        });
        LinearLayout header = new LinearLayout(this); header.setGravity(Gravity.CENTER_VERTICAL);
        if (!page.equals("home")) {
            Button back = circleButton("←", this::back);
            LinearLayout.LayoutParams backParams = new LinearLayout.LayoutParams(dp(40), dp(40));
            backParams.rightMargin = dp(12);
            header.addView(back, backParams);
        }
        TextView heading = label(title(page), page.equals("home") ? 27 : 22, INK); heading.setTypeface(null, Typeface.BOLD);
        if (page.equals("home")) homeNetwork = heading;
        header.addView(heading, new LinearLayout.LayoutParams(0, -2, 1));
        if (page.equals("home")) {
            Button settings = link("设置", () -> show("settings")); settings.setPadding(dp(8), dp(4), dp(8), dp(4));
            header.addView(settings, new LinearLayout.LayoutParams(dp(80), -2));
        }
        shell.addView(header);
        ScrollView scroll = new ScrollView(this); scroll.setFillViewport(true);
        content = new LinearLayout(this); content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(0, dp(20), 0, dp(16)); scroll.addView(content);
        shell.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(shell);
        switch (page) {
            case "account": accountPage(); break;
            case "settings": settingsPage(); break;
            case "network": networkPage(); break;
            case "background": backgroundPage(); break;
            case "help": helpPage(); break;
            default: homePage(); break;
        }
        renderSystemSettings(); renderStatus();
    }
    private String title(String value) {
        switch (value) {
            case "settings": return "设置";
            case "account": return "校园网账号";
            case "network": return "连接诊断";
            case "background": return "后台与通知";
            case "help": return "使用帮助";
            default: return "校园网";
        }
    }
    private TextView label(String text, int size, int color) {
        TextView view = new TextView(this); view.setText(text); view.setTextSize(size);
        view.setTextColor(color); view.setLineSpacing(dp(4), 1); return view;
    }
    private void paragraph(String text) {
        TextView view = label(text, 15, MUTED); view.setPadding(0, dp(4), 0, dp(16)); content.addView(view);
    }
    private void section(String text) {
        TextView view = label(text, 18, INK); view.setTypeface(null, Typeface.BOLD);
        view.setPadding(0, dp(16), 0, dp(8)); content.addView(view);
    }
    private Button button(String text, Runnable action) {
        Button view = new Button(this); view.setText(text); view.setTextSize(16); view.setTextColor(BLUE);
        view.setAllCaps(false); view.setMinHeight(dp(52)); view.setMinimumHeight(dp(52)); view.setMinWidth(0); view.setMinimumWidth(0);
        view.setPadding(dp(16), dp(10), dp(16), dp(10)); view.setElevation(0); view.setStateListAnimator(null);
        view.setBackground(ripple(Color.WHITE, true)); view.setOnClickListener(v -> action.run());
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.bottomMargin = dp(12); view.setLayoutParams(params);
        return view;
    }
    private GradientDrawable shape(int color, boolean border) {
        GradientDrawable drawable = new GradientDrawable(); drawable.setColor(color); drawable.setCornerRadius(dp(6));
        if (border) drawable.setStroke(dp(1), 0xffe4e7ec); return drawable;
    }
    private RippleDrawable ripple(int color, boolean border) {
        return new RippleDrawable(ColorStateList.valueOf(0x182563eb), shape(color, border), shape(Color.WHITE, false));
    }
    private Button link(String text, Runnable action) { Button view = button(text, action); view.setBackground(ripple(Color.TRANSPARENT, false)); return view; }
    /** Round icon button used for the header back action. */
    private Button circleButton(String text, Runnable action) {
        Button view = new Button(this);
        view.setText(text); view.setTextSize(18); view.setTextColor(INK); view.setGravity(Gravity.CENTER);
        view.setAllCaps(false); view.setPadding(0, 0, 0, 0);
        view.setMinWidth(0); view.setMinimumWidth(0); view.setMinHeight(0); view.setMinimumHeight(0);
        view.setElevation(0); view.setStateListAnimator(null);
        GradientDrawable fill = new GradientDrawable(); fill.setShape(GradientDrawable.OVAL);
        fill.setColor(Color.WHITE); fill.setStroke(dp(1), 0xffe4e7ec);
        GradientDrawable mask = new GradientDrawable(); mask.setShape(GradientDrawable.OVAL); mask.setColor(Color.WHITE);
        view.setBackground(new RippleDrawable(ColorStateList.valueOf(0x182563eb), fill, mask));
        view.setOnClickListener(v -> action.run());
        return view;
    }
    private Button filled(String text, Runnable action) { Button view = button(text, action); view.setBackground(ripple(BLUE, false)); view.setTextColor(Color.WHITE); return view; }
    private LinearLayout card() {
        LinearLayout view = new LinearLayout(this); view.setOrientation(LinearLayout.VERTICAL); view.setPadding(dp(20), dp(20), dp(20), dp(20));
        view.setBackground(shape(Color.WHITE, true)); LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.bottomMargin = dp(16); content.addView(view, params); return view;
    }
    private void cardTitle(LinearLayout group, String value) {
        TextView title = label(value, 18, INK); title.setTypeface(null, Typeface.BOLD);
        title.setPadding(0, 0, 0, dp(10)); group.addView(title);
    }
    private TextView cardCopy(LinearLayout group, String value) {
        TextView detail = label(value, 14, MUTED); group.addView(detail); return detail;
    }
    private Button cardAction(LinearLayout group, String value, Runnable action) {
        Button view = link(value, action); view.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        view.setBackground(ripple(Color.TRANSPARENT, false));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.topMargin = dp(12);
        group.addView(view, params); return view;
    }
    private TextView statusBadge(LinearLayout group, int id) {
        TextView view = label("", 14, GREEN); view.setId(id); view.setTypeface(null, Typeface.BOLD);
        view.setPadding(0, dp(2), 0, dp(2));
        view.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-2, -2); params.bottomMargin = dp(10);
        group.addView(view, params); return view;
    }
    private void navigationRow(String title, String detail, Runnable action) {
        LinearLayout row = new LinearLayout(this); row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(12), dp(10), dp(12), dp(10)); row.setMinimumHeight(dp(64));
        row.setBackground(ripple(Color.WHITE, false));
        LinearLayout words = new LinearLayout(this); words.setOrientation(LinearLayout.VERTICAL);
        TextView name = label(title, 16, INK); name.setTypeface(null, Typeface.BOLD); words.addView(name);
        TextView hint = label(detail, 13, MUTED); hint.setPadding(0, dp(4), 0, 0); words.addView(hint);
        row.addView(words, new LinearLayout.LayoutParams(0, -2, 1));
        TextView arrow = label("›", 24, BLUE); arrow.setPadding(dp(12), 0, 0, 0);
        arrow.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO); row.addView(arrow);
        row.setOnClickListener(v -> action.run()); row.setFocusable(true);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.bottomMargin = dp(1); content.addView(row, params);
    }
    private void homePage() {
        LinearLayout center = new LinearLayout(this); center.setOrientation(LinearLayout.VERTICAL); center.setGravity(Gravity.CENTER_HORIZONTAL);
        center.setPadding(0, dp(12), 0, 0); content.addView(center);
        power = button("检测中", this::toggleConnection); power.setId(R.id.home_action); power.setGravity(Gravity.CENTER);
        power.setPadding(dp(10), dp(20), dp(10), dp(16)); power.setTextSize(16);
        int diameter = dp(getResources().getConfiguration().fontScale > 1.3f ? 164 : 136);
        LinearLayout.LayoutParams circle = new LinearLayout.LayoutParams(diameter, diameter); circle.bottomMargin = dp(24); center.addView(power, circle);
        statusTitle = label("正在检测连接", 28, INK); statusTitle.setId(R.id.home_status); statusTitle.setTypeface(null, Typeface.BOLD); statusTitle.setGravity(Gravity.CENTER);
        statusTitle.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE); center.addView(statusTitle);
        statusDetail = label("", 14, MUTED); statusDetail.setGravity(Gravity.CENTER); statusDetail.setPadding(0, dp(6), 0, 0); center.addView(statusDetail);
        connectionCheck = link("检查网络", this::checkConnection); connectionCheck.setTextSize(14); center.addView(connectionCheck);
        LinearLayout identity = card();
        accountCaption = label("当前账号", 13, MUTED); accountCaption.setId(R.id.home_account_caption); identity.addView(accountCaption);
        LinearLayout row = new LinearLayout(this); row.setGravity(Gravity.CENTER_VERTICAL); row.setPadding(0, dp(8), 0, 0);
        accountValue = label("", 19, INK); accountValue.setId(R.id.home_account); accountValue.setTextIsSelectable(true); row.addView(accountValue, new LinearLayout.LayoutParams(0, -2, 1));
        operatorValue = label("", 14, MUTED); operatorValue.setId(R.id.home_operator); operatorValue.setPadding(dp(12), 0, 0, 0); row.addView(operatorValue); identity.addView(row);
        connectionMeta = label("", 12, MUTED); connectionMeta.setPadding(0, dp(10), 0, 0); identity.addView(connectionMeta);
        actionHint = label("", 13, MUTED); actionHint.setGravity(Gravity.CENTER); actionHint.setPadding(0, dp(4), 0, dp(8)); content.addView(actionHint);
        notificationRow = new LinearLayout(this); notificationRow.setGravity(Gravity.CENTER_VERTICAL);
        notificationRow.addView(label("允许通知后可在通知栏快捷操作", 13, MUTED), new LinearLayout.LayoutParams(0, -2, 1));
        notificationRow.addView(link("去开启", this::notificationSettings), new LinearLayout.LayoutParams(dp(80), -2)); content.addView(notificationRow);
    }
    private int circleColor;
    private void renderPower(UiModel.View view) {
        String text;
        switch (view.action) {
            case DISCONNECT: text = "点击断开"; break;
            case CONNECT: text = "点击连接"; break;
            case SETUP: text = "设置账号"; break;
            case WIFI: text = "选择网络"; break;
            case ACCOUNT: text = "查看账号"; break;
            case CANCEL: text = "取消"; break;
            case PORTAL: text = "网页验证"; break;
            case CHECK: text = store.snapshot.inspecting ? "检测中" : "立即检测"; break;
            default: text = "查看说明";
        }
        text(power, text); power.setEnabled(!store.snapshot.inspecting);
        int color = view.tone == UiModel.Tone.GOOD ? GREEN : view.tone == UiModel.Tone.ERROR ? 0xffbd353b : view.tone == UiModel.Tone.WARNING ? 0xff995c18 : BLUE;
        if (circleColor != color) {
            circleColor = color; power.setTextColor(color);
            GradientDrawable background = new GradientDrawable(); background.setShape(GradientDrawable.OVAL);
            background.setColor(view.tone == UiModel.Tone.GOOD ? 0xffe7f4e5 : view.tone == UiModel.Tone.ERROR ? 0xffffeded : view.tone == UiModel.Tone.WARNING ? 0xfffff4df : 0xffe8f0fe);
            GradientDrawable mask = new GradientDrawable(); mask.setShape(GradientDrawable.OVAL); mask.setColor(Color.WHITE);
            power.setBackground(new RippleDrawable(ColorStateList.valueOf(0x182563eb), background, mask));
            android.graphics.drawable.Drawable icon = getDrawable(R.drawable.ic_power).mutate(); icon.setTint(color); icon.setBounds(0, 0, dp(42), dp(42));
            power.setCompoundDrawables(null, icon, null, null); power.setCompoundDrawablePadding(dp(10));
        }
    }
    private void accountPage() {
        Store.Account current = store.account();
        paragraph("填写一次，之后自动连接。密码加密保存在这台手机。");
        section("校园网账号");
        account = new EditText(this); account.setSingleLine(true); account.setHint("学号 / 校园网账号");
        account.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        account.setId(R.id.account); account.setText(restoredName != null ? restoredName : draftName != null ? draftName : current.name); restoredName = null;
        account.setSaveEnabled(false); account.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO); watch(account);
        content.addView(account);
        section("校园网密码");
        password = new EditText(this); password.setSingleLine(true);
        password.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        password.setHint(current.configured && account.getText().toString().equals(current.name) ? "留空保留已保存密码" : "输入校园网密码");
        password.setId(R.id.password); password.setSaveEnabled(false); password.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO); watch(password);
        password.setMinimumHeight(dp(56));
        LinearLayout passwordRow = new LinearLayout(this); passwordRow.setGravity(Gravity.CENTER_VERTICAL); passwordRow.addView(password, new LinearLayout.LayoutParams(0, -2, 1));
        Button reveal = link("显示", () -> { }); reveal.setContentDescription("显示密码");
        reveal.setTextSize(14); reveal.setPadding(dp(4), dp(8), dp(4), dp(8));
        reveal.setOnClickListener(v -> { boolean visible = reveal.getText().toString().equals("显示"); int cursor = password.getSelectionStart();
            password.setInputType(InputType.TYPE_CLASS_TEXT | (visible ? InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD : InputType.TYPE_TEXT_VARIATION_PASSWORD));
            password.setSelection(Math.max(0, Math.min(cursor, password.length()))); reveal.setText(visible ? "隐藏" : "显示"); reveal.setContentDescription(visible ? "隐藏密码" : "显示密码"); });
        passwordRow.addView(reveal, new LinearLayout.LayoutParams(dp(72), -2)); content.addView(passwordRow);
        section("上网方式");
        operator = new Spinner(this);
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this, android.R.layout.simple_spinner_item, new String[]{"校园网", "移动", "联通"});
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item); operator.setAdapter(adapter);
        String selected = draftOperator == null ? current.operator : draftOperator;
        operator.setSelection(restoredOperator >= 0 ? restoredOperator : selected.equals("mobile") ? 1 : selected.equals("unicom") ? 2 : 0); restoredOperator = -1;
        operator.setMinimumHeight(dp(56)); content.addView(operator, new LinearLayout.LayoutParams(-1, -2));
        paragraph("与学校认证网页上的选择保持一致");
        formError = label("", 14, 0xffa63f35); formError.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE); formError.setVisibility(View.GONE); content.addView(formError);
        Button save = filled("保存并连接", this::saveAccount); save.setId(R.id.save_account); content.addView(save);
        if (current.configured) content.addView(link("清除本机账号", () -> new AlertDialog.Builder(this)
            .setTitle("清除本机账号？").setMessage("删除这台手机保存的账号密码，并停止自动连接。当前上网连接不会被主动断开。")
            .setNegativeButton("取消", null).setPositiveButton("清除", (dialog, which) -> {
                LoginService.cancelCurrent();
                try { store.forget(); draftName = null; draftOperator = null; command(LoginService.STOP); show("account"); }
                catch (IOException ex) { error(ex.getMessage()); }
            }).show()));
    }
    private void watch(EditText field) {
        field.addTextChangedListener(new TextWatcher() {
            public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            public void onTextChanged(CharSequence s, int start, int before, int count) { }
            public void afterTextChanged(Editable text) { field.setError(null); if (formError != null) formError.setVisibility(View.GONE); }
        });
    }
    private void saveAccount() {
        String name = account.getText().toString().trim(), secret = password.getText().toString();
        String op = operator.getSelectedItemPosition() == 1 ? "mobile" : operator.getSelectedItemPosition() == 2 ? "unicom" : "";
        Store.Account original = store.account();
        AccountUpdate.Validation validation = AccountUpdate.validate(name, secret, op, original.name, original.configured);
        if (validation != null) {
            if (validation.field == AccountUpdate.Field.OPERATOR) { formError.setText(validation.message); formError.setVisibility(View.VISIBLE); operator.requestFocus(); }
            else { EditText field = validation.field == AccountUpdate.Field.ACCOUNT ? account : password; field.setError(validation.message); field.requestFocus();
                field.requestRectangleOnScreen(new android.graphics.Rect(0, 0, field.getWidth(), field.getHeight())); getSystemService(InputMethodManager.class).showSoftInput(field, InputMethodManager.SHOW_IMPLICIT); }
            return;
        }
        try {
            store.validateAccount(name, secret, op);
            PortalClient.Status consent = store.snapshot.status;
            Store.Account saved = store.account();
            if (consent.kind == PortalProtocol.Kind.ONLINE && consent.session.matches(name, op)
                    && saved.name.equals(name) && saved.operator.equals(op) && secret.isEmpty()) {
                show("home"); return;
            }
            if (consent.kind == PortalProtocol.Kind.ONLINE) {
                new AlertDialog.Builder(this).setTitle("保存并重新连接")
                    .setMessage("当前在线账号：" + PortalProtocol.mask(consent.session.account)
                        + "\n\n确认后会短暂断网，并使用新设置重新认证。新设置认证失败时保留原账号配置。")
                    .setNegativeButton("取消", null).setPositiveButton("保存并重连", (dialog, which) -> submitAccount(name, secret, op, consent)).show();
            } else submitAccount(name, secret, op, null);
        } catch (IOException ex) { error(ex.getMessage()); }
    }
    private void submitAccount(String name, String secret, String op, PortalClient.Status consent) {
        LoginService.cancelCurrent();
        if (!LoginService.updateAccount(this, name, secret, op, consent)) { formError.setText("未能开始连接，请稍后重新保存"); formError.setVisibility(View.VISIBLE); return; }
        draftName = name; draftOperator = op;
        password.setText("");
        getSystemService(InputMethodManager.class).hideSoftInputFromWindow(account.getWindowToken(), 0);
        show("home");
        requestNotificationsOnce();
    }
    private void settingsPage() {
        Store.Account account = store.account();
        navigationRow("校园网账号", account.configured ? PortalProtocol.mask(account.name) + " · " + Store.operatorLabel(account.operator) : "账号、密码与上网方式", () -> show("account"));
        autoCard = card(); automatic = new Switch(this); automatic.setId(R.id.auto_connect); automatic.setText("自动登录"); automatic.setTextSize(16); automatic.setTextColor(INK); automatic.setMinHeight(dp(48));
        automatic.setEnabled(account.configured); autoCard.addView(automatic);
        autoHint = cardCopy(autoCard, "");
        automatic.setOnCheckedChangeListener((view, checked) -> { if (!updating) command(checked ? LoginService.RESUME : LoginService.PAUSE); });
        resumeAutomatic = cardAction(autoCard, "恢复自动登录", () -> command(LoginService.RESUME));
        Switch guardian = new Switch(this);
        guardian.setText("守卫者（至少 3 台在线）");
        guardian.setChecked(store.guardian());
        guardian.setEnabled(account.configured);
        guardian.setOnCheckedChangeListener((view, checked) -> {
            try { store.guardian(checked); }
            catch (java.io.IOException ex) { error("无法保存守卫者设置"); show("settings"); }
        });
        autoCard.addView(guardian);
        cardAction(autoCard, "在线设备管理（校园网门户）", () -> {
            PortalClient.Status status = store.snapshot.status;
            if (status.kind != PortalProtocol.Kind.ONLINE || status.endpoint == null
                    || !PortalProtocol.HOST.equals(status.endpoint.host)) {
                error("请先连接校园网");
                return;
            }
            try { startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(status.endpoint.base() + "userstatus.jsp"))); }
            catch (RuntimeException ex) { error("无法打开在线设备管理页面"); }
        });
        navigationRow("后台与通知", "查看通知、电池优化与开机恢复状态", () -> show("background"));
        navigationRow("网络信息", "检查网络，导出脱敏报告", () -> show("network"));
        navigationRow("使用帮助", "使用说明与常见问题", () -> show("help"));
        TextView footer = label("v" + BuildConfig.VERSION_NAME, 13, MUTED);
        footer.setGravity(Gravity.CENTER); footer.setPadding(0, dp(10), 0, dp(16));
        content.addView(footer);
    }
    private void backgroundPage() {
        paragraph("需要锁屏后继续自动登录时，再检查这里。");
        LinearLayout notice = card(); cardTitle(notice, "连接通知");
        notificationStatus = statusBadge(notice, R.id.notification_status);
        notificationDetail = cardCopy(notice, ""); notificationButton = cardAction(notice, "查看通知设置", this::notificationSettings);
        LinearLayout battery = card(); cardTitle(battery, "后台权限");
        batteryStatus = statusBadge(battery, R.id.battery_status);
        batteryDetail = cardCopy(battery, "");
        batteryButton = cardAction(battery, "打开系统设置", this::batterySettings);
        LinearLayout startup = card(); cardTitle(startup, "重启后继续使用");
        Switch boot = new Switch(this); boot.setText("开机后恢复自动连接"); boot.setTextSize(16);
        boot.setTextColor(INK);
        boot.setEnabled(store.account().configured);
        boot.setPadding(0, dp(12), 0, dp(12)); boot.setChecked(store.boot());
        boot.setOnCheckedChangeListener((view, checked) -> {
            if (updating) return;
            try { store.boot(checked); if (checked && !store.automatic()) command(LoginService.RESUME); }
            catch (IOException ex) {
                error(ex.getMessage()); updating = true; view.setChecked(!checked); updating = false;
            }
        });
        startup.addView(boot);
        cardCopy(startup, "开启时会同时启用自动连接。重启并首次解锁后，应用会尝试恢复运行。");
        if (!store.account().configured) cardAction(startup, "先设置校园网账号", () -> show("account"));
        paragraph("部分手机还需在应用设置中允许自启动。应用无法读取这项厂商设置；系统休眠时，联网检查仍可能延后。");
        content.addView(link("打开本应用的系统设置", () -> openSettings(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + getPackageName())))));
    }
    private void networkPage() {
        LinearLayout badge = card(); netStatus = statusBadge(badge, R.id.network_status); netStatusDetail = cardCopy(badge, "");
        LinearLayout information = card();
        netName = infoRow(information, "网络名称");
        netAccount = infoRow(information, "当前账号");
        netOperator = infoRow(information, "上网方式");
        netIp = infoRow(information, "IP 地址");
        netDuration = infoRow(information, "在线时长");
        netPortal = infoRow(information, "认证地址");
        netChecked = infoRow(information, "上次检测");
        content.addView(filled("检查连接", this::checkConnection));
        PortalProtocol.Endpoint endpoint = store.snapshot.status.endpoint;
        if (endpoint != null && !PortalProtocol.trustedHost(endpoint.host) && !endpoint.host.equalsIgnoreCase(store.approvedHost())) {
            content.addView(button("核对新的认证地址", this::approvePortal));
        }
        content.addView(button("导出脱敏诊断报告", () -> {
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT).setType("text/plain")
                .addCategory(Intent.CATEGORY_OPENABLE).putExtra(Intent.EXTRA_TITLE, "SNNU诊断报告-Android.txt");
            try { startActivityForResult(intent, EXPORT); } catch (RuntimeException ex) { error("系统文件选择器不可用"); }
        }));
        paragraph("报告包含机型、连接状态、门户地址、IP 和最近日志，账号会脱敏，不包含密码。分享前请检查其中的网络信息。");
    }
    private TextView infoRow(LinearLayout group, String title) {
        if (group.getChildCount() > 0) {
            View divider = new View(this); divider.setBackgroundColor(0xffeef1f5);
            LinearLayout.LayoutParams line = new LinearLayout.LayoutParams(-1, dp(1)); group.addView(divider, line);
        }
        LinearLayout row = new LinearLayout(this); row.setGravity(Gravity.CENTER_VERTICAL); row.setPadding(0, dp(11), 0, dp(11));
        row.addView(label(title, 14, MUTED));
        TextView value = label("", 14, INK); value.setGravity(Gravity.END); value.setTextIsSelectable(true);
        row.addView(value, new LinearLayout.LayoutParams(0, -2, 1));
        group.addView(row);
        return value;
    }
    private void approvePortal() {
        PortalProtocol.Endpoint endpoint = store.snapshot.status.endpoint;
        if (endpoint == null) { error("尚未发现学校认证门户，请先检查连接"); return; }
        if (PortalProtocol.trustedHost(endpoint.host) || endpoint.host.equalsIgnoreCase(store.approvedHost())) {
            error("此地址已受信任，无需重复确认"); return;
        }
        new AlertDialog.Builder(this).setTitle("确认新的校园网认证主机")
            .setMessage("发现地址：" + endpoint.base() + "\n\n请先与学校提供的认证地址核对。确认后，校园网密码将发送到此主机；HTTP 页面标识不能证明主机身份。")
            .setNegativeButton("取消", null).setPositiveButton("确认此地址", (dialog, which) -> {
                LoginService.cancelCurrent();
                try { store.approve(endpoint.host); command(LoginService.REFRESH); }
                catch (IOException ex) { error(ex.getMessage()); }
            }).show();
    }
    private void helpPage() {
        if (PortalProtocol.failure(store.snapshot.detail) == PortalProtocol.Failure.ACCOUNT_STATE || store.pauseReason() == PortalProtocol.Failure.ACCOUNT_STATE) {
            section("当前账号需要处理");
            paragraph("请先在学校自助服务中检查余额、套餐和在线设备数，处理后回首页重新连接。");
        }
        if (store.snapshot.title.equals("设置保存失败")) { section("设置未保存"); paragraph("请清理手机存储空间，再到账号设置中重新保存。"); }
        section("开始使用");
        paragraph("先连接学校 Wi-Fi。在账号设置中填写账号、密码和上网方式，点击「保存并连接」。以后连上学校 Wi-Fi 时会自动登录。");
        content.addView(link("账号设置  ›", () -> show("account")));
        section("锁屏后不能自动登录");
        paragraph("确认自动登录已开启，再在系统设置中允许本应用后台运行；系统休眠时，重连可能延后。");
        Button background = link("检查后台与通知  ›", () -> show("background")); background.setId(R.id.help_background); content.addView(background);
        section("连接操作");
        paragraph("首页圆形按钮用于连接或断开。只想停止自动登录、保留当前认证连接时，到设置中关闭「自动登录」——后台服务会随之停止，通知也会消失，校园网仍然在线。");
        section("能上网，但没有显示已连接");
        paragraph("「Wi-Fi 可以上网」表示系统确认这个 Wi-Fi 能上网，但学校认证页没有返回在线账号。应用会自己每隔一段时间重读，也可以点「立即检测」马上读一次；两种方式都不会重新登录或断开当前连接。");
        section("账号异常或验证码");
        paragraph("密码错误时，核对账号、密码和上网方式。欠费、停用或设备数超限时，先在学校自助服务中处理。网页要求验证码时，使用学校认证页完成验证，再返回检查连接。");
        section("连接诊断");
        paragraph("诊断页显示网络名称、账号、上网方式、IP、在线时长和认证地址。网络名称固定写 SNNU，因为本工具只连接学校这一张无线网。");
        content.addView(link("连接诊断  ›", () -> show("network")));
        section("数据与隐私");
        paragraph("账号和加密密码保存在本机，不上传给开发者。学校门户使用 HTTP，认证传输没有加密。导出报告会隐藏账号和密码，分享前仍请检查 IP 等网络信息。");
        paragraph("社区开源工具，与学校官方客户端无隶属关系。");
        content.addView(link("项目与反馈  ›", () -> openSettings(new Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/shixiang-liu/SNNU-AutoLogin")))));
    }
    private void toggleConnection() {
        if (presentation == null) return;
        switch (presentation.action) {
            case SETUP: case ACCOUNT: show("account"); break;
            case WIFI: openSettings(new Intent(Settings.ACTION_WIFI_SETTINGS)); break;
            case CONNECT: command(LoginService.CONNECT); break;
            case DISCONNECT: command(LoginService.DISCONNECT); break;
            case CANCEL: command(LoginService.PAUSE); break;
            case CHECK: checkConnection(); break;
            case DETAILS: show("network"); break;
            case HELP: show(store.snapshot.title.equals("后台启动受限") ? "background" : "help"); break;
            case PORTAL: openPortal(); break;
        }
    }
    private void openPortal() {
        PortalProtocol.Endpoint endpoint = store.snapshot.status.endpoint;
        if (endpoint == null || !(PortalProtocol.trustedHost(endpoint.host) || endpoint.host.equalsIgnoreCase(store.approvedHost()))) { show("network"); return; }
        try { startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(endpoint.base() + "login.jsp"))); returningFromPortal = true; }
        catch (RuntimeException ex) { error("没有可用的浏览器，请使用系统 Wi-Fi 登录提示"); }
    }
    private void command(String action) {
        LoginService.cancelCurrent();
        if (LoginService.send(this, action) && (action.equals(LoginService.CONNECT) || action.equals(LoginService.RESUME))) requestNotificationsOnce();
        renderStatus();
    }
    private void checkConnection() {
        manualCheckAt = SystemClock.elapsedRealtime();
        if (LoginService.running()) command(LoginService.INSPECT);
        else inspector.refresh(true);
    }
    private void requestNotificationsOnce() {
        if (Build.VERSION.SDK_INT < 33 || store.notificationAsked() || checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return;
        try { store.notificationAsked(true); } catch (IOException ignored) { return; }
        requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, NOTIFICATIONS);
    }
    private void notificationSettings() {
        SystemSettingsState.Entry state = SystemSettingsState.notifications(this);
        Intent intent = new Intent(state.channelBlocked ? Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS : Settings.ACTION_APP_NOTIFICATION_SETTINGS)
            .putExtra(Settings.EXTRA_APP_PACKAGE, getPackageName());
        if (state.channelBlocked) intent.putExtra(Settings.EXTRA_CHANNEL_ID, LoginService.CHANNEL);
        openSettings(intent);
    }
    private void batterySettings() {
        // Every Android version keeps battery controls on the app's own settings page.
        openSettings(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + getPackageName())));
    }
    private void renderSystemSettings() {
        settingsReadAt = SystemClock.elapsedRealtime();
        SystemSettingsState state = SystemSettingsState.read(this); notificationsReady = state.notifications.ready;
        if (notificationStatus != null) {
            paintBadge(notificationStatus, state.notifications.title, state.notifications.tone);
            text(notificationDetail, state.notifications.detail); text(notificationButton, state.notifications.button);
        }
        if (batteryStatus != null) {
            paintBadge(batteryStatus, state.battery.title, state.battery.tone);
            text(batteryDetail, batteryDetail(state.battery)); text(batteryButton, state.battery.button);
        }
    }
    /**
     * Test builds live under a different package name, and Android stores the battery allowance per
     * package. Without naming the package, a switch granted for another build looks like a stale
     * reading of the same app.
     */
    private String batteryDetail(SystemSettingsState.Entry entry) {
        if (entry.ready || BuildConfig.APPLICATION_ID.equals("io.github.shixiangliu.snnulogin")) return entry.detail;
        return entry.detail + " 当前安装的是测试包（" + BuildConfig.APPLICATION_ID + "），系统按包名保存电池设置，需要为这个包名单独允许一次。";
    }
    private void paintBadge(TextView view, String value, UiModel.Tone tone) {
        if (TextUtils.equals(view.getText(), value)) return;
        int color = tone == UiModel.Tone.GOOD ? GREEN : tone == UiModel.Tone.WARNING ? 0xff865810 : MUTED;
        text(view, value); view.setTextColor(color);
    }
    private boolean wifiConnected() {
        ConnectivityManager manager = getSystemService(ConnectivityManager.class);
        for (Network network : manager.getAllNetworks()) {
            NetworkCapabilities capabilities = manager.getNetworkCapabilities(network);
            if (capabilities != null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) && !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return true;
        }
        return false;
    }
    private boolean wifiValidated() {
        ConnectivityManager manager = getSystemService(ConnectivityManager.class);
        for (Network network : manager.getAllNetworks()) {
            NetworkCapabilities caps = manager.getNetworkCapabilities(network);
            if (caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                    && !caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN))
                return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED);
        }
        return false;
    }
    private void renderStatus() {
        if (store == null) return;
        Store.Snapshot state = store.snapshot;
        if (state.status != lastStatus) { lastStatus = state.status; statusAt = SystemClock.elapsedRealtime(); }
        boolean wifi = wifiConnected();
        if (statusTitle != null) {
            presentation = store.presentation(wifi, wifiValidated());
            String detail = presentation.detail.replace("自动连接", "自动登录");
            // A fast read can finish between two redraws; acknowledge the tap anyway.
            if (SystemClock.elapsedRealtime() - manualCheckAt < 1200) detail = "正在重新读取认证状态…";
            text(statusTitle, presentation.title.equals("已连接校园网") ? "已连接" : presentation.title); text(statusDetail, detail); renderPower(presentation);
            text(homeNetwork, "SNNU");
            connectionCheck.setVisibility(presentation.action == UiModel.Action.CHECK || state.busy || state.inspecting ? View.INVISIBLE : View.VISIBLE);
            Store.Account saved = store.account();
            UiModel.AccountInfo identity = UiModel.accountInfo(state.status, wifi, saved.name, saved.operator);
            text(accountCaption, identity.caption); text(accountValue, identity.account); text(operatorValue, identity.operator);
            String duration = !wifi || state.status.session == null ? "" : PortalProtocol.onlineDuration(state.status.session.loginTime, java.time.LocalDateTime.now());
            String network = wifi ? "SNNU" : "尚未连接 Wi-Fi";
            String meta = duration.isEmpty() ? network : network.isEmpty() ? "已在线 " + duration : network + " · 已在线 " + duration;
            text(connectionMeta, meta); connectionMeta.setVisibility(meta.isEmpty() ? View.GONE : View.VISIBLE);
            String hint = presentation.action == UiModel.Action.DISCONNECT ? "断开会关闭自动登录，Wi-Fi 保持连接"
                : presentation.action == UiModel.Action.CONNECT ? "同时开启自动连接与掉线重连"
                : presentation.action == UiModel.Action.WIFI ? "连接学校 Wi-Fi 后，再返回应用"
                : presentation.action == UiModel.Action.SETUP ? "账号与密码仅保存在这台手机"
                : presentation.action == UiModel.Action.PORTAL ? "请保持连接学校 Wi-Fi" : "";
            text(actionHint, hint); actionHint.setVisibility(hint.isEmpty() ? View.GONE : View.VISIBLE);
            notificationRow.setVisibility(state.running && !notificationsReady ? View.VISIBLE : View.GONE);
        }
        if (automatic != null) {
            boolean requested = store.automatic(), running = LoginService.running();
            text(autoHint, !store.account().configured ? "请先设置账号" : !requested ? "已关闭，当前连接保留"
                : running ? "已开启，掉线后自动恢复" : "已开启，但后台未运行，恢复后才能自动登录");
            updating = true; automatic.setChecked(requested); updating = false;
            resumeAutomatic.setVisibility(requested && !running ? View.VISIBLE : View.GONE);
        }
        if (netStatus != null) {
            UiModel.View view = store.presentation(wifi, wifiValidated());
            paintBadge(netStatus, view.title, view.tone); text(netStatusDetail, view.detail.replace("自动连接", "自动登录"));
            Store.Account saved = store.account();
            UiModel.AccountInfo identity = UiModel.accountInfo(state.status, wifi, saved.name, saved.operator);
            // The tool is built for the school network only, so the name is known without reading the SSID.
            text(netName, wifi ? "SNNU" : "未连接");
            text(netAccount, identity.account);
            text(netOperator, identity.operator.isEmpty() ? "—" : identity.operator);
            PortalProtocol.Session session = state.status.session;
            text(netIp, session == null || session.ip.isEmpty() ? "—" : session.ip);
            String duration = session == null ? "" : PortalProtocol.onlineDuration(session.loginTime, java.time.LocalDateTime.now());
            text(netDuration, duration.isEmpty() ? "—" : duration);
            text(netPortal, state.status.endpoint == null ? "尚未发现"
                : state.status.endpoint.host + ":" + state.status.endpoint.port);
            text(netChecked, lastCheckText());
        }
    }
    private String lastCheckText() {
        if (statusAt == 0) return "—";
        long seconds = (SystemClock.elapsedRealtime() - statusAt) / 1000;
        return seconds < 2 ? "刚刚" : seconds + " 秒前";
    }
    private void text(TextView view, String value) { if (!TextUtils.equals(view.getText(), value)) view.setText(value); }
    private void openSettings(Intent intent) {
        try { startActivity(intent); } catch (RuntimeException ex) { error("系统暂不支持此入口，请在系统设置中打开"); }
    }
    private void error(String message) { Toast.makeText(this, message, Toast.LENGTH_LONG).show(); }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != EXPORT || result != RESULT_OK || data == null || data.getData() == null) return;
        try (OutputStream output = getContentResolver().openOutputStream(data.getData(), "wt")) {
            if (output == null) throw new IOException("无法打开目标文件");
            output.write(store.diagnostic().getBytes(StandardCharsets.UTF_8));
            error("诊断报告已导出");
        } catch (IOException | RuntimeException ex) { error("导出失败，请重新选择保存位置"); }
    }
    // API 33+ uses the platform OnBackInvokedDispatcher registered in onCreate.
    // This override exists solely for Android 8-12, without an AndroidX dependency.
    @android.annotation.SuppressLint("GestureBackNavigation")
    @Override public void onBackPressed() {
        back();
    }
}
