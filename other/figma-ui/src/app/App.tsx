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
  Volume2,
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
  device_kind: "Keyboard" | "Mouse" | "Gamepad" | "Display" | "Audio" | "Backend";
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
  audio_inputs?: AudioInputDevice[];
  audio_outputs?: AudioOutputDevice[];
  audio_capture_state?: AudioCaptureState;
  audio_stream_state?: AudioStreamState;
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

type LogEntry = {
  timestamp: string;
  level: string;
  target: string;
  message: string;
};

type LocalControlKind = "keyboard" | "mouse" | "gamepad" | "display" | "audio";
type LocalDevicePageKind = "all" | LocalControlKind | "remote";
type AudioInputDevice = {
  id: string;
  name: string;
  endpoint_id?: string | null;
  kind?: "Microphone" | "Loopback";
  source?: string;
  connected?: boolean;
  default?: boolean;
  muted?: boolean | null;
  level_peak?: number;
  level_rms?: number;
  sample_rate?: number | null;
  channel_count?: number | null;
  driver_detail?: string | null;
};
type AudioOutputDevice = {
  id: string;
  name: string;
  endpoint_id?: string | null;
  source?: string;
  connected?: boolean;
  default?: boolean;
  muted?: boolean | null;
  volume_percent?: number | null;
  channel_count?: number | null;
  driver_detail?: string | null;
};
type AudioCaptureState = {
  status?: "Idle" | "CapturingLocal" | "ForwardingRemote" | "Error";
  source?: "Microphone" | "Loopback" | null;
  endpoint_id?: string | null;
  level_peak?: number;
  level_rms?: number;
  sample_rate?: number | null;
  channel_count?: number | null;
  started_at_ms?: number | null;
  last_error?: string | null;
};
type AudioStreamState = {
  active?: boolean;
  target_device_id?: string | null;
  stream_id?: string | null;
  frames_sent?: number;
  frames_received?: number;
  underruns?: number;
  overruns?: number;
  latency_ms?: number | null;
  last_error?: string | null;
};
type LocalDeviceSelectItem = {
  id: string;
  name: string;
  detail: string;
  live: boolean;
  active: boolean;
};

type TauriInvoke = <T = unknown>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

type LocalControlSubscription = {
  stop: () => void;
  usesTauriBridge: boolean;
};

type ThemeMode = "light" | "dark" | "system";

const POLL_INTERVAL_MS = 1500;
const HIDDEN_MONITOR_IDS_STORAGE_KEY = "rshare.hiddenMonitorIds";
const DAEMON_IPC_BRIDGE_ENDPOINT = "/__rshare/ipc";
const DAEMON_LOGS_BRIDGE_ENDPOINT = "/__rshare/logs";
const LOCAL_CONTROLS_WS_URL = "ws://127.0.0.1:27436/local-controls";
const NETWORK_COMMANDS = new Set([
  "dashboard_state",
  "start_service",
  "stop_service",
  "get_logs",
  "clear_logs",
  "connect_device",
  "disconnect_device",
  "get_layout",
  "set_layout",
  "local_controls_state",
  "start_local_controls_stream",
  "stop_local_controls_stream",
  "run_local_input_test",
  "run_remote_latency_test",
  "set_audio_default_output",
  "set_audio_output_volume",
  "set_audio_output_mute",
  "start_audio_capture",
  "stop_audio_capture",
  "start_audio_forwarding",
  "stop_audio_forwarding",
  "run_audio_test",
]);
const WEB_NOOP_COMMANDS = new Set([
  "minimize_window",
  "toggle_maximize_window",
  "close_window",
]);

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

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

async function daemonIpcRequest(request: unknown): Promise<unknown> {
  const response = await fetch(DAEMON_IPC_BRIDGE_ENDPOINT, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });

  let payload: unknown = null;
  try {
    payload = await response.json();
  } catch {
    payload = null;
  }

  if (!response.ok) {
    const message =
      isRecord(payload) && typeof payload.error === "string"
        ? payload.error
        : `daemon 网络 IPC 请求失败：HTTP ${response.status}`;
    throw new Error(message);
  }

  return payload;
}

function daemonResponseValue<T>(response: unknown, variant: string): T {
  if (variant === "Ack" && response === "Ack") {
    return undefined as T;
  }

  if (isRecord(response)) {
    if (Object.prototype.hasOwnProperty.call(response, "Error")) {
      throw new Error(String(response.Error));
    }
    if (Object.prototype.hasOwnProperty.call(response, variant)) {
      return response[variant] as T;
    }
  }

  throw new Error(`daemon 返回了非预期响应：${JSON.stringify(response)}`);
}

async function daemonRequestValue<T>(request: unknown, variant: string): Promise<T> {
  return daemonResponseValue<T>(await daemonIpcRequest(request), variant);
}

function localInputTestKindForDaemon(
  kind: unknown,
): "KeyboardShift" | "MouseMove" | "VirtualGamepadStatus" {
  if (kind === "mouse" || kind === "mouse_move" || kind === "MouseMove") {
    return "MouseMove";
  }
  if (
    kind === "gamepad" ||
    kind === "virtual_gamepad_status" ||
    kind === "VirtualGamepadStatus"
  ) {
    return "VirtualGamepadStatus";
  }
  return "KeyboardShift";
}

async function buildNetworkDashboardState(): Promise<DashboardPayload> {
  const status = await daemonRequestValue<unknown>("Status", "Status");

  let devices: DashboardPayload["devices"] = [];
  try {
    devices = await daemonRequestValue<DashboardPayload["devices"]>("Devices", "Devices");
  } catch {
    devices = [];
  }

  let layout: unknown | null = null;
  let layoutError: string | null = null;
  try {
    layout = await daemonRequestValue<unknown>("GetLayout", "Layout");
  } catch (error) {
    layoutError = error instanceof Error ? error.message : String(error);
  }

  return {
    status,
    devices,
    layout,
    visible_layout: layout,
    layout_error: layoutError,
    auto_started: false,
  };
}

async function invokeNetworkCommand<T = unknown>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  switch (command) {
    case "dashboard_state":
      return (await buildNetworkDashboardState()) as T;
    case "start_service":
      return (await daemonRequestValue<unknown>("Status", "Status")) as T;
    case "stop_service":
      return await daemonRequestValue<T>("Shutdown", "Ack");
    case "get_logs": {
      const limit = Number(args?.limit ?? 1000);
      const response = await fetch(
        `${DAEMON_LOGS_BRIDGE_ENDPOINT}?limit=${encodeURIComponent(String(limit))}`,
      );
      const payload = await response.json().catch(() => null);
      if (!response.ok) {
        const message =
          isRecord(payload) && typeof payload.error === "string"
            ? payload.error
            : `日志网络网关请求失败：HTTP ${response.status}`;
        throw new Error(message);
      }
      return safeArray(payload as LogEntry[] | null | undefined) as T;
    }
    case "clear_logs": {
      const response = await fetch(DAEMON_LOGS_BRIDGE_ENDPOINT, { method: "DELETE" });
      const payload = await response.json().catch(() => null);
      if (!response.ok) {
        const message =
          isRecord(payload) && typeof payload.error === "string"
            ? payload.error
            : `日志清理失败：HTTP ${response.status}`;
        throw new Error(message);
      }
      return undefined as T;
    }
    case "connect_device":
      return await daemonRequestValue<T>(
        { Connect: { device_id: args?.device_id ?? args?.deviceId } },
        "Ack",
      );
    case "disconnect_device":
      return await daemonRequestValue<T>(
        { Disconnect: { device_id: args?.device_id ?? args?.deviceId } },
        "Ack",
      );
    case "get_layout":
      return await daemonRequestValue<T>("GetLayout", "Layout");
    case "set_layout":
      return await daemonRequestValue<T>({ SetLayout: { layout: args?.layout } }, "Ack");
    case "local_controls_state":
      return await daemonRequestValue<T>("LocalControls", "LocalControls");
    case "run_local_input_test":
      return await daemonRequestValue<T>(
        {
          RunLocalInputTest: {
            test: { kind: localInputTestKindForDaemon(args?.kind) },
          },
        },
        "LocalInputTest",
      );
    case "run_remote_latency_test":
      return await daemonRequestValue<T>(
        {
          RunRemoteLatencyTest: {
            device_id: args?.device_id ?? args?.deviceId,
          },
        },
        "LocalInputTest",
      );
    case "set_audio_default_output":
      return await daemonRequestValue<T>(
        { SetAudioDefaultOutput: { endpoint_id: args?.endpoint_id ?? args?.endpointId } },
        "Ack",
      );
    case "set_audio_output_volume":
      return await daemonRequestValue<T>(
        {
          SetAudioOutputVolume: {
            endpoint_id: args?.endpoint_id ?? args?.endpointId,
            volume_percent: args?.volume_percent ?? args?.volumePercent,
          },
        },
        "Ack",
      );
    case "set_audio_output_mute":
      return await daemonRequestValue<T>(
        {
          SetAudioOutputMute: {
            endpoint_id: args?.endpoint_id ?? args?.endpointId,
            muted: args?.muted,
          },
        },
        "Ack",
      );
    case "start_audio_capture":
      return await daemonRequestValue<T>(
        {
          StartAudioCapture: {
            source: args?.source ?? "Loopback",
            endpoint_id: args?.endpoint_id ?? args?.endpointId ?? null,
          },
        },
        "Ack",
      );
    case "stop_audio_capture":
      return await daemonRequestValue<T>("StopAudioCapture", "Ack");
    case "start_audio_forwarding":
      return await daemonRequestValue<T>(
        {
          StartAudioForwarding: {
            source: args?.source ?? "Loopback",
            endpoint_id: args?.endpoint_id ?? args?.endpointId ?? null,
          },
        },
        "Ack",
      );
    case "stop_audio_forwarding":
      return await daemonRequestValue<T>("StopAudioForwarding", "Ack");
    case "run_audio_test":
      return await daemonRequestValue<T>(
        {
          RunAudioTest: {
            test: {
              source: args?.source ?? "Loopback",
              endpoint_id: args?.endpoint_id ?? args?.endpointId ?? null,
            },
          },
        },
        "LocalAudioTest",
      );
    case "start_local_controls_stream":
    case "stop_local_controls_stream":
      return undefined as T;
    default:
      throw new Error(`命令 ${command} 尚未支持网络通信`);
  }
}

async function listenLocalControlEvent(
  handler: (payload: unknown) => void,
): Promise<LocalControlSubscription | null> {
  if (typeof WebSocket !== "undefined") {
    const socket = new WebSocket(LOCAL_CONTROLS_WS_URL);
    socket.addEventListener("message", (event) => {
      try {
        const payload =
          typeof event.data === "string" ? JSON.parse(event.data) : event.data;
        handler(payload);
      } catch (error) {
        handler(error instanceof Error ? error.message : String(error));
      }
    });
    socket.addEventListener("error", () => {
      handler("本机输入实时 WebSocket 不可用");
    });
    socket.addEventListener("close", (event) => {
      if (event.code !== 1000) {
        handler("本机输入实时 WebSocket 已断开");
      }
    });
    return {
      stop: () => socket.close(1000),
      usesTauriBridge: false,
    };
  }

  const unlisten = await listenTauriEvent<unknown>("local-control-event", handler);
  if (!unlisten) {
    return null;
  }

  return {
    stop: unlisten,
    usesTauriBridge: true,
  };
}

async function invokeCommand<T = unknown>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  let networkError: unknown = null;
  if (NETWORK_COMMANDS.has(command)) {
    try {
      return await invokeNetworkCommand<T>(command, args);
    } catch (error) {
      networkError = error;
    }
  }

  const invoke = getInvoke();
  if (invoke) {
    return invoke<T>(command, args);
  }

  if (WEB_NOOP_COMMANDS.has(command)) {
    return undefined as T;
  }

  if (networkError) {
    throw networkError;
  }

  throw new Error(`命令 ${command} 需要 Tauri bridge 或 daemon 网络网关`);
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
      const buttonName = event.payload?.button ?? event.payload?.last_button?.split(/\s+/)[0];
      if (buttonName && event.event_kind === "button") {
        if (eventStateIsPressed(event) || event.payload?.last_button?.toLowerCase().includes("pressed")) {
          pushUniqueString(pressedButtons, buttonName);
        } else if (eventStateIsReleased(event) || event.payload?.last_button?.toLowerCase().includes("released")) {
          removeString(pressedButtons, buttonName);
        }
      }
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
  incoming: LocalControlsSnapshot | null | undefined,
) {
  const normalized = normalizeLocalControlsSnapshot(incoming);
  if (!current) {
    return normalized;
  }
  return {
    ...normalized,
    recent_events: mergeLocalControlEvents(
      current.recent_events ?? [],
      normalized.recent_events ?? [],
    ),
  };
}

function normalizeLocalControlsSnapshot(
  incoming: LocalControlsSnapshot | null | undefined,
): LocalControlsSnapshot {
  const snapshot = (isRecord(incoming) ? incoming : {}) as Partial<LocalControlsSnapshot>;
  const keyboard = isRecord(snapshot.keyboard) ? snapshot.keyboard : {};
  const mouse = isRecord(snapshot.mouse) ? snapshot.mouse : {};
  const display = isRecord(snapshot.display) ? snapshot.display : {};
  const virtualGamepad = isRecord(snapshot.virtual_gamepad) ? snapshot.virtual_gamepad : {};

  return {
    sequence: Number(snapshot.sequence ?? 0),
    keyboard: {
      detected: Boolean(keyboard.detected),
      pressed_keys: safeArray(keyboard.pressed_keys as string[]),
      last_key: typeof keyboard.last_key === "string" ? keyboard.last_key : null,
      event_count: Number(keyboard.event_count ?? 0),
      capture_source:
        typeof keyboard.capture_source === "string" ? keyboard.capture_source : "daemon",
    },
    mouse: {
      detected: Boolean(mouse.detected),
      x: Number(mouse.x ?? 0),
      y: Number(mouse.y ?? 0),
      pressed_buttons: safeArray(mouse.pressed_buttons as string[]),
      wheel_delta_x: Number(mouse.wheel_delta_x ?? 0),
      wheel_delta_y: Number(mouse.wheel_delta_y ?? 0),
      event_count: Number(mouse.event_count ?? 0),
      move_count: Number(mouse.move_count ?? 0),
      button_event_count: Number(mouse.button_event_count ?? 0),
      button_press_count: Number(mouse.button_press_count ?? 0),
      button_release_count: Number(mouse.button_release_count ?? 0),
      wheel_event_count: Number(mouse.wheel_event_count ?? 0),
      wheel_total_x: Number(mouse.wheel_total_x ?? 0),
      wheel_total_y: Number(mouse.wheel_total_y ?? 0),
      current_display_index:
        mouse.current_display_index == null ? null : Number(mouse.current_display_index),
      current_display_id:
        typeof mouse.current_display_id === "string" ? mouse.current_display_id : null,
      display_relative_x: Number(mouse.display_relative_x ?? mouse.x ?? 0),
      display_relative_y: Number(mouse.display_relative_y ?? mouse.y ?? 0),
      capture_source: typeof mouse.capture_source === "string" ? mouse.capture_source : "daemon",
    },
    keyboard_devices: safeArray(snapshot.keyboard_devices),
    mouse_devices: safeArray(snapshot.mouse_devices),
    gamepads: safeArray(snapshot.gamepads),
    audio_inputs: safeArray(snapshot.audio_inputs),
    audio_outputs: safeArray(snapshot.audio_outputs),
    audio_capture_state: snapshot.audio_capture_state,
    audio_stream_state: snapshot.audio_stream_state,
    display: {
      display_count: Number(display.display_count ?? 1),
      virtual_x: Number(display.virtual_x ?? 0),
      virtual_y: Number(display.virtual_y ?? 0),
      primary_width: Number(display.primary_width ?? 1920),
      primary_height: Number(display.primary_height ?? 1080),
      layout_width: Number(display.layout_width ?? display.primary_width ?? 1920),
      layout_height: Number(display.layout_height ?? display.primary_height ?? 1080),
      displays: safeArray(display.displays as LocalControlsSnapshot["display"]["displays"]),
    },
    capture_backend: isRecord(snapshot.capture_backend) ? snapshot.capture_backend : {},
    inject_backend: isRecord(snapshot.inject_backend) ? snapshot.inject_backend : {},
    privilege_state:
      typeof snapshot.privilege_state === "string" ? snapshot.privilege_state : null,
    virtual_gamepad: {
      status:
        typeof virtualGamepad.status === "string" ? virtualGamepad.status : "not_implemented",
      detail:
        typeof virtualGamepad.detail === "string"
          ? virtualGamepad.detail
          : "Virtual HID gamepad injection is not implemented in this build.",
    },
    driver: isRecord(snapshot.driver)
      ? (snapshot.driver as LocalControlsSnapshot["driver"])
      : undefined,
    recent_events: safeArray(snapshot.recent_events),
    last_error: typeof snapshot.last_error === "string" ? snapshot.last_error : null,
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

const BROWSER_GAMEPAD_BUTTONS = [
  "South",
  "East",
  "West",
  "North",
  "LeftBumper",
  "RightBumper",
  "LeftTrigger",
  "RightTrigger",
  "Select",
  "Start",
  "LeftStick",
  "RightStick",
  "DPadUp",
  "DPadDown",
  "DPadLeft",
  "DPadRight",
  "Guide",
];

function mergeBrowserGamepadState(current: LocalControlsSnapshot | null) {
  if (!current || typeof navigator.getGamepads !== "function") {
    return current;
  }

  const pads = Array.from(navigator.getGamepads()).filter(
    (gamepad): gamepad is Gamepad => Boolean(gamepad?.connected),
  );
  if (!pads.length) {
    return current;
  }

  let changed = false;
  const now = Date.now();
  const gamepads = [...(current.gamepads ?? [])];

  for (const pad of pads) {
    const gamepadId = Math.max(0, Math.min(255, pad.index));
    const existingIndex = gamepads.findIndex((item) => item.gamepad_id === gamepadId);
    const existing = existingIndex >= 0 ? gamepads[existingIndex] : null;
    const existingButtons = existing?.buttons ?? [];
    const buttons = BROWSER_GAMEPAD_BUTTONS.map((name, index) => ({
      button: name,
      pressed: Boolean(pad.buttons[index]?.pressed || (pad.buttons[index]?.value ?? 0) > 0.5),
    }));
    const pressedButtons = buttons.filter((button) => button.pressed).map((button) => button.button);
    const previousPressed = existing?.pressed_buttons ?? [];
    const previousPressedKey = previousPressed.map(normalizedGamepadButton).sort().join("|");
    const pressedKey = pressedButtons.map(normalizedGamepadButton).sort().join("|");
    const buttonsChanged =
      previousPressedKey !== pressedKey ||
      buttons.some((button, index) => existingButtons[index]?.pressed !== button.pressed);
    const lastPressed =
      pressedButtons.find(
        (button) => !previousPressed.some((previous) => normalizedGamepadButton(previous) === normalizedGamepadButton(button)),
      ) ?? null;
    const releasedCount = previousPressed.filter(
      (button) => !pressedButtons.some((pressed) => normalizedGamepadButton(pressed) === normalizedGamepadButton(button)),
    ).length;
    const leftStickX = Math.round((pad.axes[0] ?? 0) * 32767);
    const leftStickY = Math.round((pad.axes[1] ?? 0) * 32767);
    const rightStickX = Math.round((pad.axes[2] ?? 0) * 32767);
    const rightStickY = Math.round((pad.axes[3] ?? 0) * 32767);
    const leftTriggerValue = Math.round((pad.buttons[6]?.value ?? 0) * 65535);
    const rightTriggerValue = Math.round((pad.buttons[7]?.value ?? 0) * 65535);
    const axisChanged =
      !existing ||
      Math.abs((existing.left_stick_x ?? 0) - leftStickX) > 512 ||
      Math.abs((existing.left_stick_y ?? 0) - leftStickY) > 512 ||
      Math.abs((existing.right_stick_x ?? 0) - rightStickX) > 512 ||
      Math.abs((existing.right_stick_y ?? 0) - rightStickY) > 512;
    const triggerChanged =
      !existing ||
      Math.abs((existing.left_trigger ?? 0) - leftTriggerValue) > 512 ||
      Math.abs((existing.right_trigger ?? 0) - rightTriggerValue) > 512;
    const identityChanged = !existing || existing.name !== (pad.id || existing.name || `Gamepad ${gamepadId}`) || !existing.connected;
    const stateChanged = identityChanged || buttonsChanged || axisChanged || triggerChanged;

    if (!stateChanged && existing) {
      continue;
    }

    const next = {
      gamepad_id: gamepadId,
      name: pad.id || existing?.name || `Gamepad ${gamepadId}`,
      connected: true,
      buttons,
      pressed_buttons: pressedButtons,
      last_button: lastPressed ? `${lastPressed} pressed` : existing?.last_button ?? null,
      left_stick_x: leftStickX,
      left_stick_y: leftStickY,
      right_stick_x: rightStickX,
      right_stick_y: rightStickY,
      left_trigger: leftTriggerValue,
      right_trigger: rightTriggerValue,
      event_count: (existing?.event_count ?? 0) + 1,
      button_event_count: (existing?.button_event_count ?? 0) + (buttonsChanged ? 1 : 0),
      button_press_count: (existing?.button_press_count ?? 0) + (lastPressed ? 1 : 0),
      button_release_count: (existing?.button_release_count ?? 0) + releasedCount,
      axis_event_count: (existing?.axis_event_count ?? 0) + (axisChanged ? 1 : 0),
      trigger_event_count: (existing?.trigger_event_count ?? 0) + (triggerChanged ? 1 : 0),
      last_axis: triggerChanged ? "trigger" : axisChanged ? "stick" : existing?.last_axis ?? null,
      last_seen_ms: now,
    };

    changed = true;
    if (existingIndex >= 0) {
      gamepads[existingIndex] = next;
    } else {
      gamepads.push(next);
    }
  }

  return changed ? { ...current, gamepads } : current;
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

  async function refreshAll() {
    await Promise.allSettled([refreshDashboard(), refreshLocalControls()]);
    setRefreshTick((value) => value + 1);
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
    let subscription: LocalControlSubscription | null = null;

    async function startStream() {
      try {
        const nextSubscription = await listenLocalControlEvent((payload) => {
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

        if (cancelled) {
          nextSubscription?.stop();
          return;
        }

        subscription = nextSubscription;
        if (subscription?.usesTauriBridge) {
          await invokeCommand("start_local_controls_stream");
        }
      } catch (streamError) {
        setLocalControlsError(String(streamError));
      }
    }

    startStream();
    return () => {
      cancelled = true;
      const usesTauriBridge = subscription?.usesTauriBridge ?? false;
      subscription?.stop();
      if (usesTauriBridge) {
        invokeCommand("stop_local_controls_stream").catch(() => {});
      }
    };
  }, []);

  useEffect(() => {
    if (typeof navigator.getGamepads !== "function") {
      return;
    }

    const timer = window.setInterval(() => {
      setLocalControls((current) => mergeBrowserGamepadState(current));
    }, 50);

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

  async function runRemoteLatencyTest(deviceId: string) {
    setBusy(true);
    setConfirmingInputTest(null);
    try {
      const result = await invokeCommand<LocalInputTestResult>("run_remote_latency_test", {
        device_id: deviceId,
      });
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
            onClick={refreshAll}
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
                ? "rgba(197, 48, 48, 0.08)"
                : theme.accentSoft,
              color: model.service.online
                ? "#8a1f1f"
                : theme.text,
              border: `1px solid ${
                model.service.online
                  ? "rgba(197, 48, 48, 0.55)"
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
            title="鍏抽棴"
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
              localDevice={model.settings.localDevice}
              localControls={localControls}
              localControlsError={localControlsError}
              localInputTestResult={localInputTestResult}
              confirmingInputTest={confirmingInputTest}
              onRunLocalInputTest={runLocalInputTest}
              onRunRemoteLatencyTest={runRemoteLatencyTest}
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
  localDevice,
  localControls,
  localControlsError,
  localInputTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
  onRunRemoteLatencyTest,
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
  localDevice: {
    id: string;
    name: string;
    hostname: string;
  };
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onRunRemoteLatencyTest: (deviceId: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <DevicesPageWithLocalControls
      devices={devices}
      localDevice={localDevice}
      localControls={localControls}
      localControlsError={localControlsError}
      localInputTestResult={localInputTestResult}
      confirmingInputTest={confirmingInputTest}
      onRunLocalInputTest={onRunLocalInputTest}
      onRunRemoteLatencyTest={onRunRemoteLatencyTest}
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
        detail="启动守护进程并保持同一局域网后，发现的设备会同时出现在设备页和布局页。"
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
      className="flex h-9 shrink-0 items-center gap-2 rounded-md px-2.5 text-left text-xs transition"
      style={{
        border: `1px solid ${active ? theme.accent : theme.border}`,
        background: active ? theme.accentSoft : theme.frame,
        color: active ? theme.text : theme.textSub,
      }}
      onClick={() => onClick(kind)}
    >
      <span
        className="flex h-6 w-6 shrink-0 items-center justify-center"
        style={{ color: active ? theme.accent : theme.textMuted }}
      >
        {icon}
      </span>
      <span className="flex min-w-0 flex-col leading-tight">
        <span className="truncate font-medium">{title}</span>
        <span className="truncate text-[11px]" style={{ color: theme.textMuted }}>
          {detail}
        </span>
      </span>
      <span
        className="h-1.5 w-1.5 shrink-0 rounded-full"
        style={{ background: live ? theme.success : theme.textMuted, opacity: live ? 1 : 0.45 }}
      />
    </button>
  );
}

function DevicesPageWithLocalControls({
  devices,
  localDevice,
  localControls,
  localControlsError,
  localInputTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
  onRunRemoteLatencyTest,
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
  localDevice: {
    id: string;
    name: string;
    hostname: string;
  };
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onRunRemoteLatencyTest: (deviceId: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const [selectedPage, setSelectedPage] = useState<LocalDevicePageKind>("all");
  const [selectedDeviceIds, setSelectedDeviceIds] = useState<Record<string, string>>({});
  const [selectedMonitorDeviceId, setSelectedMonitorDeviceId] = useState("local");
  const [browserAudioOutputs, setBrowserAudioOutputs] = useState<AudioOutputDevice[]>([]);

  useEffect(() => {
    let cancelled = false;
    if (!navigator.mediaDevices?.enumerateDevices) {
      return;
    }
    navigator.mediaDevices
      .enumerateDevices()
      .then((devices) => {
        if (cancelled) {
          return;
        }
        setBrowserAudioOutputs(
          devices
            .filter((device) => device.kind === "audiooutput")
            .map((device, index) => ({
              id: device.deviceId || `audio-output-${index}`,
              name: device.label || (index === 0 ? "默认音频输出" : `音频输出 ${index + 1}`),
              endpoint_id: device.deviceId || undefined,
              source: "browser audiooutput",
              connected: true,
              default: index === 0,
            })),
        );
      })
      .catch(() => {
        if (!cancelled) {
          setBrowserAudioOutputs([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);
  const audioOutputs =
    localControls?.audio_outputs?.length ? localControls.audio_outputs : browserAudioOutputs;
  const audioInputs = localControls?.audio_inputs ?? [];
  const safeDevices = safeArray(devices);
  const connectedDevices = safeDevices.filter((device) => device.connected);
  const selectedRemoteDevice =
    selectedMonitorDeviceId === "local"
      ? null
      : connectedDevices.find((device) => device.id === selectedMonitorDeviceId) ?? null;
  const connectedDeviceIds = new Set(connectedDevices.map((device) => device.id));
  const monitorSnapshot = selectedRemoteDevice
    ? buildRemoteMonitorSnapshot(localControls, selectedRemoteDevice.id)
    : buildLocalMonitorSnapshot(localControls, connectedDeviceIds);
  const monitorError = selectedRemoteDevice ? null : localControlsError;

  useEffect(() => {
    if (
      selectedMonitorDeviceId !== "local" &&
      !connectedDevices.some((device) => device.id === selectedMonitorDeviceId)
    ) {
      setSelectedMonitorDeviceId("local");
    }
  }, [connectedDevices, selectedMonitorDeviceId]);

  const selectedDeviceId =
    selectedPage === "all" || selectedPage === "remote"
      ? undefined
      : selectedDeviceIds[selectedPage];
  const setSelectedDeviceId = (kind: LocalControlKind, deviceId: string) => {
    setSelectedDeviceIds((current) => ({
      ...current,
      [kind]: deviceId,
    }));
  };
  const counts = {
    all: Boolean(monitorSnapshot),
    keyboard: localInputDeviceCount(monitorSnapshot, "keyboard"),
    mouse: localInputDeviceCount(monitorSnapshot, "mouse"),
    gamepad: Math.max(1, monitorSnapshot?.gamepads?.length ?? 0),
    display: Math.max(1, monitorSnapshot?.display.display_count ?? 0),
    audio: Math.max(1, audioInputs.length + audioOutputs.length),
    remote: safeDevices.length,
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
          className="rshare-scroll flex min-h-10 items-center gap-2 overflow-x-auto px-2 py-1"
          role="tablist"
          aria-label="设备类型"
        >
        <LocalControlTypeButton
          kind="all"
          active={selectedPage === "all"}
          icon={<LayoutGrid size={18} />}
          title="全部设备"
          detail="总览"
          live={counts.all}
          onClick={setSelectedPage}
          theme={theme}
        />
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
          kind="audio"
          active={selectedPage === "audio"}
          icon={<Volume2 size={18} />}
          title="音频设备"
          detail={`${counts.audio} 个`}
          live={audioInputs.length > 0 || audioOutputs.length > 0}
          onClick={setSelectedPage}
          theme={theme}
        />
        <LocalControlTypeButton
          kind="remote"
          active={selectedPage === "remote"}
          icon={<Monitor size={18} />}
          title="远端设备"
          detail={`${counts.remote} 台`}
           live={safeDevices.some((device) => device.connected)}
          onClick={setSelectedPage}
          theme={theme}
        />
          <div className="ml-auto flex shrink-0 items-center gap-2 pl-2">
            <span className="text-xs" style={{ color: theme.textMuted }}>
              监听
            </span>
            <select
              className="h-8 max-w-[220px] rounded-md px-2 text-xs outline-none"
              style={{
                border: `1px solid ${theme.border}`,
                background: theme.frame,
                color: theme.text,
              }}
              value={selectedMonitorDeviceId}
              onChange={(event) => setSelectedMonitorDeviceId(event.currentTarget.value)}
              title="选择要监听和测试延迟的设备"
            >
              <option value="local">
                本机 · {localDevice.name}
              </option>
              {connectedDevices.map((device) => (
                <option key={device.id} value={device.id}>
                  {device.name} · {device.hostname}
                </option>
              ))}
            </select>
          </div>
        </div>
        {selectedPage !== "all" && !selectedRemoteDevice ? (
          <DeviceDriverStrip
            kind={selectedPage}
            snapshot={monitorSnapshot}
            audioOutputs={audioOutputs}
            selectedDeviceId={selectedDeviceId}
            onSelectedDeviceIdChange={(deviceId) =>
              setSelectedDeviceId(selectedPage as LocalControlKind, deviceId)
            }
            theme={theme}
          />
        ) : null}
      </div>

      <div className="min-h-0 flex-1 overflow-hidden">
        {selectedPage === "all" ? (
          <AllDevicesOverview
            snapshot={monitorSnapshot}
            audioOutputs={audioOutputs}
            selectedDeviceIds={selectedDeviceIds}
            onSelectedDeviceIdChange={setSelectedDeviceId}
            error={monitorError}
            theme={theme}
          />
        ) : selectedPage === "remote" ? (
          <RemoteDevicesPanel
            devices={safeDevices}
            onConnect={onConnect}
            onDisconnect={onDisconnect}
            busy={busy}
            theme={theme}
          />
        ) : (
          <LocalControlDriverHub
            snapshot={monitorSnapshot}
            error={monitorError}
            inputTestResult={localInputTestResult}
            confirmingInputTest={confirmingInputTest}
            selectedKind={selectedPage}
            remoteDevice={selectedRemoteDevice}
            selectedDeviceId={selectedDeviceId}
            audioOutputs={audioOutputs}
            onSelectedKindChange={setSelectedPage}
            onSelectedDeviceIdChange={(deviceId) =>
              setSelectedDeviceId(selectedPage as LocalControlKind, deviceId)
            }
            onRunInputTest={
              selectedRemoteDevice
                ? () => onRunRemoteLatencyTest(selectedRemoteDevice.id)
                : onRunLocalInputTest
            }
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

function AllDevicesOverview({
  snapshot,
  audioOutputs,
  selectedDeviceIds,
  onSelectedDeviceIdChange,
  error,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  audioOutputs: AudioOutputDevice[];
  selectedDeviceIds: Record<string, string>;
  onSelectedDeviceIdChange: (kind: LocalControlKind, deviceId: string) => void;
  error: string | null;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const keyboardSelectedId = selectedLocalDeviceId(snapshot, "keyboard", selectedDeviceIds.keyboard);
  const mouseSelectedId = selectedLocalDeviceId(snapshot, "mouse", selectedDeviceIds.mouse);
  const gamepadSelectedId = selectedLocalDeviceId(snapshot, "gamepad", selectedDeviceIds.gamepad);
  const keyboardEvents = selectedControlEvents(snapshot, "keyboard", keyboardSelectedId);
  const mouseEvents = selectedControlEvents(snapshot, "mouse", mouseSelectedId);
  const gamepadEvents = selectedControlEvents(snapshot, "gamepad", gamepadSelectedId);
  const keyboardState = keyboardMonitorState(snapshot, keyboardSelectedId, keyboardEvents);
  const mouseState = mouseMonitorState(snapshot, mouseSelectedId, mouseEvents);
  const gamepad = selectedGamepad(snapshot, gamepadSelectedId);
  const keyboardEvent = latestEvent(keyboardEvents);
  const mouseEvent = latestEvent(mouseEvents);
  const gamepadEvent = latestEvent(gamepadEvents);
  const displayEvent = latestLocalControlEvent(snapshot, "Display") ?? mouseEvent;
  const audioEvent = latestLocalControlEvent(snapshot, "Audio");
  const keyboardSelector = (
    <DeviceSelector
      compact
      items={localDeviceItems(snapshot, "keyboard")}
      selectedId={keyboardSelectedId}
      onChange={(deviceId) => onSelectedDeviceIdChange("keyboard", deviceId)}
      theme={theme}
    />
  );
  const mouseSelector = (
    <DeviceSelector
      compact
      items={localDeviceItems(snapshot, "mouse")}
      selectedId={mouseSelectedId}
      onChange={(deviceId) => onSelectedDeviceIdChange("mouse", deviceId)}
      theme={theme}
    />
  );
  const gamepadSelector = (
    <DeviceSelector
      compact
      items={localDeviceItems(snapshot, "gamepad")}
      selectedId={gamepadSelectedId}
      onChange={(deviceId) => onSelectedDeviceIdChange("gamepad", deviceId)}
      theme={theme}
    />
  );
  const displaySelector = (
    <DeviceSelector
      compact
      items={localDeviceItems(snapshot, "display")}
      selectedId={selectedDeviceIds.display}
      onChange={(deviceId) => onSelectedDeviceIdChange("display", deviceId)}
      theme={theme}
    />
  );
  const audioSelector = (
    <DeviceSelector
      compact
      items={localDeviceItems(snapshot, "audio", audioOutputs)}
      selectedId={selectedDeviceIds.audio}
      onChange={(deviceId) => onSelectedDeviceIdChange("audio", deviceId)}
      theme={theme}
    />
  );

  return (
    <section
      className="relative grid h-full min-h-0 grid-cols-1 gap-2 overflow-hidden p-2 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(220px,0.42fr)] xl:grid-rows-[minmax(0,1fr)_minmax(0,0.9fr)]"
      style={{
        border: `1px solid ${theme.border}`,
        background: theme.frame,
      }}
    >
      {error ? (
        <div
          className="absolute left-3 top-3 z-10 rounded px-3 py-2 text-xs"
          style={{
            border: "1px solid rgba(197, 48, 48, 0.45)",
            background: "rgba(94, 24, 34, 0.72)",
            color: "#ffb8c1",
          }}
        >
          本机捕获不可用：{error}
        </div>
      ) : null}

      <OverviewAnimationCard
        icon={<Keyboard size={16} />}
        title="键盘"
        live={Boolean(snapshot?.keyboard.detected)}
        event={keyboardEvent}
        selector={keyboardSelector}
        theme={theme}
        className="xl:col-span-2"
      >
        <SimulatedKeyboard
          compact
          pressedKeys={keyboardState.pressedKeys}
          lastKey={keyboardState.lastKey}
          recentEvents={keyboardEvents}
          eventCount={keyboardState.eventCount}
          theme={theme}
        />
      </OverviewAnimationCard>

      <OverviewAnimationCard
        icon={<MousePointer2 size={16} />}
        title="鼠标"
        live={Boolean(snapshot?.mouse.detected)}
        event={mouseEvent}
        selector={mouseSelector}
        theme={theme}
      >
        <SimulatedMouse
          compact
          x={mouseState.x}
          y={mouseState.y}
          pressedButtons={mouseState.pressedButtons}
          recentEvents={mouseEvents}
          wheelDeltaX={mouseState.wheelDeltaX}
          wheelDeltaY={mouseState.wheelDeltaY}
          wheelTotalX={mouseState.wheelTotalX}
          wheelTotalY={mouseState.wheelTotalY}
          eventCount={mouseState.eventCount}
          moveCount={mouseState.moveCount}
          buttonPressCount={mouseState.buttonPressCount}
          buttonReleaseCount={mouseState.buttonReleaseCount}
          wheelEventCount={mouseState.wheelEventCount}
          displayRelativeX={mouseState.displayRelativeX}
          displayRelativeY={mouseState.displayRelativeY}
          currentDisplayIndex={mouseState.currentDisplayIndex}
          currentDisplayId={mouseState.currentDisplayId}
          displays={snapshot?.display.displays ?? []}
          theme={theme}
        />
      </OverviewAnimationCard>

      <OverviewAnimationCard
        icon={<Gamepad2 size={16} />}
        title="手柄"
        live={Boolean(gamepad?.connected)}
        event={gamepadEvent}
        selector={gamepadSelector}
        theme={theme}
      >
        <SimulatedGamepad
          compact
          gamepad={gamepad}
          virtualDetail={snapshot?.virtual_gamepad.detail ?? "Virtual HID not implemented"}
          theme={theme}
        />
      </OverviewAnimationCard>

      <OverviewAnimationCard
        icon={<HardDrive size={16} />}
        title="显示设备"
        live={(snapshot?.display.display_count ?? 0) > 0}
        event={displayEvent}
        selector={displaySelector}
        theme={theme}
      >
        <DisplayActivityPreview snapshot={snapshot} theme={theme} />
      </OverviewAnimationCard>
      <OverviewAnimationCard
        icon={<Volume2 size={16} />}
        title="音频设备"
        live={Boolean((snapshot?.audio_inputs?.length ?? 0) + audioOutputs.length)}
        event={audioEvent}
        selector={audioSelector}
        theme={theme}
      >
        <AudioActivityPreview snapshot={snapshot} outputs={audioOutputs} theme={theme} />
      </OverviewAnimationCard>
    </section>
  );
}

function latestLocalControlEvent(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlEvent["device_kind"],
) {
  return [...(snapshot?.recent_events ?? [])]
    .reverse()
    .find((event) => event.device_kind === kind) ?? null;
}

function safeArray<T>(value: T[] | null | undefined): T[] {
  return Array.isArray(value) ? value : [];
}

function isLogEntry(value: unknown): value is LogEntry {
  if (!isRecord(value)) {
    return false;
  }
  return (
    typeof value.timestamp === "string" &&
    typeof value.level === "string" &&
    typeof value.target === "string" &&
    typeof value.message === "string"
  );
}

function eventDeviceKindForControlKind(kind: LocalControlKind): LocalControlEvent["device_kind"] {
  switch (kind) {
    case "keyboard":
      return "Keyboard";
    case "mouse":
      return "Mouse";
    case "gamepad":
      return "Gamepad";
    case "display":
      return "Display";
    case "audio":
      return "Audio";
  }
}

function isUnscopedDeviceSelection(deviceId: string | null | undefined) {
  return !deviceId || deviceId.endsWith("-default");
}

function selectedLocalDeviceId(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlKind,
  selectedDeviceId?: string,
  audioOutputs: AudioOutputDevice[] = [],
) {
  const devices = localDeviceItems(snapshot, kind, audioOutputs);
  if (selectedDeviceId && devices.some((device) => device.id === selectedDeviceId)) {
    return selectedDeviceId;
  }
  return devices.find((device) => device.active)?.id ?? devices[0]?.id;
}

function selectedControlEvents(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlKind,
  selectedDeviceId?: string,
) {
  const deviceKind = eventDeviceKindForControlKind(kind);
  return safeArray(snapshot?.recent_events).filter(
    (event) =>
      event.device_kind === deviceKind &&
      eventMatchesSelectedDevice(event, kind, selectedDeviceId),
  );
}

function eventMatchesSelectedDevice(
  event: LocalControlEvent,
  kind: LocalControlKind,
  selectedDeviceId?: string,
) {
  if (isUnscopedDeviceSelection(selectedDeviceId)) {
    return true;
  }

  if (kind === "gamepad") {
    const gamepadId = event.payload?.gamepad_id;
    return (
      (gamepadId !== undefined && `gamepad-${gamepadId}` === selectedDeviceId) ||
      event.device_id === selectedDeviceId
    );
  }

  if (kind === "display") {
    return event.payload?.display_id === selectedDeviceId;
  }

  if (kind === "audio") {
    return (
      event.device_id === selectedDeviceId ||
      event.payload?.endpoint_id === selectedDeviceId ||
      event.payload?.target_endpoint_id === selectedDeviceId
    );
  }

  const identifiers = [
    event.device_id,
    event.device_instance_id,
    event.capture_path,
    event.payload?.device_id,
    event.payload?.device_instance_id,
    event.payload?.origin_event_device_id,
    event.payload?.capture_path,
  ];
  return identifiers.some((identifier) => identifier === selectedDeviceId);
}

function latestEvent(events: LocalControlEvent[]) {
  return events.length ? events[events.length - 1] : null;
}

function keyboardMonitorState(
  snapshot: LocalControlsSnapshot | null,
  selectedDeviceId: string | undefined,
  events: LocalControlEvent[],
) {
  if (isUnscopedDeviceSelection(selectedDeviceId)) {
    return {
      pressedKeys: snapshot?.keyboard.pressed_keys ?? [],
      lastKey: snapshot?.keyboard.last_key ?? null,
      eventCount: snapshot?.keyboard.event_count ?? 0,
    };
  }

  const pressedKeys: string[] = [];
  let lastKey: string | null = null;
  for (const event of events) {
    const key = keyboardEventKey(event);
    if (!key) {
      continue;
    }
    lastKey = key;
    if (eventStateIsPressed(event)) {
      pushUniqueString(pressedKeys, key);
    } else if (eventStateIsReleased(event)) {
      removeString(pressedKeys, key);
    }
  }
  return {
    pressedKeys,
    lastKey,
    eventCount: events.length,
  };
}

function mouseMonitorState(
  snapshot: LocalControlsSnapshot | null,
  selectedDeviceId: string | undefined,
  events: LocalControlEvent[],
) {
  if (isUnscopedDeviceSelection(selectedDeviceId)) {
    return {
      x: snapshot?.mouse.x ?? 0,
      y: snapshot?.mouse.y ?? 0,
      pressedButtons: snapshot?.mouse.pressed_buttons ?? [],
      wheelDeltaX: snapshot?.mouse.wheel_delta_x ?? 0,
      wheelDeltaY: snapshot?.mouse.wheel_delta_y ?? 0,
      wheelTotalX: snapshot?.mouse.wheel_total_x ?? 0,
      wheelTotalY: snapshot?.mouse.wheel_total_y ?? 0,
      eventCount: snapshot?.mouse.event_count ?? 0,
      moveCount: snapshot?.mouse.move_count ?? 0,
      buttonPressCount: snapshot?.mouse.button_press_count ?? 0,
      buttonReleaseCount: snapshot?.mouse.button_release_count ?? 0,
      wheelEventCount: snapshot?.mouse.wheel_event_count ?? 0,
      displayRelativeX: snapshot?.mouse.display_relative_x ?? snapshot?.mouse.x ?? 0,
      displayRelativeY: snapshot?.mouse.display_relative_y ?? snapshot?.mouse.y ?? 0,
      currentDisplayIndex: snapshot?.mouse.current_display_index ?? null,
      currentDisplayId: snapshot?.mouse.current_display_id ?? null,
    };
  }

  const pressedButtons: string[] = [];
  let x = 0;
  let y = 0;
  let wheelDeltaX = 0;
  let wheelDeltaY = 0;
  let wheelTotalX = 0;
  let wheelTotalY = 0;
  let moveCount = 0;
  let buttonPressCount = 0;
  let buttonReleaseCount = 0;
  let wheelEventCount = 0;
  let displayRelativeX = 0;
  let displayRelativeY = 0;
  let currentDisplayIndex: number | null = null;
  let currentDisplayId: string | null = null;

  for (const event of events) {
    x = numberPayload(event, "x", x);
    y = numberPayload(event, "y", y);
    displayRelativeX = numberPayload(event, "display_relative_x", displayRelativeX);
    displayRelativeY = numberPayload(event, "display_relative_y", displayRelativeY);
    currentDisplayIndex = optionalNumberPayload(event, "display_index", currentDisplayIndex);
    currentDisplayId = event.payload?.display_id ?? currentDisplayId;

    if (event.event_kind === "move") {
      moveCount += 1;
    } else if (event.event_kind === "button") {
      const button = event.payload?.button;
      if (button) {
        if (eventStateIsPressed(event)) {
          buttonPressCount += 1;
          pushUniqueString(pressedButtons, button);
        } else if (eventStateIsReleased(event)) {
          buttonReleaseCount += 1;
          removeString(pressedButtons, button);
        }
      }
    } else if (event.event_kind === "wheel") {
      wheelEventCount += 1;
      wheelDeltaX = numberPayload(event, "delta_x", 0);
      wheelDeltaY = numberPayload(event, "delta_y", 0);
      wheelTotalX = numberPayload(event, "total_x", wheelTotalX + wheelDeltaX);
      wheelTotalY = numberPayload(event, "total_y", wheelTotalY + wheelDeltaY);
    }
  }

  return {
    x,
    y,
    pressedButtons,
    wheelDeltaX,
    wheelDeltaY,
    wheelTotalX,
    wheelTotalY,
    eventCount: events.length,
    moveCount,
    buttonPressCount,
    buttonReleaseCount,
    wheelEventCount,
    displayRelativeX,
    displayRelativeY,
    currentDisplayIndex,
    currentDisplayId,
  };
}

function selectedGamepad(
  snapshot: LocalControlsSnapshot | null,
  selectedDeviceId: string | undefined,
) {
  const gamepads = safeArray(snapshot?.gamepads);
  if (!isUnscopedDeviceSelection(selectedDeviceId)) {
    const wanted = selectedDeviceId?.replace(/^gamepad-/, "");
    const selected = gamepads.find((gamepad) => String(gamepad.gamepad_id) === wanted);
    if (selected) {
      return selected;
    }
  }
  return gamepads.find((item) => item.connected) ?? gamepads[0] ?? null;
}

function buildLocalMonitorSnapshot(
  snapshot: LocalControlsSnapshot | null,
  remoteDeviceIds: ReadonlySet<string>,
) {
  if (!snapshot) {
    return null;
  }

  return {
    ...snapshot,
    recent_events: safeArray(snapshot.recent_events).filter((event) => {
      const sourceDeviceId = event.device_id ?? event.payload?.remote_device_id;
      return !sourceDeviceId || !remoteDeviceIds.has(sourceDeviceId);
    }),
  };
}

function buildEmptyControlSnapshot(base: LocalControlsSnapshot | null): LocalControlsSnapshot {
  return {
    sequence: base?.sequence ?? 0,
    keyboard: {
      detected: false,
      pressed_keys: [],
      last_key: null,
      event_count: 0,
      capture_source: "remote diagnostic",
    },
    mouse: {
      detected: false,
      x: 0,
      y: 0,
      pressed_buttons: [],
      wheel_delta_x: 0,
      wheel_delta_y: 0,
      event_count: 0,
      move_count: 0,
      button_event_count: 0,
      button_press_count: 0,
      button_release_count: 0,
      wheel_event_count: 0,
      wheel_total_x: 0,
      wheel_total_y: 0,
      current_display_index: null,
      current_display_id: null,
      display_relative_x: 0,
      display_relative_y: 0,
      capture_source: "remote diagnostic",
    },
    keyboard_devices: [],
    mouse_devices: [],
    gamepads: [],
    audio_inputs: [],
    audio_outputs: [],
    audio_capture_state: base?.audio_capture_state,
    audio_stream_state: base?.audio_stream_state,
    display: base?.display ?? {
      display_count: 1,
      primary_width: 1920,
      primary_height: 1080,
      layout_width: 1920,
      layout_height: 1080,
      displays: [
        {
          display_id: "remote-primary",
          x: 0,
          y: 0,
          width: 1920,
          height: 1080,
          primary: true,
        },
      ],
    },
    capture_backend: base?.capture_backend ?? {},
    inject_backend: base?.inject_backend ?? {},
    privilege_state: base?.privilege_state ?? null,
    virtual_gamepad: base?.virtual_gamepad ?? {
      status: "remote",
      detail: "Remote diagnostic stream",
    },
    driver: base?.driver,
    recent_events: [],
    last_error: null,
  };
}

function buildRemoteMonitorSnapshot(
  snapshot: LocalControlsSnapshot | null,
  remoteDeviceId: string,
) {
  const remoteEvents = safeArray(snapshot?.recent_events)
    .filter((event) => {
      const sourceDeviceId = event.device_id ?? event.payload?.remote_device_id;
      return sourceDeviceId === remoteDeviceId;
    })
    .sort((left, right) => left.sequence - right.sequence);
  let remoteSnapshot = buildEmptyControlSnapshot(snapshot);
  for (const event of remoteEvents) {
    remoteSnapshot = applyLocalControlEvent(remoteSnapshot, event);
  }
  return remoteSnapshot;
}

function localDeviceItems(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlKind,
  audioOutputs: AudioOutputDevice[] = [],
): LocalDeviceSelectItem[] {
  if (kind === "keyboard") {
    const devices = safeArray(snapshot?.keyboard_devices);
    if (devices.length) {
      return devices.map((device, index) => ({
        id: device.id || `keyboard-${index}`,
        name: device.name || `键盘 ${index + 1}`,
        detail: device.driver_detail ?? device.capture_path ?? device.source ?? "keyboard",
        live: device.connected !== false,
        active: index === 0,
      }));
    }
    return [{
      id: "keyboard-default",
      name: "默认键盘",
      detail: snapshot?.keyboard.capture_source ?? "等待输入事件",
      live: Boolean(snapshot?.keyboard.detected),
      active: true,
    }];
  }

  if (kind === "mouse") {
    const devices = safeArray(snapshot?.mouse_devices);
    if (devices.length) {
      return devices.map((device, index) => ({
        id: device.id || `mouse-${index}`,
        name: device.name || `鼠标 ${index + 1}`,
        detail: device.driver_detail ?? device.capture_path ?? device.source ?? "mouse",
        live: device.connected !== false,
        active: index === 0,
      }));
    }
    return [{
      id: "mouse-default",
      name: "默认鼠标",
      detail: snapshot?.mouse.capture_source ?? "等待输入事件",
      live: Boolean(snapshot?.mouse.detected),
      active: true,
    }];
  }

  if (kind === "gamepad") {
    const gamepads = safeArray(snapshot?.gamepads);
    if (gamepads.length) {
      return gamepads.map((gamepad, index) => ({
        id: `gamepad-${gamepad.gamepad_id}`,
        name: gamepad.name || `手柄 ${gamepad.gamepad_id + 1}`,
        detail: `事件 ${gamepad.event_count ?? 0}`,
        live: Boolean(gamepad.connected),
        active: index === 0,
      }));
    }
    return [{
      id: "gamepad-default",
      name: "默认手柄",
      detail: "未连接",
      live: false,
      active: true,
    }];
  }

  if (kind === "display") {
    const displays = safeArray(snapshot?.display?.displays);
    if (displays.length) {
      return displays.map((display, index) => ({
        id: display.display_id || `display-${index}`,
        name: display.primary ? "主显示器" : `显示器 ${index + 1}`,
        detail: `${display.width} x ${display.height}`,
        live: true,
        active: Boolean(display.primary) || index === 0,
      }));
    }
    return [{
      id: "display-primary",
      name: "主显示器",
      detail: `${snapshot?.display?.primary_width ?? 1920} x ${snapshot?.display?.primary_height ?? 1080}`,
      live: Boolean(snapshot?.display?.display_count),
      active: true,
    }];
  }

  const audioInputs = safeArray(snapshot?.audio_inputs).map((device, index) => ({
    id: device.id || `audio-input-${index}`,
    name: device.name || `音频输入 ${index + 1}`,
    detail: `${device.kind ?? "Microphone"} / ${device.source ?? "audio"}`,
    live: device.connected !== false,
    active: Boolean(device.default),
  }));
  const outputs = safeArray(audioOutputs).map((device, index) => ({
    id: device.id || `audio-output-${index}`,
    name: device.name || `音频输出 ${index + 1}`,
    detail: `${device.source ?? "audio"}${device.default ? " / default" : ""}`,
    live: device.connected !== false,
    active: Boolean(device.default),
  }));
  const items = [...audioInputs, ...outputs];
  return items.length
    ? items
    : [{
        id: "audio-default",
        name: "默认音频",
        detail: "等待枚举",
        live: false,
        active: true,
      }];
}

function DeviceSelector({
  items,
  selectedId,
  onChange,
  compact = false,
  theme,
}: {
  items: LocalDeviceSelectItem[] | null | undefined;
  selectedId?: string;
  onChange?: (deviceId: string) => void;
  compact?: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const options = safeArray(items);
  if (!options.length) {
    return (
      <span className="text-xs" style={{ color: theme.textMuted }}>
        未检测到设备
      </span>
    );
  }
  const currentId =
    selectedId && options.some((item) => item.id === selectedId)
      ? selectedId
      : options.find((item) => item.active)?.id ?? options[0].id;

  return (
    <select
      className={`${compact ? "h-7 max-w-[160px]" : "h-8 max-w-[320px]"} rounded-md px-2 text-xs outline-none`}
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.035)",
        color: theme.text,
      }}
      value={currentId}
      onChange={(event) => onChange?.(event.currentTarget.value)}
      title={options.find((item) => item.id === currentId)?.detail}
    >
      {options.map((item) => (
        <option key={item.id} value={item.id}>
          {item.live ? "● " : "○ "}
          {item.name}
        </option>
      ))}
    </select>
  );
}

function OverviewAnimationCard({
  icon,
  title,
  live,
  event,
  selector,
  children,
  theme,
  className = "",
}: {
  icon: ReactNode;
  title: string;
  live: boolean;
  event: LocalControlEvent | null;
  selector?: ReactNode;
  children: ReactNode;
  theme: typeof FIGMA_DESKTOP_THEME;
  className?: string;
}) {
  return (
    <article
      className={`relative flex min-h-0 flex-col overflow-hidden ${className}`}
      style={{
        border: `1px solid ${event ? theme.accent : theme.border}`,
        background: "rgba(255,255,255,0.02)",
      }}
    >
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        <span style={{ color: event ? theme.accent : theme.textMuted }}>{icon}</span>
        <span className="text-sm font-medium">{title}</span>
        {selector ? <div className="min-w-0">{selector}</div> : null}
        <span
          className="ml-auto h-2.5 w-2.5 rounded-full"
          style={{ background: live ? theme.success : theme.textMuted }}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-hidden px-3 pb-3">{children}</div>
      {event ? (
        <span
          key={event.sequence}
          className="pointer-events-none absolute right-4 top-4 h-3 w-3 rounded-full animate-ping"
          style={{ background: theme.accent }}
        />
      ) : null}
    </article>
  );
}

function DisplayActivityPreview({
  snapshot,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const displays =
    snapshot?.display.displays?.length
      ? snapshot.display.displays
      : [
          {
            display_id: "primary",
            x: snapshot?.display.virtual_x ?? 0,
            y: snapshot?.display.virtual_y ?? 0,
            width: snapshot?.display.primary_width ?? 1920,
            height: snapshot?.display.primary_height ?? 1080,
            primary: true,
          },
        ];
  const minX = Math.min(...displays.map((display) => display.x));
  const minY = Math.min(...displays.map((display) => display.y));
  const maxX = Math.max(...displays.map((display) => display.x + display.width));
  const maxY = Math.max(...displays.map((display) => display.y + display.height));
  const totalWidth = Math.max(1, maxX - minX);
  const totalHeight = Math.max(1, maxY - minY);
  const activeIndex = snapshot?.mouse.current_display_index ?? 0;
  const activeDisplay = displays[activeIndex] ?? displays[0];
  const cursorX =
    ((activeDisplay.x - minX + (snapshot?.mouse.display_relative_x ?? snapshot?.mouse.x ?? 0)) /
      totalWidth) *
    100;
  const cursorY =
    ((activeDisplay.y - minY + (snapshot?.mouse.display_relative_y ?? snapshot?.mouse.y ?? 0)) /
      totalHeight) *
    100;

  const cursorAbsoluteX = activeDisplay.x + (snapshot?.mouse.display_relative_x ?? snapshot?.mouse.x ?? 0);
  const cursorAbsoluteY = activeDisplay.y + (snapshot?.mouse.display_relative_y ?? snapshot?.mouse.y ?? 0);
  const strokeWidth = Math.max(totalWidth, totalHeight) / 260;
  const labelSize = Math.max(totalHeight / 22, 42);

  return (
    <div
      className="relative flex h-full min-h-[180px] items-center justify-center overflow-hidden p-3"
      style={{
        border: `1px solid ${theme.border}`,
        backgroundImage:
          "linear-gradient(rgba(255,255,255,0.045) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.045) 1px, transparent 1px)",
        backgroundSize: "24px 24px",
      }}
    >
      <svg
        className="h-full w-full"
        viewBox={`${minX} ${minY} ${totalWidth} ${totalHeight}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label="display layout preview"
      >
        {displays.map((display, index) => (
          <g key={display.display_id}>
            <rect
              x={display.x}
              y={display.y}
              width={display.width}
              height={display.height}
              fill={index === activeIndex ? theme.accentSoft : "rgba(255,255,255,0.045)"}
              stroke={index === activeIndex ? theme.accent : theme.border}
              strokeWidth={strokeWidth * (index === activeIndex ? 2 : 1)}
            />
            <text
              x={display.x + display.width * 0.04}
              y={display.y + display.height * 0.12}
              fill={theme.textMuted}
              fontSize={labelSize}
            >
              {display.primary ? "Primary" : `Display ${index + 1}`}
            </text>
            <text
              x={display.x + display.width * 0.04}
              y={display.y + display.height * 0.88}
              fill={theme.textMuted}
              fontSize={labelSize * 0.9}
            >
              {display.width} x {display.height}
            </text>
          </g>
        ))}
        <circle
          cx={Math.min(maxX, Math.max(minX, cursorAbsoluteX))}
          cy={Math.min(maxY, Math.max(minY, cursorAbsoluteY))}
          r={Math.max(totalWidth, totalHeight) / 70}
          fill={theme.accent}
          opacity="0.24"
        />
        <circle
          cx={Math.min(maxX, Math.max(minX, cursorAbsoluteX))}
          cy={Math.min(maxY, Math.max(minY, cursorAbsoluteY))}
          r={Math.max(totalWidth, totalHeight) / 140}
          fill={theme.accent}
        />
      </svg>
    </div>
  );
}

function AudioActivityPreview({
  snapshot,
  outputs,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  outputs: AudioOutputDevice[];
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const inputs = snapshot?.audio_inputs ?? [];
  const capture = snapshot?.audio_capture_state;
  const stream = snapshot?.audio_stream_state;
  const loopbackLevel = Math.max(
    0,
    ...inputs.filter((input) => input.kind === "Loopback").map((input) => input.level_peak ?? 0),
    capture?.source === "Loopback" ? (capture.level_peak ?? 0) : 0,
  );
  const micLevel = Math.max(
    0,
    ...inputs.filter((input) => input.kind !== "Loopback").map((input) => input.level_peak ?? 0),
    capture?.source === "Microphone" ? (capture.level_peak ?? 0) : 0,
  );
  const defaultOutput = outputs.find((output) => output.default) ?? outputs[0];
  const status = stream?.active
    ? "远端转发"
    : capture?.status === "CapturingLocal"
      ? "本机捕获"
      : "空闲";

  return (
    <div
      className="flex h-full min-h-[180px] flex-col justify-center gap-4 overflow-hidden p-4"
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <AudioLevelBar label="系统回环" value={loopbackLevel} theme={theme} />
      <AudioLevelBar label="麦克风" value={micLevel} theme={theme} />
      <div className="grid grid-cols-2 gap-3 text-xs">
        <InfoRow label="输出" value={defaultOutput?.name ?? "无"} theme={theme} />
        <InfoRow label="状态" value={status} theme={theme} />
        <InfoRow label="输入" value={String(inputs.length)} theme={theme} />
        <InfoRow label="端点" value={String(outputs.length)} theme={theme} />
      </div>
    </div>
  );
}

function AudioLevelBar({
  label,
  value,
  theme,
}: {
  label: string;
  value: number;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const normalized = Math.max(0, Math.min(100, value));
  return (
    <div>
      <div className="mb-1 flex items-center justify-between text-xs" style={{ color: theme.textMuted }}>
        <span>{label}</span>
        <span>{Math.round(normalized)}%</span>
      </div>
      <div className="h-3 overflow-hidden rounded-sm" style={{ background: "rgba(255,255,255,0.055)" }}>
        <div
          className="h-full transition-[width]"
          style={{
            width: `${normalized}%`,
            background: theme.accent,
          }}
        />
      </div>
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
}

function LocalControlDriverHub({
  snapshot,
  error,
  inputTestResult,
  confirmingInputTest,
  selectedKind,
  remoteDevice,
  selectedDeviceId,
  audioOutputs = [],
  onSelectedKindChange,
  onSelectedDeviceIdChange,
  onRunInputTest,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  error: string | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  selectedKind: LocalControlKind;
  remoteDevice?: {
    id: string;
    name: string;
    hostname: string;
  } | null;
  selectedDeviceId?: string;
  audioOutputs?: AudioOutputDevice[];
  onSelectedKindChange: (kind: LocalControlKind) => void;
  onSelectedDeviceIdChange?: (deviceId: string) => void;
  onRunInputTest: (kind: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const selectedDevices = localDeviceItems(snapshot, selectedKind, audioOutputs);
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
        <div className="mb-2 flex h-9 shrink-0 items-center gap-2">
          {remoteDevice ? (
            <div className="min-w-0 text-xs" style={{ color: theme.textMuted }}>
              正在监听 {remoteDevice.name} · {remoteDevice.hostname}
            </div>
          ) : (
            <DeviceSelector
              items={selectedDevices}
              selectedId={selectedDeviceId}
              onChange={onSelectedDeviceIdChange}
              theme={theme}
            />
          )}
        </div>
        <LocalControlDetail
          kind={selectedKind}
          snapshot={snapshot}
          selectedDeviceId={selectedDeviceId}
          remoteDevice={remoteDevice}
          inputTestResult={inputTestResult}
          confirmingInputTest={confirmingInputTest}
          onRunInputTest={onRunInputTest}
          theme={theme}
        />
      </div>
    </section>
  );
}

function DeviceDriverStrip({
  kind,
  snapshot,
  audioOutputs = [],
  selectedDeviceId,
  onSelectedDeviceIdChange,
  theme,
}: {
  kind: LocalDevicePageKind;
  snapshot: LocalControlsSnapshot | null;
  audioOutputs?: AudioOutputDevice[];
  selectedDeviceId?: string;
  onSelectedDeviceIdChange?: (deviceId: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  if (kind === "all" || kind === "remote") {
    return null;
  }
  const devices = localDeviceItems(snapshot, kind, audioOutputs).filter((device) => device.live);
  if (!devices.length) {
    return null;
  }
  return (
    <div className="rshare-scroll flex shrink-0 gap-2 overflow-x-auto px-3 py-2" style={{ borderBottom: `1px solid ${theme.border}`, background: theme.frame }}>
      {devices.map((device) => (
        <button
          key={device.id}
          type="button"
          className="max-w-[260px] shrink-0 truncate rounded-md px-3 py-1.5 text-sm"
          style={{
            border: `1px solid ${selectedDeviceId === device.id || (!selectedDeviceId && device.active) ? theme.accent : theme.border}`,
            background: selectedDeviceId === device.id || (!selectedDeviceId && device.active) ? theme.accentSoft : "rgba(255,255,255,0.04)",
            color: theme.text,
          }}
          onClick={() => onSelectedDeviceIdChange?.(device.id)}
          title={device.detail}
        >
          <span className="mr-2 inline-block h-2 w-2 rounded-full" style={{ background: device.live ? theme.success : theme.textMuted }} />
          {device.name}
        </button>
      ))}
    </div>
  );
}

function LocalControlDetail({
  kind,
  snapshot,
  selectedDeviceId,
  remoteDevice,
  inputTestResult,
  confirmingInputTest,
  onRunInputTest,
  theme,
}: {
  kind: LocalControlKind;
  snapshot: LocalControlsSnapshot | null;
  remoteDevice?: {
    id: string;
    name: string;
    hostname: string;
  } | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  selectedDeviceId?: string;
  onRunInputTest: (kind: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const effectiveSelectedDeviceId = selectedLocalDeviceId(snapshot, kind, selectedDeviceId);
  const recentEvents = selectedControlEvents(snapshot, kind, effectiveSelectedDeviceId);
  if (kind === "keyboard") {
    const keyboardState = keyboardMonitorState(snapshot, effectiveSelectedDeviceId, recentEvents);
    const keyboardEvents = recentEvents.slice(-12).reverse();
    const actionLabel = remoteDevice
      ? "发送延迟探测"
      : confirmingInputTest === "keyboard"
        ? "再次点击执行 Shift 测试"
        : "真实注入测试";
    return (
      <div className="grid h-full min-h-0 grid-rows-[minmax(0,1fr)_150px] gap-3">
        <SimulatedKeyboard pressedKeys={keyboardState.pressedKeys} lastKey={keyboardState.lastKey} recentEvents={recentEvents} eventCount={keyboardState.eventCount} theme={theme} />
        <div className="grid min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_420px]"><InputTestAction label={actionLabel} result={inputTestResult} disabled={remoteDevice ? false : !snapshot} onClick={() => onRunInputTest("keyboard")} theme={theme} /><KeyboardEventLog events={keyboardEvents} theme={theme} /></div>
      </div>
    );
  }
  if (kind === "mouse") {
    const mouseState = mouseMonitorState(snapshot, effectiveSelectedDeviceId, recentEvents);
    const mouseEvents = recentEvents.slice(-12).reverse();
    const actionLabel = remoteDevice
      ? "发送延迟探测"
      : confirmingInputTest === "mouse"
        ? "再次点击执行移动测试"
        : "真实注入测试";
    return (
      <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]">
        <SimulatedMouse x={mouseState.x} y={mouseState.y} pressedButtons={mouseState.pressedButtons} recentEvents={recentEvents} wheelDeltaX={mouseState.wheelDeltaX} wheelDeltaY={mouseState.wheelDeltaY} wheelTotalX={mouseState.wheelTotalX} wheelTotalY={mouseState.wheelTotalY} eventCount={mouseState.eventCount} moveCount={mouseState.moveCount} buttonPressCount={mouseState.buttonPressCount} buttonReleaseCount={mouseState.buttonReleaseCount} wheelEventCount={mouseState.wheelEventCount} displayRelativeX={mouseState.displayRelativeX} displayRelativeY={mouseState.displayRelativeY} currentDisplayIndex={mouseState.currentDisplayIndex} currentDisplayId={mouseState.currentDisplayId} displays={snapshot?.display.displays ?? []} theme={theme} />
        <div className="flex min-h-0 flex-col gap-3"><MouseEventLog events={mouseEvents} theme={theme} /><InputTestAction label={actionLabel} result={inputTestResult} disabled={remoteDevice ? false : !snapshot} onClick={() => onRunInputTest("mouse")} theme={theme} /></div>
      </div>
    );
  }
  if (kind === "gamepad") {
    const gamepad = selectedGamepad(snapshot, effectiveSelectedDeviceId);
    const gamepadEvents = recentEvents.slice(-12).reverse();
    return <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]"><SimulatedGamepad gamepad={gamepad} virtualDetail={snapshot?.virtual_gamepad.detail ?? "Virtual HID not implemented"} theme={theme} /><GamepadEventLog events={gamepadEvents} theme={theme} /></div>;
  }
  if (kind === "audio") {
    return <AudioDetail snapshot={snapshot} audioOutputs={snapshot?.audio_outputs ?? []} theme={theme} />;
  }
  return <DisplayActivityPreview snapshot={snapshot} theme={theme} />;
}

function AudioDetail({ snapshot, audioOutputs, theme }: { snapshot: LocalControlsSnapshot | null; audioOutputs: AudioOutputDevice[]; theme: typeof FIGMA_DESKTOP_THEME }) {
  const inputs = snapshot?.audio_inputs ?? [];
  const audioEvents = (snapshot?.recent_events ?? []).filter((event) => event.device_kind === "Audio").slice(-8).reverse();
  const stream = snapshot?.audio_stream_state;
  const capture = snapshot?.audio_capture_state;
  const selectedInput = inputs.find((device) => device.default) ?? inputs[0];
  const selectedOutput = audioOutputs.find((device) => device.default) ?? audioOutputs[0];
  const startForwarding = () => void invokeCommand("start_audio_forwarding", { source: selectedInput?.kind === "Microphone" ? "Microphone" : "Loopback", endpoint_id: selectedInput?.endpoint_id ?? null });
  return (
    <div className="grid h-full min-h-0 grid-cols-1 gap-3 overflow-hidden xl:grid-cols-[minmax(0,1fr)_360px]">
      <div className="grid min-h-0 grid-rows-[minmax(0,0.9fr)_minmax(0,1fr)] gap-3">
        <section className="min-h-0 overflow-hidden p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}>
          <div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold">音频输入 / 回环</h3><span className="text-xs" style={{ color: theme.textMuted }}>{capture?.status ?? "Idle"}</span></div>
          <div className="grid h-[calc(100%-34px)] grid-cols-1 gap-3 overflow-hidden lg:grid-cols-2">{inputs.length ? inputs.map((device) => <AudioDeviceCard key={device.id} title={device.name} subtitle={`${device.kind === "Loopback" ? "系统输出回环" : "麦克风"} / ${device.source ?? "Core Audio"}`} live={device.connected !== false} defaultDevice={Boolean(device.default)} level={device.level_peak ?? 0} meta={[`${device.sample_rate ?? 48000} Hz`, `${device.channel_count ?? 2} ch`, device.muted ? "muted" : "unmuted"]} actions={<><button type="button" className="rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => void invokeCommand("start_audio_capture", { source: device.kind === "Loopback" ? "Loopback" : "Microphone", endpoint_id: device.endpoint_id ?? null })}>捕获</button><button type="button" className="rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => void invokeCommand("start_audio_forwarding", { source: device.kind === "Loopback" ? "Loopback" : "Microphone", endpoint_id: device.endpoint_id ?? null })}>转发</button></>} theme={theme} />) : <EmptyPanel title="未发现音频输入" detail="等待 Windows Core Audio 枚举或浏览器权限。" theme={theme} />}</div>
        </section>
        <section className="min-h-0 overflow-hidden p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}>
          <div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold">音频输出</h3><span className="text-xs" style={{ color: theme.textMuted }}>{audioOutputs.length} endpoint</span></div>
          <div className="grid h-[calc(100%-34px)] grid-cols-1 gap-3 overflow-hidden lg:grid-cols-2 xl:grid-cols-3">{audioOutputs.length ? audioOutputs.map((device) => <AudioDeviceCard key={device.id} title={device.name} subtitle={`${device.source ?? "Windows Core Audio"}${device.default ? " / default" : ""}`} live={device.connected !== false} defaultDevice={Boolean(device.default)} level={typeof device.volume_percent === "number" ? device.volume_percent : 0} meta={[typeof device.volume_percent === "number" ? `${device.volume_percent}%` : "unknown volume", device.muted ? "muted" : "unmuted", `${device.channel_count ?? 2} ch`]} actions={<><button type="button" className="rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => device.endpoint_id ? void invokeCommand("set_audio_output_mute", { endpoint_id: device.endpoint_id, muted: !device.muted }) : undefined}>{device.muted ? "取消静音" : "静音"}</button><input className="min-w-0 flex-1" type="range" min={0} max={100} defaultValue={device.volume_percent ?? 0} disabled={!device.endpoint_id} onChange={(event) => device.endpoint_id ? void invokeCommand("set_audio_output_volume", { endpoint_id: device.endpoint_id, volume_percent: Number(event.currentTarget.value) }) : undefined} /></>} theme={theme} />) : <EmptyPanel title="未发现音频输出" detail="等待 Windows Core Audio 输出端点枚举。" theme={theme} />}</div>
        </section>
      </div>
      <aside className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)] gap-3 overflow-hidden">
        <section className="p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 text-sm font-semibold">远端音频</div><div className="grid grid-cols-2 gap-2 text-xs"><InfoRow label="目标" value={stream?.target_device_id?.slice(0, 8) ?? "无"} theme={theme} /><InfoRow label="状态" value={stream?.active ? "转发中" : (capture?.status ?? "Idle")} theme={theme} /><InfoRow label="延迟" value={stream?.latency_ms ? `${stream.latency_ms} ms` : "-"} theme={theme} /><InfoRow label="帧" value={String(stream?.frames_sent ?? 0)} theme={theme} /></div><div className="mt-3 flex gap-2"><button type="button" className="flex-1 rounded-md px-3 py-2 text-xs" style={secondaryButtonStyle(theme)} onClick={startForwarding}>开始转发</button><button type="button" className="flex-1 rounded-md px-3 py-2 text-xs" style={dangerButtonStyle(theme)} onClick={() => void invokeCommand("stop_audio_forwarding")}>停止</button></div></section>
        <section className="p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 text-sm font-semibold">当前端点</div><InfoRow label="输入" value={selectedInput?.name ?? "无"} theme={theme} /><InfoRow label="输出" value={selectedOutput?.name ?? "无"} theme={theme} /></section>
        <section className="min-h-0 overflow-hidden p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold">音频记录</h3><span className="text-xs" style={{ color: theme.textMuted }}>最近 {audioEvents.length} 条</span></div><div className="rshare-scroll h-full overflow-auto text-xs">{audioEvents.length ? audioEvents.map((event) => <div key={event.sequence} className="mb-2 grid grid-cols-[78px_minmax(0,1fr)] gap-2"><span style={{ color: theme.textMuted }}>{formatEventTime(event.timestamp_ms)}</span><span className="truncate">{event.summary}</span></div>) : <div style={{ color: theme.textMuted }}>等待音频事件</div>}</div></section>
      </aside>
    </div>
  );
}

function AudioDeviceCard({ title, subtitle, live, defaultDevice, level, meta, actions, theme }: { title: string; subtitle: string; live: boolean; defaultDevice: boolean; level: number; meta: string[]; actions: ReactNode; theme: typeof FIGMA_DESKTOP_THEME }) {
  return <div className="flex min-h-0 flex-col justify-between gap-3 rounded-md p-3" style={{ border: `1px solid ${defaultDevice ? theme.accent : theme.border}`, background: defaultDevice ? theme.accentSoft : "rgba(255,255,255,0.035)" }}><div className="min-w-0"><div className="flex items-center gap-2"><span className="h-2.5 w-2.5 rounded-full" style={{ background: live ? theme.success : theme.textMuted }} /><div className="truncate text-sm font-semibold">{title}</div></div><div className="mt-1 truncate text-xs" style={{ color: theme.textMuted }}>{subtitle}</div></div><AudioLevelBar label="level" value={level} theme={theme} /><div className="flex flex-wrap gap-2 text-[11px]" style={{ color: theme.textMuted }}>{meta.map((item) => <span key={item}>{item}</span>)}</div><div className="flex items-center gap-2">{actions}</div></div>;
}
function secondaryButtonStyle(theme: typeof FIGMA_DESKTOP_THEME) { return { border: `1px solid ${theme.accent}`, background: theme.accentSoft, color: theme.text }; }
function dangerButtonStyle(_theme: typeof FIGMA_DESKTOP_THEME) { return { border: "1px solid rgba(197, 48, 48, 0.55)", background: "rgba(197, 48, 48, 0.12)", color: "#8a1f2d" }; }
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
  compact = false,
}: {
  pressedKeys: string[];
  lastKey: string | null;
  recentEvents: LocalControlEvent[];
  eventCount: number;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
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
      {compact ? null : (
      <div className="mb-3 flex shrink-0 flex-wrap items-center justify-between gap-2">
        <div className="text-sm font-medium">键盘按键测试</div>
        <div className="flex flex-wrap items-center gap-2 text-xs" style={{ color: theme.textMuted }}>
          <KeyboardLegend tone="idle" label="未按过" theme={theme} />
          <KeyboardLegend tone="tested" label="按过后" theme={theme} />
          <KeyboardLegend tone="pressed" label="激活状态" theme={theme} />
        </div>
      </div>
      )}
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
      {compact ? null : (
      <div className="mt-3 grid shrink-0 grid-cols-3 gap-2 text-xs xl:grid-cols-6">
        <KeyboardSignal label="最后按键" value={lastKey ? keyDisplayName(lastKey) : "无"} theme={theme} />
        <KeyboardSignal label="按下状态" value={activeCount ? `${activeCount} 个按下` : "无"} theme={theme} />
        <KeyboardSignal label="已测按键" value={`${testedCount}/104`} theme={theme} />
        <KeyboardSignal label="按下次数" value={String(pressedCount)} theme={theme} />
        <KeyboardSignal label="抬起次数" value={String(releasedCount)} theme={theme} />
        <KeyboardSignal label="总事件数" value={String(eventCount)} theme={theme} />
      </div>
      )}
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
          最近 {events.length} 条        </div>
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
          最近 {events.length} 条        </div>
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

function normalizeInputToken(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function mouseButtonAliases(name: string) {
  const aliases: Record<string, string[]> = {
    Left: ["left", "button0", "button1", "primary", "mouseleft"],
    Right: ["right", "button2", "button3", "secondary", "mouseright"],
    Middle: ["middle", "middlebutton", "button1", "button3", "wheel", "wheelbutton", "auxiliary", "mousemiddle"],
    Back: ["back", "x1", "xbutton1", "button4", "button8", "browserback", "side1", "other1", "other4", "other8", "unknown1", "unknown4", "unknown8"],
    Forward: ["forward", "x2", "xbutton2", "button5", "button9", "browserforward", "side2", "other2", "other5", "other9", "unknown2", "unknown5", "unknown9"],
  };
  return aliases[name] ?? [name];
}

function mouseButtonPressed(buttons: string[], name: string) {
  const wanted = new Set(mouseButtonAliases(name).map(normalizeInputToken));
  return buttons.some((button) =>
    wanted.has(normalizeInputToken(button)),
  );
}

function mouseButtonEventTokens(event: LocalControlEvent) {
  return [
    event.payload?.button,
    event.payload?.button_name,
    event.payload?.name,
    event.payload?.pressed_buttons,
    event.summary,
  ]
    .filter((value): value is string => Boolean(value))
    .flatMap((value) => value.split(/[,\s/]+/).filter(Boolean));
}

function mouseButtonRecentlyActive(events: LocalControlEvent[], name: string) {
  const wanted = new Set(mouseButtonAliases(name).map(normalizeInputToken));
  return events
    .filter((event) => event.device_kind === "Mouse" && event.event_kind === "button")
    .slice(-8)
    .some((event) =>
      mouseButtonEventTokens(event).some((token) => wanted.has(normalizeInputToken(token))),
    );
}

function SimulatedMouse({
  x,
  y,
  pressedButtons,
  recentEvents,
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
  compact = false,
}: {
  x: number;
  y: number;
  pressedButtons: string[];
  recentEvents: LocalControlEvent[];
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
  compact?: boolean;
}) {
  const leftDown = mouseButtonPressed(pressedButtons, "Left") || mouseButtonRecentlyActive(recentEvents, "Left");
  const rightDown = mouseButtonPressed(pressedButtons, "Right") || mouseButtonRecentlyActive(recentEvents, "Right");
  const middleDown = mouseButtonPressed(pressedButtons, "Middle") || mouseButtonRecentlyActive(recentEvents, "Middle");
  const backDown = mouseButtonPressed(pressedButtons, "Back") || mouseButtonRecentlyActive(recentEvents, "Back");
  const forwardDown = mouseButtonPressed(pressedButtons, "Forward") || mouseButtonRecentlyActive(recentEvents, "Forward");
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
  const wheelLabel =
    wheelDeltaY > 0
      ? "↑"
      : wheelDeltaY < 0
        ? "↓"
        : wheelDeltaX > 0
          ? "→"
          : wheelDeltaX < 0
            ? "←"
            : "W";

  if (compact) {
    return (
      <div
        className="flex h-full min-h-0 items-center justify-center overflow-hidden p-2"
        style={{
          border: `1px solid ${theme.border}`,
          background: "rgba(255,255,255,0.025)",
        }}
      >
        <svg
          className="h-full max-h-[260px] w-full max-w-[180px]"
          viewBox="0 0 240 230"
          role="img"
          aria-label="mouse input preview"
          preserveAspectRatio="xMidYMid meet"
        >
          <path
            d="M120 18 C154 18 180 52 180 104 V160 C180 193 154 214 120 214 C86 214 60 193 60 160 V104 C60 52 86 18 120 18 Z"
            fill="rgba(255,255,255,0.05)"
            stroke={theme.border}
            strokeWidth="1.5"
          />
          <path d="M120 18 L120 100" stroke={theme.border} strokeWidth="1.5" />
          <path
            d="M63 101 C64 49 89 20 120 18 L120 101 Z"
            fill={leftDown ? theme.accentSoft : "rgba(255,255,255,0.04)"}
            stroke={leftDown ? theme.accent : theme.border}
          />
          <path
            d="M120 18 C151 20 176 49 177 101 L120 101 Z"
            fill={rightDown ? theme.accentSoft : "rgba(255,255,255,0.04)"}
            stroke={rightDown ? theme.accent : theme.border}
          />
          <rect
            x="112"
            y="54"
            width="15"
            height="38"
            rx="7.5"
            fill={middleDown || wheelActive ? theme.accentSoft : "rgba(255,255,255,0.08)"}
            stroke={middleDown || wheelActive ? theme.accent : theme.border}
          />
          {wheelActive ? (
            <text x="119.5" y="48" textAnchor="middle" fill={theme.accent} fontSize="15" fontWeight="700">
              {wheelLabel}
            </text>
          ) : null}
          <rect x="50" y="110" width="13" height="44" rx="6.5" fill={backDown ? theme.accentSoft : "rgba(255,255,255,0.04)"} stroke={backDown ? theme.accent : theme.border} strokeWidth={backDown ? 2 : 1.2} />
          <rect x="47" y="160" width="13" height="44" rx="6.5" fill={forwardDown ? theme.accentSoft : "rgba(255,255,255,0.04)"} stroke={forwardDown ? theme.accent : theme.border} strokeWidth={forwardDown ? 2 : 1.2} />
          <text x="91" y="72" fill={leftDown ? theme.accent : theme.textMuted} fontSize="12">L</text>
          <text x="145" y="72" fill={rightDown ? theme.accent : theme.textMuted} fontSize="12">R</text>
          <text x="119.5" y="78" textAnchor="middle" fill={middleDown || wheelActive ? theme.accent : theme.textMuted} fontSize="10" fontWeight={middleDown || wheelActive ? 700 : 400}>{wheelLabel}</text>
          <text x="20" y="137" fill={backDown ? theme.accent : theme.textMuted} fontSize="11">Back</text>
          <text x="26" y="188" fill={forwardDown ? theme.accent : theme.textMuted} fontSize="11">Fwd</text>
        </svg>
      </div>
    );
  }

  return (
    <div
      className={
        compact
          ? "flex h-full min-h-0 items-center justify-center p-3"
          : "grid h-full min-h-0 grid-cols-1 gap-4 p-4 xl:grid-cols-[minmax(220px,320px)_minmax(0,1fr)]"
      }
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="flex items-center justify-center">
        <svg className="h-full max-h-[360px] w-full max-w-[240px]" viewBox="0 0 220 280" role="img" aria-label="mouse input preview">
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
          {wheelActive ? (
            <text x="110" y="58" textAnchor="middle" fill={theme.accent} fontSize="18" fontWeight="700">
              {wheelLabel}
            </text>
          ) : null}
          <rect x="42" y="132" width="14" height="46" rx="7" fill={backDown ? theme.accentSoft : "rgba(255,255,255,0.04)"} stroke={backDown ? theme.accent : theme.border} strokeWidth={backDown ? 2 : 1.5} />
          <rect x="38" y="182" width="14" height="46" rx="7" fill={forwardDown ? theme.accentSoft : "rgba(255,255,255,0.04)"} stroke={forwardDown ? theme.accent : theme.border} strokeWidth={forwardDown ? 2 : 1.5} />
          <text x="82" y="82" fill={theme.textMuted} fontSize="11">L</text>
          <text x="134" y="82" fill={theme.textMuted} fontSize="11">R</text>
          <text x="110" y="92" textAnchor="middle" fill={middleDown || wheelActive ? theme.accent : theme.textMuted} fontSize="11" fontWeight={middleDown || wheelActive ? 700 : 400}>{wheelLabel}</text>
          <text x="8" y="158" fill={backDown ? theme.accent : theme.textMuted} fontSize="10">Back</text>
          <text x="8" y="210" fill={forwardDown ? theme.accent : theme.textMuted} fontSize="10">Fwd</text>
        </svg>
      </div>
      {compact ? null : (
      <div className="flex min-w-0 flex-col gap-3">
        {compact ? null : (
        <>
        <div className="text-sm font-medium">鼠标实时绘制</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          鍏ㄥ眬 {Math.round(x)}, {Math.round(y)} / {displayName} 鍐?{Math.round(displayRelativeX)}, {Math.round(displayRelativeY)} 路 {display.width} x {display.height} @ {display.x}, {display.y}
        </div>
        </>
        )}
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
            婊氳疆 螖 {wheelDeltaX}, {wheelDeltaY}
          </div>
        </div>
        {compact ? null : (
        <div className="grid shrink-0 grid-cols-2 gap-2 text-xs 2xl:grid-cols-4">
          <KeyboardSignal label="Left" value={leftDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Middle" value={middleDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Right" value={rightDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Back" value={backDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="Forward" value={forwardDown ? "pressed" : "idle"} theme={theme} />
          <KeyboardSignal label="移动" value={String(moveCount)} theme={theme} />
          <KeyboardSignal label="按下/抬起" value={`${buttonPressCount}/${buttonReleaseCount}`} theme={theme} />
          <KeyboardSignal label="婊氳疆" value={`${wheelEventCount} / ${wheelTotalX}, ${wheelTotalY}`} theme={theme} />
          <KeyboardSignal label="事件" value={String(eventCount)} theme={theme} />
        </div>
        )}
      </div>
      )}
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
  return normalizeInputToken(name);
}

function gamepadButtonAliases(name: string) {
  const aliases: Record<string, string[]> = {
    South: ["South", "ButtonSouth", "A", "ButtonA", "FaceDown", "Cross"],
    East: ["East", "ButtonEast", "B", "ButtonB", "FaceRight", "Circle"],
    West: ["West", "ButtonWest", "X", "ButtonX", "FaceLeft", "Square"],
    North: ["North", "ButtonNorth", "Y", "ButtonY", "FaceUp", "Triangle"],
    A: ["A", "South", "ButtonSouth", "ButtonA"],
    B: ["B", "East", "ButtonEast", "ButtonB"],
    X: ["X", "West", "ButtonWest", "ButtonX"],
    Y: ["Y", "North", "ButtonNorth", "ButtonY"],
    LeftBumper: ["LeftBumper", "LeftShoulder", "LB", "L1"],
    RightBumper: ["RightBumper", "RightShoulder", "RB", "R1"],
    LeftTrigger: ["LeftTrigger", "LeftTrigger2", "LT", "L2"],
    RightTrigger: ["RightTrigger", "RightTrigger2", "RT", "R2"],
    LeftStick: ["LeftStick", "LeftThumb", "LeftThumbstick", "L3"],
    RightStick: ["RightStick", "RightThumb", "RightThumbstick", "R3"],
    Select: ["Select", "Back", "Share"],
    Start: ["Start", "Menu", "Options"],
    Guide: ["Guide", "Mode", "Home", "Xbox"],
    DPadUp: ["DPadUp", "DpadUp", "DPad Up", "Up"],
    DPadDown: ["DPadDown", "DpadDown", "DPad Down", "Down"],
    DPadLeft: ["DPadLeft", "DpadLeft", "DPad Left", "Left"],
    DPadRight: ["DPadRight", "DpadRight", "DPad Right", "Right"],
  };
  return aliases[name] ?? [name];
}

function gamepadButtonTokens(gamepad: LocalGamepadSnapshot | null) {
  if (!gamepad) {
    return [];
  }
  return [
    ...gamepadPressedButtons(gamepad),
  ].filter(Boolean);
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
  const wanted = names.flatMap(gamepadButtonAliases).map(normalizedGamepadButton);
  return gamepadButtonTokens(gamepad).some((button) => {
    const actual = normalizedGamepadButton(button);
    return wanted.some((candidate) => actual === candidate || actual.includes(candidate));
  });
}

function stickOffset(value: number) {
  const normalized = Math.max(-1, Math.min(1, Number(value ?? 0) / 32767));
  return normalized * 16;
}

function stickPercent(value: number) {
  const normalized = Math.max(-1, Math.min(1, Number(value ?? 0) / 32767));
  return Math.round(normalized * 100);
}

function triggerFill(value: number) {
  return clampPercent((Number(value ?? 0) / 65535) * 100);
}

function SimulatedGamepad({
  gamepad,
  virtualDetail,
  theme,
  compact = false,
}: {
  gamepad: LocalGamepadSnapshot | null;
  virtualDetail: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
}) {
  const connected = Boolean(gamepad?.connected);
  const pressed = gamepadPressedButtons(gamepad);
  const leftStickX = stickOffset(gamepad?.left_stick_x ?? 0);
  const leftStickY = -stickOffset(gamepad?.left_stick_y ?? 0);
  const rightStickX = stickOffset(gamepad?.right_stick_x ?? 0);
  const rightStickY = -stickOffset(gamepad?.right_stick_y ?? 0);
  const leftTrigger = triggerFill(gamepad?.left_trigger ?? 0);
  const rightTrigger = triggerFill(gamepad?.right_trigger ?? 0);
  const isDark = theme.canvas === FIGMA_DESKTOP_THEME.canvas;
  const bodyFill = isDark ? "rgba(255,255,255,0.018)" : "rgba(255,255,255,0.12)";
  const bodyStroke = isDark ? "rgba(255,255,255,0.22)" : "#b8c0cb";
  const controlFill = isDark ? "rgba(255,255,255,0.018)" : "rgba(255,255,255,0.2)";
  const controlInnerFill = isDark ? "rgba(255,255,255,0.012)" : "rgba(255,255,255,0.14)";
  const controlStroke = isDark ? "rgba(255,255,255,0.2)" : "#c3c9d2";
  const activeFill = isDark ? "rgba(91,139,214,0.26)" : "rgba(77,126,214,0.16)";
  const activeStrongFill = isDark ? "rgba(91,139,214,0.62)" : "rgba(77,126,214,0.46)";
  const buttonFill = (active: boolean) => (active ? activeFill : controlFill);
  const buttonStroke = (active: boolean) => (active ? theme.accent : controlStroke);
  const buttonText = (active: boolean) => (active ? theme.accent : theme.textSub);
  const leftBumperActive = gamepadButtonActive(gamepad, ["LeftBumper", "LB"]);
  const rightBumperActive = gamepadButtonActive(gamepad, ["RightBumper", "RB"]);
  const leftTriggerActive = leftTrigger > 2 || gamepadButtonActive(gamepad, ["LeftTrigger", "LT"]);
  const rightTriggerActive = rightTrigger > 2 || gamepadButtonActive(gamepad, ["RightTrigger", "RT"]);
  const leftStickActive = gamepadButtonActive(gamepad, ["LeftStick", "LeftThumb"]);
  const rightStickActive = gamepadButtonActive(gamepad, ["RightStick", "RightThumb"]);
  const guideActive = gamepadButtonActive(gamepad, ["Guide", "Mode"]);
  const faceButtons = [
    { label: "Y", x: 506, y: 176, names: ["North", "Y"], color: "#2f9a48" },
    { label: "X", x: 468, y: 214, names: ["West", "X"], color: theme.accent },
    { label: "B", x: 544, y: 214, names: ["East", "B"], color: "#c94f4f" },
    { label: "A", x: 506, y: 252, names: ["South", "A"], color: "#2f9a48" },
  ];
  const dpadButtons = [
    { key: "up", names: ["DPadUp"], path: "M0 -33 L9 -18 H-9 Z", activePath: "M-18 -50 H18 V-17 H-18 Z" },
    { key: "down", names: ["DPadDown"], path: "M0 33 L9 18 H-9 Z", activePath: "M-18 17 H18 V50 H-18 Z" },
    { key: "left", names: ["DPadLeft"], path: "M-33 0 L-18 -9 V9 Z", activePath: "M-50 -18 H-17 V18 H-50 Z" },
    { key: "right", names: ["DPadRight"], path: "M33 0 L18 -9 V9 Z", activePath: "M17 -18 H50 V18 H17 Z" },
  ];
  const gamepadViewBox = compact ? "88 28 544 400" : "0 0 720 430";

  return (
    <div
      className={
        compact
          ? "grid h-full min-h-0 grid-rows-[minmax(0,1fr)] gap-3 p-3"
          : "grid h-full min-h-0 grid-rows-[minmax(0,1fr)_auto] gap-3 p-4"
      }
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className="min-h-0">
        {compact ? null : (
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
        )}
        <svg
          className={compact ? "h-full min-h-[220px] w-full" : "h-full min-h-[340px] w-full"}
          viewBox={gamepadViewBox}
          role="img"
          aria-label="gamepad input preview"
          preserveAspectRatio="xMidYMid meet"
        >
          <g>
            <path
              d="M196 148 C230 118 279 116 310 136 C329 148 344 153 360 153 C376 153 391 148 410 136 C441 116 490 118 524 148 C566 187 590 267 607 338 C624 410 587 440 540 426 C512 418 495 388 466 348 C448 323 421 306 384 306 H336 C299 306 272 323 254 348 C225 388 208 418 180 426 C133 440 96 410 113 338 C130 267 154 187 196 148 Z"
              fill={bodyFill}
              stroke={bodyStroke}
              strokeWidth="2"
            />
            <path
              d="M227 143 C255 130 293 130 319 146 H401 C427 130 465 130 493 143"
              fill="none"
              stroke={isDark ? "rgba(255,255,255,0.11)" : "rgba(116,123,132,0.22)"}
              strokeWidth="2"
              strokeLinecap="round"
            />
          </g>

          <g transform="translate(254 42)">
            <rect x="0" y="0" width="82" height="28" rx="8" fill={buttonFill(leftTriggerActive)} stroke={buttonStroke(leftTriggerActive)} />
            <rect x="0" y="0" width={leftTrigger > 0 ? Math.max(3, leftTrigger * 0.82) : 0} height="28" rx="8" fill={activeFill} />
            <text x="18" y="18" fill={buttonText(leftTriggerActive)} fontSize="12" fontWeight="600">LT</text>
            <text x="94" y="18" fill={theme.textMuted} fontSize="12">{Math.round(leftTrigger)}%</text>
          </g>
          <g transform="translate(384 42)">
            <rect x="0" y="0" width="82" height="28" rx="8" fill={buttonFill(rightTriggerActive)} stroke={buttonStroke(rightTriggerActive)} />
            <rect x="0" y="0" width={rightTrigger > 0 ? Math.max(3, rightTrigger * 0.82) : 0} height="28" rx="8" fill={activeFill} />
            <text x="18" y="18" fill={buttonText(rightTriggerActive)} fontSize="12" fontWeight="600">RT</text>
            <text x="94" y="18" fill={theme.textMuted} fontSize="12">{Math.round(rightTrigger)}%</text>
          </g>
          <g transform="translate(242 83)">
            <rect x="0" y="0" width="108" height="32" rx="10" fill={buttonFill(leftBumperActive)} stroke={buttonStroke(leftBumperActive)} strokeWidth="2" />
            <text x="54" y="21" textAnchor="middle" fill={buttonText(leftBumperActive)} fontSize="14" fontWeight="600">LB</text>
          </g>
          <g transform="translate(370 83)">
            <rect x="0" y="0" width="108" height="32" rx="10" fill={buttonFill(rightBumperActive)} stroke={buttonStroke(rightBumperActive)} strokeWidth="2" />
            <text x="54" y="21" textAnchor="middle" fill={buttonText(rightBumperActive)} fontSize="14" fontWeight="600">RB</text>
          </g>

          <g transform="translate(224 202)">
            <circle r="44" fill={controlInnerFill} stroke={controlStroke} strokeWidth="2" />
            <circle r="30" fill={buttonFill(leftStickActive)} stroke={buttonStroke(leftStickActive)} strokeWidth="2" />
            <circle cx={leftStickX} cy={leftStickY} r="12" fill={leftStickActive ? activeStrongFill : theme.accentSoft} stroke={theme.accent} strokeWidth="2" />
          </g>

          <g transform="translate(244 310)">
            <path d="M-20 -52 H20 C27 -52 32 -47 32 -40 V-24 H48 C60 -24 65 -19 65 -8 V8 C65 19 60 24 48 24 H32 V40 C32 47 27 52 20 52 H-20 C-27 52 -32 47 -32 40 V24 H-48 C-60 24 -65 19 -65 8 V-8 C-65 -19 -60 -24 -48 -24 H-32 V-40 C-32 -47 -27 -52 -20 -52 Z" fill={controlFill} stroke={controlStroke} strokeWidth="2" />
            {dpadButtons.map((button) => {
              const active = gamepadButtonActive(gamepad, button.names);
              return (
                <g key={button.key}>
                  <path d={button.activePath} fill={active ? activeFill : "transparent"} />
                  <path d={button.path} fill={active ? theme.accent : theme.textMuted} opacity={active ? 1 : 0.76} />
                </g>
              );
            })}
          </g>

          <g transform="translate(384 300)">
            <circle r="44" fill={controlInnerFill} stroke={controlStroke} strokeWidth="2" />
            <circle r="30" fill={buttonFill(rightStickActive)} stroke={buttonStroke(rightStickActive)} strokeWidth="2" />
            <circle cx={rightStickX} cy={rightStickY} r="12" fill={rightStickActive ? activeStrongFill : theme.accentSoft} stroke={theme.accent} strokeWidth="2" />
          </g>

          <g transform="translate(317 213)">
            <rect x="-28" y="-13" width="56" height="26" rx="13" fill={buttonFill(gamepadButtonActive(gamepad, ["Select"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["Select"]))} strokeWidth="2" />
            <rect x="72" y="-13" width="56" height="26" rx="13" fill={buttonFill(gamepadButtonActive(gamepad, ["Start"]))} stroke={buttonStroke(gamepadButtonActive(gamepad, ["Start"]))} strokeWidth="2" />
            <circle cx="50" cy="35" r="19" fill={buttonFill(guideActive)} stroke={buttonStroke(guideActive)} strokeWidth="2" />
            <text x="0" y="5" textAnchor="middle" fill={buttonText(gamepadButtonActive(gamepad, ["Select"]))} fontSize="11" fontWeight="600">Select</text>
            <text x="100" y="5" textAnchor="middle" fill={buttonText(gamepadButtonActive(gamepad, ["Start"]))} fontSize="11" fontWeight="600">Start</text>
            <path d="M45 35 L50 30 L55 35 L50 40 Z" fill={guideActive ? theme.accent : theme.textMuted} opacity={guideActive ? 1 : 0.7} />
          </g>

          {faceButtons.map((button) => {
            const active = gamepadButtonActive(gamepad, button.names);
            return (
              <g key={button.label}>
                <circle cx={button.x} cy={button.y} r="27" fill={buttonFill(active)} stroke={buttonStroke(active)} strokeWidth="2" />
                <text
                  x={button.x}
                  y={button.y + 7}
                  textAnchor="middle"
                  fill={active ? theme.accent : button.color}
                  fontSize="22"
                  fontWeight="700"
                >
                  {button.label}
                </text>
              </g>
            );
          })}

        </svg>
      </div>
      {compact ? null : (
      <div className="grid shrink-0 grid-cols-2 gap-2 text-xs lg:grid-cols-4">
        <KeyboardSignal label="已按下" value={pressed.length ? pressed.join(", ") : "无"} theme={theme} />
        <KeyboardSignal label="最近按键" value={gamepad?.last_button ?? "无"} theme={theme} />
        <KeyboardSignal label="按下/抬起" value={`${gamepad?.button_press_count ?? 0}/${gamepad?.button_release_count ?? 0}`} theme={theme} />
        <KeyboardSignal label="按键事件" value={String(gamepad?.button_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="摇杆事件" value={String(gamepad?.axis_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="扳机事件" value={String(gamepad?.trigger_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="总事件数" value={String(gamepad?.event_count ?? 0)} theme={theme} />
        <KeyboardSignal
          label="摇杆"
          value={`L ${stickPercent(gamepad?.left_stick_x ?? 0)}, ${stickPercent(gamepad?.left_stick_y ?? 0)} / R ${stickPercent(gamepad?.right_stick_x ?? 0)}, ${stickPercent(gamepad?.right_stick_y ?? 0)}`}
          theme={theme}
        />
      </div>
      )}
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
          最近 {events.length} 条        </div>
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

function localInputDeviceCount(
  snapshot: LocalControlsSnapshot | null,
  kind: "keyboard" | "mouse",
) {
  if (!snapshot) {
    return 0;
  }

  const devices =
    kind === "keyboard" ? snapshot.keyboard_devices : snapshot.mouse_devices;
  if (devices?.length) {
    return devices.length;
  }

  const detected =
    kind === "keyboard" ? snapshot.keyboard.detected : snapshot.mouse.detected;
  return detected ? 1 : 0;
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
                ? "rgba(197, 48, 48, 0.08)"
                : theme.accentSoft,
              color: service.online ? "#9f1f2d" : theme.accent,
              border: `1px solid ${
                service.online
                  ? "rgba(197, 48, 48, 0.55)"
                  : theme.accent
              }`,
              opacity: busy ? 0.7 : 1,
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
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [filter, setFilter] = useState<"all" | "error" | "warn" | "info" | "debug">("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(false);

  const loadLogs = async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invokeCommand<LogEntry[] | null>("get_logs", { limit: 1000 });
      setLogs(safeArray(result).filter(isLogEntry));
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

  const filteredLogs = safeArray(logs).filter(log => {
    if (filter === "all") return true;
    return log.level.toLowerCase() === filter;
  });

  const getLevelColor = (level: string | null | undefined) => {
    switch (String(level ?? "").toLowerCase()) {
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
              查看守护进程的运行日志。
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




