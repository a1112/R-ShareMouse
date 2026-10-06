package org.rsharemouse.mobile;

import android.content.Intent;
import android.os.Bundle;
import android.content.pm.ActivityInfo;
import android.graphics.Color;
import android.view.View;
import android.view.LayoutInflater;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.EditText;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.util.HashSet;
import java.net.URI;
import java.util.Set;
import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import com.google.zxing.client.android.Intents;
import com.journeyapps.barcodescanner.ScanContract;
import com.journeyapps.barcodescanner.ScanOptions;

public final class MainActivity extends ComponentActivity {
    private static final String PREFS = "mobile_connection";
    private static final String SAVED_URL = "gateway_url";
    private static final String SAVED_HOST_NAME = "gateway_name";
    private static final String SAVED_HOST_ADDRESS = "gateway_address";

    private View connectionView;
    private View controllerView;
    private EditText urlInput;
    private TextView feedback;
    private WebView webView;
    private String activeUrl;
    private DiscoveryClient discovery;
    private final Set<String> discoveredAddresses = new HashSet<>();
    private LinearLayout discoveredHosts;
    private TextView discoveryStatus;
    private Button keyboardModeButton;
    private Button touchModeButton;
    private Button gamepadModeButton;
    private View manualSection;
    private Button manualToggle;
    private TextView controllerConnectionStatus;
    private String controlMode = "touch";
    private final ActivityResultLauncher<ScanOptions> scanner = registerForActivityResult(
            new ScanContract(), result -> {
                if (result.getContents() != null) connect(result.getContents());
                else if (result.getOriginalIntent() != null
                        && result.getOriginalIntent().getBooleanExtra(Intents.Scan.MISSING_CAMERA_PERMISSION, false)) {
                    feedback.setText("相机权限不可用。请使用自动发现或手动连接。");
                }
            });

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        discovery = new DiscoveryClient(this);
        setContentView(R.layout.activity_main);
        connectionView = findViewById(R.id.connection_view);
        controllerView = findViewById(R.id.controller_view);
        urlInput = findViewById(R.id.url_input);
        feedback = findViewById(R.id.feedback_text);
        webView = findViewById(R.id.controller_webview);
        discoveredHosts = findViewById(R.id.discovered_hosts);
        discoveryStatus = findViewById(R.id.discovery_status);
        keyboardModeButton = findViewById(R.id.keyboard_mode_button);
        touchModeButton = findViewById(R.id.touch_mode_button);
        gamepadModeButton = findViewById(R.id.gamepad_mode_button);
        manualSection = findViewById(R.id.manual_section);
        manualToggle = findViewById(R.id.manual_toggle);
        controllerConnectionStatus = findViewById(R.id.controller_connection_status);
        updateModeButtons();

        String savedUrl = getSharedPreferences(PREFS, MODE_PRIVATE).getString(SAVED_URL, "");
        urlInput.setText(savedUrl);
        webView.getSettings().setJavaScriptEnabled(true);
        webView.getSettings().setDomStorageEnabled(true);
        webView.getSettings().setAllowFileAccess(false);
        webView.getSettings().setAllowContentAccess(false);
        webView.getSettings().setMixedContentMode(android.webkit.WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        webView.getSettings().setSafeBrowsingEnabled(true);
        webView.setWebViewClient(new WebViewClient() {
            @Override
            public void onPageFinished(WebView view, String url) {
                if (activeUrl != null && activeUrl.equals(url)) {
                    controllerConnectionStatus.setText("● 已连接");
                    controllerConnectionStatus.setTextColor(Color.rgb(67, 217, 155));
                    view.evaluateJavascript("document.body.classList.add('android-host');window.rshareSetMode && window.rshareSetMode('" + controlMode + "')", null);
                }
            }

            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                return true;
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame() && activeUrl != null
                        && activeUrl.equals(request.getUrl().toString())) {
                    showConnectionError();
                }
            }

            @Override
            public void onReceivedHttpError(WebView view, WebResourceRequest request, WebResourceResponse errorResponse) {
                if (request.isForMainFrame() && activeUrl != null
                        && activeUrl.equals(request.getUrl().toString())) {
                    showConnectionError();
                }
            }
        });

        findViewById(R.id.connect_button).setOnClickListener(view -> connect(urlInput.getText().toString()));
        findViewById(R.id.disconnect_button).setOnClickListener(view -> disconnect());
        findViewById(R.id.refresh_button).setOnClickListener(view -> startDiscovery());
        manualToggle.setOnClickListener(view -> toggleManualConnection());
        findViewById(R.id.settings_button).setOnClickListener(view -> toggleManualConnection());
        touchModeButton.setOnClickListener(view -> setControlMode("touch"));
        keyboardModeButton.setOnClickListener(view -> setControlMode("keyboard"));
        gamepadModeButton.setOnClickListener(view -> setControlMode("gamepad"));
        findViewById(R.id.scan_button).setOnClickListener(view -> {
            try {
                scanner.launch(new ScanOptions()
                        .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                        .setPrompt(getString(R.string.scan_prompt))
                        .setBeepEnabled(false)
                        .setOrientationLocked(true));
            } catch (RuntimeException error) {
                feedback.setText("无法启动扫码，请使用自动发现或手动连接。");
            }
        });
        handleSharedUrl(getIntent());
        startDiscovery();
        if (activeUrl == null && !savedUrl.isEmpty()) connect(savedUrl);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleSharedUrl(intent);
    }

    private void handleSharedUrl(Intent intent) {
        if (intent == null || !Intent.ACTION_SEND.equals(intent.getAction())) {
            return;
        }
        String sharedUrl = intent.getStringExtra(Intent.EXTRA_TEXT);
        if (sharedUrl != null) {
            connect(sharedUrl);
        }
    }

    private void connect(String input) {
        String url = MobileUrl.parse(input);
        if (url == null) {
            feedback.setText(R.string.invalid_url);
            return;
        }
        feedback.setText("");
        urlInput.setText(url);
        getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(SAVED_URL, url).apply();
        activeUrl = url;
        String address = URI.create(url).getHost();
        String savedAddress = getSharedPreferences(PREFS, MODE_PRIVATE).getString(SAVED_HOST_ADDRESS, "");
        String hostLabel = address.equals(savedAddress)
                ? getSharedPreferences(PREFS, MODE_PRIVATE).getString(SAVED_HOST_NAME, address)
                : address;
        ((TextView) findViewById(R.id.controller_host)).setText(hostLabel);
        connectionView.setVisibility(View.GONE);
        controllerView.setVisibility(View.VISIBLE);
        webView.loadUrl(url);
    }

    private void startDiscovery() {
        discoveredAddresses.clear();
        discoveredHosts.removeAllViews();
        discoveryStatus.setText("正在搜索同一局域网内的电脑…");
        discovery.discover(host -> runOnUiThread(() -> showDiscoveredHost(host)),
                () -> runOnUiThread(() -> {
                    if (discoveredAddresses.isEmpty()) discoveryStatus.setText("未发现电脑。请确认电脑端移动网关已启用，且双方处于同一 Wi-Fi。");
                    else discoveryStatus.setText("选择电脑，随后在电脑端确认配对。");
                }));
    }

    private void showDiscoveredHost(DiscoveryClient.Host host) {
        if (!discoveredAddresses.add(host.address)) return;
        discoveryStatus.setText("选择电脑，随后在电脑端确认配对。");
        View card = LayoutInflater.from(this).inflate(R.layout.item_discovered_host, discoveredHosts, false);
        LinearLayout.LayoutParams cardParams = new LinearLayout.LayoutParams(-1, -2);
        cardParams.bottomMargin = dp(12);
        discoveredHosts.addView(card, cardParams);
        TextView name = card.findViewById(R.id.host_name);
        name.setText(host.name);
        ((TextView) card.findViewById(R.id.host_address)).setText(host.address);
        Button pair = card.findViewById(R.id.host_pair_button);
        TextView status = card.findViewById(R.id.host_pair_status);
        pair.setOnClickListener(view -> beginPairing(host, pair, status));
    }

    private void beginPairing(DiscoveryClient.Host host, Button pair, TextView status) {
        pair.setEnabled(false);
        pair.setText("等待电脑端确认…");
        status.setText("● 等待电脑端确认");
        feedback.setText("请在电脑的 设置 → 移动端控制 中允许这台手机。");
        discovery.requestPair(host, android.os.Build.MANUFACTURER + " " + android.os.Build.MODEL,
                id -> discovery.waitForApproval(host, id,
                        token -> runOnUiThread(() -> {
                            getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                                    .putString(SAVED_HOST_NAME, host.name)
                                    .putString(SAVED_HOST_ADDRESS, host.address).apply();
                            connect(host.baseUrl() + "/mobile?t=" + token);
                        }),
                        error -> runOnUiThread(() -> pairingFailed(pair, status, error))),
                error -> runOnUiThread(() -> pairingFailed(pair, status, error)));
    }

    private void pairingFailed(Button pair, TextView status, Exception error) {
        pair.setEnabled(true);
        pair.setText("重新请求配对");
        status.setText("● 配对未完成");
        feedback.setText("配对失败：" + error.getMessage());
    }

    private void toggleManualConnection() {
        boolean expanded = manualSection.getVisibility() != View.VISIBLE;
        manualSection.setVisibility(expanded ? View.VISIBLE : View.GONE);
        manualToggle.setText(expanded ? "－  收起手动连接" : "＋  手动连接电脑");
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private void setControlMode(String mode) {
        controlMode = mode;
        updateModeButtons();
        setRequestedOrientation("touch".equals(mode) ? ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT
                : ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE);
        if (activeUrl != null) {
            webView.evaluateJavascript("window.rshareSetMode && window.rshareSetMode('" + mode + "')", null);
        }
    }

    private void updateModeButtons() {
        Button[] buttons = {touchModeButton, keyboardModeButton, gamepadModeButton};
        String[] modes = {"touch", "keyboard", "gamepad"};
        for (int i = 0; i < buttons.length; i++) {
            boolean selected = modes[i].equals(controlMode);
            buttons[i].setBackgroundResource(selected ? R.drawable.mobile_selected_tab : R.drawable.mobile_outline_button);
            buttons[i].setTextColor(selected ? Color.rgb(74, 233, 167) : Color.rgb(179, 200, 187));
        }
    }

    private void showConnectionError() {
        disconnect();
        feedback.setText(R.string.load_error);
    }

    private void disconnect() {
        if (!"touch".equals(controlMode)) setControlMode("touch");
        activeUrl = null;
        webView.evaluateJavascript("window.dispatchEvent(new Event('pagehide'))", value -> webView.loadUrl("about:blank"));
        controllerView.setVisibility(View.GONE);
        connectionView.setVisibility(View.VISIBLE);
    }

    @Override
    protected void onPause() {
        if (activeUrl != null) {
            webView.evaluateJavascript("window.dispatchEvent(new Event('pagehide'))", null);
        }
        webView.onPause();
        super.onPause();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (webView != null) {
            webView.onResume();
            if (activeUrl != null) {
                webView.evaluateJavascript("window.dispatchEvent(new Event('pageshow'))", null);
            }
        }
    }

    @Override
    public void onBackPressed() {
        if (activeUrl != null) {
            disconnect();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onDestroy() {
        discovery.close();
        webView.destroy();
        super.onDestroy();
    }
}
