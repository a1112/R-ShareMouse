import { useEffect, useState, type ReactNode } from "react";
import {
  FileText,
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
import {
  buildDesktopViewModel,
  updateRememberedLayoutFromVisibleMonitors,
} from "./desktop-model.mjs";
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

type DesktopPage = "layout" | "devices" | "logs" | "settings";

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
  layout?: unknown | null;
  visible_layout?: unknown | null;
  layout_error?: string | null;
  auto_started?: boolean;
};

type TauriInvoke = <T = unknown>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

type ThemeMode = "light" | "dark" | "system";

const POLL_INTERVAL_MS = 1500;
const HIDDEN_MONITOR_IDS_STORAGE_KEY = "rshare.hiddenMonitorIds";

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

function loadHiddenMonitorIds(): Set<string> {
  try {
    const rawValue = window.localStorage.getItem(HIDDEN_MONITOR_IDS_STORAGE_KEY);
    const parsed = rawValue ? JSON.parse(rawValue) : [];
    return new Set(
      Array.isArray(parsed)
        ? parsed.filter((id): id is string => typeof id === "string")
        : [],
    );
  } catch {
    return new Set();
  }
}

function saveHiddenMonitorIds(hiddenMonitorIds: ReadonlySet<string>) {
  try {
    window.localStorage.setItem(
      HIDDEN_MONITOR_IDS_STORAGE_KEY,
      JSON.stringify([...hiddenMonitorIds]),
    );
  } catch {
    // Visibility is still preserved in memory when storage is unavailable.
  }
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

function getLayoutMonitors(
  layoutMonitors: Array<Record<string, unknown>>,
  hiddenMonitorIds: ReadonlySet<string> = new Set(),
): MonitorData[] {
  return layoutMonitors.map((monitor) => ({
    id: String(monitor.id),
    displayId:
      monitor.displayId == null ? undefined : String(monitor.displayId),
    rememberedX:
      monitor.rememberedX == null ? undefined : Number(monitor.rememberedX),
    rememberedY:
      monitor.rememberedY == null ? undefined : Number(monitor.rememberedY),
    visibleX:
      monitor.visibleX == null ? undefined : Number(monitor.visibleX),
    visibleY:
      monitor.visibleY == null ? undefined : Number(monitor.visibleY),
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
    enabled: Boolean(monitor.enabled) && !hiddenMonitorIds.has(String(monitor.id)),
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
  const [hiddenMonitorIds, setHiddenMonitorIds] = useState<Set<string>>(
    loadHiddenMonitorIds,
  );

  const model = buildDesktopViewModel(payload);
  const layoutDevices = getLayoutDevices(model.layout.devices);
  const layoutMonitors = getLayoutMonitors(model.layout.monitors, hiddenMonitorIds);
  const isDark = themeMode === "system" ? systemPrefersDark : themeMode === "dark";
  const theme = getDesktopTheme(isDark);
  const chrome = buildPageChrome(page, theme);
  const footerStatus = buildFooterStatus(model);
  const headerMetrics = getHeaderMetrics();

  async function refreshDashboard() {
    try {
      const snapshot = await invokeCommand<DashboardPayload>("dashboard_state");
      setPayload(snapshot);
      setError(snapshot.layout_error ? `布局异常：${snapshot.layout_error}` : null);
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

  useEffect(() => {
    saveHiddenMonitorIds(hiddenMonitorIds);
  }, [hiddenMonitorIds]);

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

  async function saveLayoutFromMonitors(monitors: MonitorData[]) {
    const rememberedLayout = model.layout.remembered;
    if (!rememberedLayout) {
      return;
    }

    const nextLayout = updateRememberedLayoutFromVisibleMonitors(
      rememberedLayout,
      monitors,
    );
    setBusy(true);
    try {
      await invokeCommand("set_layout", { layout: nextLayout });
      setPayload((current) => ({
        ...current,
        layout: nextLayout,
        layout_error: null,
      }));
      setError(null);
      await refreshDashboard();
    } catch (layoutSaveError) {
      const message = `布局未保存：${String(layoutSaveError)}`;
      setPayload((current) => ({
        ...current,
        layout_error: message,
      }));
      setError(message);
    } finally {
      setBusy(false);
    }
  }

  function handleMonitorVisibilityChange(monitorId: string, enabled: boolean) {
    setHiddenMonitorIds((current) => {
      const next = new Set(current);
      if (enabled) {
        next.delete(monitorId);
      } else {
        next.add(monitorId);
      }
      return next;
    });
  }

  async function handleWindow(command: "minimize_window" | "toggle_maximize_window" | "close_window") {
    try {
      await invokeCommand(command);
    } catch (windowError) {
      setError(String(windowError));
    }
  }

  return (
    <div
      className="flex h-full min-h-0 flex-col overflow-hidden"
      style={{
        background: theme.frame,
        color: theme.text,
      }}
    >
      <header
        className="flex shrink-0 items-center"
        style={{
          height: headerMetrics.headerHeight,
          borderBottom: `1px solid ${theme.border}`,
          background: theme.toolbar,
          paddingLeft: headerMetrics.headerPaddingX,
          paddingRight: headerMetrics.headerPaddingX,
        }}
        data-tauri-drag-region="true"
      >
        <div
          className="flex shrink-0 items-center"
          style={{ gap: headerMetrics.windowGap }}
          data-tauri-drag-region="false"
        >
          <WindowButton
            onClick={() => handleWindow("close_window")}
            title="关闭"
            tone="close"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <X size={8} strokeWidth={3} />
          </WindowButton>
          <WindowButton
            onClick={() => handleWindow("minimize_window")}
            title="最小化"
            tone="minimize"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <Minus size={9} strokeWidth={3} />
          </WindowButton>
          <WindowButton
            onClick={() => handleWindow("toggle_maximize_window")}
            title="最大化"
            tone="maximize"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <Maximize2 size={8} strokeWidth={3} />
          </WindowButton>
        </div>

        <div
          className="ml-4 flex shrink-0 items-center"
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

        <div className="min-w-6 flex-1 self-stretch" data-tauri-drag-region="true" />

        <div
          className="flex shrink-0 items-center"
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
              onMonitorsCommit={saveLayoutFromMonitors}
              onMonitorVisibilityChange={handleMonitorVisibilityChange}
              footerText={
                model.layout.error
                  ? `布局未保存：${model.layout.error}`
                  : "布局来自守护进程记忆；离线设备已隐藏。"
              }
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

          {page === "logs" ? (
            <LogsPage theme={theme} />
          ) : null}

          {page === "settings" ? (
            <SettingsPage
              acceptance={model.acceptance}
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
  acceptance,
  localDevice,
  inputMode,
  privilegeState,
  service,
  themeMode,
  onThemeModeChange,
  onToggleService,
  busy,
  theme,
}: {
  acceptance: {
    daemonOnline: boolean;
    backgroundReady: boolean;
    trayOwnedByDaemon: boolean;
    trayState: string;
    localEndpoint: string;
    discoveredDevices: number;
    connectedDevices: number;
    visibleLayoutDevices: number;
    inputReady: boolean;
    dualMachineReady: boolean;
    nextStep: string;
    autoStarted: boolean;
    checks: Array<{
      key: string;
      label: string;
      state: "pass" | "warn" | "block";
      detail: string;
    }>;
  };
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
}) {
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
                background: theme.accentSoft,
                color: theme.accent,
              }}
            >
              <Monitor size={18} />
            </div>
            <div>
              <h2 className="text-lg font-semibold">实机验收</h2>
              <p className="text-sm" style={{ color: theme.textMuted }}>
                打开另一台机器前，先确认后台、布局和输入主链路都已就绪。
              </p>
            </div>
          </div>

          <div className="mb-4 flex flex-wrap gap-2 text-xs">
            <AcceptanceBadge
              label={acceptance.daemonOnline ? "Daemon 在线" : "Daemon 离线"}
              state={acceptance.daemonOnline ? "pass" : "block"}
              theme={theme}
            />
            <AcceptanceBadge
              label={acceptance.autoStarted ? "Desktop 已自动拉起" : "未发生自动拉起"}
              state={acceptance.autoStarted ? "warn" : "pass"}
              theme={theme}
            />
            <AcceptanceBadge
              label={`托盘 ${acceptance.trayState}`}
              state={
                acceptance.trayOwnedByDaemon
                  ? acceptance.trayState === "Running"
                    ? "pass"
                    : "warn"
                  : "block"
              }
              theme={theme}
            />
          </div>

          <div className="space-y-3">
            {acceptance.checks.map((check) => (
              <div
                key={check.key}
                className="flex items-start gap-3 rounded-md px-4 py-3"
                style={{
                  border: `1px solid ${theme.border}`,
                  background: theme.frame,
                }}
              >
                <AcceptanceDot state={check.state} theme={theme} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <div className="text-sm font-medium">{check.label}</div>
                    <AcceptanceBadge label={acceptanceStateLabel(check.state)} state={check.state} theme={theme} />
                  </div>
                  <div className="mt-1 text-sm leading-6" style={{ color: theme.textMuted }}>
                    {check.detail}
                  </div>
                </div>
              </div>
            ))}
          </div>

          <div
            className="mt-4 rounded-md px-4 py-3 text-sm"
            style={{
              border: `1px solid ${theme.border}`,
              background: theme.frame,
            }}
          >
            <div
              className="mb-2 text-xs uppercase tracking-[0.16em]"
              style={{ color: theme.textMuted }}
            >
              下一步
            </div>
            <div className="font-medium">{acceptance.nextStep}</div>
          </div>
        </section>
      </div>
    </div>
  );
}

function acceptanceStateLabel(state: "pass" | "warn" | "block") {
  if (state === "pass") {
    return "通过";
  }

  if (state === "warn") {
    return "待确认";
  }

  return "阻塞";
}

function acceptanceStateStyle(
  state: "pass" | "warn" | "block",
  theme: typeof FIGMA_DESKTOP_THEME,
) {
  if (state === "pass") {
    return {
      background: "rgba(73, 179, 92, 0.16)",
      color: "#8de29d",
      dot: theme.success,
    };
  }

  if (state === "warn") {
    return {
      background: "rgba(214, 166, 75, 0.14)",
      color: "#e5c37a",
      dot: "#d6a64b",
    };
  }

  return {
    background: "rgba(197, 48, 48, 0.18)",
    color: "#ffb5c0",
    dot: theme.danger,
  };
}

function AcceptanceBadge({
  label,
  state,
  theme,
}: {
  label: string;
  state: "pass" | "warn" | "block";
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const style = acceptanceStateStyle(state, theme);

  return (
    <span
      className="rounded px-2 py-1"
      style={{
        background: style.background,
        color: style.color,
        border: `1px solid ${theme.border}`,
      }}
    >
      {label}
    </span>
  );
}

function AcceptanceDot({
  state,
  theme,
}: {
  state: "pass" | "warn" | "block";
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const style = acceptanceStateStyle(state, theme);

  return (
    <div
      className="mt-1 h-2.5 w-2.5 rounded-full"
      style={{ background: style.dot }}
    />
  );
}

function WindowButton({
  children,
  onClick,
  title,
  tone,
  theme,
  size,
  hitSize,
}: {
  children: ReactNode;
  onClick: () => void;
  title: string;
  tone: "close" | "minimize" | "maximize";
  theme: typeof FIGMA_DESKTOP_THEME;
  size: number;
  hitSize: number;
}) {
  const control = {
    close: {
      background: "#ff5f57",
      border: "#e0443e",
      color: "#5d1613",
    },
    minimize: {
      background: "#ffbd2e",
      border: "#dea123",
      color: "#684914",
    },
    maximize: {
      background: "#28c840",
      border: "#1da433",
      color: "#0d4f19",
    },
  }[tone];

  return (
    <button
      type="button"
      className="flex items-center justify-center rounded-full transition"
      onClick={onClick}
      title={title}
      style={{
        width: hitSize,
        height: hitSize,
        color: control.color,
      }}
      onMouseEnter={(event) => {
        event.currentTarget.style.backgroundColor = "rgba(255,255,255,0.06)";
      }}
      onMouseLeave={(event) => {
        event.currentTarget.style.backgroundColor = "transparent";
      }}
    >
      <span
        className="flex items-center justify-center rounded-full"
        style={{
          width: size,
          height: size,
          background: control.background,
          border: `1px solid ${control.border}`,
          boxShadow: `0 0 0 1px ${theme.frame}66`,
        }}
      >
        {children}
      </span>
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

function LogsPage({ theme }: { theme: typeof FIGMA_DESKTOP_THEME }) {
  const [logs, setLogs] = useState<Array<{
    timestamp: string;
    level: string;
    target: string;
    message: string;
  }>>([]);
  const [filter, setFilter] = useState<"all" | "error" | "warn" | "info" | "debug">("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(false);

  const loadLogs = async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invokeCommand<{
        timestamp: string;
        level: string;
        target: string;
        message: string;
      }[]>("get_logs", { limit: 1000 });
      setLogs(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const clearLogs = async () => {
    try {
      await invokeCommand("clear_logs");
      setLogs([]);
    } catch (err) {
      setError(String(err));
    }
  };

  useEffect(() => {
    loadLogs();
  }, []);

  useEffect(() => {
    if (!autoRefresh) return;
    const timer = setInterval(loadLogs, 2000);
    return () => clearInterval(timer);
  }, [autoRefresh]);

  const filteredLogs = logs.filter(log => {
    if (filter === "all") return true;
    return log.level.toLowerCase() === filter;
  });

  const getLevelColor = (level: string) => {
    switch (level.toLowerCase()) {
      case "error": return "#ffb5c0";
      case "warn": return "#e5c37a";
      case "info": return "#8de29d";
      default: return theme.textMuted;
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="mb-4 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="flex h-11 w-11 items-center justify-center rounded-md" style={{ background: theme.accentSoft }}>
            <FileText size={18} />
          </div>
          <div>
            <h2 className="text-lg font-semibold">服务日志</h2>
            <p className="text-sm" style={{ color: theme.textMuted }}>
              查看守护进程的运行日志
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <button
            className="rounded-md px-3 py-2 text-sm transition"
            style={{
              background: autoRefresh ? theme.accentSoft : theme.frame,
              border: `1px solid ${autoRefresh ? theme.accent : theme.border}`,
            }}
            onClick={() => setAutoRefresh(!autoRefresh)}
          >
            {autoRefresh ? "停止刷新" : "自动刷新"}
          </button>
          <button
            className="rounded-md px-3 py-2 text-sm transition"
            style={{ background: theme.accentSoft, border: `1px solid ${theme.border}` }}
            onClick={loadLogs}
            disabled={loading}
          >
            刷新
          </button>
          <button
            className="rounded-md px-3 py-2 text-sm transition"
            style={{ background: "rgba(197, 48, 48, 0.18)", border: `1px solid rgba(197, 48, 48, 0.35)` }}
            onClick={clearLogs}
          >
            清空
          </button>
        </div>
      </div>

      <div className="mb-3 flex gap-2">
        {(["all", "error", "warn", "info"] as const).map(level => (
          <button
            key={level}
            className="rounded-md px-3 py-1.5 text-sm"
            style={{
              background: filter === level ? theme.accentSoft : theme.frame,
              border: `1px solid ${filter === level ? theme.accent : theme.border}`,
            }}
            onClick={() => setFilter(level)}
          >
            {level === "all" ? "全部" : level.toUpperCase()}
          </button>
        ))}
      </div>

      {error && (
        <div className="mb-3 rounded-md px-4 py-3 text-sm"
          style={{ background: "rgba(94, 24, 34, 0.55)", border: "1px solid rgba(197, 48, 48, 0.45)", color: "#ffb8c1" }}>
          {error}
        </div>
      )}

      <div className="flex-1 overflow-auto rounded-md p-4 font-mono text-xs"
        style={{ background: theme.frame, border: `1px solid ${theme.border}` }}>
        {filteredLogs.length === 0 ? (
          <div className="flex h-full items-center justify-center" style={{ color: theme.textMuted }}>
            {loading ? "加载中..." : "暂无日志"}
          </div>
        ) : (
          <div className="space-y-1">
            {filteredLogs.map((log, i) => (
              <div key={i} className="flex gap-3">
                <span style={{ color: theme.textMuted, minWidth: "140px" }}>
                  {log.timestamp}
                </span>
                <span style={{ color: getLevelColor(log.level), minWidth: "50px" }}>
                  {log.level.toUpperCase()}
                </span>
                <span style={{ color: theme.textMuted, minWidth: "120px" }}>
                  {log.target}
                </span>
                <span style={{ color: theme.text }}>
                  {log.message}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="mt-2 text-xs" style={{ color: theme.textMuted }}>
        显示 {filteredLogs.length} / {logs.length} 条日志
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
