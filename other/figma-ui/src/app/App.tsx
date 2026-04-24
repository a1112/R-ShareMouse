import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  FileText,
  Gamepad2,
  HardDrive,
  Keyboard,
  LayoutGrid,
  Maximize2,
  Minus,
  Monitor,
  MousePointer2,
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

function useElementSize<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

  useEffect(() => {
    const node = ref.current;
    if (!node) {
      return undefined;
    }
    if (typeof window === "undefined") {
      return undefined;
    }

    const update = () => {
      setSize({ width: node.clientWidth, height: node.clientHeight });
    };

    update();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    }

    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      setSize({
        width: rect?.width ?? node.clientWidth,
        height: rect?.height ?? node.clientHeight,
      });
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return [ref, size] as const;
}

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

type LocalControlEvent = {
  sequence: number;
  timestamp_ms: number;
  device_kind: "Keyboard" | "Mouse" | "Gamepad" | "Display" | "Backend";
  event_kind: string;
  summary: string;
  device_id?: string | null;
  device_instance_id?: string | null;
  capture_path?: string | null;
  source?: "Hardware" | "Injected" | "InjectedLoopback" | "DriverTest" | "VirtualDevice" | "System";
  payload?: Record<string, string>;
};

type LocalControlsSnapshot = {
  sequence: number;
  keyboard: {
    detected: boolean;
    pressed_keys: string[];
    last_key?: string | null;
    event_count: number;
    capture_source: string;
  };
  mouse: {
    detected: boolean;
    x: number;
    y: number;
    pressed_buttons: string[];
    wheel_delta_x: number;
    wheel_delta_y: number;
    event_count: number;
    move_count?: number;
    button_event_count?: number;
    button_press_count?: number;
    button_release_count?: number;
    wheel_event_count?: number;
    wheel_total_x?: number;
    wheel_total_y?: number;
    current_display_index?: number | null;
    current_display_id?: string | null;
    display_relative_x?: number;
    display_relative_y?: number;
    capture_source: string;
  };
  keyboard_devices?: Array<{
    id: string;
    name: string;
    source: string;
    connected: boolean;
    driver_detail?: string | null;
    device_instance_id?: string | null;
    capture_path?: string | null;
    event_count?: number;
    last_event_ms?: number;
    capabilities?: string[];
  }>;
  mouse_devices?: Array<{
    id: string;
    name: string;
    source: string;
    connected: boolean;
    driver_detail?: string | null;
    device_instance_id?: string | null;
    capture_path?: string | null;
    event_count?: number;
    last_event_ms?: number;
    capabilities?: string[];
  }>;
  gamepads: Array<{
    gamepad_id: number;
    name: string;
    connected: boolean;
    buttons: Array<{ button: string | Record<string, unknown>; pressed: boolean }>;
    pressed_buttons?: string[];
    last_button?: string | null;
    left_stick_x: number;
    left_stick_y: number;
    right_stick_x: number;
    right_stick_y: number;
    left_trigger: number;
    right_trigger: number;
    event_count: number;
    button_event_count?: number;
    button_press_count?: number;
    button_release_count?: number;
    axis_event_count?: number;
    trigger_event_count?: number;
    last_axis?: string | null;
    last_seen_ms: number;
  }>;
  display: {
    display_count: number;
    virtual_x?: number;
    virtual_y?: number;
    primary_width: number;
    primary_height: number;
    layout_width: number;
    layout_height: number;
    displays?: Array<{
      display_id: string;
      x: number;
      y: number;
      width: number;
      height: number;
      primary: boolean;
    }>;
  };
  capture_backend: Record<string, unknown>;
  inject_backend: Record<string, unknown>;
  privilege_state?: string | null;
  virtual_gamepad: {
    status: string;
    detail: string;
  };
  driver?: {
    status: string;
    device_path?: string | null;
    version?: string | null;
    filter_active: boolean;
    vhid_active: boolean;
    test_signing_required: boolean;
    last_error?: string | null;
  };
  recent_events: LocalControlEvent[];
  last_error?: string | null;
};

type LocalInputTestResult = {
  status: "Success" | "PermissionDenied" | "BackendUnavailable" | "Failed" | "Unsupported";
  message: string;
};

type LocalControlKind = "keyboard" | "mouse" | "gamepad" | "display";
type LocalDevicePageKind = LocalControlKind | "remote";

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

async function listenTauriEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
): Promise<null | (() => void)> {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      event?: {
        listen?: (
          event: string,
          handler: (event: { payload: T }) => void,
        ) => Promise<() => void>;
      };
    };
  };

  const listen = tauriWindow.__TAURI__?.event?.listen;
  if (!listen) {
    return null;
  }

  return listen(eventName, (event) => handler(event.payload));
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

function applyLocalControlEvent(
  snapshot: LocalControlsSnapshot,
  event: LocalControlEvent,
): LocalControlsSnapshot {
  const recentEvents = mergeLocalControlEvents(snapshot.recent_events ?? [], [event]);
  const next: LocalControlsSnapshot = {
    ...snapshot,
    sequence: Math.max(snapshot.sequence ?? 0, event.sequence ?? 0),
    recent_events: recentEvents,
  };

  if (event.device_kind === "Keyboard") {
    const key = keyboardEventKey(event);
    const pressedKeys = [...(snapshot.keyboard.pressed_keys ?? [])];
    if (key) {
      const pressed = eventStateIsPressed(event);
      const released = eventStateIsReleased(event);
      if (pressed) {
        pushUniqueString(pressedKeys, key);
      } else if (released) {
        removeString(pressedKeys, key);
      }
      next.keyboard = {
        ...snapshot.keyboard,
        detected: true,
        last_key: key,
        pressed_keys: pressedKeys,
        event_count: Number(snapshot.keyboard.event_count ?? 0) + 1,
      };
    }
  } else if (event.device_kind === "Mouse") {
    const pressedButtons = [...(snapshot.mouse.pressed_buttons ?? [])];
    const button = event.payload?.button;
    if (button) {
      if (eventStateIsPressed(event)) {
        pushUniqueString(pressedButtons, button);
      } else if (eventStateIsReleased(event)) {
        removeString(pressedButtons, button);
      }
    }
    next.mouse = {
      ...snapshot.mouse,
      detected: true,
      x: numberPayload(event, "x", snapshot.mouse.x),
      y: numberPayload(event, "y", snapshot.mouse.y),
      wheel_delta_x: numberPayload(event, "delta_x", snapshot.mouse.wheel_delta_x),
      wheel_delta_y: numberPayload(event, "delta_y", snapshot.mouse.wheel_delta_y),
      wheel_total_x: numberPayload(event, "total_x", snapshot.mouse.wheel_total_x ?? 0),
      wheel_total_y: numberPayload(event, "total_y", snapshot.mouse.wheel_total_y ?? 0),
      display_relative_x: numberPayload(
        event,
        "display_relative_x",
        snapshot.mouse.display_relative_x ?? snapshot.mouse.x,
      ),
      display_relative_y: numberPayload(
        event,
        "display_relative_y",
        snapshot.mouse.display_relative_y ?? snapshot.mouse.y,
      ),
      current_display_index: optionalNumberPayload(
        event,
        "display_index",
        snapshot.mouse.current_display_index ?? null,
      ),
      current_display_id: event.payload?.display_id ?? snapshot.mouse.current_display_id ?? null,
      pressed_buttons: pressedButtons,
      event_count: Number(snapshot.mouse.event_count ?? 0) + 1,
      move_count:
        Number(snapshot.mouse.move_count ?? 0) + (event.event_kind === "move" ? 1 : 0),
      button_event_count:
        Number(snapshot.mouse.button_event_count ?? 0) +
        (event.event_kind === "button" ? 1 : 0),
      button_press_count:
        Number(snapshot.mouse.button_press_count ?? 0) +
        (event.event_kind === "button" && eventStateIsPressed(event) ? 1 : 0),
      button_release_count:
        Number(snapshot.mouse.button_release_count ?? 0) +
        (event.event_kind === "button" && eventStateIsReleased(event) ? 1 : 0),
      wheel_event_count:
        Number(snapshot.mouse.wheel_event_count ?? 0) +
        (event.event_kind === "wheel" ? 1 : 0),
    };
  } else if (event.device_kind === "Gamepad") {
    const gamepadId = optionalNumberPayload(event, "gamepad_id", null);
    if (gamepadId !== null) {
      const gamepads = [...(snapshot.gamepads ?? [])];
      const existingIndex = gamepads.findIndex((item) => item.gamepad_id === gamepadId);
      const existing = existingIndex >= 0 ? gamepads[existingIndex] : null;
      const pressedButtons =
        event.payload?.pressed_buttons !== undefined
          ? event.payload.pressed_buttons.split(",").map((item) => item.trim()).filter(Boolean)
          : existing?.pressed_buttons ?? [];
      const updated = {
        gamepad_id: gamepadId,
        name: event.payload?.name ?? existing?.name ?? `Gamepad ${gamepadId}`,
        connected:
          event.event_kind === "disconnected"
            ? false
            : event.event_kind === "connected"
              ? true
              : existing?.connected ?? true,
        buttons:
          existing?.buttons ??
          pressedButtons.map((button) => ({
            button,
            pressed: true,
          })),
        pressed_buttons: pressedButtons,
        last_button: event.payload?.last_button ?? existing?.last_button ?? null,
        left_stick_x: numberPayload(event, "left_stick_x", existing?.left_stick_x ?? 0),
        left_stick_y: numberPayload(event, "left_stick_y", existing?.left_stick_y ?? 0),
        right_stick_x: numberPayload(event, "right_stick_x", existing?.right_stick_x ?? 0),
        right_stick_y: numberPayload(event, "right_stick_y", existing?.right_stick_y ?? 0),
        left_trigger: numberPayload(event, "left_trigger", existing?.left_trigger ?? 0),
        right_trigger: numberPayload(event, "right_trigger", existing?.right_trigger ?? 0),
        event_count: numberPayload(event, "event_count", Number(existing?.event_count ?? 0) + 1),
        button_event_count: numberPayload(
          event,
          "button_event_count",
          existing?.button_event_count ?? 0,
        ),
        button_press_count: numberPayload(
          event,
          "button_press_count",
          existing?.button_press_count ?? 0,
        ),
        button_release_count: numberPayload(
          event,
          "button_release_count",
          existing?.button_release_count ?? 0,
        ),
        axis_event_count: numberPayload(
          event,
          "axis_event_count",
          existing?.axis_event_count ?? 0,
        ),
        trigger_event_count: numberPayload(
          event,
          "trigger_event_count",
          existing?.trigger_event_count ?? 0,
        ),
        last_axis: event.payload?.last_axis ?? existing?.last_axis ?? null,
        last_seen_ms: event.timestamp_ms ?? existing?.last_seen_ms ?? 0,
      };
      if (existingIndex >= 0) {
        gamepads[existingIndex] = updated;
      } else {
        gamepads.push(updated);
      }
      next.gamepads = gamepads;
    }
  }

  return next;
}

function mergeLocalControlSnapshot(
  current: LocalControlsSnapshot | null,
  incoming: LocalControlsSnapshot,
) {
  if (!current) {
    return incoming;
  }
  return {
    ...incoming,
    recent_events: mergeLocalControlEvents(
      current.recent_events ?? [],
      incoming.recent_events ?? [],
    ),
  };
}

function mergeLocalControlEvents(
  existing: LocalControlEvent[],
  incoming: LocalControlEvent[],
) {
  const bySequence = new Map<number, LocalControlEvent>();
  for (const event of [...existing, ...incoming]) {
    bySequence.set(event.sequence, event);
  }

  const sorted = Array.from(bySequence.values()).sort((a, b) => a.sequence - b.sequence);
  const tail = sorted.slice(-64);
  const keyboardTail = sorted.filter((event) => event.device_kind === "Keyboard").slice(-24);
  const gamepadTail = sorted.filter((event) => event.device_kind === "Gamepad").slice(-12);
  const retained = new Map<number, LocalControlEvent>();
  for (const event of [...tail, ...keyboardTail, ...gamepadTail]) {
    retained.set(event.sequence, event);
  }
  return Array.from(retained.values())
    .sort((a, b) => a.sequence - b.sequence)
    .slice(-96);
}

function numberPayload(event: LocalControlEvent, key: string, fallback: number) {
  const value = event.payload?.[key];
  if (value === undefined) {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function optionalNumberPayload(
  event: LocalControlEvent,
  key: string,
  fallback: number | null,
) {
  const value = event.payload?.[key];
  if (value === undefined) {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function pushUniqueString(values: string[], value: string) {
  if (!values.some((item) => normalizeKeyToken(item) === normalizeKeyToken(value))) {
    values.push(value);
  }
}

function removeString(values: string[], value: string) {
  const normalized = normalizeKeyToken(value);
  const index = values.findIndex((item) => normalizeKeyToken(item) === normalized);
  if (index >= 0) {
    values.splice(index, 1);
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
  const [localControls, setLocalControls] = useState<LocalControlsSnapshot | null>(null);
  const [localControlsError, setLocalControlsError] = useState<string | null>(null);
  const [localInputTestResult, setLocalInputTestResult] =
    useState<LocalInputTestResult | null>(null);
  const [confirmingInputTest, setConfirmingInputTest] = useState<string | null>(null);
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

  async function refreshLocalControls() {
    try {
      const snapshot = await invokeCommand<LocalControlsSnapshot>("local_controls_state");
      setLocalControls((current) => mergeLocalControlSnapshot(current, snapshot));
      setLocalControlsError(null);
    } catch (localError) {
      setLocalControlsError(String(localError));
    }
  }

  useEffect(() => {
    refreshDashboard();
    refreshLocalControls();
    const timer = window.setInterval(() => {
      refreshDashboard();
      refreshLocalControls();
      setRefreshTick((value) => value + 1);
    }, POLL_INTERVAL_MS);

    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: null | (() => void) = null;

    async function startStream() {
      try {
        unlisten = await listenTauriEvent<unknown>("local-control-event", (payload) => {
          if (typeof payload === "string") {
            setLocalControlsError(payload);
            return;
          }

          const response = payload as {
            LocalControls?: LocalControlsSnapshot;
            LocalControlEvent?: LocalControlEvent;
          };
          if (response.LocalControls) {
            setLocalControls((current) => mergeLocalControlSnapshot(current, response.LocalControls!));
            setLocalControlsError(null);
          } else if (response.LocalControlEvent) {
            setLocalControls((current) => {
              if (!current) {
                return current;
              }
              return applyLocalControlEvent(current, response.LocalControlEvent!);
            });
            setLocalControlsError(null);
          }
        });

        if (!cancelled) {
          await invokeCommand("start_local_controls_stream");
        }
      } catch (streamError) {
        setLocalControlsError(String(streamError));
      }
    }

    startStream();
    return () => {
      cancelled = true;
      unlisten?.();
      invokeCommand("stop_local_controls_stream").catch(() => {});
    };
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

  async function runLocalInputTest(kind: string) {
    if (confirmingInputTest !== kind) {
      setConfirmingInputTest(kind);
      return;
    }

    setBusy(true);
    setConfirmingInputTest(null);
    try {
      const result = await invokeCommand<LocalInputTestResult>("run_local_input_test", { kind });
      setLocalInputTestResult(result);
      await refreshLocalControls();
    } catch (testError) {
      setLocalInputTestResult({
        status: "Failed",
        message: String(testError),
      });
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

        <div
          className="ml-2 flex h-full shrink-0 items-center"
          style={{ gap: headerMetrics.windowGap }}
          data-tauri-drag-region="false"
        >
          <WindowButton
            onClick={() => handleWindow("minimize_window")}
            title="最小化"
            tone="minimize"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <Minus size={12} strokeWidth={2} />
          </WindowButton>
          <WindowButton
            onClick={() => handleWindow("toggle_maximize_window")}
            title="最大化"
            tone="maximize"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <Maximize2 size={10} strokeWidth={2} />
          </WindowButton>
          <WindowButton
            onClick={() => handleWindow("close_window")}
            title="关闭"
            tone="close"
            theme={theme}
            size={headerMetrics.windowButtonSize}
            hitSize={headerMetrics.windowButtonHitSize}
          >
            <X size={13} strokeWidth={2} />
          </WindowButton>
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
            padding: page === "devices" ? 0 : chrome.contentPadding,
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
              localControls={localControls}
              localControlsError={localControlsError}
              localInputTestResult={localInputTestResult}
              confirmingInputTest={confirmingInputTest}
              onRunLocalInputTest={runLocalInputTest}
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
  localControls,
  localControlsError,
  localInputTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
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
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <DevicesPageWithLocalControls
      devices={devices}
      localControls={localControls}
      localControlsError={localControlsError}
      localInputTestResult={localInputTestResult}
      confirmingInputTest={confirmingInputTest}
      onRunLocalInputTest={onRunLocalInputTest}
      onConnect={onConnect}
      onDisconnect={onDisconnect}
      busy={busy}
      theme={theme}
    />
  );

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
    <div className="rshare-scroll grid h-full grid-cols-1 gap-3 overflow-auto xl:grid-cols-2">
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

function DevicesPageWithLocalControls({
  devices,
  localControls,
  localControlsError,
  localInputTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
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
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const [selectedPage, setSelectedPage] = useState<LocalDevicePageKind>("keyboard");
  const counts = {
    keyboard: localInputDeviceCount(localControls, "keyboard"),
    mouse: localInputDeviceCount(localControls, "mouse"),
    gamepad: Math.max(1, localControls?.gamepads?.length ?? 0),
    display: Math.max(1, localControls?.display.display_count ?? 0),
    remote: devices.length,
  };

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div
        className="shrink-0 overflow-hidden"
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.toolbar,
        }}
      >
        <div
          className="rshare-scroll flex min-h-12 items-center gap-2 overflow-x-auto px-3 py-2"
          role="tablist"
          aria-label="设备类型"
        >
        <LocalControlTypeButton
          kind="keyboard"
          active={selectedPage === "keyboard"}
          icon={<Keyboard size={18} />}
          title="键盘"
          detail={`${counts.keyboard} 个`}
          live={Boolean(localControls?.keyboard.detected)}
          onClick={setSelectedPage}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="mouse"
          active={selectedPage === "mouse"}
          icon={<MousePointer2 size={18} />}
          title="鼠标"
          detail={`${counts.mouse} 个`}
          live={Boolean(localControls?.mouse.detected)}
          onClick={setSelectedPage}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="gamepad"
          active={selectedPage === "gamepad"}
          icon={<Gamepad2 size={18} />}
          title="手柄"
          detail={`${counts.gamepad} 个`}
          live={Boolean(localControls?.gamepads?.some((item) => item.connected))}
          onClick={setSelectedPage}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="display"
          active={selectedPage === "display"}
          icon={<HardDrive size={18} />}
          title="显示设备"
          detail={`${counts.display} 个`}
          live={(localControls?.display.display_count ?? 0) > 0}
          onClick={setSelectedPage}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="remote"
          active={selectedPage === "remote"}
          icon={<Monitor size={18} />}
          title="远端设备"
          detail={`${counts.remote} 台`}
          live={devices.some((device) => device.connected)}
          onClick={setSelectedPage}
          theme={theme}
        />
        </div>
        <DeviceDriverStrip kind={selectedPage} snapshot={localControls} theme={theme} />
      </div>

      <div className="min-h-0 flex-1 overflow-hidden">
        {selectedPage === "remote" ? (
          <RemoteDevicesPanel
            devices={devices}
            onConnect={onConnect}
            onDisconnect={onDisconnect}
            busy={busy}
            theme={theme}
          />
        ) : (
          <LocalControlDriverHub
            snapshot={localControls}
            error={localControlsError}
            inputTestResult={localInputTestResult}
            confirmingInputTest={confirmingInputTest}
            selectedKind={selectedPage}
            onSelectedKindChange={setSelectedPage}
            onRunInputTest={onRunLocalInputTest}
            theme={theme}
          />
        )}
      </div>
    </div>
  );

  return (
    <div className="flex h-full flex-col gap-4 overflow-auto">
      <LocalControlCenter
        snapshot={localControls}
        error={localControlsError}
        inputTestResult={localInputTestResult}
        confirmingInputTest={confirmingInputTest}
        onRunInputTest={onRunLocalInputTest}
        theme={theme}
      />

      <section>
        <div className="mb-3 flex items-center justify-between gap-3">
          <div>
            <h2 className="text-base font-semibold">远端设备</h2>
            <p className="mt-1 text-sm" style={{ color: theme.textMuted }}>
              发现的共享设备仍在这里连接和断开。
            </p>
          </div>
          <StatusPill
            label={devices.length ? `${devices.length} 台` : "未发现"}
            tone={devices.length ? "info" : "muted"}
            theme={theme}
          />
        </div>

        {!devices.length ? (
          <EmptyPanel
            title="尚未发现设备"
            detail="启动 daemon 并保持同一局域网后，发现到的远端设备会显示在这里。"
            theme={theme}
          />
        ) : (
          <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
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
                      <StatusPill
                        label={device.connected ? "已连接" : "已发现"}
                        tone={device.connected ? "success" : "muted"}
                        theme={theme}
                      />
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
                      color: device.connected ? "#ffb5c0" : theme.text,
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
                  <InfoRow
                    label="布局映射"
                    value={device.connected ? "已联动" : "空闲"}
                    theme={theme}
                  />
                </div>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function RemoteDevicesPanel({
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
        detail="启动 daemon 并保持同一局域网后，发现到的远端设备会显示在这里。"
        theme={theme}
      />
    );
  }

  return (
    <div className="rshare-scroll grid h-full grid-cols-1 gap-3 overflow-auto xl:grid-cols-2">
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
                <StatusPill
                  label={device.connected ? "已连接" : "已发现"}
                  tone={device.connected ? "success" : "muted"}
                  theme={theme}
                />
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
                color: device.connected ? "#ffb5c0" : theme.text,
                border: `1px solid ${
                  device.connected ? "rgba(197, 48, 48, 0.35)" : theme.accent
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

function LocalControlCenter({
  snapshot,
  error,
  inputTestResult,
  confirmingInputTest,
  onRunInputTest,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  error: string | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunInputTest: (kind: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const [selectedKind, setSelectedKind] = useState<LocalControlKind>("keyboard");

  return (
    <LocalControlDriverHub
      snapshot={snapshot}
      error={error}
      inputTestResult={inputTestResult}
      confirmingInputTest={confirmingInputTest}
      selectedKind={selectedKind}
      onSelectedKindChange={setSelectedKind}
      onRunInputTest={onRunInputTest}
      theme={theme}
    />
  );

  const recentEvents = snapshot?.recent_events ?? [];
  const latestEvent = recentEvents.length ? recentEvents[recentEvents.length - 1] : undefined;
  const gamepad = snapshot?.gamepads?.find((item) => item.connected) ?? snapshot?.gamepads?.[0];
  const backendDetail = snapshot
    ? `${backendHealthLabel(snapshot.capture_backend)} capture / ${backendHealthLabel(snapshot.inject_backend)} inject`
    : "daemon IPC unavailable";

  return (
    <section
      className="p-5"
      style={{
        background: theme.sidebar,
        border: `1px solid ${theme.border}`,
        boxShadow: theme.panelShadow,
      }}
    >
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">本机控制设备</h1>
          <p className="mt-1 text-sm" style={{ color: theme.textMuted }}>
            使用 daemon 捕获链路显示键盘、鼠标、手柄和显示设备反馈。
          </p>
        </div>
        <StatusPill
          label={error ? "不可用" : snapshot ? "实时反馈" : "等待 daemon"}
          tone={error ? "danger" : snapshot ? "success" : "muted"}
          theme={theme}
        />
      </div>

      {error ? (
        <div
          className="mb-4 px-4 py-3 text-sm"
          style={{
            border: "1px solid rgba(197, 48, 48, 0.45)",
            background: "rgba(94, 24, 34, 0.45)",
            color: "#ffb8c1",
          }}
        >
          本机驱动中心不可用：{error}
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-4">
        <LocalDevicePanel
          icon={<Keyboard size={18} />}
          title="键盘"
          status={snapshot?.keyboard.detected ? "捕获中" : "未检测"}
          actionLabel={confirmingInputTest === "keyboard" ? "再次点击执行 Shift 测试" : "真实注入测试"}
          onAction={() => onRunInputTest("keyboard")}
          actionDisabled={!snapshot}
          theme={theme}
        >
          <InfoRow label="最近按键" value={snapshot?.keyboard.last_key ?? "无"} theme={theme} />
          <InfoRow
            label="按下状态"
            value={
              snapshot?.keyboard.pressed_keys?.length
                ? snapshot.keyboard.pressed_keys.join(", ")
                : "无"
            }
            theme={theme}
          />
          <InfoRow label="事件计数" value={String(snapshot?.keyboard.event_count ?? 0)} theme={theme} />
          <InfoRow label="捕获来源" value={snapshot?.keyboard.capture_source ?? "未知"} theme={theme} />
        </LocalDevicePanel>

        <LocalDevicePanel
          icon={<MousePointer2 size={18} />}
          title="鼠标"
          status={snapshot?.mouse.detected ? "捕获中" : "未检测"}
          actionLabel={confirmingInputTest === "mouse" ? "再次点击执行移动测试" : "真实注入测试"}
          onAction={() => onRunInputTest("mouse")}
          actionDisabled={!snapshot}
          theme={theme}
        >
          <InfoRow
            label="坐标"
            value={`${Math.round(snapshot?.mouse.x ?? 0)}, ${Math.round(snapshot?.mouse.y ?? 0)}`}
            theme={theme}
          />
          <InfoRow
            label="按钮"
            value={
              snapshot?.mouse.pressed_buttons?.length
                ? snapshot.mouse.pressed_buttons.join(", ")
                : "无"
            }
            theme={theme}
          />
          <InfoRow
            label="滚轮"
            value={`${snapshot?.mouse.wheel_delta_x ?? 0}, ${snapshot?.mouse.wheel_delta_y ?? 0}`}
            theme={theme}
          />
          <InfoRow label="事件计数" value={String(snapshot?.mouse.event_count ?? 0)} theme={theme} />
        </LocalDevicePanel>

        <LocalDevicePanel
          icon={<Gamepad2 size={18} />}
          title="手柄"
          status={gamepad?.connected ? "gilrs 已识别" : "等待连接"}
          theme={theme}
        >
          <InfoRow label="设备" value={gamepad?.name ?? "未识别"} theme={theme} />
          <InfoRow
            label="摇杆"
            value={`L ${Number(gamepad?.left_stick_x ?? 0).toFixed(2)}, ${Number(gamepad?.left_stick_y ?? 0).toFixed(2)} / R ${Number(gamepad?.right_stick_x ?? 0).toFixed(2)}, ${Number(gamepad?.right_stick_y ?? 0).toFixed(2)}`}
            theme={theme}
          />
          <InfoRow
            label="扳机"
            value={`L ${Number(gamepad?.left_trigger ?? 0).toFixed(2)} / R ${Number(gamepad?.right_trigger ?? 0).toFixed(2)}`}
            theme={theme}
          />
          <InfoRow
            label="虚拟手柄"
            value={snapshot?.virtual_gamepad.detail ?? "Virtual HID not implemented"}
            theme={theme}
          />
        </LocalDevicePanel>

        <LocalDevicePanel
          icon={<HardDrive size={18} />}
          title="显示设备"
          status={(snapshot?.display.display_count ?? 0) > 0 ? "已读取" : "未读取"}
          theme={theme}
        >
          <InfoRow label="显示器数量" value={String(snapshot?.display.display_count ?? 0)} theme={theme} />
          <InfoRow
            label="主显示器"
            value={`${snapshot?.display.primary_width ?? 0} x ${snapshot?.display.primary_height ?? 0}`}
            theme={theme}
          />
          <InfoRow
            label="布局尺寸"
            value={`${snapshot?.display.layout_width ?? 0} x ${snapshot?.display.layout_height ?? 0}`}
            theme={theme}
          />
          <InfoRow label="后端" value={backendDetail} theme={theme} />
        </LocalDevicePanel>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
        <InfoRow label="权限状态" value={snapshot?.privilege_state ?? "未知"} theme={theme} />
        <InfoRow
          label="最近反馈"
          value={latestEvent ? `${latestEvent.device_kind}: ${latestEvent.summary}` : "暂无"}
          theme={theme}
        />
        <InfoRow
          label="注入结果"
          value={
            inputTestResult
              ? `${inputTestResult.status}: ${inputTestResult.message}`
              : "需要二段确认后执行"
          }
          theme={theme}
        />
      </div>
      {isInjectedFeedback(latestEvent?.source) ? (
        <div className="mt-3 text-xs" style={{ color: theme.textMuted }}>
          最近反馈可能为注入回环，已按驱动来源标记。
        </div>
      ) : null}
    </section>
  );
}

function LocalControlDriverHub({
  snapshot,
  error,
  inputTestResult,
  confirmingInputTest,
  selectedKind,
  onSelectedKindChange,
  onRunInputTest,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  error: string | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  selectedKind: LocalControlKind;
  onSelectedKindChange: (kind: LocalControlKind) => void;
  onRunInputTest: (kind: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <section className="flex h-full min-h-0 flex-col overflow-hidden">
      {error ? (
        <div
          className="shrink-0 px-3 py-2 text-xs"
          style={{
            border: "1px solid rgba(197, 48, 48, 0.45)",
            background: "rgba(94, 24, 34, 0.45)",
            color: "#ffb8c1",
          }}
        >
          本机驱动中心不可用：{error}
        </div>
      ) : null}

      <div
        className="min-h-0 flex-1 overflow-hidden p-2"
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.frame,
        }}
      >
        <LocalControlDetail
          kind={selectedKind}
          snapshot={snapshot}
          inputTestResult={inputTestResult}
          confirmingInputTest={confirmingInputTest}
          onRunInputTest={onRunInputTest}
          theme={theme}
        />
      </div>
    </section>
  );

  return (
    <section
      className="p-5"
      style={{
        background: theme.sidebar,
        border: `1px solid ${theme.border}`,
        boxShadow: theme.panelShadow,
      }}
    >
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">本机控制设备</h1>
          <p className="mt-1 text-sm" style={{ color: theme.textMuted }}>
            实机输入设备概览、底层捕获反馈和受控注入测试入口。
          </p>
        </div>
        <StatusPill
          label={error ? "不可用" : snapshot ? "daemon 实时" : "等待 daemon"}
          tone={error ? "danger" : snapshot ? "success" : "muted"}
          theme={theme}
        />
      </div>

      {error ? (
        <div
          className="mb-4 px-4 py-3 text-sm"
          style={{
            border: "1px solid rgba(197, 48, 48, 0.45)",
            background: "rgba(94, 24, 34, 0.45)",
            color: "#ffb8c1",
          }}
        >
          本机驱动中心不可用：{error}。远端设备列表仍可继续使用。
        </div>
      ) : null}

      <div
        className="mb-4 flex flex-wrap items-center gap-2 px-3 py-2"
        role="tablist"
        aria-label="本机设备类型"
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.toolbar,
        }}
      >
        <LocalControlTypeButton
          kind="keyboard"
          active={selectedKind === "keyboard"}
          icon={<Keyboard size={18} />}
          title="键盘"
          detail={`${counts.keyboard} 个实机输入`}
          live={Boolean(snapshot?.keyboard.detected)}
          onClick={onSelectedKindChange}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="mouse"
          active={selectedKind === "mouse"}
          icon={<MousePointer2 size={18} />}
          title="鼠标"
          detail={`${counts.mouse} 个实机输入`}
          live={Boolean(snapshot?.mouse.detected)}
          onClick={onSelectedKindChange}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="gamepad"
          active={selectedKind === "gamepad"}
          icon={<Gamepad2 size={18} />}
          title="手柄"
          detail={`${counts.gamepad} 个入口`}
          live={Boolean(snapshot?.gamepads?.some((item) => item.connected))}
          onClick={onSelectedKindChange}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="display"
          active={selectedKind === "display"}
          icon={<HardDrive size={18} />}
          title="显示设备"
          detail={`${counts.display} 个显示器`}
          live={(snapshot?.display.display_count ?? 0) > 0}
          onClick={onSelectedKindChange}
          theme={theme}
        />
      </div>

      <div className="grid min-h-[540px] grid-cols-1 gap-3 xl:grid-cols-[280px_minmax(0,1fr)]">
        <aside
          className="p-3"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.frame,
          }}
        >
          <div className="mb-3 text-xs uppercase tracking-[0.16em]" style={{ color: theme.textMuted }}>
            实机列表
          </div>
          <div className="flex flex-col gap-2">
            {devices.map((device) => (
              <button
                key={device.id}
                type="button"
                className="rounded-md px-3 py-3 text-left transition"
                style={{
                  border: `1px solid ${device.active ? theme.accent : theme.border}`,
                  background: device.active ? theme.accentSoft : "rgba(255,255,255,0.025)",
                  color: theme.text,
                }}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-sm font-medium">{device.name}</span>
                  <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: device.live ? theme.success : theme.textMuted }} />
                </div>
                <div className="mt-1 text-xs" style={{ color: theme.textMuted }}>
                  {device.detail}
                </div>
              </button>
            ))}
          </div>
          <div className="mt-3 text-xs leading-5" style={{ color: theme.textMuted }}>
            多键盘、多鼠标的独立枚举需要后续接入更底层设备捕获；当前先显示 daemon 聚合链路。
          </div>
        </aside>

        <div
          className="p-4"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.frame,
          }}
        >
          <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
            <div>
              <h3 className="text-base font-semibold">{selectedDevice?.name ?? "未检测到设备"}</h3>
              <p className="mt-1 text-sm" style={{ color: theme.textMuted }}>
                {selectedDevice?.detail ?? "等待 daemon 上报本机设备。"}
              </p>
            </div>
            <StatusPill
              label={selectedDevice?.live ? "有反馈" : "等待输入"}
              tone={selectedDevice?.live ? "success" : "muted"}
              theme={theme}
            />
          </div>

          <LocalControlDetail
            kind={selectedKind}
            snapshot={snapshot}
            inputTestResult={inputTestResult}
            confirmingInputTest={confirmingInputTest}
            onRunInputTest={onRunInputTest}
            theme={theme}
          />
        </div>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
        <InfoRow label="权限状态" value={snapshot?.privilege_state ?? "未知"} theme={theme} />
        <InfoRow
          label="后端"
          value={
            snapshot
              ? `${backendHealthLabel(snapshot.capture_backend)} / ${backendHealthLabel(snapshot.inject_backend)}`
              : "daemon IPC unavailable"
          }
          theme={theme}
        />
        <InfoRow
          label="最近反馈"
          value={latestEvent ? `${latestEvent.device_kind}: ${latestEvent.summary}` : "暂无"}
          theme={theme}
        />
      </div>
      {isInjectedFeedback(latestEvent?.source) ? (
        <div className="mt-3 text-xs" style={{ color: theme.textMuted }}>
          最近反馈可能为注入回环，已按驱动来源标记。
        </div>
      ) : null}
    </section>
  );
}

function localInputDeviceCount(snapshot: LocalControlsSnapshot | null, kind: "keyboard" | "mouse") {
  const devices = kind === "keyboard" ? snapshot?.keyboard_devices ?? [] : snapshot?.mouse_devices ?? [];
  const summary = summarizeRawInputDevices(devices);
  return Math.max(1, summary.physicalGroups.length || (devices.length ? 1 : 0));
}

function localDeviceItems(snapshot: LocalControlsSnapshot | null, kind: LocalControlKind) {
  if (kind === "keyboard") {
    return localInputDeviceItems(
      "keyboard",
      "本机键盘",
      snapshot?.keyboard_devices ?? [],
      snapshot?.keyboard.capture_source ?? "daemon aggregate",
      Boolean(snapshot?.keyboard.detected),
    );
  }

  if (kind === "mouse") {
    return localInputDeviceItems(
      "mouse",
      "本机鼠标",
      snapshot?.mouse_devices ?? [],
      snapshot?.mouse.capture_source ?? "daemon aggregate",
      Boolean(snapshot?.mouse.detected),
    );
  }

  if (kind === "gamepad") {
    const gamepads = snapshot?.gamepads ?? [];
    if (!gamepads.length) {
      return [
        {
          id: "gamepad-empty",
          name: "Gamepad slot",
          detail: "gilrs waiting for device",
          live: false,
          active: true,
        },
      ];
    }
    return gamepads.map((gamepad, index) => ({
      id: `gamepad-${gamepad.gamepad_id}`,
      name: gamepad.name || `Gamepad ${gamepad.gamepad_id}`,
      detail: `id ${gamepad.gamepad_id} / events ${gamepad.event_count}`,
      live: Boolean(gamepad.connected),
      active: index === 0,
    }));
  }

  const displays = snapshot?.display.displays ?? [];
  if (displays.length) {
    return displays.map((display, index) => ({
      id: display.display_id,
      name: display.primary ? "Primary display" : `Display ${index + 1}`,
      detail: `${display.width} x ${display.height} @ ${display.x}, ${display.y}`,
      live: true,
      active: display.primary,
    }));
  }

  const displayCount = Math.max(1, snapshot?.display.display_count ?? 0);
  return Array.from({ length: displayCount }, (_, index) => ({
    id: `display-${index}`,
    name: index === 0 ? "Primary display" : `Display ${index + 1}`,
    detail:
      index === 0
        ? `${snapshot?.display.primary_width ?? 0} x ${snapshot?.display.primary_height ?? 0}`
        : "detail pending platform enumeration",
    live: (snapshot?.display.display_count ?? 0) > index,
    active: index === 0,
  }));
}

function localInputDeviceItems(
  kind: "keyboard" | "mouse",
  title: string,
  devices: NonNullable<LocalControlsSnapshot["keyboard_devices"]>,
  captureSource: string,
  detected: boolean,
) {
  const summary = summarizeRawInputDevices(devices);
  const detailParts = [captureSource];
  if (devices.length) {
    detailParts.push(`${summary.physicalGroups.length || 1} physical group`);
    detailParts.push(`${devices.length} HID collection`);
  }
  if (summary.virtualGroups.length) {
    detailParts.push(`${summary.virtualGroups.length} virtual loopback`);
  }

  const aggregate = {
    id: `${kind}-aggregate`,
    name: title,
    detail: detailParts.join(" / "),
    live: detected,
    active: true,
  };

  if (summary.physicalGroups.length <= 1) {
    return [aggregate];
  }

  return [
    aggregate,
    ...summary.physicalGroups.map((group, index) => ({
      id: `${kind}-group-${group.key}`,
      name: group.label || `${title} ${index + 1}`,
      detail: `${group.devices.length} HID collection / ${group.detail}`,
      live: group.devices.some((device) => device.connected),
      active: false,
    })),
  ];
}

function summarizeRawInputDevices(devices: NonNullable<LocalControlsSnapshot["keyboard_devices"]>) {
  const groups = new Map<
    string,
    {
      key: string;
      label: string;
      detail: string;
      virtual: boolean;
      devices: typeof devices;
    }
  >();

  for (const device of devices) {
    const detail = device.driver_detail ?? device.device_instance_id ?? device.id ?? "";
    const key = rawInputGroupKey(detail);
    const existing = groups.get(key);
    if (existing) {
      existing.devices.push(device);
      continue;
    }
    groups.set(key, {
      key,
      label: rawInputGroupLabel(detail),
      detail: rawInputShortDetail(detail),
      virtual: isVirtualRawInputDevice(detail),
      devices: [device],
    });
  }

  const values = Array.from(groups.values());
  return {
    physicalGroups: values.filter((group) => !group.virtual),
    virtualGroups: values.filter((group) => group.virtual),
  };
}

function rawInputGroupKey(detail: string) {
  const normalized = detail.toUpperCase();
  if (normalized.includes("HID_DEVICE_SYSTEM_VHF")) {
    return "virtual:vhf";
  }

  const acpi = normalized.match(/ACPI#([^#]+)/);
  if (acpi) {
    return `acpi:${acpi[1]}`;
  }

  const hid = normalized.match(/HID#(VID_[0-9A-F]{4}&PID_[0-9A-F]{4}(?:&MI_[0-9A-F]{2})?)/);
  if (hid) {
    return `hid:${hid[1]}`;
  }

  return normalized.replace(/&COL\d+/g, "").split("#{")[0] || normalized;
}

function rawInputGroupLabel(detail: string) {
  const normalized = detail.toUpperCase();
  if (normalized.includes("HID_DEVICE_SYSTEM_VHF")) {
    return "RShare Virtual HID";
  }
  const acpi = normalized.match(/ACPI#([^#]+)/);
  if (acpi) {
    return `Built-in device ${acpi[1]}`;
  }
  const hid = normalized.match(/VID_([0-9A-F]{4})&PID_([0-9A-F]{4})/);
  if (hid) {
    return `HID ${hid[1]}:${hid[2]}`;
  }
  return "HID device group";
}

function rawInputShortDetail(detail: string) {
  if (!detail) {
    return "raw input";
  }
  return detail
    .replace(/^\\\\\?\\/i, "")
    .replace(/#\{.*$/i, "")
    .replace(/&COL\d+/gi, "");
}

function isVirtualRawInputDevice(detail: string) {
  return detail.toUpperCase().includes("HID_DEVICE_SYSTEM_VHF");
}

function DeviceDriverStrip({
  kind,
  snapshot,
  theme,
}: {
  kind: LocalDevicePageKind;
  snapshot: LocalControlsSnapshot | null;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  if (kind === "remote") {
    return null;
  }

  const devices = localDeviceItems(snapshot, kind).filter((device) => device.live);
  const driverText =
    kind === "keyboard"
      ? snapshot?.keyboard.capture_source
      : kind === "mouse"
        ? snapshot?.mouse.capture_source
        : kind === "gamepad"
          ? snapshot?.virtual_gamepad.detail
          : driverStatusLabel(snapshot);

  if (!devices.length && !driverText) {
    return null;
  }

  return (
    <div
      className="rshare-scroll flex min-h-11 items-center gap-2 overflow-x-auto border-t px-3 py-2"
      style={{ borderTopColor: theme.border }}
    >
      {devices.map((device) => (
        <span
          key={device.id}
          className="inline-flex h-8 min-w-36 max-w-64 shrink-0 items-center gap-2 rounded-md px-3 text-sm"
          style={{
            border: `1px solid ${theme.border}`,
            background: "rgba(255,255,255,0.72)",
            color: theme.text,
          }}
          title={`${device.name} / ${device.detail}`}
        >
          <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: theme.success }} />
          <span className="truncate">{device.name}</span>
        </span>
      ))}
      {driverText ? (
        <span
          className="inline-flex h-8 min-w-44 max-w-[420px] shrink-0 items-center rounded-md px-3 text-sm"
          style={{
            border: `1px solid ${theme.border}`,
            background: "rgba(255,255,255,0.58)",
            color: theme.textMuted,
          }}
          title={driverText}
        >
          <span className="truncate">{driverText}</span>
        </span>
      ) : null}
    </div>
  );
}

function LocalControlTypeButton({
  kind,
  active,
  icon,
  title,
  detail,
  live,
  onClick,
  theme,
}: {
  kind: LocalDevicePageKind;
  active: boolean;
  icon: ReactNode;
  title: string;
  detail: string;
  live: boolean;
  onClick: (kind: LocalDevicePageKind) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      className="flex h-10 min-w-[132px] shrink-0 items-center justify-center gap-2 rounded-md px-3 text-sm transition"
      style={{
        border: `1px solid ${active ? theme.accent : "transparent"}`,
        background: active ? theme.accentSoft : "rgba(255,255,255,0.035)",
        color: theme.text,
      }}
      onClick={() => onClick(kind)}
    >
      <span className="shrink-0">{icon}</span>
      <span className="font-medium">{title}</span>
      <span className="text-xs" style={{ color: theme.textMuted }}>
        {detail}
      </span>
      <span className="h-2 w-2 rounded-full" style={{ background: live ? theme.success : theme.textMuted }} />
    </button>
  );
}

function LocalControlDetail({
  kind,
  snapshot,
  inputTestResult,
  confirmingInputTest,
  onRunInputTest,
  theme,
}: {
  kind: LocalControlKind;
  snapshot: LocalControlsSnapshot | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunInputTest: (kind: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const gamepad = snapshot?.gamepads?.find((item) => item.connected) ?? snapshot?.gamepads?.[0];

  if (kind === "keyboard") {
    const keyboardEvents = (snapshot?.recent_events ?? [])
      .filter((event) => event.device_kind === "Keyboard")
      .slice(-12)
      .reverse();
    return (
      <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]">
        <div className="min-h-0">
          <SimulatedKeyboard
            pressedKeys={snapshot?.keyboard.pressed_keys ?? []}
            lastKey={snapshot?.keyboard.last_key ?? null}
            recentEvents={snapshot?.recent_events ?? []}
            eventCount={snapshot?.keyboard.event_count ?? 0}
            theme={theme}
          />
        </div>
        <div className="flex min-h-0 flex-col gap-3">
          <KeyboardEventLog events={keyboardEvents} theme={theme} />
          <InputTestAction
            label={confirmingInputTest === "keyboard" ? "再次点击执行 Shift 测试" : "真实注入测试"}
            result={inputTestResult}
            disabled={!snapshot}
            onClick={() => onRunInputTest("keyboard")}
            theme={theme}
          />
        </div>
      </div>
    );
  }

  if (kind === "mouse") {
    const mouseEvents = (snapshot?.recent_events ?? [])
      .filter((event) => event.device_kind === "Mouse")
      .slice(-12)
      .reverse();
    return (
      <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]">
        <div className="min-h-0">
          <SimulatedMouse
            x={snapshot?.mouse.x ?? 0}
            y={snapshot?.mouse.y ?? 0}
            pressedButtons={snapshot?.mouse.pressed_buttons ?? []}
            wheelDeltaX={snapshot?.mouse.wheel_delta_x ?? 0}
            wheelDeltaY={snapshot?.mouse.wheel_delta_y ?? 0}
            wheelTotalX={snapshot?.mouse.wheel_total_x ?? 0}
            wheelTotalY={snapshot?.mouse.wheel_total_y ?? 0}
            eventCount={snapshot?.mouse.event_count ?? 0}
            moveCount={snapshot?.mouse.move_count ?? 0}
            buttonPressCount={snapshot?.mouse.button_press_count ?? 0}
            buttonReleaseCount={snapshot?.mouse.button_release_count ?? 0}
            wheelEventCount={snapshot?.mouse.wheel_event_count ?? 0}
            displayRelativeX={snapshot?.mouse.display_relative_x ?? snapshot?.mouse.x ?? 0}
            displayRelativeY={snapshot?.mouse.display_relative_y ?? snapshot?.mouse.y ?? 0}
            currentDisplayIndex={snapshot?.mouse.current_display_index ?? null}
            currentDisplayId={snapshot?.mouse.current_display_id ?? null}
            displays={snapshot?.display.displays ?? []}
            theme={theme}
          />
        </div>
        <div className="flex min-h-0 flex-col gap-3">
          <MouseEventLog events={mouseEvents} theme={theme} />
          <InputTestAction
            label={confirmingInputTest === "mouse" ? "再次点击执行移动测试" : "真实注入测试"}
            result={inputTestResult}
            disabled={!snapshot}
            onClick={() => onRunInputTest("mouse")}
            theme={theme}
          />
        </div>
      </div>
    );
  }

  if (kind === "gamepad") {
    const gamepadEvents = (snapshot?.recent_events ?? [])
      .filter((event) => event.device_kind === "Gamepad")
      .slice(-12)
      .reverse();
    return (
      <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]">
        <div className="min-h-0">
          <SimulatedGamepad
            gamepad={gamepad ?? null}
            virtualDetail={snapshot?.virtual_gamepad.detail ?? "Virtual HID not implemented"}
            theme={theme}
          />
        </div>
        <GamepadEventLog events={gamepadEvents} theme={theme} />
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
      <InfoRow label="显示器数量" value={String(snapshot?.display.display_count ?? 0)} theme={theme} />
      <InfoRow label="主显示器" value={`${snapshot?.display.primary_width ?? 0} x ${snapshot?.display.primary_height ?? 0}`} theme={theme} />
      <InfoRow label="布局尺寸" value={`${snapshot?.display.layout_width ?? 0} x ${snapshot?.display.layout_height ?? 0}`} theme={theme} />
      <InfoRow label="显示设置" value="跳转能力待接入平台设置入口" theme={theme} />
    </div>
  );
}

const KEYBOARD_ROWS: Array<Array<{ label: string; codes: string[]; width?: number }>> = [
  [
    { label: "Esc", codes: ["Escape"], width: 1.2 },
    { label: "F1", codes: ["F1", "Raw(112)"] },
    { label: "F2", codes: ["F2", "Raw(113)"] },
    { label: "F3", codes: ["F3", "Raw(114)"] },
    { label: "F4", codes: ["F4", "Raw(115)"] },
    { label: "F5", codes: ["F5", "Raw(116)"] },
    { label: "F6", codes: ["F6", "Raw(117)"] },
    { label: "F7", codes: ["F7", "Raw(118)"] },
    { label: "F8", codes: ["F8", "Raw(119)"] },
    { label: "F9", codes: ["F9", "Raw(120)"] },
    { label: "F10", codes: ["F10", "Raw(121)"] },
    { label: "F11", codes: ["F11", "Raw(122)"] },
    { label: "F12", codes: ["F12", "Raw(123)"] },
    { label: "PrtSc", codes: ["PrintScreen", "Snapshot", "Raw(44)"] },
    { label: "Scroll", codes: ["ScrollLock", "Raw(145)"] },
    { label: "Pause", codes: ["Pause", "Raw(19)"] },
  ],
  [
    { label: "`", codes: ["Raw(192)"] },
    { label: "1", codes: ["Char(49)", "Raw(49)"] },
    { label: "2", codes: ["Char(50)", "Raw(50)"] },
    { label: "3", codes: ["Char(51)", "Raw(51)"] },
    { label: "4", codes: ["Char(52)", "Raw(52)"] },
    { label: "5", codes: ["Char(53)", "Raw(53)"] },
    { label: "6", codes: ["Char(54)", "Raw(54)"] },
    { label: "7", codes: ["Char(55)", "Raw(55)"] },
    { label: "8", codes: ["Char(56)", "Raw(56)"] },
    { label: "9", codes: ["Char(57)", "Raw(57)"] },
    { label: "0", codes: ["Char(48)", "Raw(48)"] },
    { label: "-", codes: ["Raw(189)"] },
    { label: "=", codes: ["Raw(187)"] },
    { label: "Backspace", codes: ["Backspace", "Raw(8)"], width: 2 },
    { label: "Ins", codes: ["Insert", "Raw(45)"] },
    { label: "Home", codes: ["Home", "Raw(36)"] },
    { label: "PgUp", codes: ["PageUp", "Raw(33)"] },
    { label: "Num", codes: ["NumLock", "Raw(144)"] },
    { label: "/", codes: ["KeypadDivide", "Raw(111)"] },
    { label: "*", codes: ["KeypadMultiply", "Raw(106)"] },
    { label: "-", codes: ["KeypadSubtract", "Raw(109)"] },
  ],
  [
    { label: "Tab", codes: ["Tab", "Raw(9)"], width: 1.5 },
    { label: "Q", codes: ["Char(81)", "Raw(81)"] },
    { label: "W", codes: ["Char(87)", "Raw(87)"] },
    { label: "E", codes: ["Char(69)", "Raw(69)"] },
    { label: "R", codes: ["Char(82)", "Raw(82)"] },
    { label: "T", codes: ["Char(84)", "Raw(84)"] },
    { label: "Y", codes: ["Char(89)", "Raw(89)"] },
    { label: "U", codes: ["Char(85)", "Raw(85)"] },
    { label: "I", codes: ["Char(73)", "Raw(73)"] },
    { label: "O", codes: ["Char(79)", "Raw(79)"] },
    { label: "P", codes: ["Char(80)", "Raw(80)"] },
    { label: "[", codes: ["Raw(219)"] },
    { label: "]", codes: ["Raw(221)"] },
    { label: "\\", codes: ["Raw(220)"], width: 1.5 },
    { label: "Del", codes: ["Delete", "Raw(46)"] },
    { label: "End", codes: ["End", "Raw(35)"] },
    { label: "PgDn", codes: ["PageDown", "Raw(34)"] },
    { label: "7", codes: ["Keypad7", "Raw(103)"] },
    { label: "8", codes: ["Keypad8", "Raw(104)"] },
    { label: "9", codes: ["Keypad9", "Raw(105)"] },
    { label: "+", codes: ["KeypadAdd", "Raw(107)"] },
  ],
  [
    { label: "Caps", codes: ["CapsLock", "Raw(20)"], width: 1.8 },
    { label: "A", codes: ["Char(65)", "Raw(65)"] },
    { label: "S", codes: ["Char(83)", "Raw(83)"] },
    { label: "D", codes: ["Char(68)", "Raw(68)"] },
    { label: "F", codes: ["Char(70)", "Raw(70)"] },
    { label: "G", codes: ["Char(71)", "Raw(71)"] },
    { label: "H", codes: ["Char(72)", "Raw(72)"] },
    { label: "J", codes: ["Char(74)", "Raw(74)"] },
    { label: "K", codes: ["Char(75)", "Raw(75)"] },
    { label: "L", codes: ["Char(76)", "Raw(76)"] },
    { label: ";", codes: ["Raw(186)"] },
    { label: "'", codes: ["Raw(222)"] },
    { label: "Enter", codes: ["Enter", "Raw(13)"], width: 2.2 },
    { label: "4", codes: ["Keypad4", "Raw(100)"] },
    { label: "5", codes: ["Keypad5", "Raw(101)"] },
    { label: "6", codes: ["Keypad6", "Raw(102)"] },
    { label: "+", codes: ["KeypadAdd", "Raw(107)"] },
  ],
  [
    { label: "Shift", codes: ["ShiftLeft", "Raw(16)", "Raw(160)"], width: 2.3 },
    { label: "Z", codes: ["Char(90)", "Raw(90)"] },
    { label: "X", codes: ["Char(88)", "Raw(88)"] },
    { label: "C", codes: ["Char(67)", "Raw(67)"] },
    { label: "V", codes: ["Char(86)", "Raw(86)"] },
    { label: "B", codes: ["Char(66)", "Raw(66)"] },
    { label: "N", codes: ["Char(78)", "Raw(78)"] },
    { label: "M", codes: ["Char(77)", "Raw(77)"] },
    { label: ",", codes: ["Raw(188)"] },
    { label: ".", codes: ["Raw(190)"] },
    { label: "/", codes: ["Raw(191)"] },
    { label: "Shift", codes: ["ShiftRight", "Raw(16)", "Raw(161)"], width: 2.7 },
    { label: "Up", codes: ["Up", "Raw(38)"] },
    { label: "1", codes: ["Keypad1", "Raw(97)"] },
    { label: "2", codes: ["Keypad2", "Raw(98)"] },
    { label: "3", codes: ["Keypad3", "Raw(99)"] },
    { label: "Enter", codes: ["KeypadEnter", "Raw(13)"] },
  ],
  [
    { label: "Ctrl", codes: ["ControlLeft", "Raw(17)", "Raw(162)"], width: 1.5 },
    { label: "Win", codes: ["SuperLeft", "Raw(91)"], width: 1.3 },
    { label: "Alt", codes: ["AltLeft", "Raw(18)", "Raw(164)"], width: 1.3 },
    { label: "Space", codes: ["Space", "Raw(32)"], width: 6 },
    { label: "Alt", codes: ["AltRight", "Raw(18)", "Raw(165)"], width: 1.3 },
    { label: "Win", codes: ["SuperRight", "Raw(92)"], width: 1.3 },
    { label: "Menu", codes: ["Raw(93)"], width: 1.3 },
    { label: "Ctrl", codes: ["ControlRight", "Raw(17)", "Raw(163)"], width: 1.5 },
    { label: "←", codes: ["Left", "Raw(37)"] },
    { label: "↓", codes: ["Down", "Raw(40)"] },
    { label: "→", codes: ["Right", "Raw(39)"] },
    { label: "0", codes: ["Keypad0", "Raw(96)"], width: 2 },
    { label: ".", codes: ["KeypadDecimal", "Raw(110)"] },
    { label: "Enter", codes: ["KeypadEnter", "Raw(13)"] },
  ],
];

function normalizeKeyToken(value: string | null | undefined) {
  return String(value ?? "").toLowerCase().replace(/\s/g, "");
}

function keyboardEventKey(event: LocalControlEvent | null | undefined) {
  if (!event || event.device_kind !== "Keyboard") {
    return null;
  }
  if (event.payload?.key) {
    return normalizeIncomingKeyName(event.payload.key);
  }
  const match = event.summary.match(/Key\s+(.+?)\s+(Pressed|Released|Down|Up)$/i);
  return normalizeIncomingKeyName(match?.[1] ?? null);
}

function normalizeIncomingKeyName(value: string | null | undefined) {
  if (!value) {
    return null;
  }
  const letter = value.match(/^Key([A-Z])$/i);
  if (letter) {
    return `Char(${letter[1].toUpperCase().charCodeAt(0)})`;
  }
  const digit = value.match(/^Num([0-9])$/i);
  if (digit) {
    return `Char(${digit[1].charCodeAt(0)})`;
  }
  return value;
}

function eventStateIsPressed(event: LocalControlEvent) {
  const state = event.payload?.state ?? event.summary;
  return /\b(pressed|down)\b/i.test(state);
}

function eventStateIsReleased(event: LocalControlEvent) {
  const state = event.payload?.state ?? event.summary;
  return /\b(released|up)\b/i.test(state);
}

function keyboardEventLabel(event: LocalControlEvent) {
  return keyDisplayName(keyboardEventKey(event) ?? event.summary);
}

function keyDisplayName(value: string) {
  const raw = value.match(/^Raw\((\d+)\)$/i);
  if (raw) {
    const vk = Number(raw[1]);
    return VK_DISPLAY_NAMES[vk] ?? `VK ${vk}`;
  }
  const char = value.match(/^Char\((\d+)\)$/i);
  if (char) {
    return String.fromCharCode(Number(char[1]));
  }
  return value;
}

function keyboardEventTime(event: LocalControlEvent) {
  if (!event.timestamp_ms) {
    return `#${event.sequence}`;
  }
  return new Date(event.timestamp_ms).toLocaleTimeString("zh-CN", {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  });
}

function keyboardEventMatchesKey(
  event: LocalControlEvent,
  key: { label: string; codes: string[] },
) {
  const eventKey = keyboardEventKey(event);
  if (!eventKey) {
    return false;
  }
  const normalizedEventKey = normalizeKeyToken(eventKey);
  return [key.label, ...key.codes]
    .map((value) => normalizeKeyToken(value))
    .includes(normalizedEventKey);
}

function keyVisualState(
  key: { label: string; codes: string[] },
  pressedKeys: string[],
  recentEvents: LocalControlEvent[],
  lastKey: string | null,
) {
  const normalizedPressed = new Set(
    pressedKeys.map((value) => normalizeKeyToken(value)),
  );
  const candidates = [key.label, ...key.codes].map((value) =>
    normalizeKeyToken(value),
  );
  if (candidates.some((candidate) => normalizedPressed.has(candidate))) {
    return "pressed";
  }
  if (
    recentEvents.some((event) => keyboardEventMatchesKey(event, key))
    || Boolean(lastKey && candidates.includes(normalizeKeyToken(lastKey)))
  ) {
    return "tested";
  }
  return "idle";
}

function keyboardUniqueTestedCount(events: LocalControlEvent[]) {
  return new Set(
    events
      .map((event) => keyboardEventKey(event))
      .filter((key): key is string => Boolean(key))
      .map((key) => normalizeKeyToken(key)),
  ).size;
}

const KEYBOARD_CANVAS_WIDTH = 1340;
const KEYBOARD_CANVAS_HEIGHT = 294;

function SimulatedKeyboard({
  pressedKeys,
  lastKey,
  recentEvents,
  eventCount,
  theme,
}: {
  pressedKeys: string[];
  lastKey: string | null;
  recentEvents: LocalControlEvent[];
  eventCount: number;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const activeCount = pressedKeys.length;
  const keyboardEvents = recentEvents.filter((event) => event.device_kind === "Keyboard");
  const testedCount = keyboardUniqueTestedCount(keyboardEvents);
  const pressedCount = keyboardEvents.filter(eventStateIsPressed).length;
  const releasedCount = keyboardEvents.filter(eventStateIsReleased).length;
  const [keyboardFrameRef, keyboardFrameSize] = useElementSize<HTMLDivElement>();
  const keyboardScale = Math.min(
    1,
    Math.max(
      0.2,
      Math.min(
        (keyboardFrameSize.width || KEYBOARD_CANVAS_WIDTH) / KEYBOARD_CANVAS_WIDTH,
        (keyboardFrameSize.height || KEYBOARD_CANVAS_HEIGHT) / KEYBOARD_CANVAS_HEIGHT,
      ),
    ),
  );
  const scaledKeyboardWidth = KEYBOARD_CANVAS_WIDTH * keyboardScale;
  const scaledKeyboardHeight = KEYBOARD_CANVAS_HEIGHT * keyboardScale;
  return (
    <div
      className="flex h-full min-h-0 flex-col overflow-hidden p-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="mb-3 flex shrink-0 flex-wrap items-center justify-between gap-2">
        <div className="text-sm font-medium">键盘按键测试</div>
        <div className="flex flex-wrap items-center gap-2 text-xs" style={{ color: theme.textMuted }}>
          <KeyboardLegend tone="idle" label="未按过" theme={theme} />
          <KeyboardLegend tone="tested" label="按过后" theme={theme} />
          <KeyboardLegend tone="pressed" label="激活状态" theme={theme} />
        </div>
      </div>
      <div ref={keyboardFrameRef} className="min-h-0 flex-1 overflow-hidden">
        <div
          className="mx-auto"
          style={{ width: scaledKeyboardWidth, height: scaledKeyboardHeight }}
        >
          <div
            className="flex flex-col gap-1.5"
            style={{
              width: KEYBOARD_CANVAS_WIDTH,
              height: KEYBOARD_CANVAS_HEIGHT,
              transform: `scale(${keyboardScale})`,
              transformOrigin: "top left",
            }}
          >
        {KEYBOARD_ROWS.map((row, rowIndex) => (
          <div key={rowIndex} className="flex gap-1.5">
            {row.map((key, keyIndex) => {
              const state = keyVisualState(key, pressedKeys, keyboardEvents, lastKey);
              const pressed = state === "pressed";
              const tested = state === "tested";
              return (
                <div
                  key={`${rowIndex}-${keyIndex}-${key.label}`}
                  className="flex h-11 items-center justify-start rounded-md px-2 text-sm transition"
                  style={{
                    flex: `${key.width ?? 1} 0 0`,
                    minWidth: 36,
                    border: `1px solid ${pressed ? theme.accent : tested ? "rgba(80, 140, 245, 0.45)" : theme.border}`,
                    background: pressed
                      ? theme.accent
                      : tested
                        ? "rgba(80, 140, 245, 0.18)"
                        : "rgba(255,255,255,0.055)",
                    color: pressed ? "#ffffff" : theme.text,
                    boxShadow: pressed
                      ? `inset 0 -4px 0 rgba(0,0,0,0.24), 0 0 0 1px ${theme.accent}`
                      : tested
                        ? "inset 0 -3px 0 rgba(80, 140, 245, 0.35)"
                        : "inset 0 -2px 0 rgba(0,0,0,0.18)",
                    transform: pressed ? "translateY(1px)" : "translateY(0)",
                  }}
                >
                  <span className="truncate">{key.label}</span>
                </div>
              );
            })}
          </div>
        ))}
          </div>
        </div>
      </div>
      <div className="mt-3 grid shrink-0 grid-cols-3 gap-2 text-xs xl:grid-cols-6">
        <KeyboardSignal label="最后按键" value={lastKey ? keyDisplayName(lastKey) : "无"} theme={theme} />
        <KeyboardSignal label="按下状态" value={activeCount ? `${activeCount} 个按下` : "无"} theme={theme} />
        <KeyboardSignal label="已测按键" value={`${testedCount}/104`} theme={theme} />
        <KeyboardSignal label="按下次数" value={String(pressedCount)} theme={theme} />
        <KeyboardSignal label="抬起次数" value={String(releasedCount)} theme={theme} />
        <KeyboardSignal label="总事件数" value={String(eventCount)} theme={theme} />
      </div>
    </div>
  );
}

function KeyboardEventLog({
  events,
  theme,
}: {
  events: LocalControlEvent[];
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col p-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="mb-2 flex shrink-0 items-center justify-between gap-3">
        <div className="text-sm font-medium">按键记录</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          最近 {events.length} 条
        </div>
      </div>
      <div className="rshare-scroll min-h-0 flex-1 space-y-1.5 overflow-auto pr-1">
        {events.length ? (
          events.map((event) => (
            <div
              key={`${event.sequence}-${event.summary}`}
              className="grid grid-cols-[74px_minmax(0,1fr)_36px] items-center gap-2 text-xs"
              style={{ color: theme.text }}
            >
              <span style={{ color: theme.textMuted }}>{keyboardEventTime(event)}</span>
              <span
                className="truncate rounded px-2 py-1 text-center font-medium"
                style={{
                  background: eventStateIsPressed(event)
                    ? theme.accentSoft
                    : "rgba(255,255,255,0.065)",
                }}
              >
                {keyboardEventLabel(event)}
              </span>
              <span style={{ color: theme.textMuted }}>
                {eventStateIsPressed(event) ? "按下" : eventStateIsReleased(event) ? "抬起" : "事件"}
              </span>
            </div>
          ))
        ) : (
          <div className="text-xs" style={{ color: theme.textMuted }}>
            等待键盘输入
          </div>
        )}
      </div>
    </div>
  );
}

function MouseEventLog({
  events,
  theme,
}: {
  events: LocalControlEvent[];
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col p-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="mb-2 flex shrink-0 items-center justify-between gap-3">
        <div className="text-sm font-medium">鼠标记录</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          最近 {events.length} 条
        </div>
      </div>
      <div className="rshare-scroll min-h-0 flex-1 space-y-1.5 overflow-auto pr-1">
        {events.length ? (
          events.map((event) => (
            <div
              key={`${event.sequence}-${event.summary}`}
              className="grid grid-cols-[74px_minmax(0,1fr)] items-center gap-2 text-xs"
              style={{ color: theme.text }}
            >
              <span style={{ color: theme.textMuted }}>{keyboardEventTime(event)}</span>
              <span
                className="truncate rounded px-2 py-1"
                style={{ background: "rgba(255,255,255,0.055)" }}
              >
                {mouseEventLabel(event)}
              </span>
            </div>
          ))
        ) : (
          <div className="text-xs" style={{ color: theme.textMuted }}>
            等待鼠标输入
          </div>
        )}
      </div>
    </div>
  );
}

function mouseEventLabel(event: LocalControlEvent) {
  if (event.event_kind === "move") {
    const display = event.payload?.display_id ? ` / ${event.payload.display_id}` : "";
    return `${event.payload?.x ?? "0"}, ${event.payload?.y ?? "0"}${display}`;
  }
  if (event.event_kind === "button") {
    return `${event.payload?.button ?? "Button"} ${event.payload?.state ?? ""}`.trim();
  }
  if (event.event_kind === "wheel") {
    const dx = Number(event.payload?.delta_x ?? 0);
    const dy = Number(event.payload?.delta_y ?? 0);
    if (dx !== 0) {
      return `${dx > 0 ? "水平右滚" : "水平左滚"} ${Math.abs(dx)}`;
    }
    return `${dy > 0 ? "向上滚动" : "向下滚动"} ${Math.abs(dy)}`;
  }
  return event.summary;
}

function KeyboardLegend({
  tone,
  label,
  theme,
}: {
  tone: "idle" | "tested" | "pressed";
  label: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const background =
    tone === "pressed" ? theme.accent : tone === "tested" ? "rgba(80, 140, 245, 0.22)" : "rgba(255,255,255,0.075)";
  return (
    <span className="inline-flex items-center gap-1.5">
      <span
        className="inline-block h-3 w-3 rounded-sm"
        style={{
          background,
          border: `1px solid ${tone === "idle" ? theme.border : "rgba(80, 140, 245, 0.55)"}`,
        }}
      />
      {label}
    </span>
  );
}

const VK_DISPLAY_NAMES: Record<number, string> = {
  8: "Backspace",
  9: "Tab",
  13: "Enter",
  16: "Shift",
  17: "Ctrl",
  18: "Alt",
  19: "Pause",
  20: "Caps",
  27: "Esc",
  32: "Space",
  33: "PgUp",
  34: "PgDn",
  35: "End",
  36: "Home",
  37: "Left",
  38: "Up",
  39: "Right",
  40: "Down",
  44: "PrtSc",
  45: "Ins",
  46: "Del",
  48: "0",
  49: "1",
  50: "2",
  51: "3",
  52: "4",
  53: "5",
  54: "6",
  55: "7",
  56: "8",
  57: "9",
  65: "A",
  66: "B",
  67: "C",
  68: "D",
  69: "E",
  70: "F",
  71: "G",
  72: "H",
  73: "I",
  74: "J",
  75: "K",
  76: "L",
  77: "M",
  78: "N",
  79: "O",
  80: "P",
  81: "Q",
  82: "R",
  83: "S",
  84: "T",
  85: "U",
  86: "V",
  87: "W",
  88: "X",
  89: "Y",
  90: "Z",
  91: "Win",
  92: "Win",
  93: "Menu",
  96: "Numpad 0",
  97: "Numpad 1",
  98: "Numpad 2",
  99: "Numpad 3",
  100: "Numpad 4",
  101: "Numpad 5",
  102: "Numpad 6",
  103: "Numpad 7",
  104: "Numpad 8",
  105: "Numpad 9",
  106: "Numpad *",
  107: "Numpad +",
  109: "Numpad -",
  110: "Numpad .",
  111: "Numpad /",
  112: "F1",
  113: "F2",
  114: "F3",
  115: "F4",
  116: "F5",
  117: "F6",
  118: "F7",
  119: "F8",
  120: "F9",
  121: "F10",
  122: "F11",
  123: "F12",
  144: "Num",
  145: "Scroll",
  160: "Shift",
  161: "Shift",
  162: "Ctrl",
  163: "Ctrl",
  164: "Alt",
  165: "Alt",
  186: ";",
  187: "=",
  188: ",",
  189: "-",
  190: ".",
  191: "/",
  192: "`",
  219: "[",
  220: "\\",
  221: "]",
  222: "'",
};

function KeyboardSignal({
  label,
  value,
  theme,
}: {
  label: string;
  value: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div className="rounded px-3 py-2" style={{ background: "rgba(255,255,255,0.035)" }}>
      <div style={{ color: theme.textMuted }}>{label}</div>
      <div className="mt-1 truncate" style={{ color: theme.text }}>{value}</div>
    </div>
  );
}

function mouseButtonPressed(buttons: string[], name: string) {
  return buttons.some((button) => button.toLowerCase() === name.toLowerCase());
}

function SimulatedMouse({
  x,
  y,
  pressedButtons,
  wheelDeltaX,
  wheelDeltaY,
  wheelTotalX,
  wheelTotalY,
  eventCount,
  moveCount,
  buttonPressCount,
  buttonReleaseCount,
  wheelEventCount,
  displayRelativeX,
  displayRelativeY,
  currentDisplayIndex,
  currentDisplayId,
  displays,
  theme,
}: {
  x: number;
  y: number;
  pressedButtons: string[];
  wheelDeltaX: number;
  wheelDeltaY: number;
  wheelTotalX: number;
  wheelTotalY: number;
  eventCount: number;
  moveCount: number;
  buttonPressCount: number;
  buttonReleaseCount: number;
  wheelEventCount: number;
  displayRelativeX: number;
  displayRelativeY: number;
  currentDisplayIndex: number | null;
  currentDisplayId: string | null;
  displays: NonNullable<LocalControlsSnapshot["display"]["displays"]>;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const leftDown = mouseButtonPressed(pressedButtons, "Left");
  const rightDown = mouseButtonPressed(pressedButtons, "Right");
  const middleDown = mouseButtonPressed(pressedButtons, "Middle");
  const backDown = mouseButtonPressed(pressedButtons, "Back");
  const forwardDown = mouseButtonPressed(pressedButtons, "Forward");
  const activeDisplay =
    currentDisplayIndex !== null && currentDisplayIndex >= 0
      ? displays[currentDisplayIndex] ?? null
      : null;
  const fallbackDisplay = displays[0] ?? {
    display_id: "primary",
    x: 0,
    y: 0,
    width: 1920,
    height: 1080,
    primary: true,
  };
  const display = activeDisplay ?? fallbackDisplay;
  const padX = clampPercent((displayRelativeX / Math.max(1, display.width)) * 100);
  const padY = clampPercent((displayRelativeY / Math.max(1, display.height)) * 100);
  const displayName =
    currentDisplayId ??
    (activeDisplay ? `display-${(currentDisplayIndex ?? 0) + 1}` : "虚拟桌面");
  const wheelActive = wheelDeltaX !== 0 || wheelDeltaY !== 0;

  return (
    <div
      className="grid h-full min-h-0 grid-cols-1 gap-4 p-4 xl:grid-cols-[minmax(220px,320px)_minmax(0,1fr)]"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="flex items-center justify-center">
        <svg className="h-full max-h-[360px] w-full max-w-[280px]" viewBox="0 0 220 280" role="img" aria-label="mouse input preview">
          <rect x="54" y="18" width="112" height="236" rx="56" fill="rgba(255,255,255,0.05)" stroke={theme.border} />
          <path
            d="M110 18 L110 116"
            stroke={theme.border}
            strokeWidth="2"
          />
          <path
            d="M57 116 C58 52 82 20 110 18 L110 116 Z"
            fill={leftDown ? theme.accentSoft : "rgba(255,255,255,0.04)"}
            stroke={leftDown ? theme.accent : theme.border}
          />
          <path
            d="M110 18 C138 20 162 52 163 116 L110 116 Z"
            fill={rightDown ? theme.accentSoft : "rgba(255,255,255,0.04)"}
            stroke={rightDown ? theme.accent : theme.border}
          />
          <rect
            x="101"
            y="64"
            width="18"
            height="46"
            rx="9"
            fill={middleDown || wheelActive ? theme.accentSoft : "rgba(255,255,255,0.08)"}
            stroke={middleDown || wheelActive ? theme.accent : theme.border}
          />
          <path d="M54 132 C43 143 39 160 41 184" fill="none" stroke={backDown ? theme.accent : theme.border} strokeWidth={backDown ? 6 : 4} strokeLinecap="round" />
          <path d="M166 132 C177 143 181 160 179 184" fill="none" stroke={forwardDown ? theme.accent : theme.border} strokeWidth={forwardDown ? 6 : 4} strokeLinecap="round" />
          <text x="82" y="82" fill={theme.textMuted} fontSize="11">L</text>
          <text x="134" y="82" fill={theme.textMuted} fontSize="11">R</text>
          <text x="105" y="92" fill={theme.textMuted} fontSize="9">W</text>
          <text x="16" y="164" fill={backDown ? theme.accent : theme.textMuted} fontSize="10">Back</text>
          <text x="178" y="164" fill={forwardDown ? theme.accent : theme.textMuted} fontSize="10">Fwd</text>
        </svg>
      </div>
      <div className="flex min-w-0 flex-col gap-3">
        <div className="text-sm font-medium">鼠标实时绘制</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          全局 {Math.round(x)}, {Math.round(y)} / {displayName} 内 {Math.round(displayRelativeX)}, {Math.round(displayRelativeY)} · {display.width} x {display.height} @ {display.x}, {display.y}
        </div>
        <div
          className="relative min-h-0 flex-1 overflow-hidden rounded"
          style={{
            border: `1px solid ${theme.border}`,
            backgroundImage:
              "linear-gradient(rgba(255,255,255,0.045) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.045) 1px, transparent 1px)",
            backgroundSize: "20px 20px",
          }}
        >
          {displays.length ? (
            <div className="absolute left-3 top-3 flex max-w-[60%] flex-wrap gap-1">
              {displays.map((item, index) => (
                <span
                  key={item.display_id}
                  className="rounded px-2 py-1 text-[11px]"
                  style={{
                    border: `1px solid ${index === currentDisplayIndex ? theme.accent : theme.border}`,
                    background: index === currentDisplayIndex ? theme.accentSoft : "rgba(255,255,255,0.055)",
                    color: theme.text,
                  }}
                >
                  {index + 1}: {item.x},{item.y}
                </span>
              ))}
            </div>
          ) : null}
          <div
            className="absolute h-4 w-4 rounded-full"
            style={{
              left: `${padX}%`,
              top: `${padY}%`,
              transform: "translate(-50%, -50%)",
              background: theme.accent,
              boxShadow: `0 0 0 6px ${theme.accentSoft}`,
            }}
          />
          <div
            className="absolute bottom-3 right-3 rounded px-2 py-1 text-xs"
            style={{
              background: "rgba(255,255,255,0.065)",
              color: theme.textMuted,
            }}
          >
            滚轮 Δ {wheelDeltaX}, {wheelDeltaY}
          </div>
        </div>
        <div className="grid shrink-0 grid-cols-2 gap-2 text-xs 2xl:grid-cols-4">
          <KeyboardSignal label="Left" value={leftDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Middle" value={middleDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Right" value={rightDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Back" value={backDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Forward" value={forwardDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="移动" value={String(moveCount)} theme={theme} />
          <KeyboardSignal label="按下/抬起" value={`${buttonPressCount}/${buttonReleaseCount}`} theme={theme} />
          <KeyboardSignal label="滚轮" value={`${wheelEventCount} / ${wheelTotalX}, ${wheelTotalY}`} theme={theme} />
          <KeyboardSignal label="事件" value={String(eventCount)} theme={theme} />
        </div>
      </div>
    </div>
  );
}

function clampPercent(value: number) {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.min(100, Math.max(0, value));
}

type LocalGamepadSnapshot = LocalControlsSnapshot["gamepads"][number];

function gamepadButtonName(button: string | Record<string, unknown>) {
  if (typeof button === "string") {
    return button;
  }
  const key = Object.keys(button)[0];
  if (!key) {
    return "Unknown";
  }
  const value = button[key];
  return value === null || value === undefined ? key : `${key}(${String(value)})`;
}

function normalizedGamepadButton(name: string) {
  return name.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function gamepadPressedButtons(gamepad: LocalGamepadSnapshot | null) {
  if (!gamepad) {
    return [];
  }
  if (gamepad.pressed_buttons?.length) {
    return gamepad.pressed_buttons;
  }
  return (gamepad.buttons ?? [])
    .filter((button) => button.pressed)
    .map((button) => gamepadButtonName(button.button));
}

function gamepadButtonActive(gamepad: LocalGamepadSnapshot | null, names: string[]) {
  const wanted = names.map(normalizedGamepadButton);
  return gamepadPressedButtons(gamepad).some((button) =>
    wanted.includes(normalizedGamepadButton(button)),
  );
}

function stickOffset(value: number) {
  const normalized = Math.max(-1, Math.min(1, Number(value ?? 0) / 32767));
  return normalized * 22;
}

function triggerFill(value: number) {
  return clampPercent((Number(value ?? 0) / 65535) * 100);
}

function SimulatedGamepad({
  gamepad,
  virtualDetail,
  theme,
}: {
  gamepad: LocalGamepadSnapshot | null;
  virtualDetail: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const connected = Boolean(gamepad?.connected);
  const pressed = gamepadPressedButtons(gamepad);
  const leftStickX = stickOffset(gamepad?.left_stick_x ?? 0);
  const leftStickY = -stickOffset(gamepad?.left_stick_y ?? 0);
  const rightStickX = stickOffset(gamepad?.right_stick_x ?? 0);
  const rightStickY = -stickOffset(gamepad?.right_stick_y ?? 0);
  const leftTrigger = triggerFill(gamepad?.left_trigger ?? 0);
  const rightTrigger = triggerFill(gamepad?.right_trigger ?? 0);
  const buttonFill = (active: boolean) => (active ? theme.accent : "rgba(255,255,255,0.075)");
  const buttonStroke = (active: boolean) => (active ? theme.accent : theme.border);
  const faceButtons = [
    { label: "Y", x: 500, y: 118, names: ["North", "Y"] },
    { label: "X", x: 460, y: 158, names: ["West", "X"] },
    { label: "B", x: 540, y: 158, names: ["East", "B"] },
    { label: "A", x: 500, y: 198, names: ["South", "A"] },
  ];

  return (
    <div
      className="grid h-full min-h-0 grid-rows-[minmax(0,1fr)_auto] gap-3 p-4"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="min-h-0">
        <div className="mb-2 flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="truncate text-sm font-medium">{gamepad?.name ?? "Gamepad"}</div>
            <div className="truncate text-xs" style={{ color: theme.textMuted }}>
              {connected ? "gilrs connected" : "waiting for device"} / {virtualDetail}
            </div>
          </div>
          <span
            className="shrink-0 rounded px-2 py-1 text-xs"
            style={{
              background: connected ? "rgba(45, 170, 91, 0.16)" : "rgba(255,255,255,0.05)",
              color: connected ? "#2fa55a" : theme.textMuted,
            }}
          >
            {connected ? "live" : "idle"}
          </span>
        </div>
        <svg className="h-full min-h-[260px] w-full" viewBox="0 0 640 360" role="img" aria-label="gamepad input preview">
          <path
            d="M176 86 C230 54 410 54 464 86 C542 132 594 234 570 288 C552 328 492 316 450 258 C424 224 392 214 320 214 C248 214 216 224 190 258 C148 316 88 328 70 288 C46 234 98 132 176 86 Z"
            fill="rgba(255,255,255,0.045)"
            stroke={theme.border}
            strokeWidth="2"
          />
          <rect x="156" y="56" width="110" height="28" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["LeftBumper"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["LeftBumper"]))} />
          <rect x="374" y="56" width="110" height="28" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["RightBumper"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["RightBumper"]))} />
          <rect x="174" y="24" width="74" height="20" rx="6" fill="rgba(255,255,255,0.055)" stroke={theme.border} />
          <rect x="392" y="24" width="74" height="20" rx="6" fill="rgba(255,255,255,0.055)" stroke={theme.border} />
          <rect x="174" y="24" width={Math.max(2, leftTrigger * 0.74)} height="20" rx="6" fill={theme.accentSoft} />
          <rect x="392" y="24" width={Math.max(2, rightTrigger * 0.74)} height="20" rx="6" fill={theme.accentSoft} />
          <text x="184" y="75" fill={theme.textMuted} fontSize="12">LB</text>
          <text x="440" y="75" fill={theme.textMuted} fontSize="12">RB</text>
          <text x="197" y="39" fill={theme.textMuted} fontSize="10">LT {Math.round(leftTrigger)}%</text>
          <text x="415" y="39" fill={theme.textMuted} fontSize="10">RT {Math.round(rightTrigger)}%</text>

          <g transform="translate(168 172)">
            <rect x="-18" y="-54" width="36" height="108" rx="8" fill="rgba(255,255,255,0.055)" stroke={theme.border} />
            <rect x="-54" y="-18" width="108" height="36" rx="8" fill="rgba(255,255,255,0.055)" stroke={theme.border} />
            <rect x="-18" y="-54" width="36" height="36" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["DPadUp"]))} />
            <rect x="-18" y="18" width="36" height="36" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["DPadDown"]))} />
            <rect x="-54" y="-18" width="36" height="36" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["DPadLeft"]))} />
            <rect x="18" y="-18" width="36" height="36" rx="8" fill={buttonFill(gamepadButtonActive(gamepad, ["DPadRight"]))} />
          </g>

          <g transform="translate(262 238)">
            <circle r="50" fill={buttonFill(gamepadButtonActive(gamepad, ["LeftStick"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["LeftStick"]))} />
            <circle cx={leftStickX} cy={leftStickY} r="22" fill={theme.accentSoft} stroke={theme.accent} />
            <text x="-20" y="78" fill={theme.textMuted} fontSize="12">L {Math.round(gamepad?.left_stick_x ?? 0)}, {Math.round(gamepad?.left_stick_y ?? 0)}</text>
          </g>

          <g transform="translate(388 238)">
            <circle r="50" fill={buttonFill(gamepadButtonActive(gamepad, ["RightStick"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["RightStick"]))} />
            <circle cx={rightStickX} cy={rightStickY} r="22" fill={theme.accentSoft} stroke={theme.accent} />
            <text x="-20" y="78" fill={theme.textMuted} fontSize="12">R {Math.round(gamepad?.right_stick_x ?? 0)}, {Math.round(gamepad?.right_stick_y ?? 0)}</text>
          </g>

          {faceButtons.map((button) => {
            const active = gamepadButtonActive(gamepad, button.names);
            return (
              <g key={button.label}>
                <circle cx={button.x} cy={button.y} r="24" fill={buttonFill(active)} stroke={buttonStroke(active)} strokeWidth="2" />
                <text x={button.x} y={button.y + 5} textAnchor="middle" fill={active ? theme.text : theme.textMuted} fontSize="16" fontWeight="600">{button.label}</text>
              </g>
            );
          })}

          <rect x="286" y="145" width="48" height="24" rx="10" fill={buttonFill(gamepadButtonActive(gamepad, ["Select"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["Select"]))} />
          <rect x="346" y="145" width="48" height="24" rx="10" fill={buttonFill(gamepadButtonActive(gamepad, ["Start"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["Start"]))} />
          <circle cx="320" cy="186" r="17" fill={buttonFill(gamepadButtonActive(gamepad, ["Guide"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["Guide"]))} />
          <text x="310" y="162" fill={theme.textMuted} fontSize="10" textAnchor="middle">Select</text>
          <text x="370" y="162" fill={theme.textMuted} fontSize="10" textAnchor="middle">Start</text>
        </svg>
      </div>
      <div className="grid shrink-0 grid-cols-2 gap-2 text-xs lg:grid-cols-4">
        <KeyboardSignal label="Pressed" value={pressed.length ? pressed.join(", ") : "none"} theme={theme} />
        <KeyboardSignal label="Last" value={gamepad?.last_button ?? "none"} theme={theme} />
        <KeyboardSignal label="Press/Release" value={`${gamepad?.button_press_count ?? 0}/${gamepad?.button_release_count ?? 0}`} theme={theme} />
        <KeyboardSignal label="Button events" value={String(gamepad?.button_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="Stick events" value={String(gamepad?.axis_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="Trigger events" value={String(gamepad?.trigger_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="Total" value={String(gamepad?.event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="Last axis" value={gamepad?.last_axis ?? "none"} theme={theme} />
      </div>
    </div>
  );
}

function GamepadEventLog({
  events,
  theme,
}: {
  events: LocalControlEvent[];
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col p-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="mb-2 flex shrink-0 items-center justify-between gap-3">
        <div className="text-sm font-medium">手柄记录</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          最近 {events.length} 条
        </div>
      </div>
      <div className="rshare-scroll min-h-0 flex-1 space-y-1.5 overflow-auto pr-1">
        {events.length ? (
          events.map((event) => (
            <div
              key={`${event.sequence}-${event.summary}`}
              className="grid grid-cols-[74px_minmax(0,1fr)] items-center gap-2 text-xs"
              style={{ color: theme.text }}
            >
              <span style={{ color: theme.textMuted }}>{keyboardEventTime(event)}</span>
              <span
                className="truncate rounded px-2 py-1"
                style={{ background: "rgba(255,255,255,0.055)" }}
              >
                {gamepadEventLabel(event)}
              </span>
            </div>
          ))
        ) : (
          <div className="text-xs" style={{ color: theme.textMuted }}>
            等待手柄输入
          </div>
        )}
      </div>
    </div>
  );
}

function gamepadEventLabel(event: LocalControlEvent) {
  if (event.event_kind === "connected") {
    return `connected ${event.payload?.name ?? event.payload?.gamepad_id ?? ""}`.trim();
  }
  if (event.event_kind === "disconnected") {
    return `disconnected ${event.payload?.gamepad_id ?? ""}`.trim();
  }
  if (event.payload?.last_button) {
    return event.payload.last_button;
  }
  if (event.payload?.last_axis) {
    return `${event.payload.last_axis} ${event.payload.left_stick_x ?? 0}, ${event.payload.left_stick_y ?? 0}`;
  }
  const pressed = event.payload?.pressed_buttons;
  return pressed ? `pressed ${pressed || "none"}` : event.summary;
}

function DriverControlStrip({
  items,
  theme,
}: {
  items: string[];
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex flex-wrap gap-2 p-3 lg:col-span-2"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      {items.map((item) => (
        <button
          key={item}
          type="button"
          className="rounded px-3 py-2 text-xs"
          style={{
            border: `1px solid ${theme.border}`,
            background: "rgba(255,255,255,0.035)",
            color: theme.textMuted,
          }}
          disabled
          title="待底层驱动能力接入"
        >
          {item}
        </button>
      ))}
    </div>
  );
}

function InputTestAction({
  label,
  result,
  disabled,
  onClick,
  theme,
}: {
  label: string;
  result: LocalInputTestResult | null;
  disabled: boolean;
  onClick: () => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="shrink-0 p-3"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="text-sm font-medium">真实注入</div>
        <button
          type="button"
          className="rounded-md px-3 py-2 text-sm transition"
          style={{
            border: `1px solid ${theme.accent}`,
            background: theme.accentSoft,
            color: theme.text,
          }}
          disabled={disabled}
          onClick={onClick}
        >
          {label}
        </button>
      </div>
      <div className="mt-2 truncate text-xs" style={{ color: theme.textMuted }} title={result ? `${result.status}: ${result.message}` : "尚未执行"}>
        {result ? `${result.status}: ${result.message}` : "尚未执行"}
      </div>
    </div>
  );
}

function LocalDevicePanel({
  icon,
  title,
  status,
  actionLabel,
  onAction,
  actionDisabled,
  children,
  theme,
}: {
  icon: ReactNode;
  title: string;
  status: string;
  actionLabel?: string;
  onAction?: () => void;
  actionDisabled?: boolean;
  children: ReactNode;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <article
      className="flex min-h-[320px] flex-col p-4"
      style={{
        border: `1px solid ${theme.border}`,
        background: theme.frame,
      }}
    >
      <div className="mb-3 flex items-start gap-3">
        <div
          className="flex h-10 w-10 items-center justify-center rounded-md"
          style={{ background: theme.accentSoft, color: theme.accent }}
        >
          {icon}
        </div>
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-semibold">{title}</h3>
          <div className="mt-1 text-xs" style={{ color: theme.textMuted }}>
            {status}
          </div>
        </div>
      </div>
      <div className="grid flex-1 grid-cols-1 gap-2">{children}</div>
      {actionLabel && onAction ? (
        <button
          type="button"
          className="mt-3 rounded-md px-3 py-2 text-sm transition"
          style={{
            border: `1px solid ${theme.accent}`,
            background: theme.accentSoft,
            color: theme.text,
          }}
          disabled={actionDisabled}
          onClick={onAction}
        >
          {actionLabel}
        </button>
      ) : null}
    </article>
  );
}

function isInjectedFeedback(source: LocalControlEvent["source"] | undefined) {
  return source === "Injected" || source === "InjectedLoopback" || source === "VirtualDevice";
}

function eventSourceLabel(event: LocalControlEvent) {
  const source = event.source ?? "Hardware";
  const path = event.capture_path ?? event.device_id ?? "daemon";
  return `${source} / ${path}`;
}

function driverStatusLabel(snapshot: LocalControlsSnapshot | null) {
  if (!snapshot?.driver) {
    return "fallback";
  }
  const version = snapshot.driver.version ? ` ${snapshot.driver.version}` : "";
  const filter = snapshot.driver.filter_active ? " filter" : "";
  const vhid = snapshot.driver.vhid_active ? " vhid" : "";
  return `${snapshot.driver.status}${version}${filter}${vhid}`;
}

function backendHealthLabel(backend: Record<string, unknown> | null | undefined) {
  const name =
    typeof backend?.mode === "string"
      ? backend.mode
      : typeof backend?.kind === "string"
        ? backend.kind
        : "unknown";
  const health = typeof backend?.health === "string" ? backend.health : "unknown";
  return `${name} ${health}`;
}

function StatusPill({
  label,
  tone,
  theme,
}: {
  label: string;
  tone: "success" | "danger" | "info" | "muted";
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const palette = {
    success: ["rgba(73, 179, 92, 0.16)", "#8de29d"],
    danger: ["rgba(197, 48, 48, 0.18)", "#ffb8c1"],
    info: [theme.accentSoft, theme.text],
    muted: ["rgba(255,255,255,0.04)", theme.textSub],
  } as const;
  const [background, color] = palette[tone];
  return (
    <span
      className="shrink-0 rounded px-2 py-0.5 text-xs"
      style={{ background, color }}
    >
      {label}
    </span>
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
    <div className="rshare-scroll grid h-full grid-cols-1 gap-3 overflow-auto xl:grid-cols-[1.1fr_0.9fr]">
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
      hoverBackground: "#c42b1c",
      hoverColor: "#ffffff",
    },
    minimize: {
      hoverBackground: "rgba(255,255,255,0.08)",
      hoverColor: theme.text,
    },
    maximize: {
      hoverBackground: "rgba(255,255,255,0.08)",
      hoverColor: theme.text,
    },
  }[tone];

  return (
    <button
      type="button"
      className="flex items-center justify-center transition"
      onClick={onClick}
      title={title}
      style={{
        width: hitSize,
        height: "100%",
        minHeight: hitSize,
        color: theme.textSub,
        borderRadius: 0,
      }}
      onMouseEnter={(event) => {
        event.currentTarget.style.backgroundColor = control.hoverBackground;
        event.currentTarget.style.color = control.hoverColor;
      }}
      onMouseLeave={(event) => {
        event.currentTarget.style.backgroundColor = "transparent";
        event.currentTarget.style.color = theme.textSub;
      }}
    >
      <span
        className="flex items-center justify-center"
        style={{
          width: size,
          height: size,
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

      <div className="rshare-scroll flex-1 overflow-auto rounded-md p-4 font-mono text-xs"
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
