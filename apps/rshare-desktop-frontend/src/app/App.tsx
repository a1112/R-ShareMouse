import {
  createContext,
  memo,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  ExternalLink,
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
  QrCode,
  RotateCcw,
  Settings,
  Smartphone,
  Square,
  Upload,
  Volume2,
  Wifi,
  X,
} from "lucide-react";

import MonitorManager, {
  DeviceData as LayoutDevice,
  MonitorData,
} from "./components/MonitorManager";
import MobileController from "./MobileController";

const TopologyCommitProbe = memo(function TopologyCommitProbe() {
  const topology = useUiStore(selectTopologyProjection);
  useLayoutEffect(() => {
    (
      globalThis as typeof globalThis & {
        __rsharePerfRecordTopologyCommit?: () => void;
      }
    ).__rsharePerfRecordTopologyCommit?.();
  }, [topology]);
  return null;
});

const PaintCommitProbe = memo(function PaintCommitProbe() {
  const input = useUiStore(selectInputVisuals);
  const topology = useUiStore(selectTopologyProjection);
  const status = useUiStore((state) => state.connections.status);

  useEffect(() => {
    const observedAt = Number(input.pointer?.observed_at_ms);
    if (Number.isFinite(observedAt)) {
      (
        globalThis as typeof globalThis & {
          __rsharePerfRecordInputPaint?: (observedAt: number) => void;
        }
      ).__rsharePerfRecordInputPaint?.(observedAt);
    }
    const transitionObservedAt = Number(
      input.lastDiscreteTransition?.observed_at_ms,
    );
    const transitionSequence = Number(
      input.lastDiscreteTransition?.perf_sequence,
    );
    if (
      Number.isFinite(transitionObservedAt) &&
      Number.isInteger(transitionSequence)
    ) {
      (
        globalThis as typeof globalThis & {
          __rsharePerfRecordDiscretePaint?: (
            observedAt: number,
            sequence: number,
          ) => void;
        }
      ).__rsharePerfRecordDiscretePaint?.(
        transitionObservedAt,
        transitionSequence,
      );
    }
  }, [input]);

  useEffect(() => {
    const observedAt = Number(topology.layout?.observed_at_ms);
    if (!Number.isFinite(observedAt)) return;
    (
      globalThis as typeof globalThis & {
        __rsharePerfRecordTopologyStatusPaint?: (
          observedAt: number,
        ) => void;
      }
    ).__rsharePerfRecordTopologyStatusPaint?.(observedAt);
  }, [topology]);

  useEffect(() => {
    const observedAt = Number(status?.observed_at_ms);
    if (!Number.isFinite(observedAt)) return;
    (
      globalThis as typeof globalThis & {
        __rsharePerfRecordTopologyStatusPaint?: (
          observedAt: number,
        ) => void;
      }
    ).__rsharePerfRecordTopologyStatusPaint?.(observedAt);
  }, [status]);

  return null;
});
import {
  buildDesktopViewModel,
  buildDisplaySettingsViewModel,
  buildBrowserGamepadRecentEvents,
  buildDeviceGalleryItems,
  buildDeviceTypeSummaries,
  buildEndpointAcceptance,
  buildLocalDeviceSelectItems,
  buildMobileAccessViewModel,
  buildRemoteControlSnapshot,
  buildRemoteLatencySummary,
  buildVirtualDisplayViewModel,
  describeAudioEndpoint,
  endpointEventToLocalControlEvent,
  projectUiInputToLocalControls,
  updateRememberedLayoutFromVisibleMonitors,
} from "./desktop-model.mjs";
import {
  buildFooterStatus,
  getDeviceConsoleSections,
  getDeviceSimulatorChrome,
  formatNetworkGatewayError,
  getHeaderMetrics,
  getHardwareAssetPresetOptions,
  getLocalControlRefreshTiming,
  getMouseDetailLayoutClasses,
  getMouseSimulatorLayoutClasses,
  getPageLabels,
  getSettingsLayoutSections,
  getThemeModeOptions,
  preventBrowserNavigationEvent,
} from "./desktop-shell.mjs";
import {
  buildPageChrome,
  FIGMA_DESKTOP_THEME,
  getDesktopTheme,
} from "./desktop-theme.mjs";
import {
  BUILTIN_HARDWARE_ASSET_MANIFESTS,
  buildGamepadAnalogFeedback,
  buildHardwareAssetChoices,
  normalizeHardwareAssetManifest,
  resolveActiveHardwareRegions,
  resolveSelectedHardwareAsset,
} from "./hardware-assets.mjs";
import {
  UiStateClient,
  createTauriUiStateConnector,
  createWebSocketUiStateConnector,
} from "./ui-state-client.mjs";
import {
  createUiStateAppBindings,
  selectDashboardPayload,
  selectDiagnostics,
  selectHasAuthoritativeSnapshot,
  selectInputVisuals,
  selectTopologyProjection,
  createOwnerlessStreamCoordinator,
} from "./ui-store.mjs";
import { uiStateStore, useUiStore } from "./use-ui-store";
import {
  createDisplayCaptureObjectUrl,
  createDisplayCaptureUrlStore,
  mapWithConcurrency,
} from "./display-capture.mjs";

type DesktopPage = "layout" | "devices" | "logs" | "settings";
type SettingsSectionKey =
  | "local"
  | "service"
  | "mobile"
  | "hardware"
  | "input"
  | "appearance"
  | "acceptance";

const DEVICE_CONSOLE_SECTIONS = getDeviceConsoleSections();
const DEVICE_SIMULATOR_CHROME = getDeviceSimulatorChrome();
const SETTINGS_LAYOUT_SECTIONS = getSettingsLayoutSections() as Array<{
  key: SettingsSectionKey;
  label: string;
  description: string;
}>;

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

function usePreventBrowserNavigationEvents() {
  useEffect(() => {
    const options: AddEventListenerOptions = { capture: true, passive: false };
    const handleNavigationTrigger = (event: Event) => {
      preventBrowserNavigationEvent(event);
    };
    const mouseEventNames = [
      "mousedown",
      "mouseup",
      "auxclick",
      "pointerdown",
      "pointerup",
    ];

    for (const eventName of mouseEventNames) {
      window.addEventListener(eventName, handleNavigationTrigger, options);
    }
    window.addEventListener("keydown", handleNavigationTrigger, options);

    return () => {
      for (const eventName of mouseEventNames) {
        window.removeEventListener(eventName, handleNavigationTrigger, options);
      }
      window.removeEventListener("keydown", handleNavigationTrigger, options);
    };
  }, []);
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
  capabilities?: unknown | null;
  display_inventory?: unknown | null;
  auto_started?: boolean;
};

type CapabilityOverview = {
  available: boolean;
  localDeviceId: string | null;
  devices: Array<{
    id: string;
    name: string;
    hostname: string;
    connected: boolean;
    local: boolean;
    capabilities: Array<{
      kind: string;
      label: string;
      state: string;
      stateLabel: string;
      reason?: string | null;
    }>;
  }>;
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
      adapter_id?: string | null;
      target_id?: string | null;
      friendly_name?: string | null;
      name?: string | null;
      device_name?: string | null;
      x: number;
      y: number;
      width: number;
      height: number;
      work_x?: number;
      work_y?: number;
      work_width?: number;
      work_height?: number;
      orientation?: DisplayOrientation;
      scale_percent?: number | null;
      dpi_x?: number | null;
      dpi_y?: number | null;
      raw_dpi_x?: number | null;
      raw_dpi_y?: number | null;
      refresh_rate_millihz?: number | null;
      bits_per_pixel?: number | null;
      active?: boolean;
      primary: boolean;
      modes?: DisplayModeInfo[];
      write_capabilities?: DisplayWriteCapabilities;
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
    filter_queue_capacity?: number;
    filter_queue_depth?: number;
    filter_queued_events?: number;
    filter_dropped_events?: number;
    filter_keyboard_connects?: number;
    filter_mouse_connects?: number;
    filter_keyboard_events?: number;
    filter_mouse_events?: number;
    test_signing_required: boolean;
    last_error?: string | null;
  };
  recent_events: LocalControlEvent[];
  latency_feedback?: unknown | null;
  last_error?: string | null;
};

type DisplayOrientation =
  | "Landscape"
  | "Portrait"
  | "LandscapeFlipped"
  | "PortraitFlipped";

type DisplayWriteCapabilities = {
  resolution?: boolean;
  refresh_rate?: boolean;
  orientation?: boolean;
  primary?: boolean;
  position?: boolean;
  scale?: boolean;
  capture?: boolean;
};

type DisplayModeInfo = {
  width: number;
  height: number;
  refresh_rate_millihz?: number | null;
  orientation?: DisplayOrientation;
  bits_per_pixel?: number | null;
};

type DisplayOperationStatus =
  | "Success"
  | "Unsupported"
  | "PermissionDenied"
  | "InvalidDisplay"
  | "InvalidMode"
  | "RequiresSystemSettings"
  | "ApplyFailed";

type DisplayCaptureResult = {
  request_id: string;
  status: DisplayOperationStatus;
  message?: string | null;
  payload?: {
    capture_id: string;
    display_id: string;
    mime_type: string;
    width: number;
    height: number;
    byte_length: number;
  } | null;
};

type DisplaySettingsUpdateResult = {
  status: DisplayOperationStatus;
  message?: string | null;
};

type DisplaySettingsDisplayView = {
  id: string;
  index: number;
  title: string;
  name: string;
  deviceName: string | null;
  x: number;
  y: number;
  width: number;
  height: number;
  workArea: { x: number; y: number; width: number; height: number };
  primary: boolean;
  active: boolean;
  orientation: DisplayOrientation;
  scalePercent: number | null;
  refreshRateMillihz: number | null;
  bitsPerPixel: number | null;
  dpi: { x: number | null; y: number | null; rawX: number | null; rawY: number | null };
  resolutionLabel: string;
  scaleLabel: string;
  refreshRateLabel: string;
  resolutionOptions: Array<{ value: string; label: string; width: number; height: number }>;
  refreshRateOptions: Array<{ value: string; label: string; refreshRateMillihz: number }>;
  writeCapabilities: {
    resolution: boolean;
    refreshRate: boolean;
    orientation: boolean;
    primary: boolean;
    position: boolean;
    scale: boolean;
    capture: boolean;
  };
};

type DisplaySettingsViewModel = {
  displays: DisplaySettingsDisplayView[];
  selectedDisplay: DisplaySettingsDisplayView;
  selectedDisplayId: string | null;
  bounds: { minX: number; minY: number; maxX: number; maxY: number; width: number; height: number };
};

type VirtualDisplaySnapshot = {
  id: string;
  width: number;
  height: number;
  refresh_rate_millihz?: number | null;
  name?: string | null;
  status: string;
  display_id?: string | null;
  message?: string | null;
};

type VirtualDisplayOperationResult = {
  status: string;
  display?: VirtualDisplaySnapshot | null;
  message?: string | null;
};

type LocalInputTestResult = {
  status: "Success" | "PermissionDenied" | "BackendUnavailable" | "Failed" | "Unsupported";
  message: string;
  kind?: string | null;
  targetId?: string | null;
  successCount?: number;
  totalCount?: number;
  averageElapsedMs?: number | null;
  maxElapsedMs?: number | null;
};

type MobileAccessSnapshot = {
  enabled: boolean;
  bind_address: string;
  page_url: string;
  token: string;
  last_client_addr?: string | null;
  last_client_seen_at_ms?: number | null;
  client_count?: number;
};

type RemoteLatencySummary = {
  state: "idle" | "pending" | "pass" | "warn" | "fail";
  message: string;
  networkRoundTripMs: number | null;
  estimatedOneWayMs: number | null;
  rawRoundTripMs: number | null;
  remoteProcessingMs: number | null;
  direction: string | null;
  timestampMs: number | null;
};

type EndpointEvent = {
  event_id: number;
  sequence: number;
  timestamp_ms: number;
  endpoint_id: string;
  origin_endpoint_id: string;
  device: {
    device_id: string;
    instance_id?: string | null;
    display_name: string;
    kind: string;
    attribution: string;
  };
  direction: string;
  source: string;
  kind: string;
  payload: {
    kind?: string;
    data?: Record<string, unknown>;
    [key: string]: unknown;
  };
  correlation_id?: string | null;
};

type EndpointInjectTarget = "Local" | { Remote: string };

type EndpointInjectResult = {
  correlation_id: string;
  target: EndpointInjectTarget;
  accepted: boolean;
  backend_kind?: string | null;
  health: unknown;
  elapsed_ms: number;
  loopback_event_id?: number | null;
  error?: string | null;
};

type LogEntry = {
  timestamp: string;
  level: string;
  target: string;
  message: string;
};

type LocalControlKind = "keyboard" | "mouse" | "gamepad" | "display" | "audio";
type LocalDevicePageKind = "overview" | LocalControlKind | "remote";
type AudioInputDevice = {
  id: string;
  name: string;
  endpoint_id?: string | null;
  kind?: "Microphone" | "Loopback";
  form_factor?: string;
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
  form_factor?: string;
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

type UiStateEnvelope = {
  type: string;
  payload: Record<string, unknown>;
};

type UiStateTransportOptions = {
  cursor: { boot_id: string; revision: number } | null;
  onEnvelope: (envelope: unknown) => void;
  onDisconnect: (error: unknown) => void;
  signal?: AbortSignal;
};

type LocalControlSubscription = {
  stop: () => void;
  usesTauriBridge: boolean;
};

type ThemeMode = "light" | "dark" | "system";

const LOCAL_CONTROL_REFRESH_TIMING = getLocalControlRefreshTiming();
const LOCAL_CONTROL_EVENT_FLUSH_MS = LOCAL_CONTROL_REFRESH_TIMING.eventFlushMs;
const HIDDEN_MONITOR_IDS_STORAGE_KEY = "rshare.hiddenMonitorIds";
const HARDWARE_RIG_VARIANT_STORAGE_KEY = "rshare.hardwareRigVariant";
const HARDWARE_ASSET_KEYBOARD_STORAGE_KEY = "rshare.hardwareAsset.keyboard";
const HARDWARE_ASSET_MOUSE_STORAGE_KEY = "rshare.hardwareAsset.mouse";
const HARDWARE_ASSET_GAMEPAD_STORAGE_KEY = "rshare.hardwareAsset.gamepad";
const DAEMON_IPC_BRIDGE_ENDPOINT = "/__rshare/ipc";
const DISPLAY_CAPTURE_BRIDGE_ENDPOINT = "/__rshare/display-capture";
const DAEMON_LOGS_BRIDGE_ENDPOINT = "/__rshare/logs";
const DAEMON_SERVICE_BRIDGE_ENDPOINT = "/__rshare/service";
const LOCAL_CONTROLS_WS_URL = "ws://127.0.0.1:27436/local-controls";
const UI_STATE_WS_PATH = "/ui-state";
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
  "mobile_access",
  "endpoint_events_state",
  "inject_endpoint_event",
  "start_local_controls_stream",
  "stop_local_controls_stream",
  "start_endpoint_events_stream",
  "stop_endpoint_events_stream",
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
  "identify_displays",
  "update_display_settings",
  "open_display_settings",
  "list_virtual_displays",
  "create_virtual_display",
  "remove_virtual_display",
]);
const WEB_NOOP_COMMANDS = new Set([
  "minimize_window",
  "toggle_maximize_window",
  "close_window",
]);

const PAGE_LABELS: Array<{ key: DesktopPage; label: string }> = getPageLabels();

function getInvoke(): TauriInvoke | null {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      core?: {
        invoke?: TauriInvoke;
        convertFileSrc?: (filePath: string, protocol?: string) => string;
      };
    };
  };

  return tauriWindow.__TAURI__?.core?.invoke ?? null;
}

function getConvertFileSrc(): ((filePath: string, protocol?: string) => string) | null {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      core?: {
        convertFileSrc?: (filePath: string, protocol?: string) => string;
      };
    };
  };

  return tauriWindow.__TAURI__?.core?.convertFileSrc ?? null;
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

function uiStateWebSocketUrl(): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}${UI_STATE_WS_PATH}`;
}

function listenTauriEventTransport(
  eventName: string,
  handler: (event: { payload: unknown }) => void,
): Promise<() => void> {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      event?: {
        listen?: (
          event: string,
          handler: (event: { payload: unknown }) => void,
        ) => Promise<() => void>;
      };
    };
  };
  const listen = tauriWindow.__TAURI__?.event?.listen;
  if (!listen) {
    return Promise.reject(new Error("Tauri UI 状态事件桥不可用"));
  }
  return listen(eventName, handler);
}

async function connectUiStateTransport(
  options: UiStateTransportOptions,
): Promise<unknown> {
  const invoke = getInvoke();
  const tauriWindow = window as Window & {
    __TAURI__?: { event?: { listen?: unknown } };
  };
  if (invoke && typeof tauriWindow.__TAURI__?.event?.listen === "function") {
    return createTauriUiStateConnector({
      invoke,
      listen: listenTauriEventTransport,
    })(options);
  }
  return createWebSocketUiStateConnector({
    WebSocket: window.WebSocket,
    url: uiStateWebSocketUrl(),
  })(options);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function daemonIpcRequest(request: unknown): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(DAEMON_IPC_BRIDGE_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(request),
    });
  } catch (error) {
    throw new Error(formatNetworkGatewayError(error, "daemon IPC"));
  }

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

function isDaemonIpcUnavailable(error: unknown): boolean {
  const message = errorMessage(error);
  return (
    message.includes("ECONNREFUSED") ||
    message.includes("Connection refused") ||
    message.includes("Failed to fetch") ||
    message.includes("网关不可用")
  );
}

async function daemonServiceRequest<T>(
  action: "start" | "stop",
  variant: string,
): Promise<T> {
  let response: Response;
  try {
    response = await fetch(DAEMON_SERVICE_BRIDGE_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ action }),
    });
  } catch (error) {
    throw new Error(formatNetworkGatewayError(error, "daemon 服务"));
  }
  const payload = await response.json().catch(() => null);
  if (!response.ok) {
    const message =
      isRecord(payload) && typeof payload.error === "string"
        ? payload.error
        : `daemon 服务网关请求失败：HTTP ${response.status}`;
    throw new Error(message);
  }
  return daemonResponseValue<T>(payload, variant);
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

function newCorrelationId(prefix: string) {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `${prefix}-${crypto.randomUUID()}`;
  }
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function endpointInjectTarget(deviceId?: string | null): EndpointInjectTarget {
  return deviceId ? { Remote: deviceId } : "Local";
}

function endpointInjectStatusFromError(error: string): LocalInputTestResult["status"] {
  const status: LocalInputTestResult["status"] =
    error === "PermissionDenied"
      ? "PermissionDenied"
      : error === "BackendUnavailable" ||
          error === "BackendDegraded" ||
          error === "TargetDisconnected" ||
          error === "Timeout"
        ? "BackendUnavailable"
        : error === "UnsupportedEvent"
          ? "Unsupported"
          : "Failed";
  return status;
}

function endpointInjectResultsToLocalInputTestResult(
  results: EndpointInjectResult[],
  context: { kind: string; targetId?: string | null },
): LocalInputTestResult {
  const totalCount = results.length;
  const successCount = results.filter((result) => result.accepted).length;
  const latencies = results
    .map((result) => Number(result.elapsed_ms))
    .filter((value) => Number.isFinite(value));
  const averageElapsedMs = latencies.length
    ? Math.round(latencies.reduce((sum, value) => sum + value, 0) / latencies.length)
    : null;
  const maxElapsedMs = latencies.length ? Math.max(...latencies) : null;
  const failed = results.find((result) => !result.accepted);
  const error = failed?.error ?? (totalCount ? "Failed" : "Timeout");
  const status: LocalInputTestResult["status"] =
    totalCount > 0 && successCount === totalCount
      ? "Success"
      : endpointInjectStatusFromError(error);
  const latencyText =
    averageElapsedMs == null || maxElapsedMs == null
      ? ""
      : `，平均 ${averageElapsedMs} ms，最大 ${maxElapsedMs} ms`;
  return {
    status,
    message:
      status === "Success"
        ? `Endpoint 注入完成：${successCount}/${totalCount} 成功${latencyText}`
        : `Endpoint 注入失败：${error}（${successCount}/${totalCount} 成功${latencyText}）`,
    kind: context.kind,
    targetId: context.targetId ?? null,
    successCount,
    totalCount,
    averageElapsedMs,
    maxElapsedMs,
  };
}

function endpointInjectRequests(
  kind: string,
  snapshot: LocalControlsSnapshot | null,
): Array<Record<string, unknown>> {
  if (kind === "mouse") {
    return [
      {
        correlation_id: newCorrelationId("mouse-move"),
        device_kind: "Mouse",
        payload: {
          kind: "MouseMove",
          data: {
            x: Number(snapshot?.mouse?.x ?? 0) + 8,
            y: Number(snapshot?.mouse?.y ?? 0) + 8,
            display_id: snapshot?.mouse?.current_display_id ?? null,
          },
        },
        mode: "TestLoopback",
        timeout_ms: 1000,
      },
    ];
  }

  return [
    {
      correlation_id: newCorrelationId("key-press"),
      device_kind: "Keyboard",
      payload: {
        kind: "Keyboard",
        data: {
          key: "ShiftLeft",
          state: "Pressed",
        },
      },
      mode: "TestLoopback",
      timeout_ms: 1000,
    },
    {
      correlation_id: newCorrelationId("key-release"),
      device_kind: "Keyboard",
      payload: {
        kind: "Keyboard",
        data: {
          key: "ShiftLeft",
          state: "Released",
        },
      },
      mode: "TestLoopback",
      timeout_ms: 1000,
    },
  ];
}

function endpointEventFilter(endpointId?: string | null) {
  return {
    endpoint_id: endpointId ?? null,
    device_id: null,
    kinds: [],
    sources: [],
    include_loopback: true,
  };
}

async function buildNetworkDashboardState(): Promise<DashboardPayload> {
  let autoStarted = false;
  let status: unknown;
  try {
    status = await daemonRequestValue<unknown>("Status", "Status");
  } catch (error) {
    if (!isDaemonIpcUnavailable(error)) {
      throw error;
    }
    status = await daemonServiceRequest<unknown>("start", "Status");
    autoStarted = true;
  }

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
    auto_started: autoStarted,
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
      return (await daemonServiceRequest<unknown>("start", "Status")) as T;
    case "stop_service":
      return await daemonServiceRequest<T>("stop", "Ack");
    case "get_logs": {
      const limit = Number(args?.limit ?? 1000);
      let response: Response;
      try {
        response = await fetch(
          `${DAEMON_LOGS_BRIDGE_ENDPOINT}?limit=${encodeURIComponent(String(limit))}`,
        );
      } catch (error) {
        throw new Error(formatNetworkGatewayError(error, "日志"));
      }
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
      let response: Response;
      try {
        response = await fetch(DAEMON_LOGS_BRIDGE_ENDPOINT, { method: "DELETE" });
      } catch (error) {
        throw new Error(formatNetworkGatewayError(error, "日志"));
      }
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
    case "mobile_access":
      return await daemonRequestValue<T>("MobileAccess", "MobileAccess");
    case "endpoint_events_state":
      return await daemonRequestValue<T>(
        {
          EndpointEvents: {
            filter: args?.filter ?? endpointEventFilter(null),
            after_sequence: args?.after_sequence ?? args?.afterSequence ?? null,
            limit: args?.limit ?? 128,
          },
        },
        "EndpointEvents",
      );
    case "inject_endpoint_event":
      return await daemonRequestValue<T>(
        {
          InjectEndpointEvent: {
            target: args?.target ?? "Local",
            request: args?.request,
          },
        },
        "EndpointInjectResult",
      );
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
    case "identify_displays":
      return await daemonRequestValue<T>(
        {
          IdentifyDisplays: {
            duration_ms: args?.duration_ms ?? args?.durationMs ?? 2500,
          },
        },
        "DisplayIdentify",
      );
    case "update_display_settings":
      return await daemonRequestValue<T>(
        {
          UpdateDisplaySettings: args?.request ?? args,
        },
        "DisplaySettingsUpdated",
      );
    case "open_display_settings":
      return await daemonRequestValue<T>("OpenDisplaySettings", "Ack");
    case "list_virtual_displays":
      return await daemonRequestValue<T>("ListVirtualDisplays", "VirtualDisplays");
    case "create_virtual_display":
      return await daemonRequestValue<T>(
        { CreateVirtualDisplay: args?.request ?? args },
        "VirtualDisplayOperation",
      );
    case "remove_virtual_display":
      return await daemonRequestValue<T>(
        { RemoveVirtualDisplay: args?.request ?? args },
        "VirtualDisplayOperation",
      );
    case "start_local_controls_stream":
    case "stop_local_controls_stream":
    case "start_endpoint_events_stream":
    case "stop_endpoint_events_stream":
      return undefined as T;
    default:
      throw new Error(`命令 ${command} 尚未支持网络通信`);
  }
}

async function listenLocalControlEvent(
  handler: (payload: unknown) => void,
): Promise<LocalControlSubscription | null> {
  const unlisten = await listenTauriEvent<unknown>("local-control-event", handler);
  if (unlisten) {
    return {
      stop: unlisten,
      usesTauriBridge: true,
    };
  }

  if (typeof WebSocket !== "undefined") {
    const socket = new WebSocket(LOCAL_CONTROLS_WS_URL);
    let intentionalClose = false;
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
      if (!intentionalClose) {
        handler("本机输入实时 WebSocket 不可用");
      }
    });
    socket.addEventListener("close", (event) => {
      if (!intentionalClose && event.code !== 1000) {
        handler("本机输入实时 WebSocket 已断开");
      }
    });
    const closeSocket = () => {
      intentionalClose = true;
      if (socket.readyState === WebSocket.CONNECTING) {
        socket.addEventListener("open", () => socket.close(1000), { once: true });
        return;
      }
      if (
        socket.readyState === WebSocket.OPEN ||
        socket.readyState === WebSocket.CLOSING
      ) {
        socket.close(1000);
      }
    };
    return {
      stop: closeSocket,
      usesTauriBridge: false,
    };
  }
  return null;
}

async function listenEndpointEvent(
  handler: (payload: unknown) => void,
): Promise<LocalControlSubscription | null> {
  const unlisten = await listenTauriEvent<unknown>("endpoint-event", handler);
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

async function captureDisplayBinary(
  displayId: string,
  maxWidth: number,
): Promise<Uint8Array> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<unknown>("capture_display_binary", {
      displayId,
      maxWidth,
    });
    if (value instanceof Uint8Array) return value;
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new Error("Tauri display capture did not return a binary response");
  }

  const response = await fetch(DISPLAY_CAPTURE_BRIDGE_ENDPOINT, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ display_id: displayId, max_width: maxWidth }),
  });
  if (!response.ok) {
    throw new Error(`display capture bridge failed: HTTP ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

const localControlsStreamCoordinator = createOwnerlessStreamCoordinator({
  start: () => invokeCommand("start_local_controls_stream"),
  stop: () => invokeCommand("stop_local_controls_stream"),
});

const endpointEventsStreamCoordinator = createOwnerlessStreamCoordinator({
  start: () =>
    invokeCommand("start_endpoint_events_stream", {
      filter: endpointEventFilter(null),
    }),
  stop: () => invokeCommand("stop_endpoint_events_stream"),
});

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
      next.keyboard_devices = upsertEndpointInputDevice(
        snapshot.keyboard_devices,
        event,
        "keyboard",
      );
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
    next.mouse_devices = upsertEndpointInputDevice(
      snapshot.mouse_devices,
      event,
      "mouse",
    );
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

function applyEndpointEvents(
  snapshot: LocalControlsSnapshot | null,
  endpointEvents: EndpointEvent[],
): LocalControlsSnapshot {
  const events = safeArray(endpointEvents)
    .map((event) => endpointEventToLocalControlEvent(event) as LocalControlEvent)
    .sort((left, right) => left.sequence - right.sequence);
  const base = snapshot ?? buildEmptyControlSnapshot(null);
  return events.reduce((next, event) => applyLocalControlEvent(next, event), base);
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
  const generatedEvents: LocalControlEvent[] = [];
  let sequenceBase = Math.max(
    Number(current.sequence ?? 0),
    ...(current.recent_events ?? []).map((event) => Number(event.sequence ?? 0)),
  );

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

    const browserEvents = buildBrowserGamepadRecentEvents(existing, next, {
      sequenceBase,
      timestampMs: now,
    }) as LocalControlEvent[];
    sequenceBase += browserEvents.length;
    generatedEvents.push(...browserEvents);

    changed = true;
    if (existingIndex >= 0) {
      gamepads[existingIndex] = next;
    } else {
      gamepads.push(next);
    }
  }

  if (!changed) {
    return current;
  }

  return {
    ...current,
    sequence: Math.max(Number(current.sequence ?? 0), sequenceBase),
    gamepads,
    recent_events: generatedEvents.length
      ? mergeLocalControlEvents(current.recent_events ?? [], generatedEvents)
      : current.recent_events ?? [],
  };
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

function upsertEndpointInputDevice<T extends NonNullable<LocalControlsSnapshot["keyboard_devices"]>[number]>(
  devices: T[] | undefined,
  event: LocalControlEvent,
  source: "keyboard" | "mouse",
) {
  const deviceId = event.payload?.device_id;
  if (!deviceId) {
    return devices ?? [];
  }

  const next = [...(devices ?? [])];
  const index = next.findIndex(
    (device) =>
      normalizeDeviceIdentifier(device.id) === normalizeDeviceIdentifier(deviceId) ||
      normalizeDeviceIdentifier(device.device_instance_id) ===
        normalizeDeviceIdentifier(event.device_instance_id),
  );
  const existing = index >= 0 ? next[index] : null;
  const updated = {
    id: existing?.id ?? deviceId,
    name:
      event.payload?.device_display_name ??
      existing?.name ??
      (source === "keyboard" ? "Endpoint Keyboard" : "Endpoint Mouse"),
    source: existing?.source ?? event.capture_path ?? "endpoint",
    connected: true,
    driver_detail: existing?.driver_detail ?? event.capture_path ?? null,
    device_instance_id: existing?.device_instance_id ?? event.device_instance_id ?? null,
    capture_path: existing?.capture_path ?? event.capture_path ?? null,
    event_count: Number(existing?.event_count ?? 0) + 1,
    last_event_ms: event.timestamp_ms,
    capabilities: existing?.capabilities ?? [],
  } as T;

  if (index >= 0) {
    next[index] = updated;
  } else {
    next.push(updated);
  }
  return next;
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
    deviceKind: monitor.deviceKind === "remote" ? "remote" : "local",
    resWidth: Number(monitor.resWidth),
    resHeight: Number(monitor.resHeight),
    color: String(monitor.color),
    x: Number(monitor.x),
    y: Number(monitor.y),
    w: Number(monitor.w),
    h: Number(monitor.h),
    primary: Boolean(monitor.primary),
    enabled: Boolean(monitor.enabled) && !hiddenMonitorIds.has(String(monitor.id)),
    orientation:
      monitor.orientation == null ? null : String(monitor.orientation),
    scalePercent:
      monitor.scalePercent == null ? null : Number(monitor.scalePercent),
    refreshRateMillihz:
      monitor.refreshRateMillihz == null ? null : Number(monitor.refreshRateMillihz),
    writeCapabilities: isRecord(monitor.writeCapabilities)
      ? {
          resolution: Boolean(monitor.writeCapabilities.resolution),
          refreshRate: Boolean(monitor.writeCapabilities.refreshRate),
          orientation: Boolean(monitor.writeCapabilities.orientation),
          primary: Boolean(monitor.writeCapabilities.primary),
          position: Boolean(monitor.writeCapabilities.position),
          scale: Boolean(monitor.writeCapabilities.scale),
          capture: Boolean(monitor.writeCapabilities.capture),
        }
      : undefined,
  }));
}

function isMobileControllerPath() {
  if (typeof window === "undefined") {
    return false;
  }
  return window.location.pathname.replace(/\/+$/, "") === "/mobile";
}

export default function App() {
  if (isMobileControllerPath()) {
    return <MobileController />;
  }
  return <DesktopApp />;
}

function DesktopApp() {
  const performanceProbesEnabled =
    typeof window !== "undefined" &&
    (
      window as Window & {
        __rsharePerfEnableStoreAccess?: boolean;
      }
    ).__rsharePerfEnableStoreAccess === true;
  usePreventBrowserNavigationEvents();

  const [page, setPage] = useState<DesktopPage>("layout");
  const payload = useUiStore(selectDashboardPayload) as DashboardPayload;
  const [busy, setBusy] = useState(false);
  const [themeMode, setThemeMode] = useState<ThemeMode>("system");
  const [systemPrefersDark, setSystemPrefersDark] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [localControls, setLocalControls] = useState<LocalControlsSnapshot | null>(null);
  const [localControlsError, setLocalControlsError] = useState<string | null>(null);
  const [mobileAccess, setMobileAccess] = useState<MobileAccessSnapshot | null>(null);
  const [mobileAccessError, setMobileAccessError] = useState<string | null>(null);
  const [localInputTestResult, setLocalInputTestResult] =
    useState<LocalInputTestResult | null>(null);
  const [remoteLatencyTestResult, setRemoteLatencyTestResult] =
    useState<LocalInputTestResult | null>(null);
  const [confirmingInputTest, setConfirmingInputTest] = useState<string | null>(null);
  const [uiStreamHealthy, setUiStreamHealthy] = useState(false);
  const [refreshTick, setRefreshTick] = useState(0);
  const [hiddenMonitorIds, setHiddenMonitorIds] = useState<Set<string>>(
    loadHiddenMonitorIds,
  );
  const [hardwareAssetCatalog, setHardwareAssetCatalog] =
    useState<HardwareAssetCatalogState>({
      assets: [],
      installed: [],
      loading: true,
      error: null,
    });
  const [selectedHardwareAssetIds, setSelectedHardwareAssetIds] =
    useState<Record<HardwareRigKind, string>>(loadSelectedHardwareAssetIds);
  const endpointSequencesRef = useRef<Record<string, number>>({});
  const uiStreamHealthyRef = useRef(false);

  const model = buildDesktopViewModel(
    payload,
    localControls ? { display: localControls.display } : null,
  );
  const layoutDevices = getLayoutDevices(model.layout.devices);
  const layoutMonitors = getLayoutMonitors(model.layout.monitors, hiddenMonitorIds);
  const isDark = themeMode === "system" ? systemPrefersDark : themeMode === "dark";
  const theme = getDesktopTheme(isDark);
  const chrome = buildPageChrome(page, theme);
  const footerStatus = buildFooterStatus(model);
  const headerMetrics = getHeaderMetrics();
  const endpointIds = [
    typeof payload.status === "object" &&
    payload.status &&
    "device_id" in payload.status
      ? String((payload.status as { device_id?: unknown }).device_id ?? "")
      : "",
    ...safeArray(payload.devices).map((device) => device.id),
  ].filter((id, index, values) => id && values.indexOf(id) === index);
  const endpointPollKey = endpointIds.join("|");

  async function refreshDashboard(expectedUiVersion?: number) {
    const expectedVersion =
      expectedUiVersion ?? uiStateStore.currentVersion();
    try {
      const snapshot = await invokeCommand<DashboardPayload>("dashboard_state");
      if (expectedVersion !== uiStateStore.currentVersion()) {
        return;
      }
      uiStateStore.applyDashboardSnapshot(snapshot, expectedVersion);
      setError(snapshot.layout_error ? `布局异常：${snapshot.layout_error}` : null);
    } catch (refreshError) {
      if (expectedVersion !== uiStateStore.currentVersion()) {
        return;
      }
      setError(errorMessage(refreshError));
    }
  }

  async function refreshLocalControls() {
    try {
      const snapshot = await invokeCommand<LocalControlsSnapshot>("local_controls_state");
      setLocalControls((current) => mergeLocalControlSnapshot(current, snapshot));
      setLocalControlsError(null);
    } catch (localError) {
      setLocalControlsError(errorMessage(localError));
    }
  }

  async function refreshMobileAccess() {
    try {
      const snapshot = await invokeCommand<MobileAccessSnapshot>("mobile_access");
      setMobileAccess(snapshot);
      setMobileAccessError(null);
    } catch (mobileError) {
      setMobileAccess(null);
      setMobileAccessError(errorMessage(mobileError));
    }
  }

  async function refreshHardwareAssets() {
    setHardwareAssetCatalog((current) => ({ ...current, loading: true, error: null }));
    try {
      const builtinAssets = await loadBuiltinHardwareRigAssets();
      const installedResult = await loadInstalledHardwareRigAssets();
      setHardwareAssetCatalog({
        assets: [...builtinAssets, ...installedResult.assets],
        installed: installedResult.installed,
        loading: false,
        error: installedResult.error,
      });
    } catch (assetError) {
      setHardwareAssetCatalog((current) => ({
        ...current,
        loading: false,
        error: String(assetError),
      }));
    }
  }

  async function refreshAll() {
    await refreshDashboard();
    setRefreshTick((value) => value + 1);
    void refreshLocalControls();
    void refreshMobileAccess();
    void refreshHardwareAssets();
  }

  function setSelectedHardwareAssetId(kind: HardwareRigKind, assetId: string) {
    setSelectedHardwareAssetIds((current) => ({
      ...current,
      [kind]: assetId,
    }));
  }

  async function importHardwareAssetFile(file: File) {
    const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
    await invokeCommand<InstalledHardwareAsset>("import_hardware_asset", { bytes });
    await refreshHardwareAssets();
  }

  async function exportHardwareAssetFile(assetId: string) {
    const bytes = await invokeCommand<number[]>("export_hardware_asset", { assetId });
    const asset =
      hardwareAssetCatalog.assets.find((item) => item.id === assetId) ??
      hardwareAssetCatalog.installed.find((item) => item.id === assetId);
    downloadHardwareAssetPackage(
      new Uint8Array(bytes),
      `${safeDownloadName(asset?.name ?? assetId)}.rshare-asset.zip`,
    );
  }

  const hardwareAssetContext: HardwareAssetContextValue = {
    assets: hardwareAssetCatalog.assets,
    installed: hardwareAssetCatalog.installed,
    selectedIds: selectedHardwareAssetIds,
    loading: hardwareAssetCatalog.loading,
    error: hardwareAssetCatalog.error,
    setSelectedId: setSelectedHardwareAssetId,
    refresh: refreshHardwareAssets,
    importFile: importHardwareAssetFile,
    exportAsset: exportHardwareAssetFile,
  };

  useEffect(() => {
    let cancelled = false;
    const bindings = createUiStateAppBindings({
      store: uiStateStore,
      loadFallbackSnapshot: () =>
        invokeCommand<DashboardPayload>("dashboard_state"),
      onFallbackApplied: (snapshot: DashboardPayload) => {
        setError(
          snapshot.layout_error ? `布局异常：${snapshot.layout_error}` : null,
        );
      },
    });
    const client = new UiStateClient({
      connect: connectUiStateTransport,
      onEnvelope: (envelope: UiStateEnvelope) => {
        bindings.onEnvelope(envelope);
        if (envelope.type === "snapshot" || envelope.type === "delta") {
          if (
            envelope.type === "snapshot" ||
            (isRecord(envelope.payload.change) &&
              envelope.payload.change.type === "topology")
          ) {
            setError(null);
          }
        }
      },
      onStatus: (status: { state?: string }) => {
        const healthy = status.state === "healthy";
        uiStreamHealthyRef.current = healthy;
        if (!cancelled) {
          setUiStreamHealthy(healthy);
        }
        return bindings.onStatus(status);
      },
    });

    void client.start();
    return () => {
      cancelled = true;
      void client.stop();
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    const expectedUiVersion = uiStateStore.currentVersion();
    refreshDashboard(expectedUiVersion).finally(() => {
      if (!cancelled) {
        if (!uiStreamHealthyRef.current) {
          void refreshLocalControls();
        }
        void refreshMobileAccess();
      }
    });
    refreshHardwareAssets();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }
    window.localStorage.setItem(
      HARDWARE_ASSET_KEYBOARD_STORAGE_KEY,
      selectedHardwareAssetIds.keyboard,
    );
    window.localStorage.setItem(
      HARDWARE_ASSET_MOUSE_STORAGE_KEY,
      selectedHardwareAssetIds.mouse,
    );
    window.localStorage.setItem(
      HARDWARE_ASSET_GAMEPAD_STORAGE_KEY,
      selectedHardwareAssetIds.gamepad,
    );
  }, [selectedHardwareAssetIds]);

  useEffect(() => {
    if (uiStreamHealthy) {
      return;
    }
    let cancelled = false;
    let subscription: LocalControlSubscription | null = null;
    let streamLease: any = null;
    let flushTimer: number | null = null;
    const pendingEvents: LocalControlEvent[] = [];

    const clearFlushTimer = () => {
      if (flushTimer !== null) {
        window.clearTimeout(flushTimer);
        flushTimer = null;
      }
    };

    const drainPendingEvents = () => {
      const events = pendingEvents.splice(0, pendingEvents.length);
      return events;
    };

    const applyPendingEvents = () => {
      flushTimer = null;
      const events = drainPendingEvents();
      if (!events.length) {
        return;
      }
      setLocalControls((current) => {
        if (!current) {
          pendingEvents.unshift(...events);
          return current;
        }
        return events.reduce(
          (next, event) => applyLocalControlEvent(next, event),
          current,
        );
      });
      setLocalControlsError(null);
    };

    const scheduleEventFlush = () => {
      if (flushTimer !== null) {
        return;
      }
      flushTimer = window.setTimeout(applyPendingEvents, LOCAL_CONTROL_EVENT_FLUSH_MS);
    };

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
            clearFlushTimer();
            const queuedEvents = drainPendingEvents();
            setLocalControls((current) => {
              const next = mergeLocalControlSnapshot(current, response.LocalControls!);
              return queuedEvents.reduce(
                (snapshot, event) => applyLocalControlEvent(snapshot, event),
                next,
              );
            });
            setLocalControlsError(null);
          } else if (response.LocalControlEvent) {
            pendingEvents.push(response.LocalControlEvent);
            scheduleEventFlush();
          }
        });

        if (cancelled) {
          nextSubscription?.stop();
          return;
        }

        subscription = nextSubscription;
        if (subscription?.usesTauriBridge) {
          streamLease = localControlsStreamCoordinator.acquire();
          await streamLease.ready;
        }
      } catch (streamError) {
        if (!cancelled) {
          setLocalControlsError(errorMessage(streamError));
        }
      }
    }

    startStream();
    return () => {
      cancelled = true;
      clearFlushTimer();
      const usesTauriBridge = subscription?.usesTauriBridge ?? false;
      subscription?.stop();
      if (usesTauriBridge && streamLease) {
        localControlsStreamCoordinator.release(streamLease).catch(() => {});
      }
    };
  }, [uiStreamHealthy]);

  useEffect(() => {
    if (uiStreamHealthy) {
      return;
    }
    if (!endpointIds.length) {
      return;
    }

    let cancelled = false;
    let subscription: LocalControlSubscription | null = null;
    let streamLease: any = null;

    const rememberSequences = (events: EndpointEvent[]) => {
      for (const event of events) {
        const endpointId = event.endpoint_id;
        if (!endpointId) {
          continue;
        }
        endpointSequencesRef.current[endpointId] = Math.max(
          endpointSequencesRef.current[endpointId] ?? 0,
          Number(event.sequence ?? 0),
        );
      }
    };

    const applyEvents = (events: EndpointEvent[]) => {
      if (!events.length) {
        return;
      }
      rememberSequences(events);
      setLocalControls((current) => applyEndpointEvents(current, events));
      setLocalControlsError(null);
    };

    const handleEndpointPayload = (payload: unknown) => {
      if (typeof payload === "string") {
        setLocalControlsError(payload);
        return;
      }
      const response = payload as {
        EndpointEvents?: EndpointEvent[];
        EndpointEvent?: EndpointEvent;
      };
      if (response.EndpointEvents) {
        applyEvents(response.EndpointEvents);
      } else if (response.EndpointEvent) {
        applyEvents([response.EndpointEvent]);
      }
    };

    async function startEndpointStream() {
      subscription = await listenEndpointEvent(handleEndpointPayload);
      if (cancelled) {
        subscription?.stop();
        return;
      }
      if (subscription?.usesTauriBridge) {
        streamLease = endpointEventsStreamCoordinator.acquire();
        await streamLease.ready;
      }
    }

    startEndpointStream().catch((streamError) => {
      if (!cancelled) {
        setLocalControlsError(errorMessage(streamError));
      }
    });

    return () => {
      cancelled = true;
      const usesTauriBridge = subscription?.usesTauriBridge ?? false;
      subscription?.stop();
      if (usesTauriBridge && streamLease) {
        endpointEventsStreamCoordinator.release(streamLease).catch(() => {});
      }
    };
  }, [endpointPollKey, uiStreamHealthy]);

  useEffect(() => {
    if (uiStreamHealthy) {
      return;
    }
    if (typeof navigator.getGamepads !== "function") {
      return;
    }

    const timer = window.setInterval(() => {
      setLocalControls((current) => mergeBrowserGamepadState(current));
    }, 50);

    return () => window.clearInterval(timer);
  }, [uiStreamHealthy]);

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
      setError(errorMessage(actionError));
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
      setError(errorMessage(actionError));
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
      setError(errorMessage(actionError));
    } finally {
      setBusy(false);
    }
  }

  async function runEndpointInputTest(kind: string, remoteDeviceId?: string) {
    if (confirmingInputTest !== kind) {
      setConfirmingInputTest(kind);
      return;
    }

    setBusy(true);
    setConfirmingInputTest(null);
    try {
      const results: EndpointInjectResult[] = [];
      const currentUiState = uiStateStore.getState();
      const effectiveLocalControls = projectUiInputToLocalControls(
        currentUiState.inputVisuals,
        currentUiState.topology,
        localControls,
        { authoritative: currentUiState.bootId !== null },
      ) as LocalControlsSnapshot;
      for (const request of endpointInjectRequests(kind, effectiveLocalControls)) {
        const result = await invokeCommand<EndpointInjectResult>("inject_endpoint_event", {
          target: endpointInjectTarget(remoteDeviceId),
          request,
        });
        results.push(result);
        if (!result.accepted) {
          break;
        }
      }
      setLocalInputTestResult(
        endpointInjectResultsToLocalInputTestResult(results, {
          kind,
          targetId: remoteDeviceId ?? null,
        }),
      );
      await refreshLocalControls();
    } catch (testError) {
      setLocalInputTestResult({
        status: "Failed",
        message: errorMessage(testError),
        kind,
        targetId: remoteDeviceId ?? null,
      });
    } finally {
      setBusy(false);
    }
  }

  async function runRemoteEndpointInputTest(deviceId: string, kind: string) {
    await runEndpointInputTest(kind, deviceId);
  }

  async function runRemoteLatencyProbe(deviceId: string) {
    setBusy(true);
    try {
      const result = await invokeCommand<LocalInputTestResult>("run_remote_latency_test", {
        device_id: deviceId,
      });
      setRemoteLatencyTestResult({
        ...result,
        targetId: deviceId,
      });
      await refreshLocalControls();
    } catch (latencyError) {
      setRemoteLatencyTestResult({
        status: "Failed",
        message: errorMessage(latencyError),
        targetId: deviceId,
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
      setError(null);
      await refreshDashboard();
    } catch (layoutSaveError) {
      const message = `布局未保存：${errorMessage(layoutSaveError)}`;
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

  async function handleMonitorContextAction(actionId: string, monitor: MonitorData) {
    if (
      actionId !== "open-display-settings" &&
      !actionId.startsWith("change-")
    ) {
      return;
    }

    try {
      await invokeCommand("open_display_settings", {
        display_id: monitor.displayId ?? monitor.id,
      });
      setError(null);
    } catch (displaySettingsError) {
      setError(errorMessage(displaySettingsError));
    }
  }

  async function handleWindow(command: "minimize_window" | "toggle_maximize_window" | "close_window") {
    try {
      await invokeCommand(command);
    } catch (windowError) {
      setError(errorMessage(windowError));
    }
  }

  return (
    <HardwareAssetContext.Provider value={hardwareAssetContext}>
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
        {performanceProbesEnabled ? (
          <>
            <TopologyCommitProbe />
            <PaintCommitProbe />
          </>
        ) : null}
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
              onMonitorContextAction={handleMonitorContextAction}
            />
          ) : null}

          {page === "devices" ? (
            <DevicesPage
              busy={busy}
              devices={model.devices}
              capabilities={model.capabilities}
              visibleLayout={model.layout.visible}
              localDevice={model.settings.localDevice}
              latencyFeedback={model.latencyFeedback}
              localControls={localControls}
              localControlsError={localControlsError}
              localInputTestResult={localInputTestResult}
              remoteLatencyTestResult={remoteLatencyTestResult}
              confirmingInputTest={confirmingInputTest}
              onRunLocalInputTest={runEndpointInputTest}
              onRunRemoteEndpointInputTest={runRemoteEndpointInputTest}
              onRunRemoteLatencyTest={runRemoteLatencyProbe}
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
              mobileAccess={mobileAccess}
              mobileAccessError={mobileAccessError}
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
    </HardwareAssetContext.Provider>
  );
}

function DevicesPage({
  devices,
  capabilities,
  visibleLayout,
  localDevice,
  latencyFeedback,
  localControls,
  localControlsError,
  localInputTestResult,
  remoteLatencyTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
  onRunRemoteEndpointInputTest,
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
    ipAddress?: string;
    connected: boolean;
    online: boolean;
    lastSeenLabel: string;
  }>;
  capabilities: CapabilityOverview;
  visibleLayout: unknown | null;
  localDevice: {
    id: string;
    name: string;
    hostname: string;
  };
  latencyFeedback: unknown | null;
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  remoteLatencyTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onRunRemoteEndpointInputTest: (deviceId: string, kind: string) => void;
  onRunRemoteLatencyTest: (deviceId: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const inputVisuals = useUiStore(selectInputVisuals);
  const topology = useUiStore(selectTopologyProjection);
  const liveDiagnostics = useUiStore(selectDiagnostics);
  const hasAuthoritativeSnapshot = useUiStore(selectHasAuthoritativeSnapshot);
  const effectiveLocalControls = projectUiInputToLocalControls(
    inputVisuals,
    topology,
    localControls,
    { authoritative: hasAuthoritativeSnapshot },
  ) as LocalControlsSnapshot;

  return (
    <DevicesPageWithLocalControls
      devices={devices}
      capabilities={capabilities}
      visibleLayout={visibleLayout}
      localDevice={localDevice}
      latencyFeedback={liveDiagnostics ?? latencyFeedback}
      localControls={effectiveLocalControls}
      localControlsError={localControlsError}
      localInputTestResult={localInputTestResult}
      remoteLatencyTestResult={remoteLatencyTestResult}
      confirmingInputTest={confirmingInputTest}
      onRunLocalInputTest={onRunLocalInputTest}
      onRunRemoteEndpointInputTest={onRunRemoteEndpointInputTest}
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
            <InfoRow label="IP" value={device.ipAddress ?? device.address} theme={theme} />
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
      className="flex h-8 w-full shrink-0 items-center gap-1.5 rounded-md px-2.5 text-left text-xs transition"
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
      <span className="flex min-w-0 items-baseline gap-1.5 leading-none">
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

function DeviceTreeNodeButton({
  active,
  icon,
  title,
  detail,
  badge,
  live,
  onClick,
  theme,
}: {
  active: boolean;
  icon: ReactNode;
  title: string;
  detail: string;
  badge?: string;
  live: boolean;
  onClick: () => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <button
      type="button"
      className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-xs transition"
      style={{
        border: `1px solid ${active ? theme.accent : theme.border}`,
        background: active ? theme.accentSoft : theme.frame,
        color: active ? theme.text : theme.textSub,
      }}
      onClick={onClick}
      title={fullDeviceTooltip(title, detail)}
    >
      <span
        className="flex h-6 w-6 shrink-0 items-center justify-center"
        style={{ color: active ? theme.accent : theme.textMuted }}
      >
        {icon}
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="block min-w-0 truncate font-medium">{title}</span>
          {badge ? (
            <span
              className="shrink-0 rounded px-1.5 py-0.5 text-[10px]"
              style={{ background: theme.accentSoft, color: theme.accent }}
            >
              {badge}
            </span>
          ) : null}
        </span>
        <span className="block truncate text-[10px]" style={{ color: theme.textMuted }}>
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
  capabilities,
  visibleLayout,
  localDevice,
  latencyFeedback,
  localControls,
  localControlsError,
  localInputTestResult,
  remoteLatencyTestResult,
  confirmingInputTest,
  onRunLocalInputTest,
  onRunRemoteEndpointInputTest,
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
    ipAddress?: string;
    connected: boolean;
    online: boolean;
    lastSeenLabel: string;
  }>;
  capabilities: CapabilityOverview;
  visibleLayout: unknown | null;
  localDevice: {
    id: string;
    name: string;
    hostname: string;
  };
  latencyFeedback: unknown | null;
  localControls: LocalControlsSnapshot | null;
  localControlsError: string | null;
  localInputTestResult: LocalInputTestResult | null;
  remoteLatencyTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunLocalInputTest: (kind: string) => void;
  onRunRemoteEndpointInputTest: (deviceId: string, kind: string) => void;
  onRunRemoteLatencyTest: (deviceId: string) => void;
  onConnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  busy: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const [selectedPage, setSelectedPage] = useState<LocalDevicePageKind>("overview");
  const [selectedDeviceIds, setSelectedDeviceIds] = useState<Record<string, string>>({});
  const [selectedMonitorDeviceId, setSelectedMonitorDeviceId] = useState("local");
  const [expandedDeviceKinds, setExpandedDeviceKinds] = useState<Record<string, boolean>>({});
  const [localTreeExpanded, setLocalTreeExpanded] = useState(true);
  const [expandedRemoteDevices, setExpandedRemoteDevices] = useState<Record<string, boolean>>({});
  const [browserAudioOutputs, setBrowserAudioOutputs] = useState<AudioOutputDevice[]>([]);
  const { selectedIds } = useHardwareAssetCatalog();
  const hardwareRigVariant = hardwareRigVariantForManifest(selectedIds.keyboard);

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
  const endpointAcceptance = buildEndpointAcceptance(
    localControls,
    safeDevices,
    localInputTestResult,
  );
  const selectedRemoteDevice =
    selectedMonitorDeviceId === "local"
      ? null
      : safeDevices.find((device) => device.id === selectedMonitorDeviceId) ?? null;
  const remoteMonitorSnapshotFor = (device: (typeof safeDevices)[number]) =>
    buildRemoteControlSnapshot({
      baseSnapshot: localControls,
      device,
      capabilities,
      visibleLayout,
    }) as LocalControlsSnapshot;
  const latencyFeedbackSnapshot =
    latencyFeedback == null
      ? localControls
      : ({
          ...(localControls ?? {}),
          latency_feedback: latencyFeedback,
        } as LocalControlsSnapshot);
  const selectedRemoteLatency = selectedRemoteDevice
    ? (buildRemoteLatencySummary(
        latencyFeedbackSnapshot,
        selectedRemoteDevice.id,
      ) as RemoteLatencySummary)
    : null;
  const scopedRemoteLatencyTestResult =
    remoteLatencyTestResult && selectedRemoteDevice?.id === remoteLatencyTestResult.targetId
      ? remoteLatencyTestResult
      : null;
  const remoteDeviceIds = new Set(safeDevices.map((device) => device.id));
  const monitorSnapshot = selectedRemoteDevice
    ? remoteMonitorSnapshotFor(selectedRemoteDevice)
    : buildLocalMonitorSnapshot(localControls, remoteDeviceIds);
  const monitorError = selectedRemoteDevice ? null : localControlsError;
  const handleMonitorDeviceChange = (deviceId: string) => {
    setSelectedMonitorDeviceId(deviceId);
    const selected = safeDevices.find((device) => device.id === deviceId);
    if (selected && !selected.connected && !busy) {
      onConnect(selected.id);
    }
  };

  useEffect(() => {
    if (
      selectedMonitorDeviceId !== "local" &&
      !safeDevices.some((device) => device.id === selectedMonitorDeviceId)
    ) {
      setSelectedMonitorDeviceId("local");
    }
  }, [safeDevices, selectedMonitorDeviceId]);

  const selectedDeviceId =
    selectedPage === "overview" || selectedPage === "remote"
      ? undefined
      : selectedDeviceIds[selectedPage];
  const setSelectedDeviceId = (kind: LocalControlKind, deviceId: string) => {
    setSelectedDeviceIds((current) => ({
      ...current,
      [kind]: deviceId,
    }));
  };
  const counts = {
    keyboard: localInputDeviceCount(monitorSnapshot, "keyboard"),
    mouse: localInputDeviceCount(monitorSnapshot, "mouse"),
    gamepad: Math.max(1, monitorSnapshot?.gamepads?.length ?? 0),
    display: Math.max(1, monitorSnapshot?.display.display_count ?? 0),
    audio: Math.max(1, audioInputs.length + audioOutputs.length),
    remote: safeDevices.length,
  };
  const deviceTypeTabs = buildDeviceTypeSummaries(counts);
  const controlTreeTabs = deviceTypeTabs.filter(
    (tab) => tab.kind !== "remote",
  );
  const tabIcons: Record<LocalDevicePageKind, ReactNode> = {
    overview: <LayoutGrid size={16} />,
    keyboard: <Keyboard size={16} />,
    mouse: <MousePointer2 size={16} />,
    gamepad: <Gamepad2 size={16} />,
    display: <HardDrive size={16} />,
    audio: <Volume2 size={16} />,
    remote: <Monitor size={16} />,
  };
  const tabLive: Record<LocalDevicePageKind, boolean> = {
    overview: Boolean(monitorSnapshot),
    keyboard: Boolean(localControls?.keyboard.detected),
    mouse: Boolean(localControls?.mouse.detected),
    gamepad: Boolean(localControls?.gamepads?.some((item) => item.connected)),
    display: (localControls?.display.display_count ?? 0) > 0,
    audio: audioInputs.length > 0 || audioOutputs.length > 0,
    remote: safeDevices.some((device) => device.connected),
  };
  const toggleDeviceKind = (kind: LocalDevicePageKind) => {
    setExpandedDeviceKinds((current) => ({
      ...current,
      [kind]: !current[kind],
    }));
  };
  const toggleRemoteDevice = (deviceId: string) => {
    setExpandedRemoteDevices((current) => ({
      ...current,
      [deviceId]: !current[deviceId],
    }));
  };
  const selectLocalRoot = () => {
    setSelectedMonitorDeviceId("local");
    setSelectedPage("overview");
  };
  const selectLocalKind = (kind: LocalDevicePageKind) => {
    setSelectedMonitorDeviceId("local");
    setSelectedPage(kind);
    if (kind !== "remote") {
      const aggregate = localDeviceItems(monitorSnapshot, kind as LocalControlKind, audioOutputs)[0];
      if (aggregate) {
        setSelectedDeviceId(kind as LocalControlKind, aggregate.id);
      }
    }
  };
  const selectRemoteKind = (deviceId: string, kind: LocalDevicePageKind) => {
    handleMonitorDeviceChange(deviceId);
    setSelectedPage(kind);
    if (kind !== "remote") {
      const selected = safeDevices.find((device) => device.id === deviceId);
      if (!selected) {
        return;
      }
      const remoteSnapshot = remoteMonitorSnapshotFor(selected);
      const aggregate = localDeviceItems(remoteSnapshot, kind as LocalControlKind, [])[0];
      if (aggregate) {
        setSelectedDeviceId(kind as LocalControlKind, aggregate.id);
      }
    }
  };
  const selectedCapabilityDevice = capabilities?.devices?.find((device) =>
    selectedMonitorDeviceId === "local"
      ? device.local
      : device.id === selectedMonitorDeviceId,
  );
  const capabilityChips = selectedCapabilityDevice?.capabilities?.slice(0, 5) ?? [];
  const [deviceConsoleRef, deviceConsoleSize] = useElementSize<HTMLDivElement>();
  const compactDeviceConsole = deviceConsoleSize.width > 0 && deviceConsoleSize.width < 980;

  return (
    <div
      ref={deviceConsoleRef}
      className="flex h-full min-h-0 overflow-hidden"
    >
      <div
        className="flex w-[250px] shrink-0 flex-col overflow-hidden"
        style={{
          borderRight: `1px solid ${theme.border}`,
          background: theme.sidebar,
        }}
      >
        <div
          className="rshare-scroll flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto py-3 pr-3 pl-0"
          role="tree"
          aria-label="设备树"
        >
          <div className="mb-1 px-4">
            <div className="text-[11px] uppercase tracking-[0.16em]" style={{ color: theme.textMuted }}>
              Device Console
            </div>
            <div className="mt-1 text-sm font-semibold">设备控制</div>
            {capabilities?.available ? (
              <div className="mt-2 flex flex-wrap gap-1">
                {capabilityChips.map((capability) => (
                  <span
                    key={capability.kind}
                    className="rounded px-1.5 py-0.5 text-[10px]"
                    title={capability.reason ?? `${capability.label}: ${capability.stateLabel}`}
                    style={{
                      border: `1px solid ${theme.border}`,
                      background:
                        capability.state === "Available"
                          ? "rgba(73,179,92,0.13)"
                          : capability.state === "Experimental"
                            ? "rgba(214,166,75,0.16)"
                            : capability.state === "Degraded"
                              ? "rgba(214,166,75,0.10)"
                              : "rgba(255,255,255,0.035)",
                      color:
                        capability.state === "Available"
                          ? theme.success
                          : capability.state === "Unavailable"
                            ? theme.textMuted
                            : theme.textSub,
                    }}
                  >
                    {capability.label} · {capability.stateLabel}
                  </span>
                ))}
              </div>
            ) : null}
          </div>

          <div className="flex flex-col gap-1">
            <div className="flex items-center gap-1">
              <button
                type="button"
                className="flex h-8 w-6 shrink-0 items-center justify-center rounded"
                style={{ color: theme.textMuted }}
                onClick={(event) => {
                  event.stopPropagation();
                  setLocalTreeExpanded((value) => !value);
                }}
                title={localTreeExpanded ? "收起本机" : "展开本机"}
              >
                {localTreeExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </button>
              <DeviceTreeNodeButton
                active={selectedMonitorDeviceId === "local" && selectedPage === "overview"}
                icon={<Monitor size={16} />}
                title={localDevice.name}
                detail={localDevice.hostname || "本机"}
                badge="本机"
                live={Boolean(monitorSnapshot)}
                onClick={selectLocalRoot}
                theme={theme}
              />
            </div>
            {localTreeExpanded ? (
              <div className="ml-2 flex flex-col gap-1">
                {controlTreeTabs.map((tab) => {
            const kind = tab.kind as LocalControlKind;
            const expanded = Boolean(expandedDeviceKinds[kind]);
            const children =
              localDeviceItems(monitorSnapshot, kind as LocalControlKind, audioOutputs)
                .filter((device) => device.live)
                .slice(1);
            return (
              <div key={kind} className="flex flex-col gap-1">
                <div className="flex items-center gap-1">
                  <button
                    type="button"
                    className="flex h-8 w-6 shrink-0 items-center justify-center rounded"
                    style={{
                      color: children.length ? theme.textMuted : "transparent",
                    }}
                    disabled={!children.length}
                    onClick={(event) => {
                      event.stopPropagation();
                      toggleDeviceKind(kind);
                    }}
                    title={expanded ? "收起" : "展开"}
                  >
                    {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                  </button>
                  <LocalControlTypeButton
                    kind={kind}
                    active={selectedMonitorDeviceId === "local" && selectedPage === kind}
                    icon={tabIcons[kind]}
                    title={tab.title}
                    detail={tab.detail}
                    live={tabLive[kind]}
                    onClick={selectLocalKind}
                    theme={theme}
                  />
                </div>
                {expanded && children.length ? (
                  <div className="ml-3 flex flex-col gap-1">
                    {children.map((device) => (
                      <button
                        key={device.id}
                        type="button"
                        className="truncate rounded-md px-2 py-1.5 text-left text-xs"
                        style={{
                          border: `1px solid ${selectedDeviceIds[kind] === device.id ? theme.accent : theme.border}`,
                          background:
                            selectedDeviceIds[kind] === device.id
                              ? theme.accentSoft
                              : "rgba(255,255,255,0.035)",
                          color: theme.textSub,
                        }}
                        onClick={() => {
                          setSelectedMonitorDeviceId("local");
                          setSelectedPage(kind);
                          setSelectedDeviceId(kind as LocalControlKind, device.id);
                        }}
                        title={fullDeviceTooltip(device.name, device.detail)}
                      >
                        <span
                          className="mr-2 inline-block h-2 w-2 rounded-full"
                          style={{ background: device.live ? theme.success : theme.textMuted }}
                        />
                        {device.name}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            );
                })}
              </div>
            ) : null}
          </div>

          {safeDevices.map((device) => {
            const expanded = Boolean(expandedRemoteDevices[device.id]);
            const remoteSnapshot = remoteMonitorSnapshotFor(device);
            const remoteCounts = deviceTreeCounts(remoteSnapshot, []);
            const remoteTabs = buildDeviceTypeSummaries(remoteCounts).filter(
              (tab) => tab.kind !== "remote",
            );
            return (
              <div key={device.id} className="flex flex-col gap-1">
                <div className="flex items-center gap-1">
                  <button
                    type="button"
                    className="flex h-8 w-6 shrink-0 items-center justify-center rounded"
                    style={{ color: theme.textMuted }}
                    onClick={(event) => {
                      event.stopPropagation();
                      toggleRemoteDevice(device.id);
                    }}
                    title={expanded ? "收起局域网设备" : "展开局域网设备"}
                  >
                    {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                  </button>
                  <DeviceTreeNodeButton
                    active={selectedMonitorDeviceId === device.id && selectedPage === "overview"}
                    icon={<Monitor size={16} />}
                    title={device.name}
                    detail={device.ipAddress || device.address || device.hostname || (device.connected ? "已连接" : "已发现")}
                    live={device.connected}
                    onClick={() => {
                      handleMonitorDeviceChange(device.id);
                      setSelectedPage("overview");
                    }}
                    theme={theme}
                  />
                </div>
                {expanded ? (
                  <div className="ml-2 flex flex-col gap-1">
                    {remoteTabs.map((tab) => {
                      const kind = tab.kind as LocalControlKind;
                      return (
                        <div key={`${device.id}-${kind}`} className="flex items-center gap-1">
                          <span className="h-8 w-6 shrink-0" />
                          <LocalControlTypeButton
                            kind={kind}
                            active={selectedMonitorDeviceId === device.id && selectedPage === kind}
                            icon={tabIcons[kind]}
                            title={tab.title}
                            detail={tab.detail}
                            live={remoteCounts[kind] > 0 || device.connected}
                            onClick={() => selectRemoteKind(device.id, kind)}
                            theme={theme}
                          />
                        </div>
                      );
                    })}
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      </div>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {DEVICE_CONSOLE_SECTIONS.endpointAcceptance ? (
          <EndpointAcceptanceStrip acceptance={endpointAcceptance} theme={theme} />
        ) : null}
        {selectedRemoteDevice ? (
          <RemoteLatencyPanel
            device={selectedRemoteDevice}
            summary={selectedRemoteLatency}
            result={scopedRemoteLatencyTestResult}
            busy={busy}
            onRun={() => onRunRemoteLatencyTest(selectedRemoteDevice.id)}
            theme={theme}
          />
        ) : null}
        <div className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {selectedPage === "overview" ? (
            <AllDevicesOverview
              snapshot={monitorSnapshot}
              audioOutputs={selectedRemoteDevice ? [] : audioOutputs}
              remoteDevices={selectedRemoteDevice ? [] : safeDevices}
              error={monitorError}
              hardwareRigVariant={hardwareRigVariant}
              theme={theme}
            />
          ) : (
            <LocalControlDriverHub
              snapshot={monitorSnapshot}
              error={monitorError}
              inputTestResult={localInputTestResult}
              confirmingInputTest={confirmingInputTest}
              selectedKind={selectedPage === "remote" ? "keyboard" : selectedPage}
              remoteDevice={selectedRemoteDevice}
              selectedDeviceId={selectedDeviceId}
              audioOutputs={selectedRemoteDevice ? [] : audioOutputs}
              onSelectedKindChange={(kind) => setSelectedPage(kind)}
              onSelectedDeviceIdChange={(deviceId) =>
                selectedPage !== "remote"
                  ? setSelectedDeviceId(selectedPage as LocalControlKind, deviceId)
                  : undefined
              }
              onRunInputTest={
                selectedRemoteDevice
                  ? (kind) => onRunRemoteEndpointInputTest(selectedRemoteDevice.id, kind)
                  : onRunLocalInputTest
              }
              onRefreshLocalControls={refreshLocalControls}
              hardwareRigVariant={hardwareRigVariant}
              compactLayout={compactDeviceConsole}
              theme={theme}
            />
          )}
        </div>
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
        onRefreshLocalControls={refreshLocalControls}
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
                      {device.hostname} · {device.ipAddress ?? device.address}
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
                  <InfoRow label="IP" value={device.ipAddress ?? device.address} theme={theme} />
                  <InfoRow label="端点" value={device.address} theme={theme} />
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

function EndpointAcceptanceStrip({
  acceptance,
  theme,
}: {
  acceptance: {
    ready: boolean;
    checks: Array<{
      key: string;
      label: string;
      state: "pass" | "warn" | "block";
      detail: string;
    }>;
  };
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="flex shrink-0 items-center gap-2 overflow-hidden px-4 py-2 text-xs"
      style={{
        borderBottom: `1px solid ${theme.border}`,
        background: theme.toolbar,
      }}
    >
      <div className="mr-1 shrink-0 font-medium" style={{ color: theme.textSub }}>
        双机验收
      </div>
      <div className="rshare-scroll flex min-w-0 flex-1 gap-2 overflow-x-auto">
        {acceptance.checks.map((check) => (
          <div
            key={check.key}
            className="flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1"
            style={{
              border: `1px solid ${theme.border}`,
              background: theme.frame,
            }}
            title={check.detail}
          >
            <AcceptanceDot state={check.state} theme={theme} />
            <span style={{ color: theme.text }}>{check.label}</span>
            <AcceptanceBadge
              label={acceptanceStateLabel(check.state)}
              state={check.state}
              theme={theme}
            />
          </div>
        ))}
      </div>
      <span
        className="shrink-0 rounded px-2 py-1"
        style={{
          background: acceptance.ready ? "rgba(73, 179, 92, 0.16)" : "rgba(214, 166, 75, 0.14)",
          color: acceptance.ready ? "#8de29d" : "#f0c36b",
        }}
      >
        {acceptance.ready ? "可开始边缘切换" : "待完成端侧闭环"}
      </span>
    </div>
  );
}

function latencyMetricValue(value: number | null | undefined) {
  return value == null ? "-" : `${value} ms`;
}

function RemoteLatencyPanel({
  device,
  summary,
  result,
  busy,
  onRun,
  theme,
}: {
  device: {
    id: string;
    name: string;
    hostname: string;
    connected: boolean;
  };
  summary: RemoteLatencySummary | null;
  result: LocalInputTestResult | null;
  busy: boolean;
  onRun: () => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const state = summary?.state ?? "idle";
  const tone =
    state === "pass"
      ? theme.success
      : state === "pending" || state === "warn"
        ? "#d6a64b"
        : state === "fail"
          ? "#e56b6f"
          : theme.textMuted;
  return (
    <div
      className="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-4 py-2 text-xs"
      style={{
        borderBottom: `1px solid ${theme.border}`,
        background: theme.frame,
      }}
    >
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: tone }} />
          <span className="truncate font-medium" style={{ color: theme.text }}>
            {device.name} · 网络与端侧延时
          </span>
          <span className="shrink-0" style={{ color: theme.textMuted }}>
            {device.hostname}
          </span>
        </div>
        <div className="mt-1 flex flex-wrap gap-2" style={{ color: theme.textMuted }}>
          <span>RTT {latencyMetricValue(summary?.networkRoundTripMs)}</span>
          <span>单向约 {latencyMetricValue(summary?.estimatedOneWayMs)}</span>
          <span>原始 {latencyMetricValue(summary?.rawRoundTripMs)}</span>
          <span>远端处理 {latencyMetricValue(summary?.remoteProcessingMs)}</span>
          <span>{summary?.message ?? "尚未运行网络延时探测"}</span>
          {result ? <span>最近命令：{result.message}</span> : null}
        </div>
      </div>
      <button
        type="button"
        className="rounded-md px-3 py-2 text-xs transition"
        style={{
          border: `1px solid ${device.connected ? theme.accent : theme.border}`,
          background: device.connected ? theme.accentSoft : "rgba(255,255,255,0.035)",
          color: device.connected ? theme.text : theme.textMuted,
        }}
        disabled={busy || !device.connected}
        onClick={onRun}
        title={device.connected ? "发送 latency probe 到当前远端" : "先连接远端设备"}
      >
        网络延时探测
      </button>
    </div>
  );
}

function AllDevicesOverview({
  snapshot,
  audioOutputs,
  remoteDevices,
  error,
  hardwareRigVariant,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  audioOutputs: AudioOutputDevice[];
  remoteDevices: Array<{
    id: string;
    name: string;
    hostname: string;
    address: string;
    ipAddress?: string;
    connected: boolean;
    online: boolean;
    lastSeenLabel: string;
  }>;
  error: string | null;
  hardwareRigVariant: HardwareRigVariant;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const galleryItems = buildDeviceGalleryItems(snapshot, audioOutputs, remoteDevices);
  const canvasRef = useRef<HTMLDivElement | null>(null);
  const gestureScaleRef = useRef(1);
  const [zoom, setZoom] = useState(0.92);
  const [panOffset, setPanOffset] = useState({ x: 24, y: -8 });
  const [nodePositions, setNodePositions] = useState<Record<string, { x: number; y: number }>>({});
  const [draggingNode, setDraggingNode] = useState<{
    id: string;
    offsetX: number;
    offsetY: number;
  } | null>(null);
  const [panning, setPanning] = useState<{
    startX: number;
    startY: number;
    originX: number;
    originY: number;
  } | null>(null);
  const gallerySignature = galleryItems
    .map((item) => `${item.id}:${item.x}:${item.y}`)
    .join("|");
  const latestInputEvent = latestLocalControlEvent(snapshot);
  const positionedItems = galleryItems.map((item) => ({
    ...item,
    ...(nodePositions[item.id] ?? { x: item.x, y: item.y }),
  }));

  useEffect(() => {
    setNodePositions((previous) => {
      const next: Record<string, { x: number; y: number }> = {};
      for (const item of galleryItems) {
        next[item.id] = previous[item.id] ?? { x: item.x, y: item.y };
      }
      return next;
    });
  }, [gallerySignature]);

  useEffect(() => {
    if (!draggingNode && !panning) {
      return undefined;
    }

    const handleMove = (event: MouseEvent) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) {
        return;
      }
      if (draggingNode) {
        const x = (event.clientX - rect.left) / zoom - panOffset.x - draggingNode.offsetX;
        const y = (event.clientY - rect.top) / zoom - panOffset.y - draggingNode.offsetY;
        setNodePositions((previous) => ({
          ...previous,
          [draggingNode.id]: { x, y },
        }));
        return;
      }
      if (panning) {
        setPanOffset({
          x: panning.originX + (event.clientX - panning.startX) / zoom,
          y: panning.originY + (event.clientY - panning.startY) / zoom,
        });
      }
    };
    const handleUp = () => {
      setDraggingNode(null);
      setPanning(null);
    };

    window.addEventListener("mousemove", handleMove);
    window.addEventListener("mouseup", handleUp);
    return () => {
      window.removeEventListener("mousemove", handleMove);
      window.removeEventListener("mouseup", handleUp);
    };
  }, [draggingNode, panOffset, panning, zoom]);

  const beginNodeDrag = (
    event: React.MouseEvent,
    item: { id: string; x: number; y: number },
  ) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    setDraggingNode({
      id: item.id,
      offsetX: (event.clientX - rect.left) / zoom - panOffset.x - item.x,
      offsetY: (event.clientY - rect.top) / zoom - panOffset.y - item.y,
    });
  };

  const beginCanvasPan = (event: React.MouseEvent) => {
    if (event.button !== 0 && event.button !== 1 && event.button !== 2) {
      return;
    }
    event.preventDefault();
    setPanning({
      startX: event.clientX,
      startY: event.clientY,
      originX: panOffset.x,
      originY: panOffset.y,
    });
  };

  const zoomCanvasAtPoint = (clientX: number, clientY: number, deltaY: number) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) {
      return;
    }
    const nextZoom = clamp(zoom * (deltaY > 0 ? 0.9 : 1.1), 0.55, 1.75);
    const worldX = (clientX - rect.left) / zoom - panOffset.x;
    const worldY = (clientY - rect.top) / zoom - panOffset.y;
    setZoom(nextZoom);
    setPanOffset({
      x: (clientX - rect.left) / nextZoom - worldX,
      y: (clientY - rect.top) / nextZoom - worldY,
    });
  };

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return undefined;
    }
    const gesturePoint = (event: Event & { clientX?: number; clientY?: number }) => {
      const rect = canvas.getBoundingClientRect();
      return {
        clientX: typeof event.clientX === "number" ? event.clientX : rect.left + rect.width / 2,
        clientY: typeof event.clientY === "number" ? event.clientY : rect.top + rect.height / 2,
      };
    };
    const handleWheel = (event: WheelEvent) => {
      event.preventDefault();
      event.stopPropagation();
      zoomCanvasAtPoint(event.clientX, event.clientY, event.deltaY);
    };
    const handleGestureStart = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      const scale = Number((event as Event & { scale?: number }).scale ?? 1);
      gestureScaleRef.current = Number.isFinite(scale) && scale > 0 ? scale : 1;
    };
    const handleGestureChange = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      const scale = Number((event as Event & { scale?: number }).scale ?? 1);
      if (!Number.isFinite(scale) || scale <= 0) {
        return;
      }
      const previousScale = gestureScaleRef.current || 1;
      gestureScaleRef.current = scale;
      if (Math.abs(scale - previousScale) < 0.01) {
        return;
      }
      const point = gesturePoint(event as Event & { clientX?: number; clientY?: number });
      zoomCanvasAtPoint(point.clientX, point.clientY, scale > previousScale ? -120 : 120);
    };
    const handleGestureEnd = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      gestureScaleRef.current = 1;
    };
    canvas.addEventListener("wheel", handleWheel, { passive: false, capture: true });
    canvas.addEventListener("gesturestart", handleGestureStart, { passive: false });
    canvas.addEventListener("gesturechange", handleGestureChange, { passive: false });
    canvas.addEventListener("gestureend", handleGestureEnd, { passive: false });
    return () => {
      canvas.removeEventListener("wheel", handleWheel, true);
      canvas.removeEventListener("gesturestart", handleGestureStart);
      canvas.removeEventListener("gesturechange", handleGestureChange);
      canvas.removeEventListener("gestureend", handleGestureEnd);
    };
  }, [zoom, panOffset]);

  return (
    <section
      className="relative h-full min-h-0 overflow-hidden"
      style={{
        border: `1px solid ${theme.border}`,
        background: theme.canvas,
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
      <div
        ref={canvasRef}
        className="relative h-full min-h-[420px] overflow-hidden md:min-h-[520px] xl:min-h-[640px]"
        style={{
          cursor: panning ? "grabbing" : "grab",
          overscrollBehavior: "contain",
          touchAction: "none",
          backgroundImage: `radial-gradient(circle, ${theme.gridDot} 1px, transparent 1px)`,
          backgroundSize: `${28 * zoom}px ${28 * zoom}px`,
          backgroundPosition: `${panOffset.x * zoom}px ${panOffset.y * zoom}px`,
        }}
        onContextMenu={(event) => event.preventDefault()}
        onMouseDown={beginCanvasPan}
      >
        <div className="absolute left-6 top-5 z-10">
          <div className="text-xs uppercase tracking-[0.18em]" style={{ color: theme.textMuted }}>
            Device Simulator
          </div>
          <div className="mt-1 text-xl font-semibold">设备模拟台</div>
          <div className="mt-1 max-w-[520px] text-xs" style={{ color: theme.textMuted }}>
            以主显示器为中心模拟真实键盘、鼠标、音频与远端设备；滚轮缩放，拖拽空白处平移，拖拽设备调整位置。
          </div>
        </div>

        <div
          className="absolute right-6 top-5 z-10 rounded-md px-3 py-2 text-xs"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.toolbar,
            color: theme.textMuted,
          }}
        >
          <span>缩放 {Math.round(zoom * 100)}%</span>
          <span className="mx-2 opacity-50">·</span>
          <span>最新事件：{latestInputEvent?.summary ?? "等待输入"}</span>
        </div>

        <div
          className="absolute inset-0"
          style={{
            transform: `scale(${zoom}) translate(${panOffset.x}px, ${panOffset.y}px)`,
            transformOrigin: "0 0",
          }}
        >
          {positionedItems.length ? (
            positionedItems.map((item) => (
              <DeviceGalleryNode
                key={item.id}
                item={item}
                dragging={draggingNode?.id === item.id}
                onMouseDown={(event) => beginNodeDrag(event, item)}
                hardwareRigVariant={hardwareRigVariant}
                theme={theme}
              />
            ))
          ) : (
            <div
              className="absolute left-1/2 top-1/2 rounded-lg px-6 py-5 text-sm"
              style={{
                transform: "translate(-50%, -50%)",
                border: `1px solid ${theme.border}`,
                background: theme.sidebar,
                color: theme.textMuted,
              }}
            >
              暂无可展示设备，等待 daemon 返回本机控制快照。
            </div>
          )}
        </div>

        <div
          className="absolute bottom-3 left-4 z-10 rounded-md px-3 py-1.5 text-[11px]"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.toolbar,
            color: theme.textMuted,
          }}
        >
          拖拽设备 · 空白处拖拽平移 · 滚轮缩放
        </div>
      </div>
    </section>
  );
}

function DeviceGalleryNode({
  item,
  dragging,
  onMouseDown,
  hardwareRigVariant,
  theme,
}: {
  item: {
    id: string;
    kind: string;
    shape?: string;
    rigKind?: string | null;
    rigVariant?: string | null;
    title: string;
    detail: string;
    metric: string;
    live: boolean;
    activity?: Record<string, unknown>;
    x: number;
    y: number;
    w: number;
    h: number;
  };
  dragging: boolean;
  onMouseDown: (event: React.MouseEvent) => void;
  hardwareRigVariant: HardwareRigVariant;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const accent =
    item.kind === "remote" && !item.live
      ? theme.textMuted
      : item.kind === "audio"
        ? "#49b35c"
        : item.kind === "display"
          ? "#d6a64b"
          : theme.accent;
  return (
    <article
      className="absolute select-none transition-shadow"
      onMouseDown={onMouseDown}
      style={{
        left: item.x,
        top: item.y,
        width: item.w,
        height: item.h,
        cursor: dragging ? "grabbing" : "grab",
        zIndex: dragging ? 20 : item.kind === "display" ? 8 : 10,
        filter: dragging ? "drop-shadow(0 20px 34px rgba(0,0,0,0.35))" : "none",
      }}
      title={`${item.title} · ${item.detail}`}
    >
      <PhysicalDeviceShape
        item={item}
        accent={accent}
        hardwareRigVariant={hardwareRigVariant}
        theme={theme}
      />
    </article>
  );
}

type HardwareRigKind = "keyboard" | "mouse" | "gamepad";
type HardwareRigVariant = "office" | "gaming";

type HardwareRigLayerRole =
  | "base"
  | "keycaps"
  | "legendGlow"
  | "pressEffect"
  | "buttonPressEffect"
  | "wheelPulse";

type HardwareRigLayerDefinition = {
  id: string;
  role: string;
  render: "image" | "runtime";
  src?: string | null;
  opacity?: number;
};

type HardwareRigRegionShape =
  | {
      kind: "rect";
      x: number;
      y: number;
      w: number;
      h: number;
      radius?: number;
    }
  | {
      kind: "polygon";
      points: Array<{ x: number; y: number }>;
    };

type HardwareRigRegion = {
  id: string;
  label: string;
  action?: {
    kind?: string;
    codes?: string[];
    buttons?: string[];
  };
  shape: HardwareRigRegionShape;
};

type HardwareRigDefinition = {
  kind: HardwareRigKind;
  manifest: string;
  id?: string;
  name?: string;
  source?: "builtin" | "installed";
  readonly?: boolean;
  manifestPath?: string;
  folderPath?: string;
  baseSize: {
    width: number;
    height: number;
  };
  display: {
    compactWidth: number;
    compactHeight: number;
    fullWidth: number;
    fullHeight: number;
  };
  layers: HardwareRigLayerDefinition[];
  regions?: HardwareRigRegion[];
};

type HardwareRigActivity = {
  pressedKeys?: string[];
  lastKey?: string | null;
  keyboardEvents?: LocalControlEvent[];
  leftDown?: boolean;
  rightDown?: boolean;
  middleDown?: boolean;
  backDown?: boolean;
  forwardDown?: boolean;
  wheelActive?: boolean;
  wheelLabel?: string;
  recentButtons?: string[];
  pressedButtons?: string[];
  leftStickX?: number;
  leftStickY?: number;
  rightStickX?: number;
  rightStickY?: number;
  leftTrigger?: number;
  rightTrigger?: number;
};

type InstalledHardwareAsset = {
  id: string;
  name: string;
  kind: HardwareRigKind;
  manifestPath: string;
  folderPath: string;
  manifest?: Record<string, unknown>;
};

type HardwareAssetCatalogState = {
  assets: HardwareRigDefinition[];
  installed: InstalledHardwareAsset[];
  loading: boolean;
  error: string | null;
};

type HardwareAssetContextValue = HardwareAssetCatalogState & {
  selectedIds: Record<HardwareRigKind, string>;
  setSelectedId: (kind: HardwareRigKind, assetId: string) => void;
  refresh: () => Promise<void>;
  importFile: (file: File) => Promise<void>;
  exportAsset: (assetId: string) => Promise<void>;
};

const DEFAULT_HARDWARE_ASSET_CONTEXT: HardwareAssetContextValue = {
  assets: [],
  installed: [],
  loading: false,
  error: null,
  selectedIds: {
    keyboard: "builtin.keyboard.office",
    mouse: "builtin.mouse.office",
    gamepad: "builtin.gamepad.xbox",
  },
  setSelectedId: () => undefined,
  refresh: async () => undefined,
  importFile: async () => undefined,
  exportAsset: async () => undefined,
};

const HardwareAssetContext = createContext<HardwareAssetContextValue>(
  DEFAULT_HARDWARE_ASSET_CONTEXT,
);

function useHardwareAssetCatalog() {
  return useContext(HardwareAssetContext);
}

const HARDWARE_RIGS: Record<HardwareRigKind, Record<HardwareRigVariant, HardwareRigDefinition>> = {
  keyboard: {
    office: {
    kind: "keyboard",
    manifest: "/assets/hardware/live2d/keyboard/manifest.json",
    baseSize: {
      width: 1694,
      height: 544,
    },
    display: {
      compactWidth: 420,
      compactHeight: 150,
      fullWidth: 980,
      fullHeight: 300,
    },
    layers: [
      {
        id: "keyboard-base",
        role: "base",
        render: "image",
        src: "/assets/hardware/live2d/keyboard/base.png",
      },
      {
        id: "keyboard-keycap-hotspots",
        role: "keycaps",
        render: "runtime",
      },
      {
        id: "keyboard-legend-glow",
        role: "legendGlow",
        render: "runtime",
      },
      {
        id: "keyboard-press-effect",
        role: "pressEffect",
        render: "runtime",
      },
    ],
    },
    gaming: {
      kind: "keyboard",
      manifest: "/assets/hardware/live2d/keyboard/gaming/manifest.json",
      baseSize: {
        width: 1650,
        height: 547,
      },
      display: {
        compactWidth: 420,
        compactHeight: 150,
        fullWidth: 980,
        fullHeight: 300,
      },
      layers: [
        {
          id: "keyboard-gaming-base",
          role: "base",
          render: "image",
          src: "/assets/hardware/live2d/keyboard/gaming/base.png",
        },
        {
          id: "keyboard-gaming-keycap-hotspots",
          role: "keycaps",
          render: "runtime",
        },
        {
          id: "keyboard-gaming-legend-glow",
          role: "legendGlow",
          render: "runtime",
        },
        {
          id: "keyboard-gaming-press-effect",
          role: "pressEffect",
          render: "runtime",
        },
      ],
    },
  },
  mouse: {
    office: {
    kind: "mouse",
    manifest: "/assets/hardware/live2d/mouse/manifest.json",
    baseSize: {
      width: 575,
      height: 1109,
    },
    display: {
      compactWidth: 145,
      compactHeight: 245,
      fullWidth: 260,
      fullHeight: 420,
    },
    layers: [
      {
        id: "mouse-base",
        role: "base",
        render: "image",
        src: "/assets/hardware/live2d/mouse/base.png",
      },
      {
        id: "mouse-button-press-effect",
        role: "buttonPressEffect",
        render: "runtime",
      },
      {
        id: "mouse-wheel-pulse",
        role: "wheelPulse",
        render: "runtime",
      },
    ],
    },
    gaming: {
      kind: "mouse",
      manifest: "/assets/hardware/live2d/mouse/gaming/manifest.json",
      baseSize: {
        width: 652,
        height: 1154,
      },
      display: {
        compactWidth: 145,
        compactHeight: 245,
        fullWidth: 260,
        fullHeight: 420,
      },
      layers: [
        {
          id: "mouse-gaming-base",
          role: "base",
          render: "image",
          src: "/assets/hardware/live2d/mouse/gaming/base.png",
        },
        {
          id: "mouse-gaming-button-press-effect",
          role: "buttonPressEffect",
          render: "runtime",
        },
        {
          id: "mouse-gaming-wheel-pulse",
          role: "wheelPulse",
          render: "runtime",
        },
      ],
    },
  },
  gamepad: {
    office: {
      kind: "gamepad",
      manifest: "/assets/hardware/live2d/gamepad/manifest.json",
      baseSize: {
        width: 1205,
        height: 826,
      },
      display: {
        compactWidth: 280,
        compactHeight: 180,
        fullWidth: 720,
        fullHeight: 430,
      },
      layers: [
        {
          id: "gamepad-base",
          role: "base",
          render: "image",
          src: "/assets/hardware/live2d/gamepad/base.png",
        },
        {
          id: "gamepad-press-effect",
          role: "pressEffect",
          render: "runtime",
        },
      ],
    },
    gaming: {
      kind: "gamepad",
      manifest: "/assets/hardware/live2d/gamepad/manifest.json",
      baseSize: {
        width: 1205,
        height: 826,
      },
      display: {
        compactWidth: 280,
        compactHeight: 180,
        fullWidth: 720,
        fullHeight: 430,
      },
      layers: [
        {
          id: "gamepad-base",
          role: "base",
          render: "image",
          src: "/assets/hardware/live2d/gamepad/base.png",
        },
        {
          id: "gamepad-press-effect",
          role: "pressEffect",
          render: "runtime",
        },
      ],
    },
  },
};

const MOUSE_RIG_HOTSPOTS: Record<HardwareRigVariant, Array<{
  id: string;
  label: string;
  x: number;
  y: number;
  w: number;
  h: number;
  radius: number;
}>> = {
  office: [
  { id: "left", label: "L", x: 0.20, y: 0.07, w: 0.30, h: 0.40, radius: 38 },
  { id: "right", label: "R", x: 0.51, y: 0.07, w: 0.30, h: 0.40, radius: 38 },
  { id: "middle", label: "W", x: 0.43, y: 0.04, w: 0.15, h: 0.28, radius: 22 },
  { id: "back", label: "Back", x: 0.02, y: 0.35, w: 0.09, h: 0.15, radius: 12 },
  { id: "forward", label: "Fwd", x: 0.02, y: 0.53, w: 0.09, h: 0.15, radius: 12 },
  ],
  gaming: [
    { id: "left", label: "L", x: 0.21, y: 0.05, w: 0.29, h: 0.38, radius: 38 },
    { id: "right", label: "R", x: 0.51, y: 0.05, w: 0.29, h: 0.38, radius: 38 },
    { id: "middle", label: "W", x: 0.43, y: 0.03, w: 0.15, h: 0.24, radius: 22 },
    { id: "back", label: "Back", x: 0.02, y: 0.31, w: 0.10, h: 0.13, radius: 12 },
    { id: "forward", label: "Fwd", x: 0.02, y: 0.47, w: 0.10, h: 0.13, radius: 12 },
  ],
};

function builtinHardwareAssetId(kind: HardwareRigKind, variant: HardwareRigVariant) {
  return `builtin.${kind}.${variant}`;
}

function hardwareAssetStorageKey(kind: HardwareRigKind) {
  if (kind === "keyboard") {
    return HARDWARE_ASSET_KEYBOARD_STORAGE_KEY;
  }
  if (kind === "mouse") {
    return HARDWARE_ASSET_MOUSE_STORAGE_KEY;
  }
  return HARDWARE_ASSET_GAMEPAD_STORAGE_KEY;
}

function loadSelectedHardwareAssetIds(): Record<HardwareRigKind, string> {
  if (typeof window === "undefined") {
    return {
      keyboard: builtinHardwareAssetId("keyboard", "office"),
      mouse: builtinHardwareAssetId("mouse", "office"),
      gamepad: "builtin.gamepad.xbox",
    };
  }

  const legacyVariant = normalizeHardwareRigVariant(
    window.localStorage.getItem(HARDWARE_RIG_VARIANT_STORAGE_KEY),
  );
  return {
    keyboard:
      window.localStorage.getItem(HARDWARE_ASSET_KEYBOARD_STORAGE_KEY) ??
      builtinHardwareAssetId("keyboard", legacyVariant),
    mouse:
      window.localStorage.getItem(HARDWARE_ASSET_MOUSE_STORAGE_KEY) ??
      builtinHardwareAssetId("mouse", legacyVariant),
    gamepad:
      window.localStorage.getItem(HARDWARE_ASSET_GAMEPAD_STORAGE_KEY) ??
      "builtin.gamepad.xbox",
  };
}

async function loadBuiltinHardwareRigAssets(): Promise<HardwareRigDefinition[]> {
  const assets = await Promise.all(
    BUILTIN_HARDWARE_ASSET_MANIFESTS.map(async (manifestUrl) => {
      let response: Response;
      try {
        response = await fetch(manifestUrl);
      } catch (error) {
        const detail = /failed to fetch/i.test(errorMessage(error))
          ? "网络请求失败"
          : errorMessage(error);
        throw new Error(
          `内置硬件资产读取失败：${detail || manifestUrl}`,
        );
      }
      if (!response.ok) {
        throw new Error(`内置硬件资产读取失败：${manifestUrl}`);
      }
      const raw = await response.json();
      const baseUrl = manifestUrl.slice(0, manifestUrl.lastIndexOf("/") + 1);
      return hardwareRigFromManifest(raw, {
        manifestUrl,
        baseUrl,
        source: "builtin",
        readonly: true,
      });
    }),
  );
  return assets.filter(isHardwareRigDefinition);
}

async function loadInstalledHardwareRigAssets(): Promise<{
  installed: InstalledHardwareAsset[];
  assets: HardwareRigDefinition[];
  error: string | null;
}> {
  try {
    const installed =
      await invokeCommand<InstalledHardwareAsset[]>("list_hardware_assets");
    const assets = installed
      .map((asset) =>
        asset.manifest
          ? hardwareRigFromManifest(asset.manifest, {
              manifestUrl: asset.manifestPath,
              source: "installed",
              readonly: false,
              manifestPath: asset.manifestPath,
              folderPath: asset.folderPath,
              resolveUrl: (src) =>
                convertHardwareAssetFileSrc(
                  joinHardwareAssetPath(asset.folderPath, src),
                ),
            })
          : null,
      )
      .filter(isHardwareRigDefinition);
    return { installed, assets, error: null };
  } catch (error) {
    const message = errorMessage(error);
    if (message.includes("需要 Tauri bridge")) {
      return { installed: [], assets: [], error: null };
    }
    return { installed: [], assets: [], error: message };
  }
}

function hardwareRigFromManifest(
  raw: Record<string, unknown>,
  options: {
    manifestUrl: string;
    baseUrl?: string;
    source: "builtin" | "installed";
    readonly: boolean;
    manifestPath?: string;
    folderPath?: string;
    resolveUrl?: (src: string) => string;
  },
): HardwareRigDefinition | null {
  const manifest = normalizeHardwareAssetManifest(raw, {
    baseUrl: options.baseUrl ?? "",
    resolveUrl: options.resolveUrl,
  });
  if (
    manifest.kind !== "keyboard" &&
    manifest.kind !== "mouse" &&
    manifest.kind !== "gamepad"
  ) {
    return null;
  }

  const kind = manifest.kind as HardwareRigKind;
  const fallback = HARDWARE_RIGS[kind][hardwareRigVariantForManifest(manifest.id)];
  return {
    ...fallback,
    kind,
    manifest: options.manifestUrl,
    id: manifest.id,
    name: manifest.name,
    source: options.source,
    readonly: options.readonly,
    manifestPath: options.manifestPath,
    folderPath: options.folderPath,
    baseSize: manifest.baseSize,
    layers: manifest.layers,
    regions: manifest.regions,
  };
}

function hardwareRigVariantForManifest(assetId: string): HardwareRigVariant {
  return assetId.endsWith(".gaming") || assetId.includes(".gaming.")
    ? "gaming"
    : "office";
}

function isHardwareRigDefinition(
  value: HardwareRigDefinition | null,
): value is HardwareRigDefinition {
  return Boolean(value);
}

function convertHardwareAssetFileSrc(filePath: string) {
  const convert = typeof window === "undefined" ? null : getConvertFileSrc();
  return convert ? convert(filePath) : filePath;
}

function joinHardwareAssetPath(folderPath: string, relativePath: string) {
  const separator = folderPath.includes("\\") ? "\\" : "/";
  const folder = folderPath.replace(/[\\/]+$/, "");
  const relative = relativePath.replace(/[\\/]+/g, separator);
  return `${folder}${separator}${relative}`;
}

function downloadHardwareAssetPackage(bytes: Uint8Array, fileName: string) {
  if (typeof document === "undefined") {
    return;
  }
  const blob = new Blob([bytes], { type: "application/zip" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  link.style.display = "none";
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

function safeDownloadName(value: string) {
  const name = value.trim().replace(/[^\w.-]+/g, "-").replace(/^-+|-+$/g, "");
  return name || "hardware-asset";
}

function HardwareRigView({
  kind,
  variant = "office",
  activity,
  accent,
  theme,
  compact = false,
  fitToHeight = false,
  fitMaxHeight,
}: {
  kind: HardwareRigKind;
  variant?: HardwareRigVariant;
  activity: HardwareRigActivity;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
  fitToHeight?: boolean;
  fitMaxHeight?: number;
}) {
  const { assets, selectedIds } = useHardwareAssetCatalog();
  const rigVariant = normalizeHardwareRigVariant(variant);
  const fallbackRig = HARDWARE_RIGS[kind][rigVariant];
  const selectedRig = resolveSelectedHardwareAsset(
    assets,
    kind,
    selectedIds[kind],
  ) as HardwareRigDefinition | null;
  const [manifestRig, setManifestRig] = useState<HardwareRigDefinition | null>(null);

  useEffect(() => {
    let cancelled = false;
    setManifestRig(null);

    fetch(fallbackRig.manifest)
      .then((response) => (response.ok ? response.json() : null))
      .then((raw) => {
        if (cancelled || !raw) {
          return;
        }
        const baseUrl = fallbackRig.manifest.slice(
          0,
          fallbackRig.manifest.lastIndexOf("/") + 1,
        );
        const manifest = normalizeHardwareAssetManifest(raw, baseUrl);
        setManifestRig({
          ...fallbackRig,
          id: manifest.id,
          name: manifest.name,
          kind: manifest.kind,
          baseSize: manifest.baseSize,
          layers: manifest.layers,
          regions: manifest.regions,
        });
      })
      .catch(() => {
        if (!cancelled) {
          setManifestRig(null);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [fallbackRig, kind]);

  const rig = selectedRig ?? manifestRig ?? fallbackRig;
  const width = compact ? rig.display.compactWidth : rig.display.fullWidth;
  const maxHeight = compact ? rig.display.compactHeight : rig.display.fullHeight;
  const fittedHeight = fitMaxHeight
    ? `min(100%, ${fitMaxHeight}px)`
    : "100%";
  const imageLayers = rig.layers.filter((layer) => layer.render === "image" && layer.src);
  return (
    <div
      className="relative mx-auto overflow-hidden"
      style={{
        width: fitToHeight ? "auto" : "100%",
        height: fitToHeight ? fittedHeight : undefined,
        maxWidth: width,
        maxHeight: fitToHeight ? fittedHeight : maxHeight,
        aspectRatio: `${rig.baseSize.width} / ${rig.baseSize.height}`,
        filter: `drop-shadow(0 14px 22px rgba(0,0,0,0.24))`,
      }}
      role="img"
      aria-label={`${rig.name ?? kind} hardware rig`}
      data-rig-kind={kind}
      data-rig-variant={rigVariant}
      data-rig-asset={rig.id ?? ""}
      data-rig-manifest={rig.manifest}
    >
      {imageLayers.map((layer) => (
        <img
          key={layer.id}
          className="absolute inset-0 h-full w-full object-contain"
          src={layer.src}
          alt=""
          draggable={false}
          style={{
            opacity: layer.opacity ?? 1,
          }}
        />
      ))}
      {rig.regions?.length ? (
        <HardwareRigRegionOverlays
          asset={rig}
          activity={activity}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ) : kind === "keyboard" ? (
        <KeyboardRigHotspots
          activity={activity}
          variant={rigVariant}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ) : kind === "mouse" ? (
        <MouseRigHotspots
          activity={activity}
          variant={rigVariant}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ) : null}
      {kind === "gamepad" && rig.regions?.length ? (
        <GamepadAnalogOverlays
          asset={rig}
          activity={activity}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ) : null}
    </div>
  );
}

function GamepadAnalogOverlays({
  asset,
  activity,
  accent,
  theme,
  compact,
}: {
  asset: HardwareRigDefinition;
  activity: HardwareRigActivity;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  const feedback = buildGamepadAnalogFeedback(activity);
  const byId = new Map((asset.regions ?? []).map((region) => [region.id, region]));

  return (
    <>
      <GamepadTriggerDepthOverlay
        region={byId.get("gamepad.trigger.left")}
        label="LT"
        value={feedback.leftTrigger.value}
        active={feedback.leftTrigger.active}
        accent={accent}
        theme={theme}
        compact={compact}
      />
      <GamepadTriggerDepthOverlay
        region={byId.get("gamepad.trigger.right")}
        label="RT"
        value={feedback.rightTrigger.value}
        active={feedback.rightTrigger.active}
        accent={accent}
        theme={theme}
        compact={compact}
      />
      <GamepadStickOffsetOverlay
        region={byId.get("gamepad.stick.left")}
        label="L"
        x={feedback.leftStick.x}
        y={feedback.leftStick.y}
        magnitude={feedback.leftStick.magnitude}
        active={feedback.leftStick.active}
        accent={accent}
        theme={theme}
        compact={compact}
      />
      <GamepadStickOffsetOverlay
        region={byId.get("gamepad.stick.right")}
        label="R"
        x={feedback.rightStick.x}
        y={feedback.rightStick.y}
        magnitude={feedback.rightStick.magnitude}
        active={feedback.rightStick.active}
        accent={accent}
        theme={theme}
        compact={compact}
      />
    </>
  );
}

function GamepadTriggerDepthOverlay({
  region,
  label,
  value,
  active,
  accent,
  theme,
  compact,
}: {
  region?: HardwareRigRegion;
  label: string;
  value: number;
  active: boolean;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  if (!region || !active) {
    return null;
  }
  const box = hardwareRegionBounds(region);
  const fillPercent = Math.max(8, Math.round(value * 100));

  return (
    <span
      className="pointer-events-none absolute overflow-hidden rounded-full border"
      style={{
        left: `${box.x * 100}%`,
        top: `${box.y * 100}%`,
        width: `${box.w * 100}%`,
        height: `${Math.max(box.h * 100, compact ? 5 : 6)}%`,
        borderColor: `${accent}aa`,
        background: theme.frame,
        boxShadow: `0 0 18px ${accent}55`,
      }}
    >
      <span
        className="absolute inset-y-0 left-0"
        style={{
          width: `${fillPercent}%`,
          background: `linear-gradient(90deg, ${accent}55, ${accent})`,
        }}
      />
      <span
        className="absolute inset-0 flex items-center justify-center font-semibold"
        style={{
          color: "#ffffff",
          fontSize: compact ? 7 : 9,
          textShadow: "0 0 6px rgba(0,0,0,0.65)",
        }}
      >
        {label} {Math.round(value * 100)}%
      </span>
    </span>
  );
}

function GamepadStickOffsetOverlay({
  region,
  label,
  x,
  y,
  magnitude,
  active,
  accent,
  theme,
  compact,
}: {
  region?: HardwareRigRegion;
  label: string;
  x: number;
  y: number;
  magnitude: number;
  active: boolean;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  if (!region || !active) {
    return null;
  }
  const box = hardwareRegionBounds(region);
  const centerX = box.x + box.w / 2;
  const centerY = box.y + box.h / 2;
  const dotX = centerX + x * box.w * 0.34;
  const dotY = centerY + y * box.h * 0.34;
  const dotSize = compact ? 13 : 18;

  return (
    <>
      <span
        className="pointer-events-none absolute rounded-full"
        style={{
          left: `${centerX * 100}%`,
          top: `${centerY * 100}%`,
          width: dotSize * 0.7,
          height: dotSize * 0.7,
          transform: "translate(-50%, -50%)",
          border: `1px solid ${accent}88`,
          background: theme.accentSoft,
        }}
      />
      <span
        className="pointer-events-none absolute flex items-center justify-center rounded-full font-semibold"
        style={{
          left: `${dotX * 100}%`,
          top: `${dotY * 100}%`,
          width: dotSize,
          height: dotSize,
          transform: "translate(-50%, -50%)",
          background: `radial-gradient(circle at 40% 35%, #ffffff, ${accent} 42%, ${accent}88 72%)`,
          color: "#ffffff",
          fontSize: compact ? 7 : 9,
          boxShadow: `0 0 ${Math.round(14 + magnitude * 16)}px ${accent}cc`,
          textShadow: "0 0 5px rgba(0,0,0,0.65)",
        }}
      >
        {label}
      </span>
    </>
  );
}

function hardwareRegionBounds(region: HardwareRigRegion) {
  const shape = region.shape;
  if (shape.kind === "rect") {
    return {
      x: shape.x,
      y: shape.y,
      w: shape.w,
      h: shape.h,
    };
  }
  const points = shape.points.length ? shape.points : [{ x: 0, y: 0 }];
  const xs = points.map((point) => point.x);
  const ys = points.map((point) => point.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return {
    x,
    y,
    w: Math.max(0.001, Math.max(...xs) - x),
    h: Math.max(0.001, Math.max(...ys) - y),
  };
}

function HardwareHotspotOverlay({
  x,
  y,
  w,
  h,
  radius,
  active,
  tested = false,
  label,
  accent,
  theme,
  compact = false,
}: {
  x: number;
  y: number;
  w: number;
  h: number;
  radius: number;
  active: boolean;
  tested?: boolean;
  label?: string;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
}) {
  if (!active && !tested) {
    return null;
  }
  return (
    <span
      className={`pointer-events-none absolute flex items-center justify-center text-[10px] font-semibold transition-all duration-75 ${
        active ? "hardware-press-flash" : tested ? "hardware-legend-glow" : ""
      }`}
      style={{
        left: `${x * 100}%`,
        top: `${y * 100}%`,
        width: `${w * 100}%`,
        height: `${h * 100}%`,
        borderRadius: radius,
        background: active
          ? `radial-gradient(circle at 50% 30%, rgba(255,255,255,0.64), ${accent}cc 34%, ${accent}66 72%, transparent 100%)`
          : `radial-gradient(circle at 50% 45%, ${accent}4d, ${accent}20 64%, transparent 100%)`,
        border: `1px solid ${active ? accent : `${accent}77`}`,
        color: active ? "#ffffff" : theme.accent,
        boxShadow: active
          ? `0 0 28px ${accent}cc, 0 0 48px ${accent}55, inset 0 -5px 0 rgba(0,0,0,0.22)`
          : `0 0 10px ${accent}33`,
        transform: active ? "translateY(2px) scale(0.98)" : "translateY(0)",
        textShadow: active ? "0 0 8px rgba(255,255,255,0.65)" : "none",
        mixBlendMode: active ? "screen" : "plus-lighter",
        fontSize: compact ? 8 : 10,
        backdropFilter: active ? "saturate(1.6)" : undefined,
      }}
    >
      {label ? (
        <span
          className={active ? "hardware-legend-glow" : undefined}
          style={{ color: "inherit" }}
        >
          {label}
        </span>
      ) : null}
    </span>
  );
}

function HardwarePolygonOverlay({
  points,
  active,
  label,
  accent,
  theme,
  compact = false,
}: {
  points: Array<{ x: number; y: number }>;
  active: boolean;
  label?: string;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
}) {
  if (!active || points.length < 3) {
    return null;
  }
  const polygon = points
    .map((point) => `${point.x * 100},${point.y * 100}`)
    .join(", ");
  const center = points.reduce(
    (sum, point) => ({ x: sum.x + point.x, y: sum.y + point.y }),
    { x: 0, y: 0 },
  );
  const centerX = (center.x / points.length) * 100;
  const centerY = (center.y / points.length) * 100;
  const strokeWidth = compact ? 1.15 : 1.35;

  return (
    <span
      className="pointer-events-none absolute inset-0 transition-all duration-75"
      style={{
        color: theme.text,
        mixBlendMode: "screen",
      }}
    >
      <svg
        className="absolute inset-0 h-full w-full hardware-press-flash overflow-visible"
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        aria-hidden="true"
        focusable="false"
        style={{
          transformOrigin: `${centerX}% ${centerY}%`,
        }}
      >
        <polygon
          points={polygon}
          fill={accent}
          fillOpacity="0.34"
          stroke={accent}
          strokeOpacity="0.95"
          strokeWidth={strokeWidth}
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
          style={{
            filter: `drop-shadow(0 0 14px ${accent}aa)`,
            paintOrder: "stroke fill",
          }}
        />
        <polygon
          points={polygon}
          fill="rgba(255,255,255,0.4)"
          fillOpacity="0.24"
        />
      </svg>
      {label ? (
        <span
          className="absolute hardware-legend-glow font-semibold"
          style={{
            left: `${centerX}%`,
            top: `${centerY}%`,
            transform: "translate(-50%, -50%)",
            color: "#ffffff",
            fontSize: compact ? 8 : 10,
            textShadow: "0 0 8px rgba(255,255,255,0.75)",
          }}
        >
          {label}
        </span>
      ) : null}
    </span>
  );
}

function HardwareRigRegionOverlays({
  asset,
  activity,
  accent,
  theme,
  compact,
}: {
  asset: HardwareRigDefinition;
  activity: HardwareRigActivity;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  const regions = resolveActiveHardwareRegions(asset, activity) as HardwareRigRegion[];

  return (
    <>
      {regions.map((region) => {
        const shape = region.shape;
        if (shape.kind === "polygon") {
          return (
            <HardwarePolygonOverlay
              key={region.id}
              points={shape.points}
              active
              label={hardwareRegionLabel(region, activity, compact)}
              accent={accent}
              theme={theme}
              compact={compact}
            />
          );
        }
        if (shape.kind === "rect") {
          return (
            <HardwareHotspotOverlay
              key={region.id}
              x={shape.x}
              y={shape.y}
              w={shape.w}
              h={shape.h}
              radius={shape.radius ?? 7}
              active
              label={hardwareRegionLabel(region, activity, compact)}
              accent={accent}
              theme={theme}
              compact={compact}
            />
          );
        }
        return null;
      })}
    </>
  );
}

function hardwareRegionLabel(
  region: HardwareRigRegion,
  activity: HardwareRigActivity,
  compact: boolean,
) {
  if (region.action?.kind === "mouse_button") {
    const buttons = region.action.buttons ?? [];
    if (
      buttons.some((button) => /^(middle|wheel)$/i.test(button)) &&
      activity.wheelLabel
    ) {
      return activity.wheelLabel;
    }
    if (region.id === "mouse.left") {
      return "L";
    }
    if (region.id === "mouse.right") {
      return "R";
    }
    if (region.id === "mouse.forward") {
      return "Fwd";
    }
  }
  return compact && region.action?.kind === "keyboard_key" ? undefined : region.label;
}

function KeyboardRigHotspots({
  activity,
  variant,
  accent,
  theme,
  compact,
}: {
  activity: HardwareRigActivity;
  variant: HardwareRigVariant;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  const pressedKeys = activity.pressedKeys ?? [];
  const keyboardEvents = activity.keyboardEvents ?? [];
  const lastKey = activity.lastKey ?? null;
  const baseWidth = HARDWARE_RIGS.keyboard[variant].baseSize.width;
  const baseHeight = HARDWARE_RIGS.keyboard[variant].baseSize.height;
  const rowYs = [30, 120, 210, 300, 390, 468];
  const keyUnit = 65;
  const keyGap = 10;
  return (
    <>
      {KEYBOARD_ROWS.map((row, rowIndex) => {
        let cursorX = 22;
        return row.map((key, keyIndex) => {
          const keyWidth = Math.max(58, (key.width ?? 1) * keyUnit);
          const state = keyVisualState(key, pressedKeys, keyboardEvents, lastKey);
          const active = state === "pressed";
          const tested = state === "tested";
          const hotspot = (
            <HardwareHotspotOverlay
              key={`${rowIndex}-${keyIndex}-${key.label}`}
              x={cursorX / baseWidth}
              y={(rowYs[rowIndex] ?? 92) / baseHeight}
              w={keyWidth / baseWidth}
              h={58 / baseHeight}
              radius={7}
              active={active}
              tested={tested}
              label={active || (!compact && tested) ? key.label : undefined}
              accent={accent}
              theme={theme}
              compact={compact}
            />
          );
          cursorX += keyWidth + keyGap;
          return hotspot;
        });
      })}
    </>
  );
}

function MouseRigHotspots({
  activity,
  variant,
  accent,
  theme,
  compact,
}: {
  activity: HardwareRigActivity;
  variant: HardwareRigVariant;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  const recentButtons = activity.recentButtons ?? [];
  const buttonActive = (name: string) =>
    Boolean(stateById[name]) || mouseButtonPressed(recentButtons, name);
  const stateById: Record<string, boolean> = {
    left: Boolean(activity.leftDown),
    right: Boolean(activity.rightDown),
    middle: Boolean(activity.middleDown || activity.wheelActive),
    back: Boolean(activity.backDown),
    forward: Boolean(activity.forwardDown),
  };
  return (
    <>
      {MOUSE_RIG_HOTSPOTS[variant].map((hotspot) => (
        <HardwareHotspotOverlay
          key={hotspot.id}
          x={hotspot.x}
          y={hotspot.y}
          w={hotspot.w}
          h={hotspot.h}
          radius={hotspot.radius}
          active={buttonActive(hotspot.id)}
          tested={hotspot.id === "middle" && Boolean(activity.wheelActive)}
          label={hotspot.id === "middle" ? activity.wheelLabel ?? hotspot.label : hotspot.label}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ))}
    </>
  );
}

function PhysicalDeviceShape({
  item,
  accent,
  hardwareRigVariant,
  theme,
}: {
  item: {
    kind: string;
    shape?: string;
    rigKind?: string | null;
    rigVariant?: string | null;
    title: string;
    detail: string;
    metric: string;
    live: boolean;
    activity?: Record<string, unknown>;
  };
  accent: string;
  hardwareRigVariant: HardwareRigVariant;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const label = (
    <div className="pointer-events-none absolute left-4 top-3 z-10">
      <div className="text-sm font-semibold" style={{ color: theme.text }}>
        {item.title}
      </div>
      <div className="mt-0.5 text-[11px]" style={{ color: theme.textMuted }}>
        {item.detail}
      </div>
    </div>
  );
  const liveDot = (
    <span
      className="absolute right-4 top-4 z-10 h-2.5 w-2.5 rounded-full"
      style={{ background: item.live ? theme.success : theme.textMuted }}
    />
  );

  if (item.shape === "monitor") {
    const activity = item.activity ?? {};
    const pointerVisible = Boolean(activity.pointerVisible);
    const screenWidth = Number(activity.width ?? 1) || 1;
    const screenHeight = Number(activity.height ?? 1) || 1;
    const pointerLeft = clamp((Number(activity.pointerX ?? 0) / screenWidth) * 100, 2, 98);
    const pointerTop = clamp((Number(activity.pointerY ?? 0) / screenHeight) * 100, 2, 98);
    return (
      <div
        className="relative h-full w-full overflow-visible"
        data-front-facing-display={DEVICE_SIMULATOR_CHROME.frontFacingDisplays ? "true" : "false"}
        data-window-texture={DEVICE_SIMULATOR_CHROME.displayWindowTexture ? "true" : "false"}
      >
        <div
          className="absolute inset-x-0 top-0 h-[76%] rounded-[18px] border-[3px]"
          style={{
            borderColor: "#1a2430",
            background:
              "linear-gradient(180deg, rgba(10,14,20,0.98), rgba(31,38,47,0.96))",
            boxShadow: `0 0 0 1px ${accent}55, 0 22px 36px rgba(0,0,0,0.34), inset 0 0 0 1px rgba(255,255,255,0.08)`,
          }}
        >
          {label}
          {liveDot}
          <div
            className="absolute inset-x-5 bottom-5 top-12 overflow-hidden rounded-xl"
            style={{
              border: `1px solid ${accent}55`,
              background:
                "radial-gradient(circle at 18% 12%, rgba(99,163,255,0.22), transparent 34%), linear-gradient(135deg, rgba(20,38,56,0.98), rgba(12,18,27,0.96))",
              boxShadow: "inset 0 0 32px rgba(0,0,0,0.42)",
            }}
          >
            <div
              className="absolute inset-0 opacity-70"
              style={{
                backgroundImage:
                  "linear-gradient(rgba(255,255,255,0.045) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.04) 1px, transparent 1px)",
                backgroundSize: "22px 22px",
              }}
            />
            <div
              className="absolute inset-x-0 top-0 flex h-6 items-center gap-1.5 px-3"
              style={{
                background: "rgba(5,10,16,0.72)",
                borderBottom: "1px solid rgba(255,255,255,0.08)",
              }}
            >
              {["#ff6b73", "#f6c861", "#4ed37d"].map((color) => (
                <span
                  key={color}
                  className="h-2 w-2 rounded-full"
                  style={{ background: color }}
                />
              ))}
              <span className="ml-2 text-[9px]" style={{ color: theme.textMuted }}>
                R-ShareMouse · Desktop
              </span>
            </div>
            <div
              className="absolute bottom-4 left-4 top-10 w-11 rounded-lg"
              style={{
                background: "rgba(255,255,255,0.055)",
                border: "1px solid rgba(255,255,255,0.07)",
              }}
            >
              {[0, 1, 2].map((index) => (
                <span
                  key={index}
                  className="mx-auto mt-3 block h-2.5 w-6 rounded-full"
                  style={{
                    background: index === 0 ? `${accent}88` : "rgba(255,255,255,0.16)",
                  }}
                />
              ))}
            </div>
            <div
              className="absolute bottom-4 left-[72px] right-4 top-10 rounded-xl"
              style={{
                background:
                  "linear-gradient(145deg, rgba(255,255,255,0.12), rgba(255,255,255,0.035))",
                border: "1px solid rgba(255,255,255,0.12)",
                boxShadow: "0 12px 24px rgba(0,0,0,0.22)",
              }}
            >
              <div
                className="absolute inset-x-0 top-0 flex h-6 items-center justify-between rounded-t-xl px-3"
                style={{
                  background: "rgba(255,255,255,0.08)",
                  borderBottom: "1px solid rgba(255,255,255,0.08)",
                }}
              >
                <span className="text-[9px]" style={{ color: theme.textSub }}>
                  显示窗口
                </span>
                <span className="text-[9px]" style={{ color: theme.textMuted }}>
                  {pointerVisible ? "Pointer live" : "Pointer idle"}
                </span>
              </div>
              <div className="absolute left-4 right-4 top-10 grid grid-cols-[1.2fr_0.8fr] gap-3">
                <div
                  className="h-12 rounded-lg"
                  style={{
                    background: `linear-gradient(135deg, ${accent}55, rgba(255,255,255,0.05))`,
                  }}
                />
                <div className="grid gap-2">
                  <span className="h-3 rounded-full" style={{ background: "rgba(255,255,255,0.18)" }} />
                  <span className="h-3 w-2/3 rounded-full" style={{ background: "rgba(255,255,255,0.12)" }} />
                  <span className="h-3 w-4/5 rounded-full" style={{ background: `${accent}55` }} />
                </div>
              </div>
              <div className="absolute bottom-4 left-4 right-4 grid grid-cols-3 gap-2">
                {[0, 1, 2].map((index) => (
                  <span
                    key={index}
                    className="h-9 rounded-lg"
                    style={{
                      background:
                        index === 1
                          ? `${accent}33`
                          : "rgba(255,255,255,0.08)",
                      border: "1px solid rgba(255,255,255,0.08)",
                    }}
                  />
                ))}
              </div>
            </div>
            {pointerVisible ? (
              <span
                className="absolute z-10 h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full"
                style={{
                  left: `${pointerLeft}%`,
                  top: `${pointerTop}%`,
                  background: accent,
                  boxShadow: `0 0 0 5px ${accent}28, 0 0 18px ${accent}`,
                }}
              />
            ) : null}
            <div className="absolute bottom-4 left-5 z-10 text-lg font-semibold" style={{ color: accent }}>
              {item.detail}
            </div>
            <div className="absolute right-5 top-8 z-10 text-[11px]" style={{ color: theme.textMuted }}>
              {pointerVisible
                ? `Pointer ${Math.round(Number(activity.pointerX ?? 0))}, ${Math.round(Number(activity.pointerY ?? 0))}`
                : "Pointer idle"}
            </div>
          </div>
        </div>
        <div
          className="absolute bottom-11 left-1/2 h-11 w-20 -translate-x-1/2 rounded-b-lg"
          style={{
            background: "linear-gradient(180deg, rgba(78,88,102,0.9), rgba(38,44,53,0.98))",
            boxShadow: "inset 0 0 0 1px rgba(255,255,255,0.08)",
          }}
        />
        <div
          className="absolute bottom-7 left-1/2 h-5 w-52 -translate-x-1/2 rounded-full"
          style={{
            background: "linear-gradient(180deg, rgba(58,65,75,0.96), rgba(22,27,34,0.98))",
            border: "1px solid rgba(255,255,255,0.12)",
            boxShadow: "0 12px 18px rgba(0,0,0,0.28)",
          }}
        />
      </div>
    );
  }

  if (item.shape === "keyboard") {
    const activity = item.activity ?? {};
    const pressedKeys = Array.isArray(activity.pressedKeys) ? activity.pressedKeys : [];
    const lastKey = typeof activity.lastKey === "string" ? activity.lastKey : null;
    const keyboardEvents = Array.isArray(activity.keyboardEvents)
      ? (activity.keyboardEvents as LocalControlEvent[])
      : [];
    return (
      <div
        className={
          DEVICE_SIMULATOR_CHROME.deviceFrames
            ? "relative flex h-full w-full items-center justify-center overflow-hidden rounded-2xl border p-4"
            : "relative flex h-full w-full items-center justify-center overflow-visible p-3"
        }
        style={
          DEVICE_SIMULATOR_CHROME.deviceFrames
            ? {
                borderColor: item.live ? accent : theme.border,
                background: `linear-gradient(180deg, ${theme.sidebar}, ${theme.frame})`,
                boxShadow: theme.panelShadow,
              }
            : {
                background: "transparent",
                boxShadow: "none",
              }
        }
      >
        {label}
        {liveDot}
        <HardwareRigView
          kind="keyboard"
          variant={hardwareRigVariant}
          activity={{ pressedKeys, lastKey, keyboardEvents }}
          accent={accent}
          theme={theme}
          compact
        />
        <div
          className={
            DEVICE_SIMULATOR_CHROME.annotationFrames
              ? "absolute bottom-3 left-5 rounded-md px-2.5 py-1 text-[11px]"
              : "absolute bottom-3 left-5 text-[11px]"
          }
          style={
            DEVICE_SIMULATOR_CHROME.annotationFrames
              ? {
                  border: `1px solid ${theme.border}`,
                  background: "rgba(255,255,255,0.05)",
                  color: theme.textSub,
                }
              : {
                  color: theme.textSub,
                  textShadow: "0 1px 10px rgba(0,0,0,0.38)",
                }
          }
        >
          最后按键 <span style={{ color: accent }}>{lastKey ?? "等待输入"}</span>
        </div>
        <div className="absolute bottom-3 right-5 text-sm font-semibold" style={{ color: accent }}>
          {item.metric}
        </div>
      </div>
    );
  }

  if (item.shape === "mouse") {
    const activity = item.activity ?? {};
    const pressedButtons = Array.isArray(activity.pressedButtons)
      ? activity.pressedButtons.map((value) => String(value))
      : [];
    const recentButtons = Array.isArray(activity.recentButtons)
      ? activity.recentButtons.map((value) => String(value))
      : [];
    const leftPressed = mouseButtonPressed(pressedButtons, "Left") || mouseButtonPressed(recentButtons, "Left");
    const rightPressed = mouseButtonPressed(pressedButtons, "Right") || mouseButtonPressed(recentButtons, "Right");
    const middlePressed = mouseButtonPressed(pressedButtons, "Middle") || mouseButtonPressed(recentButtons, "Middle");
    const backPressed = mouseButtonPressed(pressedButtons, "Back") || mouseButtonPressed(recentButtons, "Back");
    const forwardPressed = mouseButtonPressed(pressedButtons, "Forward") || mouseButtonPressed(recentButtons, "Forward");
    const wheelDeltaY = Number(activity.wheelDeltaY ?? 0);
    const wheelDeltaX = Number(activity.wheelDeltaX ?? 0);
    const pointerX = Math.round(Number(activity.x ?? 0));
    const pointerY = Math.round(Number(activity.y ?? 0));
    return (
      <div className="relative flex h-full w-full items-center justify-center overflow-hidden">
        <HardwareRigView
          kind="mouse"
          variant={hardwareRigVariant}
          activity={{
            leftDown: leftPressed,
            rightDown: rightPressed,
            middleDown: middlePressed,
            backDown: backPressed,
            forwardDown: forwardPressed,
            recentButtons,
            wheelActive: wheelDeltaX !== 0 || wheelDeltaY !== 0,
            wheelLabel: wheelDeltaY > 0 ? "↑" : wheelDeltaY < 0 ? "↓" : wheelDeltaX > 0 ? "→" : wheelDeltaX < 0 ? "←" : "W",
          }}
          accent={accent}
          theme={theme}
          compact
        />
        <div className="absolute left-0 top-3">
          {label}
        </div>
        {liveDot}
        <div className="absolute bottom-2 left-1/2 -translate-x-1/2 text-sm font-semibold" style={{ color: accent }}>
          {item.metric}
        </div>
        <div
          className={
            DEVICE_SIMULATOR_CHROME.annotationFrames
              ? "absolute bottom-8 left-1/2 -translate-x-1/2 rounded-md px-2 py-1 text-center text-[10px]"
              : "absolute bottom-8 left-1/2 -translate-x-1/2 text-center text-[10px]"
          }
          style={
            DEVICE_SIMULATOR_CHROME.annotationFrames
              ? {
                  border: `1px solid ${theme.border}`,
                  background: "rgba(255,255,255,0.05)",
                  color: theme.textSub,
                }
              : {
                  color: theme.textSub,
                  textShadow: "0 1px 10px rgba(0,0,0,0.38)",
                }
          }
        >
          <div>
            X {pointerX} · Y {pointerY}
          </div>
          <div style={{ color: wheelDeltaX || wheelDeltaY ? accent : theme.textMuted }}>
            Wheel {wheelDeltaX}, {wheelDeltaY}
          </div>
        </div>
      </div>
    );
  }

  if (item.shape === "gamepad") {
    const activity = item.activity ?? {};
    const pressedButtons = Array.isArray(activity.pressedButtons)
      ? activity.pressedButtons.map((value) => normalizeKeyToken(String(value)))
      : [];
    const feedback = buildGamepadAnalogFeedback({
      ...activity,
      pressedButtons,
    });
    const leftX = feedback.leftStick.x;
    const leftY = feedback.leftStick.y;
    const rightX = feedback.rightStick.x;
    const rightY = feedback.rightStick.y;
    const leftTrigger = feedback.leftTrigger.value;
    const rightTrigger = feedback.rightTrigger.value;
    return (
      <div className="relative flex h-full w-full items-center justify-center overflow-visible p-3">
        {label}
        {liveDot}
        <HardwareRigView
          kind="gamepad"
          variant={hardwareRigVariant}
          activity={{
            ...activity,
            pressedButtons: feedback.pressedButtons,
          }}
          accent={accent}
          theme={theme}
          compact
          fitToHeight
          fitMaxHeight={Math.max(150, item.h - 72)}
        />
        <div
          className="absolute bottom-5 left-5 text-[10px]"
          style={{
            color: theme.textSub,
            textShadow: "0 1px 10px rgba(0,0,0,0.38)",
          }}
        >
          L {Math.round(leftX * 100)}, {Math.round(leftY * 100)} · R {Math.round(rightX * 100)}, {Math.round(rightY * 100)}
          <br />
          LT {Math.round(leftTrigger * 100)}% · RT {Math.round(rightTrigger * 100)}%
        </div>
        <div className="absolute bottom-5 right-5 text-sm font-semibold" style={{ color: accent }}>
          {item.metric}
        </div>
      </div>
    );
  }

  if (item.shape === "speaker") {
    const activity = item.activity ?? {};
    const inputs = Math.max(0, Number(activity.inputs ?? 0));
    const outputs = Math.max(0, Number(activity.outputs ?? 0));
    return (
      <div className="relative h-full w-full">
        <div
          className="absolute inset-3 rounded-3xl border"
          style={{
            borderColor: item.live ? accent : theme.border,
            background: `linear-gradient(145deg, ${theme.sidebar}, ${theme.frame})`,
            boxShadow: theme.panelShadow,
          }}
        >
          {label}
          {liveDot}
          <div className="absolute bottom-8 left-1/2 flex -translate-x-1/2 items-center gap-4">
            {[72, 50].map((size) => (
              <span
                key={size}
                className="rounded-full border-4"
                style={{
                  width: size,
                  height: size,
                  borderColor: `${accent}88`,
                  boxShadow: `inset 0 0 0 10px ${accent}18`,
                }}
              />
            ))}
          </div>
          <div className="absolute bottom-24 left-5 right-5 grid gap-2">
            {[
              ["输入", inputs, theme.accent],
              ["输出", outputs, accent],
            ].map(([labelText, count, color]) => (
              <div key={String(labelText)} className="grid grid-cols-[32px_1fr_28px] items-center gap-2 text-[10px]">
                <span style={{ color: theme.textMuted }}>{labelText}</span>
                <span className="h-2 overflow-hidden rounded-full" style={{ background: "rgba(255,255,255,0.08)" }}>
                  <span
                    className="block h-full rounded-full"
                    style={{
                      width: `${clamp(Number(count) * 7, 8, 100)}%`,
                      background: String(color),
                      boxShadow: `0 0 10px ${String(color)}55`,
                    }}
                  />
                </span>
                <span style={{ color: theme.textSub }}>{String(count)}</span>
              </div>
            ))}
          </div>
          <div className="absolute bottom-5 left-5 text-sm font-semibold" style={{ color: accent }}>
            {item.metric}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className="relative h-full w-full rounded-2xl border"
      style={{
        borderColor: item.live ? accent : theme.border,
        background: `linear-gradient(145deg, ${theme.sidebar}, ${theme.frame})`,
        boxShadow: theme.panelShadow,
      }}
    >
      {label}
      {liveDot}
      <div
        className="absolute bottom-10 left-1/2 h-20 w-32 -translate-x-1/2 rounded-lg border"
        style={{
          borderColor: accent,
          background: `${accent}16`,
        }}
      >
        <Monitor className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2" size={28} style={{ color: accent }} />
      </div>
      <div className="absolute bottom-4 left-5 text-sm font-semibold" style={{ color: accent }}>
        {item.metric || (item.live ? "在线" : "待连接")}
      </div>
    </div>
  );
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function galleryKeyboardKeyActive(
  key: string,
  pressedKeys: unknown[],
  lastKey: string | null,
) {
  const candidates = galleryKeyboardKeyCandidates(key);
  const normalizedCandidates = new Set(candidates.map((value) => normalizeKeyToken(value)));
  return [...pressedKeys, lastKey]
    .filter((value): value is string => typeof value === "string")
    .some((value) => normalizedCandidates.has(normalizeKeyToken(value)));
}

function galleryKeyboardKeyCandidates(key: string) {
  const aliasMap: Record<string, string[]> = {
    Esc: ["Escape"],
    Del: ["Delete"],
    Ins: ["Insert"],
    PgUp: ["PageUp"],
    PgDn: ["PageDown"],
    Caps: ["CapsLock"],
    Ctrl: ["Control", "ControlLeft", "ControlRight"],
    Win: ["Meta", "Super", "OS", "WinLeft", "WinRight"],
    Space: ["Char(32)", "Spacebar"],
    Enter: ["Return"],
    Shift: ["ShiftLeft", "ShiftRight"],
    Alt: ["AltLeft", "AltRight"],
  };
  const candidates = [key, ...(aliasMap[key] ?? [])];
  if (key.length === 1) {
    candidates.push(`Char(${key.charCodeAt(0)})`);
    candidates.push(`Char(${key.toUpperCase().charCodeAt(0)})`);
    candidates.push(key.toLowerCase(), key.toUpperCase());
  }
  return candidates;
}

function latestLocalControlEvent(
  snapshot: LocalControlsSnapshot | null,
  kind?: LocalControlEvent["device_kind"],
) {
  return [...(snapshot?.recent_events ?? [])]
    .filter((event) => !kind || event.device_kind === kind)
    .sort((left, right) => {
      const leftTime = left.timestamp_ms ?? 0;
      const rightTime = right.timestamp_ms ?? 0;
      if (leftTime !== rightTime) {
        return rightTime - leftTime;
      }
      return right.sequence - left.sequence;
    })[0] ?? null;
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
  const selected = normalizeDeviceIdentifier(selectedDeviceId);
  return identifiers.some((identifier) => normalizeDeviceIdentifier(identifier) === selected);
}

function shouldUseAggregateAttributionFallback(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlKind,
  selectedDeviceId: string | undefined,
  scopedEvents: LocalControlEvent[],
  audioOutputs: AudioOutputDevice[] = [],
) {
  if (
    !snapshot ||
    scopedEvents.length ||
    isUnscopedDeviceSelection(selectedDeviceId) ||
    (kind !== "keyboard" && kind !== "mouse")
  ) {
    return false;
  }

  const aggregateEvents = selectedControlEvents(snapshot, kind, undefined);
  if (!aggregateEvents.length) {
    return false;
  }

  const selectedDevice = localDeviceItems(snapshot, kind, audioOutputs).find(
    (device) => device.id === selectedDeviceId,
  );
  if (!selectedDevice) {
    return false;
  }

  const attributed = aggregateEvents.some((event) =>
    eventMatchesSelectedDevice(event, kind, selectedDeviceId),
  );
  return !attributed;
}

function normalizeDeviceIdentifier(value: string | null | undefined) {
  return String(value ?? "")
    .trim()
    .replace(/^\\\\\?\\/, "")
    .replace(/^\\\?\\/, "")
    .replace(/\\+/g, "\\")
    .toLowerCase();
}

function fullDeviceTooltip(name: string | null | undefined, detail?: string | null) {
  return [name, detail]
    .filter((value): value is string => typeof value === "string" && value.trim().length > 0)
    .join("\n");
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

function deviceTreeCounts(
  snapshot: LocalControlsSnapshot | null,
  audioOutputs: AudioOutputDevice[] = [],
) {
  const audioInputs = safeArray(snapshot?.audio_inputs);
  const resolvedAudioOutputs = safeArray(snapshot?.audio_outputs).length
    ? safeArray(snapshot?.audio_outputs)
    : safeArray(audioOutputs);
  return {
    all: Boolean(snapshot),
    keyboard: localInputDeviceCount(snapshot, "keyboard"),
    mouse: localInputDeviceCount(snapshot, "mouse"),
    gamepad: safeArray(snapshot?.gamepads).length,
    display: snapshot?.display?.display_count ?? 0,
    audio: audioInputs.length + resolvedAudioOutputs.length,
    remote: 0,
  };
}

function firstAvailableControlKind(
  snapshot: LocalControlsSnapshot | null,
  audioOutputs: AudioOutputDevice[] = [],
): LocalControlKind {
  const counts = deviceTreeCounts(snapshot, audioOutputs);
  const priority: LocalControlKind[] = ["keyboard", "mouse", "gamepad", "display", "audio"];
  return priority.find((kind) => counts[kind] > 0) ?? "keyboard";
}

function localDeviceItems(
  snapshot: LocalControlsSnapshot | null,
  kind: LocalControlKind,
  audioOutputs: AudioOutputDevice[] = [],
): LocalDeviceSelectItem[] {
  return buildLocalDeviceSelectItems(snapshot, kind, audioOutputs) as LocalDeviceSelectItem[];
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
      className={`rshare-select ${compact ? "h-7 max-w-[160px]" : "h-8 max-w-[320px]"} rounded-md px-2 text-xs outline-none`}
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.035)",
        color: theme.text,
      }}
      value={currentId}
      onChange={(event) => onChange?.(event.currentTarget.value)}
      title={fullDeviceTooltip(
        options.find((item) => item.id === currentId)?.name,
        options.find((item) => item.id === currentId)?.detail,
      )}
    >
      {options.map((item) => (
        <option
          key={item.id}
          value={item.id}
          style={{ backgroundColor: theme.frame, color: theme.text }}
        >
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
    ipAddress?: string;
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
                {device.hostname} · {device.ipAddress ?? device.address}
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
            <InfoRow label="IP" value={device.ipAddress ?? device.address} theme={theme} />
            <InfoRow label="端点" value={device.address} theme={theme} />
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
  onRefreshLocalControls,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  error: string | null;
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  onRunInputTest: (kind: string) => void;
  onRefreshLocalControls?: () => Promise<void>;
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
      onRefreshLocalControls={onRefreshLocalControls}
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
  onRefreshLocalControls,
  hardwareRigVariant = "office",
  compactLayout = false,
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
  onRefreshLocalControls?: () => Promise<void>;
  hardwareRigVariant?: HardwareRigVariant;
  compactLayout?: boolean;
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
        {selectedKind === "display" ? null : (
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
        )}
        <LocalControlDetail
          kind={selectedKind}
          snapshot={snapshot}
          selectedDeviceId={selectedDeviceId}
          onSelectedDeviceIdChange={onSelectedDeviceIdChange}
          remoteDevice={remoteDevice}
          audioOutputs={audioOutputs}
          inputTestResult={inputTestResult}
          confirmingInputTest={confirmingInputTest}
          onRunInputTest={onRunInputTest}
          onRefreshLocalControls={onRefreshLocalControls}
          hardwareRigVariant={hardwareRigVariant}
          compactLayout={compactLayout}
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
  vertical = false,
}: {
  kind: LocalDevicePageKind;
  snapshot: LocalControlsSnapshot | null;
  audioOutputs?: AudioOutputDevice[];
  selectedDeviceId?: string;
  onSelectedDeviceIdChange?: (deviceId: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
  vertical?: boolean;
}) {
  if (kind === "remote") {
    return null;
  }
  const devices = localDeviceItems(snapshot, kind, audioOutputs).filter((device) => device.live);
  if (!devices.length) {
    return null;
  }
  return (
    <div
      className={`rshare-scroll flex shrink-0 gap-2 ${
        vertical ? "max-h-[170px] flex-col overflow-y-auto" : "overflow-x-auto px-3 py-2"
      }`}
      style={
        vertical
          ? { background: "transparent" }
          : { borderBottom: `1px solid ${theme.border}`, background: theme.frame }
      }
    >
      {devices.map((device) => (
        <button
          key={device.id}
          type="button"
          className={`shrink-0 truncate rounded-md px-3 py-1.5 text-left text-sm ${
            vertical ? "w-full" : "max-w-[260px]"
          }`}
          style={{
            border: `1px solid ${selectedDeviceId === device.id || (!selectedDeviceId && device.active) ? theme.accent : theme.border}`,
            background: selectedDeviceId === device.id || (!selectedDeviceId && device.active) ? theme.accentSoft : "rgba(255,255,255,0.04)",
            color: theme.text,
          }}
          onClick={() => onSelectedDeviceIdChange?.(device.id)}
          title={fullDeviceTooltip(device.name, device.detail)}
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
  audioOutputs = [],
  inputTestResult,
  confirmingInputTest,
  onRunInputTest,
  onSelectedDeviceIdChange,
  onRefreshLocalControls,
  hardwareRigVariant,
  compactLayout = false,
  theme,
}: {
  kind: LocalControlKind;
  snapshot: LocalControlsSnapshot | null;
  remoteDevice?: {
    id: string;
    name: string;
    hostname: string;
  } | null;
  audioOutputs?: AudioOutputDevice[];
  inputTestResult: LocalInputTestResult | null;
  confirmingInputTest: string | null;
  selectedDeviceId?: string;
  onRunInputTest: (kind: string) => void;
  onSelectedDeviceIdChange?: (deviceId: string) => void;
  onRefreshLocalControls?: () => Promise<void>;
  hardwareRigVariant: HardwareRigVariant;
  compactLayout?: boolean;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const effectiveSelectedDeviceId = selectedLocalDeviceId(snapshot, kind, selectedDeviceId);
  const scopedEvents = selectedControlEvents(snapshot, kind, effectiveSelectedDeviceId);
  const attributionFallback = shouldUseAggregateAttributionFallback(
    snapshot,
    kind,
    effectiveSelectedDeviceId,
    scopedEvents,
    audioOutputs,
  );
  const recentEvents = attributionFallback
    ? selectedControlEvents(snapshot, kind, undefined)
    : scopedEvents;
  const scopedInputTestResult =
    inputTestResult &&
    inputTestResult.kind === kind &&
    (remoteDevice
      ? inputTestResult.targetId === remoteDevice.id
      : !inputTestResult.targetId)
      ? inputTestResult
      : null;
  if (kind === "keyboard") {
    const keyboardState = keyboardMonitorState(snapshot, effectiveSelectedDeviceId, recentEvents);
    const keyboardEvents = recentEvents.slice(-12).reverse();
    const actionLabel = remoteDevice
      ? "远端真实注入测试"
      : confirmingInputTest === "keyboard"
        ? "再次点击执行 Shift 测试"
        : "真实注入测试";
    return (
      <div
        className={
          compactLayout
            ? "rshare-scroll grid h-full min-h-0 grid-cols-1 gap-3 overflow-auto"
            : "grid h-full min-h-0 grid-rows-[minmax(0,1fr)_150px] gap-3"
        }
      >
        <div className={compactLayout ? "relative min-h-[320px]" : "relative min-h-0"}>
          <SimulatedKeyboard pressedKeys={keyboardState.pressedKeys} lastKey={keyboardState.lastKey} recentEvents={recentEvents} eventCount={keyboardState.eventCount} hardwareRigVariant={hardwareRigVariant} theme={theme} />
          {attributionFallback ? (
            <DeviceAttributionNotice kind="键盘" theme={theme} />
          ) : null}
        </div>
        <div className="grid min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_420px]"><InputTestAction label={actionLabel} result={scopedInputTestResult} disabled={remoteDevice ? false : !snapshot} onClick={() => onRunInputTest("keyboard")} theme={theme} /><KeyboardEventLog events={keyboardEvents} theme={theme} /></div>
      </div>
    );
  }
  if (kind === "mouse") {
    const mouseState = mouseMonitorState(snapshot, effectiveSelectedDeviceId, recentEvents);
    const mouseEvents = recentEvents.slice(-12).reverse();
    const mouseLayout = getMouseDetailLayoutClasses({ compact: compactLayout });
    const actionLabel = remoteDevice
      ? "远端真实注入测试"
      : confirmingInputTest === "mouse"
        ? "再次点击执行移动测试"
        : "真实注入测试";
    return (
      <div className={mouseLayout.root}>
        <div className={mouseLayout.previewPane}>
          <SimulatedMouse x={mouseState.x} y={mouseState.y} pressedButtons={mouseState.pressedButtons} recentEvents={recentEvents} wheelDeltaX={mouseState.wheelDeltaX} wheelDeltaY={mouseState.wheelDeltaY} wheelTotalX={mouseState.wheelTotalX} wheelTotalY={mouseState.wheelTotalY} eventCount={mouseState.eventCount} moveCount={mouseState.moveCount} buttonPressCount={mouseState.buttonPressCount} buttonReleaseCount={mouseState.buttonReleaseCount} wheelEventCount={mouseState.wheelEventCount} displayRelativeX={mouseState.displayRelativeX} displayRelativeY={mouseState.displayRelativeY} currentDisplayIndex={mouseState.currentDisplayIndex} currentDisplayId={mouseState.currentDisplayId} displays={snapshot?.display.displays ?? []} hardwareRigVariant={hardwareRigVariant} theme={theme} />
          {attributionFallback ? (
            <DeviceAttributionNotice kind="鼠标" theme={theme} />
          ) : null}
        </div>
        <div className={mouseLayout.sidePane}><MouseEventLog events={mouseEvents} theme={theme} /><InputTestAction label={actionLabel} result={scopedInputTestResult} disabled={remoteDevice ? false : !snapshot} onClick={() => onRunInputTest("mouse")} theme={theme} /></div>
      </div>
    );
  }
  if (kind === "gamepad") {
    const gamepad = selectedGamepad(snapshot, effectiveSelectedDeviceId);
    const gamepadEvents = recentEvents.slice(-12).reverse();
    return <div className="grid h-full min-h-0 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_360px]"><SimulatedGamepad gamepad={gamepad} virtualDetail={snapshot?.virtual_gamepad.detail ?? "Virtual HID not implemented"} theme={theme} /><GamepadEventLog events={gamepadEvents} theme={theme} /></div>;
  }
  if (kind === "audio") {
    return <AudioDetail snapshot={snapshot} audioOutputs={audioOutputs} theme={theme} />;
  }
  return (
    <DisplaySettingsDetail
      snapshot={snapshot}
      selectedDisplayId={effectiveSelectedDeviceId}
      onSelectedDisplayIdChange={onSelectedDeviceIdChange}
      onRefreshLocalControls={onRefreshLocalControls}
      theme={theme}
    />
  );
}

function DisplaySettingsDetail({
  snapshot,
  selectedDisplayId,
  onSelectedDisplayIdChange,
  onRefreshLocalControls,
  theme,
}: {
  snapshot: LocalControlsSnapshot | null;
  selectedDisplayId?: string;
  onSelectedDisplayIdChange?: (displayId: string) => void;
  onRefreshLocalControls?: () => Promise<void>;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const view = buildDisplaySettingsViewModel(
    snapshot,
    selectedDisplayId,
  ) as DisplaySettingsViewModel;
  const selected = view.selectedDisplay;
  const [virtualDisplays, setVirtualDisplays] = useState<VirtualDisplaySnapshot[]>([]);
  const [captures, setCaptures] = useState<Record<string, string>>({});
  const captureUrlStoreRef = useRef<ReturnType<typeof createDisplayCaptureUrlStore> | null>(
    null,
  );
  if (captureUrlStoreRef.current === null) {
    captureUrlStoreRef.current = createDisplayCaptureUrlStore();
  }
  const [resolutionValue, setResolutionValue] = useState("");
  const [refreshRateValue, setRefreshRateValue] = useState("");
  const [scaleValue, setScaleValue] = useState("100");
  const [virtualModeValue, setVirtualModeValue] = useState("1920x1080@60000");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const virtualView = buildVirtualDisplayViewModel(virtualDisplays);
  const virtualCreateMode =
    virtualView.createModes.find((mode: { value: string }) => mode.value === virtualModeValue) ??
    virtualView.createModes[0];

  async function refreshVirtualDisplays() {
    const displays = await invokeCommand<VirtualDisplaySnapshot[]>("list_virtual_displays");
    setVirtualDisplays(displays);
    return displays;
  }

  useEffect(() => {
    setResolutionValue(`${selected.width}x${selected.height}`);
    setRefreshRateValue(
      selected.refreshRateMillihz ? String(selected.refreshRateMillihz) : "",
    );
    setScaleValue(String(selected.scalePercent ?? 100));
  }, [selected.id, selected.width, selected.height, selected.refreshRateMillihz, selected.scalePercent]);

  useEffect(() => {
    let cancelled = false;
    invokeCommand<VirtualDisplaySnapshot[]>("list_virtual_displays")
      .then((displays) => {
        if (!cancelled) {
          setVirtualDisplays(displays);
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setStatusMessage(`虚拟显示器状态不可用：${errorMessage(error)}`);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(
    () => () => {
      captureUrlStoreRef.current?.dispose();
    },
    [],
  );

  const scaleOptions = displayScaleOptions(selected.scalePercent);
  const resolutionChanged = resolutionValue !== `${selected.width}x${selected.height}`;
  const refreshRateChanged =
    refreshRateValue !== (selected.refreshRateMillihz ? String(selected.refreshRateMillihz) : "");
  const scaleChanged = Number(scaleValue) !== Number(selected.scalePercent ?? 100);
  const canApplyDisplayMode =
    (resolutionChanged && selected.writeCapabilities.resolution) ||
    (refreshRateChanged && selected.writeCapabilities.refreshRate) ||
    (scaleChanged && selected.writeCapabilities.scale);

  const runDisplayAction = async <T,>(
    action: string,
    task: () => Promise<T>,
    successMessage: (result: T) => string,
  ) => {
    setBusyAction(action);
    setStatusMessage(null);
    try {
      const result = await task();
      setStatusMessage(successMessage(result));
      return result;
    } catch (error) {
      setStatusMessage(`操作失败：${String(error)}`);
      return null;
    } finally {
      setBusyAction(null);
    }
  };

  const captureDisplayBackgrounds = () =>
    void runDisplayAction(
      "capture",
      async () => {
        const captureGeneration =
          captureUrlStoreRef.current?.generation() ?? 0;
        const results = await mapWithConcurrency(view.displays, 2, async (display) => {
          try {
            const response = await captureDisplayBinary(display.id, 900);
            const decoded = createDisplayCaptureObjectUrl(response);
            return {
              displayId: display.id,
              result: decoded.result as DisplayCaptureResult,
              url: decoded.url as string | null,
            };
          } catch (error) {
            return {
              displayId: display.id,
              result: {
                request_id: crypto.randomUUID(),
                status: "ApplyFailed" as DisplayOperationStatus,
                message: errorMessage(error),
                payload: null,
              },
              url: null,
            };
          }
        });
        return { captureGeneration, results };
      },
      ({ captureGeneration, results }) => {
        const failures: DisplayCaptureResult[] = [];
        let successCount = 0;
        for (const { displayId, result, url } of results) {
          if (result.status !== "Success") {
            failures.push(result);
            continue;
          }
          if (url) {
            if (
              captureUrlStoreRef.current?.replace(
                displayId,
                url,
                captureGeneration,
              )
            ) {
              successCount += 1;
            }
          }
        }
        if (successCount) {
          setCaptures(captureUrlStoreRef.current?.snapshot() ?? {});
        }
        if (successCount === 0 && failures.length) {
          const firstFailure = failures[0];
          return `桌面贴图未更新：${displayOperationMessage(
            firstFailure.status,
            firstFailure.message,
            "需要屏幕共享授权",
          )}`;
        }
        if (failures.length) {
          return `桌面贴图已更新：${successCount}/${view.displays.length} 台显示器，${failures.length} 台失败`;
        }
        return `桌面贴图已更新：${successCount}/${view.displays.length} 台显示器`;
      },
    );

  const identifyDisplays = () =>
    void runDisplayAction(
      "identify",
      () => invokeCommand<{ status: DisplayOperationStatus; message?: string | null }>(
        "identify_displays",
        { duration_ms: 2500 },
      ),
      (result) => displayOperationMessage(result.status, result.message, "正在标识显示器"),
    );

  const openSystemDisplaySettings = () =>
    void runDisplayAction(
      "open-settings",
      () => invokeCommand("open_display_settings"),
      () => "已请求打开系统显示设置",
    );

  const applyDisplaySettings = () =>
    void runDisplayAction(
      "apply",
      () => {
        const [width, height] = resolutionValue.split("x").map((value) => Number(value));
        const refreshRateMillihz = Number(refreshRateValue || selected.refreshRateMillihz);
        const scalePercent = Number(scaleValue || selected.scalePercent);
        const request: Record<string, unknown> = {
          display_id: selected.id,
        };
        if (Number.isFinite(width) && Number.isFinite(height)) {
          request.width = width;
          request.height = height;
        }
        if (Number.isFinite(refreshRateMillihz) && refreshRateMillihz > 0) {
          request.refresh_rate_millihz = refreshRateMillihz;
        }
        if (
          Number.isFinite(scalePercent) &&
          scalePercent > 0 &&
          scalePercent !== selected.scalePercent
        ) {
          request.scale_percent = scalePercent;
        }
        return invokeCommand<DisplaySettingsUpdateResult>("update_display_settings", {
          request,
        });
      },
      (result) =>
        displayOperationMessage(
          result.status,
          result.message,
          "显示设置已应用，正在等待守护进程刷新",
        ),
    );

  const createVirtualDisplay = () =>
    void runDisplayAction(
      "create-virtual-display",
      async () => {
        const request = {
          width: virtualCreateMode.width,
          height: virtualCreateMode.height,
          refresh_rate_millihz: virtualCreateMode.refreshRateMillihz,
          name: virtualView.createDefaults.name,
        };
        const result = await invokeCommand<VirtualDisplayOperationResult>(
          "create_virtual_display",
          { request },
        );
        await refreshVirtualDisplays();
        await onRefreshLocalControls?.();
        return result;
      },
      (result) => virtualDisplayOperationMessage(result),
    );

  const removeVirtualDisplay = (id: string) =>
    void runDisplayAction(
      `remove-virtual-display-${id}`,
      async () => {
        const result = await invokeCommand<VirtualDisplayOperationResult>(
          "remove_virtual_display",
          { request: { id } },
        );
        await refreshVirtualDisplays();
        await onRefreshLocalControls?.();
        return result;
      },
      (result) => virtualDisplayOperationMessage(result),
    );

  return (
    <div className="rshare-scroll h-full min-h-0 overflow-auto pr-1">
      <section
        className="min-h-full p-4"
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.sidebar,
        }}
      >
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <div className="min-w-0 flex-1">
            <h2 className="text-lg font-semibold">显示器设置</h2>
            <p className="text-sm" style={{ color: theme.textMuted }}>
              按系统显示设置组织本机屏幕、截图和可写显示参数。
            </p>
          </div>
          <button
            type="button"
            className="rounded-md px-3 py-2 text-sm"
            style={secondaryButtonStyle(theme)}
            disabled={busyAction === "identify"}
            onClick={identifyDisplays}
          >
            标识
          </button>
          <button
            type="button"
            className="rounded-md px-3 py-2 text-sm"
            style={secondaryButtonStyle(theme)}
            disabled={busyAction === "capture" || !selected.writeCapabilities.capture}
            onClick={captureDisplayBackgrounds}
            title={
              selected.writeCapabilities.capture
                ? "获取当前显示器桌面贴图"
                : "当前平台未报告可用截图后端"
            }
          >
            获取桌面贴图
          </button>
          <button
            type="button"
            className="rounded-md px-3 py-2 text-sm"
            style={secondaryButtonStyle(theme)}
            disabled={busyAction === "open-settings"}
            onClick={openSystemDisplaySettings}
          >
            系统设置
          </button>
        </div>

        <div
          className="relative mb-4 h-[320px] overflow-hidden rounded-md"
          style={{
            border: `1px solid ${theme.border}`,
            background:
              "linear-gradient(rgba(255,255,255,0.045) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.045) 1px, transparent 1px)",
            backgroundSize: "24px 24px",
          }}
        >
          <svg
            className="absolute inset-0 h-full w-full"
            viewBox={`${view.bounds.minX} ${view.bounds.minY} ${view.bounds.width} ${view.bounds.height}`}
            preserveAspectRatio="xMidYMid meet"
            role="img"
            aria-label="System display arrangement"
          >
            {view.displays.map((display) => {
              const active = display.id === selected.id;
              const capture = captures[display.id];
              const strokeWidth = Math.max(view.bounds.width, view.bounds.height) / 420;
              return (
                <g
                  key={display.id}
                  onClick={() => onSelectedDisplayIdChange?.(display.id)}
                  style={{ cursor: "pointer" }}
                >
                  <rect
                    x={display.x}
                    y={display.y}
                    width={display.width}
                    height={display.height}
                    rx={10}
                    fill={active ? theme.accentSoft : "rgba(255,255,255,0.055)"}
                    stroke={active ? theme.accent : theme.border}
                    strokeWidth={strokeWidth * (active ? 2.4 : 1.2)}
                  />
                  {capture ? (
                    <image
                      href={capture}
                      x={display.x}
                      y={display.y}
                      width={display.width}
                      height={display.height}
                      preserveAspectRatio="xMidYMid slice"
                      opacity={active ? 0.76 : 0.48}
                    />
                  ) : null}
                  <rect
                    x={display.x}
                    y={display.y}
                    width={display.width}
                    height={display.height}
                    rx={10}
                    fill={active ? "rgba(59,130,246,0.14)" : "rgba(0,0,0,0.22)"}
                    stroke={active ? theme.accent : theme.border}
                    strokeWidth={strokeWidth * (active ? 2.4 : 1.2)}
                  />
                  <text
                    x={display.x + display.width / 2}
                    y={display.y + display.height / 2}
                    textAnchor="middle"
                    dominantBaseline="middle"
                    fill={theme.text}
                    fontSize={Math.max(view.bounds.height / 13, 52)}
                    fontWeight={700}
                  >
                    {display.index + 1}
                  </text>
                  <text
                    x={display.x + display.width * 0.04}
                    y={display.y + display.height * 0.88}
                    fill={theme.textSub}
                    fontSize={Math.max(view.bounds.height / 28, 24)}
                  >
                    {display.resolutionLabel}
                  </text>
                </g>
              );
            })}
          </svg>
        </div>

        <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_360px]">
          <div className="min-w-0">
            <div className="mb-3">
              <div className="text-base font-semibold">
                {selected.title} · {selected.name}
              </div>
              <div className="text-xs" style={{ color: theme.textMuted }}>
                {selected.deviceName ?? selected.id}
              </div>
            </div>

            <div className="grid gap-3 md:grid-cols-3">
              <DisplaySettingSelect
                label="缩放"
                value={scaleValue}
                onChange={setScaleValue}
                options={scaleOptions}
                disabled={!selected.writeCapabilities.scale}
                theme={theme}
              />
              <DisplaySettingSelect
                label="显示器分辨率"
                value={resolutionValue}
                onChange={setResolutionValue}
                options={selected.resolutionOptions}
                disabled={!selected.writeCapabilities.resolution}
                theme={theme}
              />
              <DisplaySettingSelect
                label="刷新率"
                value={refreshRateValue}
                onChange={setRefreshRateValue}
                options={selected.refreshRateOptions}
                disabled={!selected.writeCapabilities.refreshRate}
                theme={theme}
              />
            </div>

            <div className="mt-4 flex flex-wrap items-center gap-2">
              <button
                type="button"
                className="rounded-md px-4 py-2 text-sm"
                style={secondaryButtonStyle(theme)}
                disabled={!canApplyDisplayMode || busyAction === "apply"}
                onClick={applyDisplaySettings}
              >
                应用显示参数
              </button>
              <span className="text-xs" style={{ color: theme.textMuted }}>
                直接写入：{displayCapabilitySummary(selected.writeCapabilities)}
              </span>
            </div>

            {statusMessage ? (
              <div
                className="mt-3 rounded-md px-3 py-2 text-xs"
                style={{
                  border: `1px solid ${theme.border}`,
                  background: theme.frame,
                  color: theme.textSub,
                }}
              >
                {statusMessage}
              </div>
            ) : null}

            <div
              className="mt-4 rounded-md p-3"
              style={{ border: `1px solid ${theme.border}`, background: theme.frame }}
            >
              <div className="mb-3 flex items-center justify-between gap-3">
                <div>
                  <div className="text-sm font-semibold">虚拟显示器</div>
                  <div className="text-xs" style={{ color: theme.textMuted }}>
                    创建后需由 Windows 虚拟显示驱动上报，系统显示设置才会出现新屏幕。
                  </div>
                </div>
                <button
                  type="button"
                  className="rounded-md px-3 py-1.5 text-xs"
                  style={secondaryButtonStyle(theme)}
                  disabled={busyAction === "refresh-virtual-display"}
                  onClick={() =>
                    void runDisplayAction(
                      "refresh-virtual-display",
                      refreshVirtualDisplays,
                      () => "虚拟显示器状态已刷新",
                    )
                  }
                >
                  刷新
                </button>
              </div>

              <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_auto]">
                <select
                  className="min-w-0 rounded-md px-3 py-2 text-sm"
                  style={inputStyle(theme)}
                  value={virtualCreateMode.value}
                  onChange={(event) => setVirtualModeValue(event.target.value)}
                  aria-label="虚拟显示器模式"
                >
                  {virtualView.createModes.map(
                    (mode: {
                      value: string;
                      label: string;
                    }) => (
                      <option key={mode.value} value={mode.value}>
                        {mode.label}
                      </option>
                    ),
                  )}
                </select>
                <button
                  type="button"
                  className="rounded-md px-3 py-2 text-sm"
                  style={secondaryButtonStyle(theme)}
                  disabled={busyAction === "create-virtual-display"}
                  onClick={createVirtualDisplay}
                >
                  创建
                </button>
              </div>

              <div className="mt-3 grid gap-2">
                {virtualView.displays.length ? (
                  virtualView.displays.map((display) => (
                    <div
                      key={display.id}
                      className="grid gap-2 rounded-md p-2 text-xs md:grid-cols-[minmax(0,1fr)_auto]"
                      style={{ border: `1px solid ${theme.border}`, color: theme.textSub }}
                    >
                      <div className="min-w-0">
                        <div className="font-medium" style={{ color: theme.text }}>
                          {display.name} · {display.resolutionLabel} · {display.refreshRateLabel}
                        </div>
                        <div className="mt-1 break-words">
                          {display.statusLabel}
                          {display.displayId ? ` · 系统显示器 ${display.displayId}` : ""}
                          {display.message ? ` · ${display.message}` : ""}
                        </div>
                      </div>
                      <button
                        type="button"
                        className="rounded-md px-3 py-1.5"
                        style={secondaryButtonStyle(theme)}
                        disabled={busyAction === `remove-virtual-display-${display.id}`}
                        onClick={() => removeVirtualDisplay(display.id)}
                      >
                        移除
                      </button>
                    </div>
                  ))
                ) : (
                  <div className="text-xs" style={{ color: theme.textMuted }}>
                    尚未创建虚拟显示器。
                  </div>
                )}
              </div>
            </div>
          </div>

          <aside className="grid gap-2 text-sm">
            <InfoRow label="坐标" value={`${selected.x}, ${selected.y}`} theme={theme} />
            <InfoRow label="尺寸" value={selected.resolutionLabel} theme={theme} />
            <InfoRow label="工作区" value={`${selected.workArea.width} × ${selected.workArea.height}`} theme={theme} />
            <InfoRow label="方向" value={displayOrientationLabel(selected.orientation)} theme={theme} />
            <InfoRow label="缩放" value={selected.scaleLabel} theme={theme} />
            <InfoRow label="刷新率" value={selected.refreshRateLabel} theme={theme} />
            <InfoRow
              label="DPI"
              value={
                selected.dpi.rawX || selected.dpi.x
                  ? `${selected.dpi.rawX ?? selected.dpi.x} / ${selected.dpi.rawY ?? selected.dpi.y}`
                  : "未知"
              }
              theme={theme}
            />
            <InfoRow label="颜色深度" value={selected.bitsPerPixel ? `${selected.bitsPerPixel} bpp` : "未知"} theme={theme} />
          </aside>
        </div>
      </section>
    </div>
  );
}

function DisplaySettingSelect({
  label,
  value,
  options,
  disabled = false,
  onChange,
  theme,
}: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  disabled?: boolean;
  onChange: (value: string) => void;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <label className="block min-w-0 text-sm">
      <span className="mb-1 block" style={{ color: theme.textSub }}>
        {label}
      </span>
      <select
        className="rshare-select h-9 w-full rounded-md px-2 text-sm outline-none"
        value={value}
        disabled={disabled || !options.length}
        onChange={(event) => onChange(event.currentTarget.value)}
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.frame,
          color: disabled ? theme.textMuted : theme.text,
        }}
      >
        {options.length ? (
          options.map((option) => (
            <option
              key={option.value}
              value={option.value}
              style={{ backgroundColor: theme.frame, color: theme.text }}
            >
              {option.label}
            </option>
          ))
        ) : (
          <option value="" style={{ backgroundColor: theme.frame, color: theme.textMuted }}>
            不可用
          </option>
        )}
      </select>
    </label>
  );
}

function displayCapabilitySummary(capabilities: DisplaySettingsDisplayView["writeCapabilities"]) {
  const labels = [
    capabilities.resolution ? "分辨率" : null,
    capabilities.refreshRate ? "刷新率" : null,
    capabilities.scale ? "缩放" : null,
    capabilities.orientation ? "方向" : null,
    capabilities.position ? "位置" : null,
    capabilities.primary ? "主屏" : null,
  ].filter(Boolean);
  return labels.length ? labels.join(" / ") : "当前后端暂不支持，请使用系统设置";
}

function displayScaleOptions(currentScale: number | null) {
  const values = new Set([100, 125, 150, 175, 200, 225, 250]);
  if (currentScale && Number.isFinite(currentScale)) {
    values.add(currentScale);
  }
  return [...values].sort((left, right) => left - right).map((value) => ({
    value: String(value),
    label: `${value}%`,
  }));
}

function displayOperationMessage(
  status: DisplayOperationStatus,
  message: string | null | undefined,
  success: string,
) {
  if (status === "Success") {
    return message ?? success;
  }
  if (status === "RequiresSystemSettings") {
    return message ?? "该设置需要在系统显示设置中完成";
  }
  return message ?? `显示操作返回 ${status}`;
}

function virtualDisplayOperationMessage(result: VirtualDisplayOperationResult) {
  if (result.status === "Created") {
    return result.message ?? "虚拟显示器创建请求已发送";
  }
  if (result.status === "Removed") {
    return result.message ?? "虚拟显示器已移除";
  }
  if (result.status === "DriverUnavailable") {
    return result.message ?? "虚拟显示驱动不可用，尚不能创建系统显示器";
  }
  if (result.status === "Unsupported") {
    return result.message ?? "当前平台不支持虚拟显示器";
  }
  if (result.status === "InvalidMode") {
    return result.message ?? "虚拟显示器参数无效";
  }
  return result.message ?? `虚拟显示操作返回 ${result.status}`;
}

function displayOrientationLabel(orientation: DisplayOrientation) {
  switch (orientation) {
    case "Portrait":
      return "纵向";
    case "LandscapeFlipped":
      return "横向（翻转）";
    case "PortraitFlipped":
      return "纵向（翻转）";
    default:
      return "横向";
  }
}

function DeviceAttributionNotice({
  kind,
  theme,
}: {
  kind: string;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <div
      className="pointer-events-none absolute right-3 top-3 z-20 max-w-[360px] rounded-md px-3 py-2 text-xs"
      style={{
        border: `1px solid ${theme.border}`,
        background: theme.popover,
        color: theme.textSub,
        boxShadow: theme.panelShadow,
      }}
    >
      已枚举到实际{kind}，但当前捕获后端未提供单设备归属；这里临时显示合并输入流。
    </div>
  );
}

function AudioDetail({ snapshot, audioOutputs, theme }: { snapshot: LocalControlsSnapshot | null; audioOutputs: AudioOutputDevice[]; theme: typeof FIGMA_DESKTOP_THEME }) {
  const inputs = snapshot?.audio_inputs ?? [];
  const resolvedOutputs = audioOutputs.length ? audioOutputs : snapshot?.audio_outputs ?? [];
  const audioEvents = (snapshot?.recent_events ?? []).filter((event) => event.device_kind === "Audio").slice(-8).reverse();
  const stream = snapshot?.audio_stream_state;
  const capture = snapshot?.audio_capture_state;
  const selectedInput = inputs.find((device) => device.default) ?? inputs[0];
  const selectedOutput = resolvedOutputs.find((device) => device.default) ?? resolvedOutputs[0];
  const startForwarding = () => void invokeCommand("start_audio_forwarding", { source: selectedInput?.kind === "Microphone" ? "Microphone" : "Loopback", endpoint_id: selectedInput?.endpoint_id ?? null });
  return (
    <div className="grid h-full min-h-0 grid-cols-1 gap-3 overflow-hidden xl:grid-cols-[minmax(0,1fr)_340px]">
      <div className="rshare-scroll min-h-0 overflow-y-auto pr-1">
        <AudioEndpointSection title="音频输入 / 回环" meta={capture?.status ?? "Idle"} columns="2xl:grid-cols-2" theme={theme}>
          {inputs.length ? (
            inputs.map((device) => {
              const endpoint = describeAudioEndpoint(device, "input");
              return (
                <AudioDeviceCard
                  key={device.id}
                  title={device.name}
                  categoryLabel={endpoint.label}
                  subtitle={endpoint.detail}
                  live={device.connected !== false}
                  defaultDevice={Boolean(device.default)}
                  level={device.level_peak ?? 0}
                  meta={[`${device.sample_rate ?? 48000} Hz`, `${device.channel_count ?? 2} ch`, device.muted ? "muted" : "unmuted"]}
                  actions={(
                    <>
                      <button type="button" className="rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => void invokeCommand("start_audio_capture", { source: device.kind === "Loopback" ? "Loopback" : "Microphone", endpoint_id: device.endpoint_id ?? null })}>捕获</button>
                      <button type="button" className="rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => void invokeCommand("start_audio_forwarding", { source: device.kind === "Loopback" ? "Loopback" : "Microphone", endpoint_id: device.endpoint_id ?? null })}>转发</button>
                    </>
                  )}
                  theme={theme}
                />
              );
            })
          ) : (
            <EmptyPanel title="未发现音频输入" detail="等待 Windows Core Audio 枚举或浏览器权限。" theme={theme} />
          )}
        </AudioEndpointSection>
        <AudioEndpointSection title="音频输出" meta={`${resolvedOutputs.length} endpoint`} columns="xl:grid-cols-2 2xl:grid-cols-3" theme={theme}>
          {resolvedOutputs.length ? (
            resolvedOutputs.map((device) => {
              const endpoint = describeAudioEndpoint(device, "output");
              return (
                <AudioDeviceCard
                  key={device.id}
                  title={device.name}
                  categoryLabel={endpoint.label}
                  subtitle={endpoint.detail}
                  live={device.connected !== false}
                  defaultDevice={Boolean(device.default)}
                  level={typeof device.volume_percent === "number" ? device.volume_percent : 0}
                  meta={[typeof device.volume_percent === "number" ? `${device.volume_percent}%` : "unknown volume", device.muted ? "muted" : "unmuted", `${device.channel_count ?? 2} ch`]}
                  actions={(
                    <>
                      <button type="button" className="shrink-0 rounded-md px-3 py-1 text-xs" style={secondaryButtonStyle(theme)} onClick={() => device.endpoint_id ? void invokeCommand("set_audio_output_mute", { endpoint_id: device.endpoint_id, muted: !device.muted }) : undefined}>{device.muted ? "取消静音" : "静音"}</button>
                      <input className="min-w-0 flex-1" type="range" min={0} max={100} defaultValue={device.volume_percent ?? 0} disabled={!device.endpoint_id} onChange={(event) => device.endpoint_id ? void invokeCommand("set_audio_output_volume", { endpoint_id: device.endpoint_id, volume_percent: Number(event.currentTarget.value) }) : undefined} />
                    </>
                  )}
                  theme={theme}
                />
              );
            })
          ) : (
            <EmptyPanel title="未发现音频输出" detail="等待 Windows Core Audio 输出端点枚举。" theme={theme} />
          )}
        </AudioEndpointSection>
      </div>
      <aside className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)] gap-3 overflow-hidden">
        <section className="p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 text-sm font-semibold">远端音频</div><div className="grid grid-cols-2 gap-2 text-xs"><InfoRow label="目标" value={stream?.target_device_id?.slice(0, 8) ?? "无"} theme={theme} /><InfoRow label="状态" value={stream?.active ? "转发中" : (capture?.status ?? "Idle")} theme={theme} /><InfoRow label="延迟" value={stream?.latency_ms ? `${stream.latency_ms} ms` : "-"} theme={theme} /><InfoRow label="帧" value={String(stream?.frames_sent ?? 0)} theme={theme} /></div><div className="mt-3 flex gap-2"><button type="button" className="flex-1 rounded-md px-3 py-2 text-xs" style={secondaryButtonStyle(theme)} onClick={startForwarding}>开始转发</button><button type="button" className="flex-1 rounded-md px-3 py-2 text-xs" style={dangerButtonStyle(theme)} onClick={() => void invokeCommand("stop_audio_forwarding")}>停止</button></div></section>
        <section className="p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 text-sm font-semibold">当前端点</div><InfoRow label="输入" value={selectedInput?.name ?? "无"} theme={theme} /><InfoRow label="输出" value={selectedOutput?.name ?? "无"} theme={theme} /></section>
        <section className="min-h-0 overflow-hidden p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}><div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold">音频记录</h3><span className="text-xs" style={{ color: theme.textMuted }}>最近 {audioEvents.length} 条</span></div><div className="rshare-scroll h-full overflow-auto text-xs">{audioEvents.length ? audioEvents.map((event) => <div key={event.sequence} className="mb-2 grid grid-cols-[78px_minmax(0,1fr)] gap-2"><span style={{ color: theme.textMuted }}>{formatEventTime(event.timestamp_ms)}</span><span className="truncate">{event.summary}</span></div>) : <div style={{ color: theme.textMuted }}>等待音频事件</div>}</div></section>
      </aside>
    </div>
  );
}

function AudioEndpointSection({
  title,
  meta,
  columns,
  children,
  theme,
}: {
  title: string;
  meta: string;
  columns: string;
  children: ReactNode;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  return (
    <section className="mb-3 min-h-0 p-4" style={{ border: `1px solid ${theme.border}`, background: "rgba(255,255,255,0.02)" }}>
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">{title}</h3>
        <span className="shrink-0 text-xs" style={{ color: theme.textMuted }}>{meta}</span>
      </div>
      <div className={`grid grid-cols-1 gap-3 ${columns}`}>{children}</div>
    </section>
  );
}

function AudioDeviceCard({ title, categoryLabel, subtitle, live, defaultDevice, level, meta, actions, theme }: { title: string; categoryLabel: string; subtitle: string; live: boolean; defaultDevice: boolean; level: number; meta: string[]; actions: ReactNode; theme: typeof FIGMA_DESKTOP_THEME }) {
  return (
    <div
      className="flex min-h-[132px] flex-col gap-3 rounded-md p-3"
      style={{
        border: `1px solid ${defaultDevice ? theme.accent : theme.border}`,
        background: defaultDevice ? theme.accentSoft : "rgba(255,255,255,0.035)",
      }}
      title={fullDeviceTooltip(title, subtitle)}
    >
      <div className="flex min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: live ? theme.success : theme.textMuted }} />
            <span className="shrink-0 rounded px-1.5 py-0.5 text-[10px]" style={{ border: `1px solid ${theme.border}`, color: theme.textSub }}>{categoryLabel}</span>
            <div className="truncate text-sm font-semibold">{title}</div>
          </div>
          <div className="mt-1 truncate text-xs" style={{ color: theme.textMuted }}>{subtitle}</div>
        </div>
        {defaultDevice ? <span className="shrink-0 rounded px-2 py-0.5 text-[10px]" style={{ background: theme.accentSoft, color: theme.accent }}>默认</span> : null}
      </div>
      <AudioLevelBar label="level" value={level} theme={theme} />
      <div className="flex flex-wrap gap-2 text-[11px]" style={{ color: theme.textMuted }}>{meta.map((item) => <span key={item}>{item}</span>)}</div>
      <div className="flex min-w-0 items-center gap-2">{actions}</div>
    </div>
  );
}
function secondaryButtonStyle(theme: typeof FIGMA_DESKTOP_THEME) { return { border: `1px solid ${theme.accent}`, background: theme.accentSoft, color: theme.text }; }
function inputStyle(theme: typeof FIGMA_DESKTOP_THEME) { return { border: `1px solid ${theme.border}`, background: theme.frame, color: theme.text }; }
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
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden">
        <HardwareRigView
          kind="keyboard"
          activity={{
            pressedKeys,
            lastKey,
            keyboardEvents,
          }}
          accent={theme.accent}
          theme={theme}
          compact={compact}
        />
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
          最近 {events.length} 条
        </div>
      </div>
      <div className="rshare-scroll min-h-0 flex-1 space-y-1.5 overflow-auto pr-1">
        {events.length ? (
          events.map((event) => (
            <div
              key={`${event.sequence}-${event.summary}`}
              className="grid grid-cols-[86px_minmax(0,1fr)] items-start gap-2 text-xs"
              style={{ color: theme.text }}
            >
              <span style={{ color: theme.textMuted }}>{keyboardEventTime(event)}</span>
              <span
                className="min-w-0 break-words rounded px-2 py-1"
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

function normalizedGamepadButton(value: string) {
  return normalizeInputToken(value);
}

function normalizeHardwareRigVariant(value: string | null | undefined): HardwareRigVariant {
  return value === "gaming" ? "gaming" : "office";
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
  const simulatorLayout = getMouseSimulatorLayoutClasses();

  if (compact) {
    return (
      <div
        className="flex h-full min-h-0 items-center justify-center overflow-hidden p-2"
        style={{
          border: `1px solid ${theme.border}`,
          background: "rgba(255,255,255,0.025)",
        }}
      >
        <MouseHardwarePreview
          leftDown={leftDown}
          rightDown={rightDown}
          middleDown={middleDown}
          backDown={backDown}
          forwardDown={forwardDown}
          wheelActive={wheelActive}
          wheelLabel={wheelLabel}
          theme={theme}
          compact
        />
      </div>
    );
  }

  return (
    <div
      className={simulatorLayout.root}
      style={{
        border: `1px solid ${theme.border}`,
        background: "rgba(255,255,255,0.025)",
      }}
    >
      <div className={simulatorLayout.previewPane}>
        <MouseHardwarePreview
          leftDown={leftDown}
          rightDown={rightDown}
          middleDown={middleDown}
          backDown={backDown}
          forwardDown={forwardDown}
          wheelActive={wheelActive}
          wheelLabel={wheelLabel}
          theme={theme}
        />
      </div>
      {compact ? null : (
      <div className={simulatorLayout.detailsPane}>
        {compact ? null : (
        <>
        <div className="text-sm font-medium">鼠标实时绘制</div>
        <div className="text-xs" style={{ color: theme.textMuted }}>
          全局 {Math.round(x)}, {Math.round(y)} / {displayName} 屏内 {Math.round(displayRelativeX)}, {Math.round(displayRelativeY)} · {display.width} x {display.height} @ {display.x}, {display.y}
        </div>
        </>
        )}
        <div
          className={simulatorLayout.pointerPad}
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
        <div className={simulatorLayout.signalGrid}>
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
        )}
      </div>
      )}
    </div>
  );
}

function MouseHardwarePreview({
  leftDown,
  rightDown,
  middleDown,
  backDown,
  forwardDown,
  wheelActive,
  wheelLabel,
  theme,
  compact = false,
}: {
  leftDown: boolean;
  rightDown: boolean;
  middleDown: boolean;
  backDown: boolean;
  forwardDown: boolean;
  wheelActive: boolean;
  wheelLabel: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
}) {
  return (
    <HardwareRigView
      kind="mouse"
      activity={{
        leftDown,
        rightDown,
        middleDown,
        backDown,
        forwardDown,
        wheelActive,
        wheelLabel,
      }}
      accent={theme.accent}
      theme={theme}
      compact={compact}
    />
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

function stickPercent(value: number) {
  const normalized = Math.max(-1, Math.min(1, Number(value ?? 0) / 32767));
  return Math.round(normalized * 100);
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
  const analogFeedback = buildGamepadAnalogFeedback({
    pressedButtons: pressed,
    leftStickX: gamepad?.left_stick_x ?? 0,
    leftStickY: gamepad?.left_stick_y ?? 0,
    rightStickX: gamepad?.right_stick_x ?? 0,
    rightStickY: gamepad?.right_stick_y ?? 0,
    leftTrigger: gamepad?.left_trigger ?? 0,
    rightTrigger: gamepad?.right_trigger ?? 0,
  });
  const leftTrigger = Math.round(analogFeedback.leftTrigger.value * 100);
  const rightTrigger = Math.round(analogFeedback.rightTrigger.value * 100);

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
        <div className="flex h-full min-h-0 items-center justify-center overflow-hidden">
          <HardwareRigView
            kind="gamepad"
            activity={{
              pressedButtons: analogFeedback.pressedButtons,
              leftStickX: gamepad?.left_stick_x ?? 0,
              leftStickY: gamepad?.left_stick_y ?? 0,
              rightStickX: gamepad?.right_stick_x ?? 0,
              rightStickY: gamepad?.right_stick_y ?? 0,
              leftTrigger: gamepad?.left_trigger ?? 0,
              rightTrigger: gamepad?.right_trigger ?? 0,
            }}
            accent={theme.accent}
            theme={theme}
            compact={compact}
            fitToHeight
            fitMaxHeight={compact ? 170 : 230}
          />
        </div>
      </div>
      {compact ? null : (
      <div className="grid shrink-0 grid-cols-2 gap-2 text-xs lg:grid-cols-4">
        <KeyboardSignal label="已按下" value={pressed.length ? pressed.join(", ") : "无"} theme={theme} />
        <KeyboardSignal label="最近按键" value={gamepad?.last_button ?? "无"} theme={theme} />
        <KeyboardSignal label="按下/抬起" value={`${gamepad?.button_press_count ?? 0}/${gamepad?.button_release_count ?? 0}`} theme={theme} />
        <KeyboardSignal label="按键事件" value={String(gamepad?.button_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="摇杆事件" value={String(gamepad?.axis_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="扳机事件" value={String(gamepad?.trigger_event_count ?? 0)} theme={theme} />
        <KeyboardSignal label="扳机" value={`LT ${leftTrigger}% / RT ${rightTrigger}%`} theme={theme} />
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
      {result ? (
        <div className="mt-3 grid grid-cols-3 gap-2 text-xs">
          <div className="rounded-md px-2 py-1.5" style={{ background: "rgba(255,255,255,0.045)" }}>
            <div style={{ color: theme.textMuted }}>平均延时</div>
            <div className="font-semibold" style={{ color: theme.text }}>
              {result.averageElapsedMs == null ? "-" : `${result.averageElapsedMs} ms`}
            </div>
          </div>
          <div className="rounded-md px-2 py-1.5" style={{ background: "rgba(255,255,255,0.045)" }}>
            <div style={{ color: theme.textMuted }}>最大延时</div>
            <div className="font-semibold" style={{ color: theme.text }}>
              {result.maxElapsedMs == null ? "-" : `${result.maxElapsedMs} ms`}
            </div>
          </div>
          <div className="rounded-md px-2 py-1.5" style={{ background: "rgba(255,255,255,0.045)" }}>
            <div style={{ color: theme.textMuted }}>注入</div>
            <div className="font-semibold" style={{ color: theme.text }}>
              {result.successCount ?? 0}/{result.totalCount ?? 0}
            </div>
          </div>
        </div>
      ) : null}
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
  const keyboardConnects = snapshot.driver.filter_keyboard_connects ?? 0;
  const mouseConnects = snapshot.driver.filter_mouse_connects ?? 0;
  const keyboardEvents = snapshot.driver.filter_keyboard_events ?? 0;
  const mouseEvents = snapshot.driver.filter_mouse_events ?? 0;
  const filterStats = snapshot.driver.filter_active
    ? ` attach ${keyboardConnects}/${mouseConnects} events ${keyboardEvents}/${mouseEvents}`
    : "";
  return `${snapshot.driver.status}${version}${filter}${vhid}${filterStats}`;
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

function HardwareAssetSettingsPanel({
  theme,
}: {
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const {
    assets,
    installed,
    selectedIds,
    loading,
    error,
    setSelectedId,
    refresh,
    importFile,
    exportAsset,
  } = useHardwareAssetCatalog();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const choices = buildHardwareAssetChoices(assets);
  const selectedKeyboard = resolveSelectedHardwareAsset(
    assets,
    "keyboard",
    selectedIds.keyboard,
  ) as HardwareRigDefinition | null;
  const selectedMouse = resolveSelectedHardwareAsset(
    assets,
    "mouse",
    selectedIds.mouse,
  ) as HardwareRigDefinition | null;
  const selectedGamepad = resolveSelectedHardwareAsset(
    assets,
    "gamepad",
    selectedIds.gamepad,
  ) as HardwareRigDefinition | null;
  const presetOptions = getHardwareAssetPresetOptions() as Array<{
    key: HardwareRigVariant;
    label: string;
  }>;
  const setHardwareRigVariant = (variant: HardwareRigVariant) => {
    setSelectedId("keyboard", builtinHardwareAssetId("keyboard", variant));
    setSelectedId("mouse", builtinHardwareAssetId("mouse", variant));
  };

  const handleImportFile = async (file: File | null | undefined) => {
    if (!file) {
      return;
    }
    setBusyAction("import");
    setMessage(null);
    try {
      await importFile(file);
      setMessage(`已导入 ${file.name}`);
    } catch (importError) {
      setMessage(`导入失败：${String(importError)}`);
    } finally {
      setBusyAction(null);
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
    }
  };

  const handleExport = async (assetId: string) => {
    setBusyAction(`export:${assetId}`);
    setMessage(null);
    try {
      await exportAsset(assetId);
      setMessage("已生成资产压缩包");
    } catch (exportError) {
      setMessage(`导出失败：${String(exportError)}`);
    } finally {
      setBusyAction(null);
    }
  };

  const renderSelect = (
    kind: HardwareRigKind,
    label: string,
    icon: ReactNode,
    selected: HardwareRigDefinition | null,
  ) => {
    const options = choices[kind] ?? [];
    return (
      <label className="block min-w-0 flex-1 text-sm">
        <span className="mb-1 flex items-center gap-2" style={{ color: theme.textSub }}>
          {icon}
          {label}
        </span>
        <select
          className="rshare-select h-9 w-full rounded-md px-2 text-sm outline-none"
          value={selected?.id ?? selectedIds[kind]}
          onChange={(event) => setSelectedId(kind, event.currentTarget.value)}
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.frame,
            color: theme.text,
          }}
        >
          {options.map((option) => (
            <option
              key={option.id}
              value={option.id}
              style={{ backgroundColor: theme.frame, color: theme.text }}
            >
              {option.name}
            </option>
          ))}
        </select>
      </label>
    );
  };

  return (
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
          <FileText size={18} />
        </div>
        <div className="min-w-0">
          <h2 className="text-lg font-semibold">硬件资产</h2>
          <p className="text-sm" style={{ color: theme.textMuted }}>
            贴图、按键区域和导入包状态。
          </p>
        </div>
      </div>

      <div
        className="mb-4 rounded-md p-3"
        style={{
          border: `1px solid ${theme.border}`,
          background: theme.frame,
        }}
      >
        <div className="mb-2 text-sm font-medium">贴图设置</div>
        <div className="grid w-full max-w-[260px] grid-cols-2 gap-1 rounded-md p-1" style={{ background: "rgba(255,255,255,0.04)" }}>
          {presetOptions.map((option) => {
            const active =
              selectedIds.keyboard === builtinHardwareAssetId("keyboard", option.key) &&
              selectedIds.mouse === builtinHardwareAssetId("mouse", option.key);
            return (
              <button
                key={option.key}
                type="button"
                className="rounded px-2 py-1.5 text-xs transition"
                style={{
                  border: `1px solid ${active ? theme.accent : "transparent"}`,
                  background: active ? theme.accentSoft : "transparent",
                  color: active ? theme.text : theme.textMuted,
                }}
                onClick={() => setHardwareRigVariant(option.key)}
              >
                {option.label}
              </button>
            );
          })}
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-3">
        {renderSelect(
          "keyboard",
          "键盘资产",
          <Keyboard size={14} />,
          selectedKeyboard,
        )}
        {renderSelect(
          "mouse",
          "鼠标资产",
          <MousePointer2 size={14} />,
          selectedMouse,
        )}
        {renderSelect(
          "gamepad",
          "手柄资产",
          <Gamepad2 size={14} />,
          selectedGamepad,
        )}
      </div>

      <div className="mt-4 flex flex-wrap gap-2">
        <input
          ref={fileInputRef}
          className="hidden"
          type="file"
          accept=".zip,.rshare-asset.zip,application/zip"
          onChange={(event) => void handleImportFile(event.currentTarget.files?.[0])}
        />
        <button
          type="button"
          className="inline-flex items-center gap-2 rounded-md px-3 py-2 text-sm transition"
          style={secondaryButtonStyle(theme)}
          disabled={busyAction === "import"}
          onClick={() => fileInputRef.current?.click()}
        >
          <Upload size={14} />
          导入
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-2 rounded-md px-3 py-2 text-sm transition"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.frame,
            color: theme.textSub,
          }}
          disabled={loading}
          onClick={() => void refresh()}
        >
          <RotateCcw size={14} />
          刷新
        </button>
      </div>

      {error || message ? (
        <div
          className="mt-3 rounded-md px-3 py-2 text-xs"
          style={{
            border: `1px solid ${theme.border}`,
            background: theme.frame,
            color: error ? "#ffb5c0" : theme.textSub,
          }}
        >
          {message ?? error}
        </div>
      ) : null}

      <div className="mt-4 space-y-2">
        {installed.length ? (
          installed.map((asset) => (
            <div
              key={asset.id}
              className="flex items-center gap-3 rounded-md px-3 py-2 text-sm"
              style={{
                border: `1px solid ${theme.border}`,
                background: theme.frame,
              }}
            >
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{asset.name}</div>
                <div className="truncate text-xs" style={{ color: theme.textMuted }}>
                  {hardwareAssetKindLabel(asset.kind)} · {asset.id}
                </div>
              </div>
              <button
                type="button"
                className="inline-flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs"
                style={{
                  border: `1px solid ${theme.accent}`,
                  background: theme.accentSoft,
                  color: theme.text,
                }}
                disabled={busyAction === `export:${asset.id}`}
                onClick={() => void handleExport(asset.id)}
              >
                <Download size={13} />
                导出
              </button>
            </div>
          ))
        ) : (
          <div
            className="rounded-md px-3 py-2 text-xs"
            style={{
              border: `1px solid ${theme.border}`,
              background: theme.frame,
              color: theme.textMuted,
            }}
          >
            当前只有内置资产。
          </div>
        )}
      </div>
    </section>
  );
}

function hardwareAssetKindLabel(kind: string) {
  if (kind === "keyboard") {
    return "键盘";
  }
  if (kind === "mouse") {
    return "鼠标";
  }
  if (kind === "gamepad") {
    return "手柄";
  }
  return kind;
}

function SettingsPage({
  acceptance,
  localDevice,
  inputMode,
  privilegeState,
  mobileAccess,
  mobileAccessError,
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
    localDisplayCount: number;
    localReady: boolean;
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
  mobileAccess: MobileAccessSnapshot | null;
  mobileAccessError: string | null;
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
  const [selectedSection, setSelectedSection] = useState<SettingsSectionKey>("local");
  const selectedSectionMeta =
    SETTINGS_LAYOUT_SECTIONS.find((section) => section.key === selectedSection) ??
    SETTINGS_LAYOUT_SECTIONS[0];
  const panelStyle = {
    background: theme.sidebar,
    border: `1px solid ${theme.border}`,
    boxShadow: theme.panelShadow,
  };
  const mutedIconStyle = {
    background: "rgba(255,255,255,0.04)",
    color: theme.textSub,
  };
  const mobileAccessView = buildMobileAccessViewModel(mobileAccess);
  const sectionSummary: Record<SettingsSectionKey, string> = {
    local: localDevice.name,
    service: service.online ? "运行中" : "已停止",
    mobile: mobileAccessView.available ? `端口 ${mobileAccessView.port ?? "未知"}` : "不可用",
    hardware: "贴图设置",
    input: inputMode.current,
    appearance:
      getThemeModeOptions().find((option) => option.key === themeMode)?.label ??
      themeMode,
    acceptance: acceptance.nextStep,
  };
  const renderSectionHeader = (
    icon: ReactNode,
    title: string,
    description: string,
    accent = false,
  ) => (
    <div className="mb-5 flex items-center gap-3">
      <div
        className="flex h-11 w-11 items-center justify-center rounded-md"
        style={accent ? { background: theme.accentSoft, color: theme.accent } : mutedIconStyle}
      >
        {icon}
      </div>
      <div className="min-w-0">
        <h2 className="text-lg font-semibold">{title}</h2>
        <p className="text-sm" style={{ color: theme.textMuted }}>
          {description}
        </p>
      </div>
    </div>
  );

  let sectionContent: ReactNode;
  if (selectedSection === "service") {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <Wifi size={18} />,
          "服务状态",
          "当前守护进程会话的快速运行信息。",
        )}

        <div className="grid gap-3 text-sm md:grid-cols-2">
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
              service.online ? "rgba(197, 48, 48, 0.55)" : theme.accent
            }`,
            opacity: busy ? 0.7 : 1,
          }}
          disabled={busy}
          onClick={onToggleService}
        >
          {service.online ? "停止服务" : "启动服务"}
        </button>
      </section>
    );
  } else if (selectedSection === "mobile") {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <Smartphone size={18} />,
          "移动端控制",
          "用手机浏览器连接本机移动网关，模拟鼠标、按键和手机输入法文本。",
          true,
        )}

        <div className="grid gap-3 text-sm md:grid-cols-2">
          <InfoRow
            label="网关"
            value={mobileAccessView.available ? "可用" : "不可用"}
            theme={theme}
          />
          <InfoRow label="监听地址" value={mobileAccessView.bindAddress} theme={theme} />
          <InfoRow label="端口" value={mobileAccessView.port == null ? "不可用" : String(mobileAccessView.port)} theme={theme} />
          <InfoRow
            label="访问令牌"
            value={mobileAccessView.token ? `${mobileAccessView.token.slice(0, 8)}...` : "不可用"}
            theme={theme}
          />
          <InfoRow
            label="手机连接"
            value={`${mobileAccessView.clientStatus} · ${mobileAccessView.clientDetail}`}
            theme={theme}
          />
        </div>

        <div
          className="mt-4 grid gap-4 rounded-md px-4 py-3 text-sm lg:grid-cols-[148px_minmax(0,1fr)]"
          style={{ border: `1px solid ${theme.border}`, background: theme.frame }}
        >
          <div
            className="flex h-[148px] w-[148px] items-center justify-center rounded-md"
            style={{
              background: mobileAccessView.qrCodeSvgDataUri ? "#ffffff" : theme.surface,
              border: `1px solid ${theme.border}`,
            }}
          >
            {mobileAccessView.qrCodeSvgDataUri ? (
              <img
                className="h-[132px] w-[132px]"
                src={mobileAccessView.qrCodeSvgDataUri}
                alt={mobileAccessView.qrCodeAlt}
              />
            ) : (
              <QrCode size={42} style={{ color: theme.textMuted }} />
            )}
          </div>
          <div className="min-w-0 self-center">
            <div className="mb-2 text-xs uppercase tracking-[0.16em]" style={{ color: theme.textMuted }}>
              {mobileAccessView.urlLabel}
            </div>
            <div className="break-all font-medium">{mobileAccessView.url}</div>
            <div className="mt-2 text-sm leading-6" style={{ color: theme.textMuted }}>
              {mobileAccessError ?? mobileAccessView.summary}
            </div>
          </div>
        </div>

        <div className="mt-4 flex flex-wrap gap-2">
          <button
            type="button"
            className="flex items-center gap-2 rounded-md px-4 py-2 text-sm transition"
            style={{
              background: theme.accentSoft,
              color: theme.accent,
              border: `1px solid ${theme.accent}`,
              opacity: mobileAccessView.available ? 1 : 0.6,
            }}
            disabled={!mobileAccessView.available}
            onClick={() => {
              void navigator.clipboard?.writeText(mobileAccessView.url);
            }}
          >
            <Copy size={14} />
            复制链接
          </button>
          <a
            className="flex items-center gap-2 rounded-md px-4 py-2 text-sm transition"
            style={{
              background: theme.frame,
              color: mobileAccessView.available ? theme.text : theme.textMuted,
              border: `1px solid ${theme.border}`,
              pointerEvents: mobileAccessView.available ? "auto" : "none",
              opacity: mobileAccessView.available ? 1 : 0.6,
            }}
            href={mobileAccessView.available ? mobileAccessView.url : undefined}
            target="_blank"
            rel="noreferrer"
          >
            <ExternalLink size={14} />
            打开
          </a>
        </div>
      </section>
    );
  } else if (selectedSection === "hardware") {
    sectionContent = <HardwareAssetSettingsPanel theme={theme} />;
  } else if (selectedSection === "input") {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <LayoutGrid size={18} />,
          "输入后端",
          "当前输入模式以及降级可见性都来自守护进程。",
        )}

        <div className="grid gap-3 text-sm md:grid-cols-2">
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
    );
  } else if (selectedSection === "appearance") {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <Settings size={18} />,
          "界面风格",
          "选择浅色、深色或跟随系统。",
        )}

        <div className="flex flex-wrap gap-2">
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
    );
  } else if (selectedSection === "acceptance") {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <Monitor size={18} />,
          "实机验收",
          "打开另一台机器前，先确认后台、布局和输入主链路都已就绪。",
          true,
        )}

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
    );
  } else {
    sectionContent = (
      <section className="p-5" style={panelStyle}>
        {renderSectionHeader(
          <Settings size={18} />,
          "本机信息",
          "当前界面显示的是守护进程快照提供的最小设置集。",
          true,
        )}

        <div className="grid gap-3 text-sm md:grid-cols-2">
          <InfoRow label="设备名" value={localDevice.name} theme={theme} />
          <InfoRow label="主机名" value={localDevice.hostname} theme={theme} />
          <InfoRow label="监听地址" value={localDevice.bindAddress} theme={theme} />
          <InfoRow label="发现端口" value={localDevice.discoveryPort == null ? "不可用" : String(localDevice.discoveryPort)} theme={theme} />
          <InfoRow label="守护进程 PID" value={localDevice.pid == null ? "不可用" : String(localDevice.pid)} theme={theme} />
          <InfoRow label="权限状态" value={privilegeState} theme={theme} />
        </div>
      </section>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden lg:flex-row">
      <aside
        className="rshare-scroll flex shrink-0 flex-col gap-2 overflow-auto p-3 lg:w-[300px]"
        style={{
          borderRight: `1px solid ${theme.border}`,
          background: theme.sidebar,
        }}
      >
        <div className="mb-2 px-2">
          <div className="text-lg font-semibold">设置</div>
          <div className="mt-1 text-xs leading-5" style={{ color: theme.textMuted }}>
            {selectedSectionMeta.description}
          </div>
        </div>
        {SETTINGS_LAYOUT_SECTIONS.map((section) => {
          const active = selectedSection === section.key;
          return (
            <button
              key={section.key}
              type="button"
              aria-pressed={active}
              className="w-full rounded-md px-3 py-2.5 text-left transition"
              style={{
                border: `1px solid ${active ? theme.accent : "transparent"}`,
                background: active ? theme.accentSoft : "transparent",
                color: active ? theme.text : theme.textSub,
              }}
              onClick={() => setSelectedSection(section.key)}
            >
              <div className="flex items-center justify-between gap-3">
                <span className="text-sm font-medium">{section.label}</span>
                <span className="truncate text-xs" style={{ color: theme.textMuted }}>
                  {sectionSummary[section.key]}
                </span>
              </div>
              <div className="mt-1 truncate text-xs" style={{ color: theme.textMuted }}>
                {section.description}
              </div>
            </button>
          );
        })}
      </aside>

      <div className="rshare-scroll min-h-0 flex-1 overflow-auto p-4">
        <div className="mx-auto max-w-[980px]">
          {sectionContent}
        </div>
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
