import { useEffect, useState, type ReactNode } from "react";
import {
  LayoutGrid,
  Maximize2,
  Minus,
  Monitor,
  Play,
  RotateCcw,
  Settings,
  Square,
  Wifi,
  X,
} from "lucide-react";

import MonitorManager, {
  DeviceData as LayoutDevice,
  MonitorData,
} from "./components/MonitorManager";
import { buildDesktopViewModel } from "./desktop-model.mjs";
import {
  buildFooterStatus,
  getHeaderMetrics,
  getPageLabels,
  getThemeModeOptions,
} from "./desktop-shell.mjs";
import {
  buildPageChrome,
  FIGMA_DESKTOP_THEME,
  getDesktopTheme,
} from "./desktop-theme.mjs";

type DesktopPage = "layout" | "devices" | "settings";

type Direction = "Left" | "Right" | "Top" | "Bottom";

type LayoutGraph = {
  version: number;
  local_device: string;
  nodes: LayoutNode[];
  links: LayoutLink[];
};

type LayoutNode = {
  device_id: string;
  displays: DisplayNode[];
};

type DisplayNode = {
  display_id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
};

type LayoutLink = {
  from_device: string;
  from_edge: Direction;
  to_device: string;
  to_edge: Direction;
};

type Config = {
  network: {
    port: number;
    bind_address: string;
    mdns_enabled: boolean;
  };
  gui: {
    minimize_to_tray: boolean;
    show_notifications: boolean;
    start_minimized: boolean;
    show_tray_icon: boolean;
  };
  input: {
    clipboard_sync: boolean;
    edge_threshold: number;
    mouse_wheel_sync: boolean;
    key_delay_ms: number;
  };
  security: {
    password_required: boolean;
    encryption: boolean;
    lan_only: boolean;
  };
  known_devices: string[];
};

const SCALE_FACTOR = 0.12;

const DEFAULT_CONFIG: Config = {
  network: { port: 27431, bind_address: "0.0.0.0", mdns_enabled: true },
  gui: {
    minimize_to_tray: true,
    show_notifications: true,
    start_minimized: false,
    show_tray_icon: true,
  },
  input: {
    clipboard_sync: true,
    edge_threshold: 10,
    mouse_wheel_sync: true,
    key_delay_ms: 0,
  },
  security: {
    password_required: false,
    encryption: true,
    lan_only: true,
  },
  known_devices: [],
};

type DashboardPayload = {
  status: unknown;
  devices: Array<{
    id: string;
    name: string;
    hostname: string;
    addresses?: string[];
    connected: boolean;
    last_seen_secs?: number | null;
  }>;
};

type TauriInvoke = <T = unknown>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

type ThemeMode = "light" | "dark" | "system";

const POLL_INTERVAL_MS = 1500;

const EMPTY_PAYLOAD: DashboardPayload = {
  status: null,
  devices: [],
};

const PAGE_LABELS: Array<{ key: DesktopPage; label: string }> = getPageLabels();

function getInvoke(): TauriInvoke | null {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      core?: {
        invoke?: TauriInvoke;
      };
    };
  };

  return tauriWindow.__TAURI__?.core?.invoke ?? null;
}

async function invokeCommand<T = unknown>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const invoke = getInvoke();
  if (!invoke) {
    throw new Error("Tauri bridge unavailable");
  }

  return invoke<T>(command, args);
}

function getLayoutDevices(layoutDevices: Array<Record<string, unknown>>): LayoutDevice[] {
  return layoutDevices.map((device) => ({
    id: String(device.id),
    name: String(device.name),
    color: String(device.color),
    online: Boolean(device.online),
    connected: Boolean(device.connected),
    type: device.type === "laptop" ? "laptop" : "desktop",
    expanded: true,
  }));
}

function getLayoutMonitors(layoutMonitors: Array<Record<string, unknown>>): MonitorData[] {
  return layoutMonitors.map((monitor) => ({
    id: String(monitor.id),
    label: String(monitor.label),
    name: String(monitor.name),
    deviceId: String(monitor.deviceId),
    resWidth: Number(monitor.resWidth),
    resHeight: Number(monitor.resHeight),
    color: String(monitor.color),
    x: Number(monitor.x),
    y: Number(monitor.y),
    w: Number(monitor.w),
    h: Number(monitor.h),
    primary: Boolean(monitor.primary),
    enabled: Boolean(monitor.enabled),
  }));
}

export default function App() {
  const [page, setPage] = useState<DesktopPage>("layout");
  const [payload, setPayload] = useState<DashboardPayload>(EMPTY_PAYLOAD);
  const [busy, setBusy] = useState(false);
  const [themeMode, setThemeMode] = useState<ThemeMode>("system");
  const [systemPrefersDark, setSystemPrefersDark] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const [config, setConfig] = useState<Config>(DEFAULT_CONFIG);
  const [editConfig, setEditConfig] = useState<Config | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);

  const model = buildDesktopViewModel(payload);
  const layoutDevices = getLayoutDevices(model.layout.devices);
  const layoutMonitors = getLayoutMonitors(model.layout.monitors);
  const isDark = themeMode === "system" ? systemPrefersDark : themeMode === "dark";
  const theme = getDesktopTheme(isDark);
  const chrome = buildPageChrome(page, theme);
  const footerStatus = buildFooterStatus(model);
  const headerMetrics = getHeaderMetrics();

  async function refreshDashboard() {
    try {
      const snapshot = await invokeCommand<DashboardPayload>("dashboard_state");
      setPayload(snapshot);
      setError(null);
    } catch (refreshError) {
      setPayload(EMPTY_PAYLOAD);
      setError(String(refreshError));
    }
  }

  useEffect(() => {
    refreshDashboard();
    const timer = window.setInterval(() => {
      refreshDashboard();
      setRefreshTick((value) => value + 1);
    }, POLL_INTERVAL_MS);

    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const applyPreference = () => setSystemPrefersDark(media.matches);

    applyPreference();
    media.addEventListener("change", applyPreference);

    return () => media.removeEventListener("change", applyPreference);
  }, []);

  async function runServiceAction(action: "start" | "stop") {
    setBusy(true);
    try {
      if (action === "start") {
        await invokeCommand("start_service");
      } else {
        await invokeCommand("stop_service");
      }
      await refreshDashboard();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  }

  async function connectDevice(deviceId: string) {
    setBusy(true);
    try {
      await invokeCommand("connect_device", { device_id: deviceId });
      await refreshDashboard();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  }

  async function disconnectDevice(deviceId: string) {
    setBusy(true);
    try {
      await invokeCommand("disconnect_device", { device_id: deviceId });
      await refreshDashboard();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  }

  async function handleWindow(command: "minimize_window" | "toggle_maximize_window" | "close_window") {
    try {
      await invokeCommand(command);
    } catch (windowError) {
      setError(String(windowError));
    }
  }

  async function loadConfig() {
    try {
      const loadedConfig = await invokeCommand<Config>("get_config");
      setConfig(loadedConfig);
      setEditConfig(null);
      setConfigError(null);
    } catch (err) {
      setConfigError(String(err));
    }
  }

  async function saveConfig(newConfig: Config) {
    setBusy(true);
    try {
      await invokeCommand("set_config", { config: newConfig });
      setConfig(newConfig);
      setEditConfig(null);
      setConfigError(null);
    } catch (err) {
      setConfigError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function startEditConfig() {
    setEditConfig({ ...config });
  }

  function cancelEditConfig() {
    setEditConfig(null);
    setConfigError(null);
  }

  async function saveLayout(monitors: Array<{ id: string; deviceId: string; x: number; y: number; w: number; h: number; resWidth: number; resHeight: number; primary: boolean }>) {
    setBusy(true);
    try {
      // Get local device ID from status
      const status = payload.status as { local_device_id?: string } | null;
      const localDeviceId = status?.local_device_id || "00000000-0000-0000-0000-000000000000";

      // Group monitors by device
      const deviceMap = new Map<string, typeof monitors>();
      for (const mon of monitors) {
        const deviceMonitors = deviceMap.get(mon.deviceId) || [];
        deviceMonitors.push(mon);
        deviceMap.set(mon.deviceId, deviceMonitors);
      }

      // Create nodes
      const nodes: LayoutNode[] = [];
      for (const [deviceId, deviceMonitors] of deviceMap) {
        const displays: DisplayNode[] = deviceMonitors.map((m, i) => ({
          display_id: m.id,
          x: Math.round((m.x / SCALE_FACTOR) / 0.12),
          y: Math.round((m.y / SCALE_FACTOR) / 0.12),
          width: m.resWidth,
          height: m.resHeight,
          primary: m.primary,
        }));
        nodes.push({ device_id: deviceId, displays });
      }

      // Create links based on adjacent monitors
      const links: LayoutLink[] = [];
      for (let i = 0; i < monitors.length; i++) {
        for (let j = i + 1; j < monitors.length; j++) {
          const a = monitors[i], b = monitors[j];
          const touchRight = Math.abs(a.x + a.w - b.x) < 6;
          const touchLeft = Math.abs(b.x + b.w - a.x) < 6;
          const touchBottom = Math.abs(a.y + a.h - b.y) < 6;
          const touchTop = Math.abs(b.y + b.h - a.y) < 6;
          const overlapH = a.y < b.y + b.h && a.y + a.h > b.y;
          const overlapV = a.x < b.x + b.w && a.x + a.w > b.x;

          if (touchRight && overlapH) {
            links.push({ from_device: a.deviceId, from_edge: "Right", to_device: b.deviceId, to_edge: "Left" });
            links.push({ from_device: b.deviceId, from_edge: "Left", to_device: a.deviceId, to_edge: "Right" });
          }
          if (touchBottom && overlapV) {
            links.push({ from_device: a.deviceId, from_edge: "Bottom", to_device: b.deviceId, to_edge: "Top" });
            links.push({ from_device: b.deviceId, from_edge: "Top", to_device: a.deviceId, to_edge: "Bottom" });
          }
        }
      }

      const layout: LayoutGraph = {
        version: 1,
        local_device: localDeviceId,
        nodes,
        links,
      };

      await invokeCommand("set_layout", { layout });
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    loadConfig();
  }, []);

  return (
    <div
      className="flex h-full min-h-0 flex-col overflow-hidden"
      style={{
        background: theme.frame,
        color: theme.text,
      }}
    >
      <header
        className="flex h-12 shrink-0 items-center"
        style={{
          borderBottom: `1px solid ${theme.border}`,
          background: theme.toolbar,
          paddingLeft: headerMetrics.headerPaddingX,
          paddingRight: headerMetrics.headerPaddingX,
        }}
        data-tauri-drag-region="true"
      >
        <div
          className="flex min-w-[204px] items-center"
          style={{ gap: headerMetrics.brandGap }}
          data-tauri-drag-region="true"
        >
          <div
            className="flex h-8 w-8 items-center justify-center rounded-md"
            style={{
              background: theme.accentSoft,
              color: theme.accent,
              border: `1px solid ${theme.border}`,
            }}
          >
            <Monitor size={16} />
          </div>
          <div className="leading-tight" data-tauri-drag-region="true">
            <div className="text-sm font-semibold">R-ShareMouse</div>
            <div className="text-[11px]" style={{ color: theme.textSub }}>
              共享桌面控制
            </div>
          </div>
        </div>

        <div
          className="ml-3 flex items-center"
          style={{ gap: headerMetrics.navGap }}
          data-tauri-drag-region="false"
        >
          {PAGE_LABELS.map((item) => (
            <button
              key={item.key}
              type="button"
              className="rounded-md text-sm transition"
              style={{
                background: page === item.key ? theme.accentSoft : "transparent",
                color:
                  page === item.key
                    ? theme.text
                    : theme.textSub,
                border:
                  page === item.key
                    ? `1px solid ${theme.accent}`
                    : "1px solid transparent",
                boxShadow:
                  page === item.key
                    ? "inset 0 0 0 1px rgba(255,255,255,0.04)"
                    : "none",
                paddingLeft: headerMetrics.navButtonPaddingX,
                paddingRight: headerMetrics.navButtonPaddingX,
                paddingTop: headerMetrics.navButtonPaddingY,
                paddingBottom: headerMetrics.navButtonPaddingY,
              }}
              onClick={() => setPage(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>

        <div
          className="ml-auto flex items-center"
          style={{ gap: headerMetrics.actionGap }}
          data-tauri-drag-region="false"
        >
          <button
            type="button"
            className="rounded-md text-sm transition"
            style={{
              border: `1px solid ${theme.border}`,
              background: theme.sidebar,
              color: theme.textSub,
              paddingLeft: headerMetrics.actionButtonPaddingX,
              paddingRight: headerMetrics.actionButtonPaddingX,
              paddingTop: headerMetrics.actionButtonPaddingY,
              paddingBottom: headerMetrics.actionButtonPaddingY,
            }}
            onClick={refreshDashboard}
            title={`刷新 ${refreshTick}`}
          >
            <span className="flex items-center gap-2">
              <RotateCcw size={14} />
              刷新
            </span>
          </button>
          <button
            type="button"
            className="rounded-md text-sm transition"
            style={{
              background: model.service.online
                ? "rgba(197, 48, 48, 0.18)"
                : theme.accentSoft,
              color: model.service.online
                ? "#ffb8c1"
                : theme.text,
              border: `1px solid ${
                model.service.online
                  ? "rgba(197, 48, 48, 0.4)"
                  : theme.accent
              }`,
              paddingLeft: headerMetrics.actionButtonPaddingX,
              paddingRight: headerMetrics.actionButtonPaddingX,
              paddingTop: headerMetrics.actionButtonPaddingY,
              paddingBottom: headerMetrics.actionButtonPaddingY,
            }}
            disabled={busy}
            onClick={() => runServiceAction(model.service.online ? "stop" : "start")}
          >
            <span className="flex items-center gap-2">
              {model.service.online ? <Square size={13} /> : <Play size={13} />}
              {model.service.online ? "停止服务" : "启动服务"}
            </span>
          </button>
          <div
            className="ml-1 flex items-center"
            style={{ gap: headerMetrics.windowGap }}
          >
            <WindowButton onClick={() => handleWindow("minimize_window")} title="最小化" theme={theme} size={headerMetrics.windowButtonSize}>
              <Minus size={14} />
            </WindowButton>
            <WindowButton onClick={() => handleWindow("toggle_maximize_window")} title="最大化" theme={theme} size={headerMetrics.windowButtonSize}>
              <Maximize2 size={13} />
            </WindowButton>
            <WindowButton danger onClick={() => handleWindow("close_window")} title="关闭" theme={theme} size={headerMetrics.windowButtonSize}>
              <X size={14} />
            </WindowButton>
          </div>
        </div>
      </header>

      <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {error ? (
          <section
            className="mx-4 mt-3 px-4 py-3 text-sm"
            style={{
              border: "1px solid rgba(197, 48, 48, 0.45)",
              background: "rgba(94, 24, 34, 0.55)",
              color: "#ffb8c1",
            }}
          >
            {error}
          </section>
        ) : null}

        <div
          className="min-h-0 flex-1 overflow-hidden"
          style={{
            padding: chrome.contentPadding,
            background: page === "layout" ? chrome.surface : theme.canvas,
          }}
        >
          {page === "layout" ? (
            <MonitorManager
              devices={layoutDevices}
              monitors={layoutMonitors}
              isDark={isDark}
              showThemeToggle={false}
              showFooter={false}
              statusText={`布局画布 · ${model.devices.length} 台远端设备`}
              footerText="拖拽只影响当前界面展示。"
              onSaveLayout={saveLayout}
              busy={busy}
            />
          ) : null}

          {page === "devices" ? (
            <DevicesPage
              busy={busy}
              devices={model.devices}
              onConnect={connectDevice}
              onDisconnect={disconnectDevice}
              theme={theme}
            />
          ) : null}

          {page === "settings" ? (
            <SettingsPage
              localDevice={model.settings.localDevice}
              inputMode={model.settings.inputMode}
              privilegeState={model.settings.privilegeState}
              service={model.service}
              themeMode={themeMode}
              onThemeModeChange={setThemeMode}
              onToggleService={() =>
                runServiceAction(model.service.online ? "stop" : "start")
              }
              busy={busy}
              theme={theme}
              config={config}
              editConfig={editConfig}
              configError={configError}
              onStartEdit={startEditConfig}
              onCancelEdit={cancelEditConfig}
              onSaveConfig={saveConfig}
              onEditConfigChange={setEditConfig}
            />
          ) : null}
        </div>

        <footer
          className="flex h-8 shrink-0 items-center gap-3 px-4 text-xs"
          style={{
            borderTop: `1px solid ${theme.border}`,
            background: theme.sidebar,
            color: theme.textMuted,
          }}
        >
          <div
            className="h-2 w-2 rounded-full"
            style={{
              background: model.service.online
                ? model.service.healthy
                  ? theme.success
                  : "#d6a64b"
                : theme.textMuted,
            }}
          />
          <span>{footerStatus.summary}</span>
          <div className="ml-auto flex items-center gap-2">
            <Wifi size={12} />
            <span>{footerStatus.endpoint}</span>
          </div>
        </footer>
      </main>
    </div>
  );
}

function DevicesPage({
  devices,
  onConnect,
  onDisconnect,
  busy,
  theme,
}: {
  devices: Array<{
    id: string;
    name: string;
    hostname: string;
    address: string;
    connected: boolean;
    online: boolean;
    lastSeenLabel: string;
  }>;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  if (!devices.length) {
    return (
      <EmptyPanel
        title="尚未发现设备"
        detail="启动守护进程并保持同一局域网后，发现到的设备会同时出现在设备页和布局页。"
        theme={theme}
      />
    );
  }

  return (
    <div className="grid h-full grid-cols-1 gap-3 overflow-auto xl:grid-cols-2">
      {devices.map((device) => (
        <article
          key={device.id}
          className="p-5"
          style={{
            background: theme.sidebar,
            border: `1px solid ${theme.border}`,
            boxShadow: theme.panelShadow,
          }}
        >
          <div className="flex items-start gap-4">
            <div
              className="flex h-12 w-12 items-center justify-center rounded-md"
              style={{
                background: theme.accentSoft,
                color: theme.accent,
              }}
            >
              <Monitor size={18} />
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <h2 className="truncate text-lg font-semibold">{device.name}</h2>
                <span
                  className="rounded px-2 py-0.5 text-xs"
                  style={{
                    background: device.connected
                      ? "rgba(73, 179, 92, 0.16)"
                      : "rgba(255,255,255,0.04)",
                    color: device.connected
                      ? "#8de29d"
                      : theme.textSub,
                  }}
                >
                  {device.connected ? "已连接" : "已发现"}
                </span>
              </div>
              <div className="mt-1 text-sm" style={{ color: theme.textMuted }}>
                {device.hostname}
              </div>
            </div>
            <button
              type="button"
              className="rounded-md px-4 py-2 text-sm transition"
              style={{
                background: device.connected
                  ? "rgba(197, 48, 48, 0.18)"
                  : theme.accentSoft,
                color: device.connected
                  ? "#ffb5c0"
                  : theme.text,
                border: `1px solid ${
                  device.connected
                    ? "rgba(197, 48, 48, 0.35)"
                    : theme.accent
                }`,
              }}
              disabled={busy}
              onClick={() =>
                device.connected ? onDisconnect(device.id) : onConnect(device.id)
              }
            >
              {device.connected ? "断开连接" : "连接"}
            </button>
          </div>

          <div className="mt-4 grid grid-cols-2 gap-3 text-sm">
            <InfoRow label="地址" value={device.address} theme={theme} />
            <InfoRow label="最近出现" value={device.lastSeenLabel} theme={theme} />
            <InfoRow label="状态" value={device.online ? "可达" : "离线"} theme={theme} />
            <InfoRow label="布局映射" value={device.connected ? "已联动" : "空闲"} theme={theme} />
          </div>
        </article>
      ))}
    </div>
  );
}

function SettingsPage({
  localDevice,
  inputMode,
  privilegeState,
  service,
  themeMode,
  onThemeModeChange,
  onToggleService,
  busy,
  theme,
  config,
  editConfig,
  configError,
  onStartEdit,
  onCancelEdit,
  onSaveConfig,
  onEditConfigChange,
}: {
  localDevice: {
    name: string;
    hostname: string;
    bindAddress: string;
    discoveryPort: number | null;
    pid: number | null;
  };
  inputMode: {
    current: string;
    available: string[];
    health: string;
    reason: string | null;
  };
  privilegeState: string;
  service: {
    online: boolean;
    healthy: boolean;
    discoveredDevices: number;
    connectedDevices: number;
  };
  themeMode: ThemeMode;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleService: () => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
  config: Config;
  editConfig: Config | null;
  configError: string | null;
  onStartEdit: () => void;
  onCancelEdit: () => void;
  onSaveConfig: (newConfig: Config) => void;
  onEditConfigChange: (newConfig: Config | null) => void;
}) {
  const activeConfig = editConfig ?? config;
  const isEditing = editConfig !== null;

  function updateConfigField<K extends keyof Config>(key: K, value: Config[K]) {
    onEditConfigChange({ ...activeConfig, [key]: value });
  }

  function updateNetworkField<K extends keyof Config["network"]>(
    key: K,
    value: Config["network"][K]
  ) {
    onEditConfigChange({
      ...activeConfig,
      network: { ...activeConfig.network, [key]: value },
    });
  }

  function updateInputField<K extends keyof Config["input"]>(
    key: K,
    value: Config["input"][K]
  ) {
    onEditConfigChange({
      ...activeConfig,
      input: { ...activeConfig.input, [key]: value },
    });
  }

  function updateGuiField<K extends keyof Config["gui"]>(
    key: K,
    value: Config["gui"][K]
  ) {
    onEditConfigChange({
      ...activeConfig,
      gui: { ...activeConfig.gui, [key]: value },
    });
  }

  return (
    <div className="grid h-full grid-cols-1 gap-3 overflow-auto xl:grid-cols-[1.1fr_0.9fr]">
      <section
        className="p-5"
        style={{
          background: theme.sidebar,
          border: `1px solid ${theme.border}`,
          boxShadow: theme.panelShadow,
        }}
      >
        <div className="mb-5 flex items-center gap-3">
          <div
            className="flex h-11 w-11 items-center justify-center rounded-md"
            style={{
              background: theme.accentSoft,
              color: theme.accent,
            }}
          >
            <Settings size={18} />
          </div>
          <div>
            <h2 className="text-lg font-semibold">本机信息</h2>
            <p className="text-sm" style={{ color: theme.textMuted }}>
              当前界面显示的是守护进程快照提供的最小设置集。
            </p>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3 text-sm">
          <InfoRow label="设备名" value={localDevice.name} theme={theme} />
          <InfoRow label="主机名" value={localDevice.hostname} theme={theme} />
          <InfoRow label="监听地址" value={localDevice.bindAddress} theme={theme} />
          <InfoRow label="发现端口" value={localDevice.discoveryPort == null ? "不可用" : String(localDevice.discoveryPort)} theme={theme} />
          <InfoRow label="守护进程 PID" value={localDevice.pid == null ? "不可用" : String(localDevice.pid)} theme={theme} />
          <InfoRow label="权限状态" value={privilegeState} theme={theme} />
        </div>
      </section>

      <div className="flex flex-col gap-4">
        <section
          className="p-5"
          style={{
            background: theme.sidebar,
            border: `1px solid ${theme.border}`,
            boxShadow: theme.panelShadow,
          }}
        >
          <div className="mb-4 flex items-center gap-3">
            <div
              className="flex h-11 w-11 items-center justify-center rounded-md"
              style={{
                background: "rgba(255,255,255,0.04)",
                color: theme.textSub,
              }}
            >
              <Wifi size={18} />
            </div>
            <div>
              <h2 className="text-lg font-semibold">服务状态</h2>
              <p className="text-sm" style={{ color: theme.textMuted }}>
                当前守护进程会话的快速运行信息。
              </p>
            </div>
          </div>

          <div className="space-y-3 text-sm">
            <InfoRow label="守护进程" value={service.online ? "运行中" : "已停止"} theme={theme} />
            <InfoRow label="健康度" value={service.healthy ? "正常" : "降级"} theme={theme} />
            <InfoRow label="已连接设备" value={String(service.connectedDevices)} theme={theme} />
            <InfoRow label="已发现设备" value={String(service.discoveredDevices)} theme={theme} />
          </div>

          <button
            type="button"
            className="mt-5 rounded-md px-4 py-2 text-sm transition"
            style={{
              background: service.online
                ? "rgba(197, 48, 48, 0.18)"
                : theme.accentSoft,
              color: service.online ? "#ffb5c0" : theme.text,
              border: `1px solid ${
                service.online
                  ? "rgba(197, 48, 48, 0.35)"
                  : theme.accent
              }`,
            }}
            disabled={busy}
            onClick={onToggleService}
          >
            {service.online ? "停止服务" : "启动服务"}
          </button>
        </section>

        <section
          className="p-5"
          style={{
            background: theme.sidebar,
            border: `1px solid ${theme.border}`,
            boxShadow: theme.panelShadow,
          }}
        >
          <div className="mb-4 flex items-center gap-3">
            <div
              className="flex h-11 w-11 items-center justify-center rounded-md"
              style={{
                background: "rgba(255,255,255,0.04)",
                color: theme.textSub,
              }}
            >
              <LayoutGrid size={18} />
            </div>
            <div>
              <h2 className="text-lg font-semibold">输入后端</h2>
              <p className="text-sm" style={{ color: theme.textMuted }}>
                当前输入模式以及降级可见性都来自守护进程。
              </p>
            </div>
          </div>

          <div className="space-y-3 text-sm">
            <InfoRow label="当前模式" value={inputMode.current} theme={theme} />
            <InfoRow label="健康度" value={inputMode.health} theme={theme} />
            <InfoRow label="原因" value={inputMode.reason ?? "无"} theme={theme} />
            <InfoRow
              label="可用后端"
              value={inputMode.available.length ? inputMode.available.join(", ") : "无"}
              theme={theme}
            />
          </div>
        </section>

        <section
          className="p-5"
          style={{
            background: theme.sidebar,
            border: `1px solid ${theme.border}`,
            boxShadow: theme.panelShadow,
          }}
        >
          <div className="mb-4 flex items-center gap-3">
            <div
              className="flex h-11 w-11 items-center justify-center rounded-md"
              style={{
                background: "rgba(255,255,255,0.04)",
                color: theme.textSub,
              }}
            >
              <Settings size={18} />
            </div>
            <div>
              <h2 className="text-lg font-semibold">界面风格</h2>
              <p className="text-sm" style={{ color: theme.textMuted }}>
                选择浅色、深色或跟随系统。
              </p>
            </div>
          </div>

          <div className="flex gap-2">
            {getThemeModeOptions().map((option) => (
              <button
                key={option.key}
                type="button"
                className="rounded-md px-4 py-2 text-sm transition"
                style={{
                  background:
                    themeMode === option.key ? theme.accentSoft : theme.frame,
                  color: themeMode === option.key ? theme.text : theme.textSub,
                  border: `1px solid ${
                    themeMode === option.key ? theme.accent : theme.border
                  }`,
                }}
                onClick={() => onThemeModeChange(option.key as ThemeMode)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </section>

        {/* Configuration Section */}
        <section
          className="p-5"
          style={{
            background: theme.sidebar,
            border: `1px solid ${theme.border}`,
            boxShadow: theme.panelShadow,
          }}
        >
          <div className="mb-4 flex items-center justify-between gap-3">
            <div className="flex items-center gap-3">
              <div
                className="flex h-11 w-11 items-center justify-center rounded-md"
                style={{
                  background: "rgba(255,255,255,0.04)",
                  color: theme.textSub,
                }}
              >
                <Settings size={18} />
              </div>
              <div>
                <h2 className="text-lg font-semibold">配置设置</h2>
                <p className="text-sm" style={{ color: theme.textMuted }}>
                  网络和输入行为配置
                </p>
              </div>
            </div>
            {!isEditing ? (
              <button
                type="button"
                className="rounded-md px-3 py-1.5 text-sm transition"
                style={{
                  background: theme.accentSoft,
                  color: theme.text,
                  border: `1px solid ${theme.accent}`,
                }}
                onClick={onStartEdit}
              >
                编辑配置
              </button>
            ) : null}
          </div>

          {isEditing ? (
            <div className="space-y-4">
              {/* Network Settings */}
              <div>
                <h3 className="mb-2 text-sm font-medium" style={{ color: theme.textSub }}>
                  网络设置
                </h3>
                <div className="space-y-2">
                  <div className="flex items-center gap-2">
                    <label className="w-20 text-sm" style={{ color: theme.textMuted }}>
                      端口
                    </label>
                    <input
                      type="number"
                      className="flex-1 rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: theme.frame,
                        border: `1px solid ${theme.border}`,
                        color: theme.text,
                      }}
                      value={activeConfig.network.port}
                      onChange={(e) =>
                        updateNetworkField("port", parseInt(e.target.value) || 27431)
                      }
                    />
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-20 text-sm" style={{ color: theme.textMuted }}>
                      绑定地址
                    </label>
                    <input
                      type="text"
                      className="flex-1 rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: theme.frame,
                        border: `1px solid ${theme.border}`,
                        color: theme.text,
                      }}
                      value={activeConfig.network.bind_address}
                      onChange={(e) =>
                        updateNetworkField("bind_address", e.target.value)
                      }
                    />
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-20 text-sm" style={{ color: theme.textMuted }}>
                      mDNS
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.network.mdns_enabled
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.network.mdns_enabled ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateNetworkField("mdns_enabled", !activeConfig.network.mdns_enabled)
                      }
                    >
                      {activeConfig.network.mdns_enabled ? "启用" : "禁用"}
                    </button>
                  </div>
                </div>
              </div>

              {/* Input Settings */}
              <div>
                <h3 className="mb-2 text-sm font-medium" style={{ color: theme.textSub }}>
                  输入设置
                </h3>
                <div className="space-y-2">
                  <div className="flex items-center gap-2">
                    <label className="w-24 text-sm" style={{ color: theme.textMuted }}>
                      剪贴板同步
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.input.clipboard_sync
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.input.clipboard_sync ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateInputField("clipboard_sync", !activeConfig.input.clipboard_sync)
                      }
                    >
                      {activeConfig.input.clipboard_sync ? "启用" : "禁用"}
                    </button>
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-24 text-sm" style={{ color: theme.textMuted }}>
                      边缘阈值
                    </label>
                    <input
                      type="number"
                      min="1"
                      max="100"
                      className="w-20 rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: theme.frame,
                        border: `1px solid ${theme.border}`,
                        color: theme.text,
                      }}
                      value={activeConfig.input.edge_threshold}
                      onChange={(e) =>
                        updateInputField(
                          "edge_threshold",
                          Math.max(1, Math.min(100, parseInt(e.target.value) || 10))
                        )
                      }
                    />
                    <span className="text-sm" style={{ color: theme.textMuted }}>
                      像素
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-24 text-sm" style={{ color: theme.textMuted }}>
                      鼠标滚轮
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.input.mouse_wheel_sync
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.input.mouse_wheel_sync ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateInputField(
                          "mouse_wheel_sync",
                          !activeConfig.input.mouse_wheel_sync
                        )
                      }
                    >
                      {activeConfig.input.mouse_wheel_sync ? "启用" : "禁用"}
                    </button>
                  </div>
                </div>
              </div>

              {/* GUI Settings */}
              <div>
                <h3 className="mb-2 text-sm font-medium" style={{ color: theme.textSub }}>
                  界面设置
                </h3>
                <div className="space-y-2">
                  <div className="flex items-center gap-2">
                    <label className="w-28 text-sm" style={{ color: theme.textMuted }}>
                      最小化到托盘
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.gui.minimize_to_tray
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.gui.minimize_to_tray ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateGuiField("minimize_to_tray", !activeConfig.gui.minimize_to_tray)
                      }
                    >
                      {activeConfig.gui.minimize_to_tray ? "启用" : "禁用"}
                    </button>
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-28 text-sm" style={{ color: theme.textMuted }}>
                      显示通知
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.gui.show_notifications
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.gui.show_notifications ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateGuiField(
                          "show_notifications",
                          !activeConfig.gui.show_notifications
                        )
                      }
                    >
                      {activeConfig.gui.show_notifications ? "启用" : "禁用"}
                    </button>
                  </div>
                  <div className="flex items-center gap-2">
                    <label className="w-28 text-sm" style={{ color: theme.textMuted }}>
                      显示托盘图标
                    </label>
                    <button
                      type="button"
                      className="rounded-md px-3 py-1.5 text-sm"
                      style={{
                        background: activeConfig.gui.show_tray_icon
                          ? theme.accentSoft
                          : theme.frame,
                        border: `1px solid ${
                          activeConfig.gui.show_tray_icon ? theme.accent : theme.border
                        }`,
                      }}
                      onClick={() =>
                        updateGuiField("show_tray_icon", !activeConfig.gui.show_tray_icon)
                      }
                    >
                      {activeConfig.gui.show_tray_icon ? "启用" : "禁用"}
                    </button>
                  </div>
                </div>
              </div>

              {configError && (
                <div
                  className="rounded-md px-3 py-2 text-sm"
                  style={{ background: "rgba(197, 48, 48, 0.16)", color: "#ffb5c0" }}
                >
                  {configError}
                </div>
              )}

              <div className="flex gap-2">
                <button
                  type="button"
                  className="rounded-md px-4 py-2 text-sm transition"
                  style={{
                    background: theme.accentSoft,
                    color: theme.text,
                    border: `1px solid ${theme.accent}`,
                  }}
                  disabled={busy}
                  onClick={() => onSaveConfig(activeConfig)}
                >
                  {busy ? "保存中..." : "保存配置"}
                </button>
                <button
                  type="button"
                  className="rounded-md px-4 py-2 text-sm transition"
                  style={{
                    background: theme.frame,
                    color: theme.textSub,
                    border: `1px solid ${theme.border}`,
                  }}
                  onClick={onCancelEdit}
                >
                  取消
                </button>
              </div>
            </div>
          ) : (
            <div className="space-y-3 text-sm">
              <InfoRow
                label="端口"
                value={String(config.network.port)}
                theme={theme}
              />
              <InfoRow
                label="绑定地址"
                value={config.network.bind_address}
                theme={theme}
              />
              <InfoRow
                label="剪贴板同步"
                value={config.input.clipboard_sync ? "启用" : "禁用"}
                theme={theme}
              />
              <InfoRow
                label="边缘阈值"
                value={`${config.input.edge_threshold} 像素`}
                theme={theme}
              />
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function WindowButton({
  children,
  onClick,
  title,
  danger = false,
  theme,
  size,
}: {
  children: ReactNode;
  onClick: () => void;
  title: string;
  danger?: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
  size: number;
}) {
  return (
    <button
      type="button"
      className="flex items-center justify-center rounded-md transition"
      onClick={onClick}
      title={title}
      style={{
        color: theme.textSub,
        width: size,
        height: size,
      }}
      onMouseEnter={(event) => {
        event.currentTarget.style.backgroundColor = danger
          ? theme.danger
          : "rgba(255,255,255,0.04)";
        event.currentTarget.style.color = danger ? "#ffffff" : theme.text;
      }}
      onMouseLeave={(event) => {
        event.currentTarget.style.backgroundColor = "transparent";
        event.currentTarget.style.color = theme.textSub;
      }}
    >
      {children}
    </button>
  );
}

function EmptyPanel({
  title,
  detail,
  theme,
}: {
  title: string;
  detail: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex h-full items-center justify-center p-8"
      style={{
        border: `1px dashed ${theme.border}`,
        background: theme.sidebar,
      }}
    >
      <div className="max-w-xl text-center">
        <div
          className="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-md"
          style={{
            background: theme.accentSoft,
            color: theme.accent,
          }}
        >
          <Monitor size={20} />
        </div>
        <h2 className="text-xl font-semibold">{title}</h2>
        <p className="mt-3 text-sm leading-6" style={{ color: theme.textMuted }}>
          {detail}
        </p>
      </div>
    </div>
  );
}

function InfoRow({
  label,
  value,
  theme,
}: {
  label: string;
  value: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="px-4 py-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: theme.frame,
      }}
    >
      <div
        className="mb-1 text-xs uppercase tracking-[0.16em]"
        style={{ color: theme.textMuted }}
      >
        {label}
      </div>
      <div className="break-all text-sm" style={{ color: theme.text }}>
        {value}
      </div>
    </div>
  );
}
