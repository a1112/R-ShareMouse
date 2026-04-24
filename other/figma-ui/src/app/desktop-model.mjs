const DEVICE_COLORS = ["#5b8bd6", "#49b35c", "#d6a64b", "#9b6ef3", "#e56b6f"];
const LOCAL_DEVICE_COLOR = "#60a5fa";
const LAYOUT_SCALE = 0.12;
const CANVAS_ORIGIN_X = 80;
const CANVAS_ORIGIN_Y = 170;

function deviceColor(index) {
  return DEVICE_COLORS[index % DEVICE_COLORS.length];
}

function buildLocalDevice(status) {
  const online = Boolean(status);

  return {
    id: status?.device_id ?? "local-device",
    kind: "local",
    name: status?.device_name ?? "本机",
    hostname: status?.hostname ?? "离线",
    color: LOCAL_DEVICE_COLOR,
    online,
    connected: false,
    type: "desktop",
    expanded: true,
    address: status?.bind_address ?? "不可用",
    port: status?.discovery_port ?? null,
    lastSeenLabel: online ? "当前机器" : "守护进程离线",
  };
}

function buildRemoteDevice(device, index) {
  const isLaptop = /book|laptop/i.test(device.name) || /macbook/i.test(device.hostname ?? "");

  return {
    id: device.id,
    kind: "remote",
    name: device.name,
    hostname: device.hostname,
    color: deviceColor(index),
    online: true,
    connected: Boolean(device.connected),
    type: isLaptop ? "laptop" : "desktop",
    expanded: true,
    address: device.addresses?.[0] ?? "未知",
    port: null,
    lastSeenLabel:
      device.last_seen_secs == null
        ? "刚刚"
        : `${device.last_seen_secs} 秒前`,
  };
}

function buildLayoutMonitor(device, index, kind) {
  const offsetX = index === 0 ? 0 : 300 + (index - 1) * 268;
  const offsetY = index === 0 ? 0 : (index % 2 === 0 ? -54 : 42);

  return {
    id: `${device.id}-monitor`,
    deviceId: device.id,
    label: index === 0 ? "A" : String.fromCharCode(65 + index),
    name: kind === "local" ? `${device.name} 显示器` : `${device.name} 屏幕`,
    resWidth: kind === "local" ? 2560 : 1920,
    resHeight: kind === "local" ? 1440 : 1080,
    color: device.color,
    x: 80 + offsetX,
    y: 170 + offsetY,
    w: kind === "local" ? 307 : 230,
    h: kind === "local" ? 174 : 130,
    primary: kind === "local",
    enabled: true,
  };
}

function findRememberedDisplay(rememberedLayout, deviceId, displayId) {
  return rememberedLayout?.nodes
    ?.find((node) => node.device_id === deviceId)
    ?.displays?.find((display) => (display.display_id ?? "primary") === displayId);
}

function buildLayoutFromVisibleGraph(visibleLayout, rememberedLayout, localDevice, remoteDevices) {
  if (!visibleLayout?.nodes?.length) {
    return null;
  }

  const deviceLookup = new Map([
    [localDevice.id, localDevice],
    ...remoteDevices.map((device) => [device.id, device]),
  ]);
  const layoutDevices = visibleLayout.nodes
    .map((node) => deviceLookup.get(node.device_id))
    .filter(Boolean);

  if (!layoutDevices.length) {
    return null;
  }

  const layoutMonitors = [];
  for (const node of visibleLayout.nodes) {
    const device = deviceLookup.get(node.device_id);
    if (!device) {
      continue;
    }

    for (const display of node.displays ?? []) {
      const monitorIndex = layoutMonitors.length;
      const width = Number(display.width ?? 1920);
      const height = Number(display.height ?? 1080);
      const displayId = display.display_id ?? "primary";
      const rememberedDisplay = findRememberedDisplay(
        rememberedLayout,
        node.device_id,
        displayId,
      );
      layoutMonitors.push({
        id: `${node.device_id}-${displayId}`,
        deviceId: node.device_id,
        displayId,
        rememberedX: Number(rememberedDisplay?.x ?? display.x ?? 0),
        rememberedY: Number(rememberedDisplay?.y ?? display.y ?? 0),
        visibleX: Number(display.x ?? 0),
        visibleY: Number(display.y ?? 0),
        label: String.fromCharCode(65 + monitorIndex),
        name:
          device.kind === "local"
            ? `${device.name} 显示器`
            : `${device.name} 屏幕`,
        resWidth: width,
        resHeight: height,
        color: device.color,
        x: CANVAS_ORIGIN_X + Number(display.x ?? 0) * LAYOUT_SCALE,
        y: CANVAS_ORIGIN_Y + Number(display.y ?? 0) * LAYOUT_SCALE,
        w: Math.max(96, Math.round(width * LAYOUT_SCALE)),
        h: Math.max(64, Math.round(height * LAYOUT_SCALE)),
        primary: Boolean(display.primary),
        enabled: true,
      });
    }
  }

  return {
    devices: layoutDevices,
    monitors: layoutMonitors,
  };
}

function primaryDisplay(node) {
  return (node.displays ?? []).find((display) => display.primary) ?? node.displays?.[0] ?? null;
}

function rebuildHorizontalLinks(nodes) {
  const sorted = [...nodes].sort((left, right) => {
    const leftDisplay = primaryDisplay(left);
    const rightDisplay = primaryDisplay(right);
    return Number(leftDisplay?.x ?? 0) - Number(rightDisplay?.x ?? 0);
  });

  const links = [];
  for (let index = 0; index < sorted.length - 1; index += 1) {
    const left = sorted[index].device_id;
    const right = sorted[index + 1].device_id;
    links.push({
      from_device: left,
      from_edge: "Right",
      to_device: right,
      to_edge: "Left",
    });
    links.push({
      from_device: right,
      from_edge: "Left",
      to_device: left,
      to_edge: "Right",
    });
  }
  return links;
}

export function updateRememberedLayoutFromVisibleMonitors(rememberedLayout, monitors) {
  if (!rememberedLayout?.nodes) {
    return rememberedLayout;
  }

  const monitorByDisplay = new Map(
    (monitors ?? []).map((monitor) => [
      `${monitor.deviceId}:${monitor.displayId ?? monitor.id?.split("-").pop() ?? "primary"}`,
      monitor,
    ]),
  );

  const nodes = rememberedLayout.nodes.map((node) => ({
      ...node,
      displays: (node.displays ?? []).map((display) => {
        const displayId = display.display_id ?? "primary";
        const monitor = monitorByDisplay.get(`${node.device_id}:${displayId}`);
        if (!monitor) {
          return { ...display };
        }

        const rememberedX = Number(monitor.rememberedX ?? display.x ?? 0);
        const rememberedY = Number(monitor.rememberedY ?? display.y ?? 0);
        const visibleX = Number(monitor.visibleX ?? rememberedX);
        const visibleY = Number(monitor.visibleY ?? rememberedY);

        return {
          ...display,
          x: Math.round(
            rememberedX +
              (Number(monitor.x) - (CANVAS_ORIGIN_X + visibleX * LAYOUT_SCALE)) /
                LAYOUT_SCALE,
          ),
          y: Math.round(
            rememberedY +
              (Number(monitor.y) - (CANVAS_ORIGIN_Y + visibleY * LAYOUT_SCALE)) /
                LAYOUT_SCALE,
          ),
        };
      }),
    }));

  return {
    ...rememberedLayout,
    nodes,
    links: rebuildHorizontalLinks(nodes),
  };
}

function parseBackendHealth(backendHealth) {
  if (!backendHealth) {
    return { health: "未知", reason: null };
  }

  if (typeof backendHealth === "string") {
    return { health: backendHealth, reason: null };
  }

  if (typeof backendHealth === "object" && backendHealth.Degraded) {
    return {
      health: "Degraded",
      reason: backendHealth.Degraded.reason ?? null,
    };
  }

  return { health: "未知", reason: null };
}

function statusCheck(pass, warn = false) {
  if (pass) {
    return "pass";
  }

  return warn ? "warn" : "block";
}

function backendDiagnosticLabel(backend) {
  if (!backend || typeof backend !== "object") {
    return "unknown unknown";
  }

  const mode = typeof backend.mode === "string" ? backend.mode : "unknown";
  const health = typeof backend.health === "string" ? backend.health : "unknown";
  return `${mode} ${health}`;
}

export function buildLocalControlsViewModel(snapshot, options = {}) {
  const error = options.error ?? null;
  const confirmingInputTest = options.confirmingInputTest ?? null;
  const keyboard = snapshot?.keyboard ?? {};
  const mouse = snapshot?.mouse ?? {};
  const display = snapshot?.display ?? {};
  const gamepads = snapshot?.gamepads ?? [];
  const gamepad = gamepads.find((item) => item.connected) ?? gamepads[0] ?? null;
  const recentEvents = snapshot?.recent_events ?? [];
  const latestEvent = recentEvents.length ? recentEvents[recentEvents.length - 1] : null;

  return {
    available: Boolean(snapshot && !error),
    error,
    keyboard: {
      status: keyboard.detected ? "capturing" : "missing",
      lastKey: keyboard.last_key ?? null,
      pressedKeys: keyboard.pressed_keys ?? [],
      eventCount: Number(keyboard.event_count ?? 0),
      captureSource: keyboard.capture_source ?? "unknown",
      testLabel:
        confirmingInputTest === "keyboard"
          ? "confirm keyboard injection"
          : "keyboard injection test",
    },
    mouse: {
      status: mouse.detected ? "capturing" : "missing",
      position: {
        x: Number(mouse.x ?? 0),
        y: Number(mouse.y ?? 0),
      },
      pressedButtons: mouse.pressed_buttons ?? [],
      wheel: {
        x: Number(mouse.wheel_delta_x ?? 0),
        y: Number(mouse.wheel_delta_y ?? 0),
        totalX: Number(mouse.wheel_total_x ?? 0),
        totalY: Number(mouse.wheel_total_y ?? 0),
        events: Number(mouse.wheel_event_count ?? 0),
      },
      eventCount: Number(mouse.event_count ?? 0),
      stats: {
        moves: Number(mouse.move_count ?? 0),
        buttonEvents: Number(mouse.button_event_count ?? 0),
        buttonPresses: Number(mouse.button_press_count ?? 0),
        buttonReleases: Number(mouse.button_release_count ?? 0),
      },
      display: {
        id: mouse.current_display_id ?? null,
        index:
          mouse.current_display_index === undefined
            ? null
            : mouse.current_display_index,
        relativeX: Number(mouse.display_relative_x ?? mouse.x ?? 0),
        relativeY: Number(mouse.display_relative_y ?? mouse.y ?? 0),
      },
      testLabel:
        confirmingInputTest === "mouse"
          ? "confirm mouse injection"
          : "mouse injection test",
    },
    gamepad: {
      status: gamepad?.connected ? "gilrs-connected" : "waiting",
      name: gamepad?.name ?? "unavailable",
      pressedButtons:
        gamepad?.pressed_buttons ??
        (gamepad?.buttons ?? [])
          .filter((button) => button.pressed)
          .map((button) =>
            typeof button.button === "string"
              ? button.button
              : Object.keys(button.button ?? {})[0] ?? "Unknown",
          ),
      sticks: {
        left: {
          x: Number(gamepad?.left_stick_x ?? 0),
          y: Number(gamepad?.left_stick_y ?? 0),
        },
        right: {
          x: Number(gamepad?.right_stick_x ?? 0),
          y: Number(gamepad?.right_stick_y ?? 0),
        },
      },
      triggers: {
        left: Number(gamepad?.left_trigger ?? 0),
        right: Number(gamepad?.right_trigger ?? 0),
      },
      stats: {
        events: Number(gamepad?.event_count ?? 0),
        buttonEvents: Number(gamepad?.button_event_count ?? 0),
        buttonPresses: Number(gamepad?.button_press_count ?? 0),
        buttonReleases: Number(gamepad?.button_release_count ?? 0),
        stickEvents: Number(gamepad?.axis_event_count ?? 0),
        triggerEvents: Number(gamepad?.trigger_event_count ?? 0),
      },
      lastButton: gamepad?.last_button ?? null,
      lastAxis: gamepad?.last_axis ?? null,
      virtualStatus: snapshot?.virtual_gamepad?.status ?? "not_implemented",
      virtualDetail:
        snapshot?.virtual_gamepad?.detail ?? "Virtual HID not implemented",
    },
    display: {
      count: Number(display.display_count ?? 0),
      primary: {
        width: Number(display.primary_width ?? 0),
        height: Number(display.primary_height ?? 0),
      },
      layout: {
        width: Number(display.layout_width ?? 0),
        height: Number(display.layout_height ?? 0),
      },
      virtualOrigin: {
        x: Number(display.virtual_x ?? 0),
        y: Number(display.virtual_y ?? 0),
      },
      displays: display.displays ?? [],
    },
    backend: {
      capture: backendDiagnosticLabel(snapshot?.capture_backend),
      inject: backendDiagnosticLabel(snapshot?.inject_backend),
      privilegeState: snapshot?.privilege_state ?? "unknown",
    },
    latestEvent: latestEvent
      ? {
          deviceKind: latestEvent.device_kind,
          summary: latestEvent.summary,
          injectedLoopback: ["Injected", "InjectedLoopback", "VirtualDevice"].includes(
            latestEvent.source,
          ),
        }
      : null,
  };
}

function trayStateLabel(state) {
  switch (state) {
    case "Running":
      return "运行中";
    case "Starting":
      return "启动中";
    case "Failed":
      return "失败";
    case "Unavailable":
    default:
      return "未接入";
  }
}

function buildAcceptanceChecks(acceptance, status, inputMode) {
  return [
    {
      key: "background",
      label: "后台服务",
      state: statusCheck(acceptance.backgroundReady),
      detail: acceptance.daemonOnline
        ? `daemon 后台运行，PID ${status?.pid ?? "未知"}`
        : "daemon 未运行，desktop 会在 IPC 不可用时尝试拉起",
    },
    {
      key: "tray",
      label: "托盘归属",
      state: statusCheck(acceptance.trayOwnedByDaemon && acceptance.trayState === "Running", acceptance.trayOwnedByDaemon),
      detail: acceptance.trayOwnedByDaemon
        ? `托盘归属 daemon，当前状态：${trayStateLabel(acceptance.trayState)}`
        : "托盘归属未声明为 daemon",
    },
    {
      key: "endpoint",
      label: "本机端点",
      state: statusCheck(acceptance.daemonOnline && acceptance.localEndpoint !== "不可用"),
      detail: acceptance.localEndpoint,
    },
    {
      key: "discovery",
      label: "局域网发现",
      state: statusCheck(acceptance.discoveredDevices > 0, acceptance.daemonOnline),
      detail: `已发现 ${acceptance.discoveredDevices} 台，已连接 ${acceptance.connectedDevices} 台`,
    },
    {
      key: "layout",
      label: "布局接管",
      state: statusCheck(acceptance.visibleLayoutDevices > 1, acceptance.daemonOnline),
      detail: `Layout 当前显示 ${acceptance.visibleLayoutDevices} 个在线节点`,
    },
    {
      key: "input",
      label: "输入后端",
      state: statusCheck(acceptance.inputReady),
      detail: `${inputMode.current} · ${inputMode.health}`,
    },
    {
      key: "dual-machine",
      label: "双机验收",
      state: statusCheck(acceptance.dualMachineReady, acceptance.daemonOnline),
      detail: acceptance.nextStep,
    },
  ];
}

function fallbackAcceptance(payload, status, remoteDevices, layout, inputMode) {
  const daemonOnline = Boolean(status);
  const backgroundReady =
    daemonOnline &&
    (status?.background_owner ?? "Daemon") === "Daemon" &&
    (status?.background_mode ?? "BackgroundProcess") === "BackgroundProcess";
  const trayOwnedByDaemon = daemonOnline && (status?.tray_owner ?? "Daemon") === "Daemon";
  const trayState = status?.tray_state ?? "Unavailable";
  const visibleLayoutDevices = payload?.visible_layout?.nodes?.length ?? layout.devices.length;
  const inputReady = daemonOnline && Boolean(status?.input_mode) && inputMode.health === "Healthy";
  const dualMachineReady =
    backgroundReady &&
    inputReady &&
    remoteDevices.length > 0 &&
    visibleLayoutDevices > 1 &&
    !payload?.layout_error;

  let nextStep = "启动守护进程后进行双机实机验收";
  if (daemonOnline && !inputReady) {
    nextStep = "检查输入后端权限或降级原因";
  } else if (daemonOnline && remoteDevices.length === 0) {
    nextStep = "打开另一台机器并保持同一局域网，等待自动发现";
  } else if (daemonOnline && !dualMachineReady) {
    nextStep = "确认设备进入 Layout 并保存布局后开始连接";
  } else if (dualMachineReady) {
    nextStep = "打开另一台机器并连接设备，开始边缘切换验收";
  }

  return {
    daemonOnline,
    backgroundReady,
    trayOwnedByDaemon,
    trayState,
    localEndpoint: status?.bind_address ?? "不可用",
    discoveredDevices: remoteDevices.length,
    connectedDevices: remoteDevices.filter((device) => device.connected).length,
    visibleLayoutDevices,
    inputReady,
    dualMachineReady,
    nextStep,
    autoStarted: Boolean(payload?.auto_started ?? status?.started_by_desktop),
  };
}

function buildAcceptance(payload, status, remoteDevices, layout, inputMode) {
  const raw = payload?.acceptance;
  const acceptance = raw
    ? {
        daemonOnline: Boolean(raw.daemon_online),
        backgroundReady: Boolean(raw.background_ready),
        trayOwnedByDaemon: Boolean(raw.tray_owned_by_daemon),
        trayState: raw.tray_state ?? "Unavailable",
        localEndpoint: raw.local_endpoint ?? status?.bind_address ?? "不可用",
        discoveredDevices: Number(raw.discovered_devices ?? remoteDevices.length),
        connectedDevices: Number(raw.connected_devices ?? 0),
        visibleLayoutDevices: Number(raw.visible_layout_devices ?? layout.devices.length),
        inputReady: Boolean(raw.input_ready),
        dualMachineReady: Boolean(raw.dual_machine_ready),
        nextStep: raw.next_step ?? "继续完成实机验收",
        autoStarted: Boolean(payload?.auto_started ?? status?.started_by_desktop),
      }
    : fallbackAcceptance(payload, status, remoteDevices, layout, inputMode);

  return {
    ...acceptance,
    checks: buildAcceptanceChecks(acceptance, status, inputMode),
  };
}

export function buildDesktopViewModel(payload) {
  const status = payload?.status ?? null;
  const localDevice = buildLocalDevice(status);
  const remoteDevices = (payload?.devices ?? []).map(buildRemoteDevice);
  const daemonLayout = buildLayoutFromVisibleGraph(
    payload?.visible_layout,
    payload?.layout,
    localDevice,
    remoteDevices,
  );
  const layoutUnavailable = Boolean(payload?.layout_error && status && !payload?.visible_layout);
  const fallbackDevices = layoutUnavailable ? [localDevice] : [localDevice, ...remoteDevices];
  const layoutDevices = daemonLayout?.devices ?? fallbackDevices;
  const layoutMonitors =
    daemonLayout?.monitors ??
    layoutDevices.map((device, index) =>
      buildLayoutMonitor(device, index, device.kind),
    );
  const backendState = parseBackendHealth(status?.backend_health);
  const inputMode = {
    current: status?.input_mode ?? "不可用",
    available: status?.available_backends ?? [],
    health: backendState.health,
    reason: backendState.reason,
  };
  const layout = {
    devices: layoutDevices,
    monitors: layoutMonitors,
    remembered: payload?.layout ?? null,
    visible: payload?.visible_layout ?? null,
    error: payload?.layout_error ?? null,
  };
  const service = {
    online: Boolean(status),
    healthy: Boolean(status?.healthy),
    label: status ? "运行中" : "已停止",
    error: status?.last_backend_error ?? payload?.layout_error ?? null,
    discoveredDevices: status?.discovered_devices ?? remoteDevices.length,
    connectedDevices: status?.connected_devices ?? 0,
    autoStarted: Boolean(payload?.auto_started ?? status?.started_by_desktop),
  };

  return {
    service,
    layout,
    devices: remoteDevices,
    settings: {
      localDevice: {
        id: localDevice.id,
        name: localDevice.name,
        hostname: localDevice.hostname,
        bindAddress: status?.bind_address ?? "不可用",
        discoveryPort: status?.discovery_port ?? null,
        pid: status?.pid ?? null,
      },
      inputMode,
      privilegeState: status?.privilege_state ?? "不可用",
    },
    acceptance: buildAcceptance(payload, status, remoteDevices, layout, inputMode),
  };
}
