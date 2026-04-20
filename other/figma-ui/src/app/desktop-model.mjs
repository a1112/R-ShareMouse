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

  return {
    service: {
      online: Boolean(status),
      healthy: Boolean(status?.healthy),
      label: status ? "运行中" : "已停止",
      error: status?.last_backend_error ?? payload?.layout_error ?? null,
      discoveredDevices: status?.discovered_devices ?? remoteDevices.length,
      connectedDevices: status?.connected_devices ?? 0,
      autoStarted: Boolean(payload?.auto_started),
    },
    layout: {
      devices: layoutDevices,
      monitors: layoutMonitors,
      remembered: payload?.layout ?? null,
      visible: payload?.visible_layout ?? null,
      error: payload?.layout_error ?? null,
    },
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
      inputMode: {
        current: status?.input_mode ?? "不可用",
        available: status?.available_backends ?? [],
        health: backendState.health,
        reason: backendState.reason,
      },
      privilegeState: status?.privilege_state ?? "不可用",
    },
  };
}
