package org.rsharemouse.mobile;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkAddress;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;
import org.json.JSONObject;
import java.io.OutputStream;
import java.io.InputStream;
import java.io.ByteArrayOutputStream;
import java.net.HttpURLConnection;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

final class DiscoveryClient {
    static final int GATEWAY_PORT = 27437;
    private final Context context;
    private final ExecutorService io = Executors.newCachedThreadPool();
    private volatile boolean closed;

    DiscoveryClient(Context context) { this.context = context.getApplicationContext(); }

    static final class Host {
        final String address;
        final String name;
        Host(String address, String name) { this.address = address; this.name = name; }
        String baseUrl() { return "http://" + address + ":" + GATEWAY_PORT; }
    }

    void discover(Consumer<Host> found, Runnable finished) {
        io.execute(() -> {
            List<String> targets = localSubnetTargets(context);
            ExecutorService probes = Executors.newFixedThreadPool(32);
            for (String address : targets) {
                probes.execute(() -> {
                    if (closed) return;
                    try {
                        JSONObject response = get("http://" + address + ":" + GATEWAY_PORT + "/api/discover",
                                "127.0.0.1".equals(address) ? 2000 : 350);
                        if ("rshare-mobile".equals(response.optString("service"))) {
                            found.accept(new Host(address, response.optString("name", address)));
                        }
                    } catch (Exception ignored) { }
                });
            }
            probes.shutdown();
            try { probes.awaitTermination(15, TimeUnit.SECONDS); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            if (!closed) finished.run();
        });
    }

    void requestPair(Host host, String deviceName, Consumer<String> success, Consumer<Exception> failure) {
        io.execute(() -> {
            try {
                JSONObject body = new JSONObject().put("device_name", deviceName);
                String id = post(host.baseUrl() + "/api/pair/request", body).getString("request_id");
                success.accept(id);
            } catch (Exception error) { failure.accept(error); }
        });
    }

    void waitForApproval(Host host, String requestId, Consumer<String> approved, Consumer<Exception> failure) {
        io.execute(() -> {
            try {
                for (int attempt = 0; attempt < 60 && !closed; attempt++) {
                    JSONObject status = get(host.baseUrl() + "/api/pair/status?request_id=" + requestId, 2000);
                    if ("approved".equals(status.optString("status"))) {
                        approved.accept(status.getString("token"));
                        return;
                    }
                    Thread.sleep(2000);
                }
                if (!closed) failure.accept(new IllegalStateException("配对已超时"));
            } catch (Exception error) { if (!closed) failure.accept(error); }
        });
    }

    void close() { closed = true; io.shutdownNow(); }

    static List<String> localSubnetTargets(Context context) {
        Set<String> targets = new LinkedHashSet<>();
        try {
            ConnectivityManager manager = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (manager == null) return DiscoveryTargets.withEmulatorLoopback(new ArrayList<>(), isEmulator());
            for (Network network : manager.getAllNetworks()) {
                NetworkCapabilities capabilities = manager.getNetworkCapabilities(network);
                if (capabilities == null || !(capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                        || capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))) continue;
                LinkProperties properties = manager.getLinkProperties(network);
                if (properties == null) continue;
                for (LinkAddress link : properties.getLinkAddresses()) {
                    InetAddress address = link.getAddress();
                    if (!(address instanceof Inet4Address) || !address.isSiteLocalAddress()) continue;
                    targets.addAll(DiscoveryTargets.forPrivateIpv4(address.getHostAddress()));
                }
            }
        } catch (Exception ignored) { }
        return DiscoveryTargets.withEmulatorLoopback(new ArrayList<>(targets), isEmulator());
    }

    private static boolean isEmulator() {
        return "ranchu".equals(Build.HARDWARE) || "goldfish".equals(Build.HARDWARE);
    }

    private static JSONObject get(String url, int timeoutMs) throws Exception {
        HttpURLConnection connection = (HttpURLConnection) new URL(url).openConnection();
        connection.setConnectTimeout(timeoutMs);
        connection.setReadTimeout(timeoutMs);
        try {
            if (connection.getResponseCode() != 200) throw new IllegalStateException("HTTP " + connection.getResponseCode());
            return readJson(connection);
        } finally { connection.disconnect(); }
    }

    private static JSONObject post(String url, JSONObject body) throws Exception {
        HttpURLConnection connection = (HttpURLConnection) new URL(url).openConnection();
        connection.setConnectTimeout(3000);
        connection.setReadTimeout(3000);
        connection.setRequestMethod("POST");
        connection.setRequestProperty("Content-Type", "application/json");
        connection.setDoOutput(true);
        byte[] bytes = body.toString().getBytes(StandardCharsets.UTF_8);
        connection.setFixedLengthStreamingMode(bytes.length);
        try {
            try (OutputStream stream = connection.getOutputStream()) {
                stream.write(bytes);
            }
            if (connection.getResponseCode() != 200) throw new IllegalStateException("HTTP " + connection.getResponseCode());
            return readJson(connection);
        } finally { connection.disconnect(); }
    }

    private static JSONObject readJson(HttpURLConnection connection) throws Exception {
        try (InputStream stream = connection.getInputStream(); ByteArrayOutputStream bytes = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[2048];
            int count;
            while ((count = stream.read(buffer)) != -1) bytes.write(buffer, 0, count);
            return new JSONObject(bytes.toString(StandardCharsets.UTF_8.name()));
        }
    }
}
