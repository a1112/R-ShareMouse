const DEVICE_COLORS = ["#5b8bd6", "#49b35c", "#d6a64b", "#9b6ef3", "#e56b6f"];
const LOCAL_DEVICE_COLOR = "#60a5fa";
const LAYOUT_SCALE = 0.12;
const CANVAS_ORIGIN_X = 80;
const CANVAS_ORIGIN_Y = 170;
const LAYOUT_COMMIT_SNAP_DISTANCE = Math.ceil(12 / LAYOUT_SCALE);
const LATENCY_HEALTHY_RTT_MS = 50;

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

function displayTitle(display, index) {
  if (display.primary) {
    return "主显示器";
  }
  return `显示器 ${index + 1}`;
}

function displayName(display, index) {
  return (
    display.friendly_name ??
    display.name ??
    display.device_name ??
    displayTitle(display, index)
  );
}

function formatDisplayRefreshRate(refreshRateMillihz) {
  const refresh = Number(refreshRateMillihz ?? 0);
  if (!Number.isFinite(refresh) || refresh <= 0) {
    return "未知";
  }
  const hertz = refresh / 1000;
  return `${Number.isInteger(hertz) ? hertz : hertz.toFixed(2)} Hz`;
}

function displayResolutionOptions(display) {
  const seen = new Set();
  return [
    ...(display.modes ?? []),
    { width: display.width, height: display.height },
  ]
    .filter((mode) => Number(mode.width) > 0 && Number(mode.height) > 0)
    .filter((mode) => {
      const key = `${mode.width}x${mode.height}`;
      if (seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    })
    .sort((left, right) => Number(right.width) * Number(right.height) - Number(left.width) * Number(left.height))
    .map((mode) => ({
      value: `${mode.width}x${mode.height}`,
      label: `${mode.width} × ${mode.height}`,
      width: Number(mode.width),
      height: Number(mode.height),
    }));
}

function displayRefreshRateOptions(display) {
  const seen = new Set();
  return [
    ...(display.modes ?? []),
    { refresh_rate_millihz: display.refresh_rate_millihz },
  ]
    .map((mode) => Number(mode.refresh_rate_millihz ?? 0))
    .filter((refresh) => Number.isFinite(refresh) && refresh > 0)
    .filter((refresh) => {
      if (seen.has(refresh)) {
        return false;
      }
      seen.add(refresh);
      return true;
    })
    .sort((left, right) => left - right)
    .map((refresh) => ({
      value: String(refresh),
      label: formatDisplayRefreshRate(refresh),
      refreshRateMillihz: refresh,
    }));
}

function displayBounds(displays) {
  const minX = Math.min(...displays.map((display) => Number(display.x ?? 0)));
  const minY = Math.min(...displays.map((display) => Number(display.y ?? 0)));
  const maxX = Math.max(
    ...displays.map((display) => Number(display.x ?? 0) + Number(display.width ?? 0)),
  );
  const maxY = Math.max(
    ...displays.map((display) => Number(display.y ?? 0) + Number(display.height ?? 0)),
  );
  return {
    minX,
    minY,
    maxX,
    maxY,
    width: Math.max(1, maxX - minX),
    height: Math.max(1, maxY - minY),
  };
}

function fallbackDisplay(snapshot) {
  const display = snapshot?.display ?? {};
  return {
    display_id: "primary",
    x: Number(display.virtual_x ?? 0),
    y: Number(display.virtual_y ?? 0),
    width: Number(display.primary_width ?? 1920),
    height: Number(display.primary_height ?? 1080),
    primary: true,
    active: Boolean(display.display_count ?? 1),
    modes: [],
    write_capabilities: {},
  };
}

export function buildDisplaySettingsViewModel(snapshot, selectedDisplayId) {
  const rawDisplays = snapshot?.display?.displays?.length
    ? snapshot.display.displays
    : [fallbackDisplay(snapshot)];
  const displays = rawDisplays.map((display, index) => {
    const id = display.display_id || `display-${index + 1}`;
    const width = Number(display.width ?? 0);
    const height = Number(display.height ?? 0);
    const scalePercent = display.scale_percent == null ? null : Number(display.scale_percent);
    const refreshRateMillihz =
      display.refresh_rate_millihz == null ? null : Number(display.refresh_rate_millihz);
    const writeCapabilities = {
      resolution: Boolean(display.write_capabilities?.resolution),
      refreshRate: Boolean(display.write_capabilities?.refresh_rate),
      orientation: Boolean(display.write_capabilities?.orientation),
      primary: Boolean(display.write_capabilities?.primary),
      position: Boolean(display.write_capabilities?.position),
      scale: Boolean(display.write_capabilities?.scale),
      capture: Boolean(display.write_capabilities?.capture),
    };
    return {
      id,
      index,
      title: displayTitle(display, index),
      name: displayName(display, index),
      deviceName: display.device_name ?? null,
      x: Number(display.x ?? 0),
      y: Number(display.y ?? 0),
      width,
      height,
      workArea: {
        x: Number(display.work_x ?? display.x ?? 0),
        y: Number(display.work_y ?? display.y ?? 0),
        width: Number(display.work_width ?? width),
        height: Number(display.work_height ?? height),
      },
      primary: Boolean(display.primary),
      active: display.active !== false,
      orientation: display.orientation ?? "Landscape",
      scalePercent,
      refreshRateMillihz,
      bitsPerPixel: display.bits_per_pixel ?? null,
      dpi: {
        x: display.dpi_x ?? null,
        y: display.dpi_y ?? null,
        rawX: display.raw_dpi_x ?? null,
        rawY: display.raw_dpi_y ?? null,
      },
      resolutionLabel: `${width} × ${height}`,
      scaleLabel: scalePercent ? `${scalePercent}%` : "未知",
      refreshRateLabel: formatDisplayRefreshRate(refreshRateMillihz),
      resolutionOptions: displayResolutionOptions({ ...display, width, height }),
      refreshRateOptions: displayRefreshRateOptions(display),
      writeCapabilities,
    };
  });
  const selectedDisplay =
    displays.find((display) => display.id === selectedDisplayId) ??
    displays.find((display) => display.primary) ??
    displays[0];
  return {
    displays,
    selectedDisplay,
    selectedDisplayId: selectedDisplay?.id ?? null,
    bounds: displayBounds(displays),
  };
}

function buildRemoteDevice(device, index) {
  const isLaptop = /book|laptop/i.test(device.name) || /macbook/i.test(device.hostname ?? "");
  const address = device.addresses?.[0] ?? "未知";

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
    address,
    ipAddress: displayAddressHost(address),
    port: displayAddressPort(address),
    lastSeenLabel:
      device.last_seen_secs == null
        ? "刚刚"
        : `${device.last_seen_secs} 秒前`,
  };
}

function displayAddressHost(address) {
  if (!address || address === "未知") {
    return "未知";
  }
  if (address.startsWith("[")) {
    const closingBracket = address.indexOf("]");
    return closingBracket > 1 ? address.slice(1, closingBracket) : address;
  }
  const lastColon = address.lastIndexOf(":");
  if (lastColon <= 0 || address.indexOf(":") !== lastColon) {
    return address;
  }
  const maybePort = address.slice(lastColon + 1);
  return /^\d+$/.test(maybePort) ? address.slice(0, lastColon) : address;
}

function displayAddressPort(address) {
  if (!address || address === "未知") {
    return null;
  }
  const match = address.match(/:(\d+)$/);
  return match ? Number(match[1]) : null;
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
    deviceKind: kind,
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

function buildLocalDisplayNameLookup(localControls) {
  return new Map(
    (localControls?.display?.displays ?? [])
      .map((display) => [
        display.display_id ?? "primary",
        display.friendly_name ?? display.name ?? display.device_name ?? null,
      ])
      .filter((entry) => entry[1]),
  );
}

function buildLocalDisplayInfoLookup(localControls) {
  return new Map(
    (localControls?.display?.displays ?? []).map((display) => [
      display.display_id ?? "primary",
      display,
    ]),
  );
}

function localNodeDisplays(node, localDisplayInfo) {
  const actualDisplays = [...localDisplayInfo.values()];
  if (!actualDisplays.length) {
    return node.displays ?? [];
  }

  const visibleById = new Map(
    (node.displays ?? []).map((display) => [display.display_id ?? "primary", display]),
  );
  const actualIds = new Set();
  const mergedDisplays = actualDisplays.map((actualDisplay, index) => {
    const displayId = actualDisplay.display_id ?? (index === 0 ? "primary" : `display-${index + 1}`);
    const visibleDisplay = visibleById.get(displayId) ?? {};
    actualIds.add(displayId);
    return {
      ...visibleDisplay,
      ...actualDisplay,
      display_id: displayId,
      x: Number(actualDisplay.x ?? visibleDisplay.x ?? 0),
      y: Number(actualDisplay.y ?? visibleDisplay.y ?? 0),
      width: Number(actualDisplay.width ?? visibleDisplay.width ?? 1920),
      height: Number(actualDisplay.height ?? visibleDisplay.height ?? 1080),
      primary: Boolean(actualDisplay.primary ?? visibleDisplay.primary),
    };
  });

  return [
    ...mergedDisplays,
    ...(node.displays ?? [])
      .filter((display) => !actualIds.has(display.display_id ?? "primary"))
      .map((display) => ({ ...display })),
  ];
}

function physicalCanvasScale(localDisplayInfo) {
  const displays = [...localDisplayInfo.values()];
  const primaryDisplay = displays.find((display) => display.primary) ?? displays[0];
  const rawDpiX = Number(primaryDisplay?.raw_dpi_x ?? primaryDisplay?.dpi_x ?? 0);
  return rawDpiX > 0 ? rawDpiX * LAYOUT_SCALE : 96 * LAYOUT_SCALE;
}

function physicalCanvasSize(display, localDisplayInfo, displayScale) {
  const physicalDisplay = localDisplayInfo.get(display.display_id ?? "primary");
  const rawDpiX = Number(physicalDisplay?.raw_dpi_x ?? physicalDisplay?.dpi_x ?? 0);
  const rawDpiY = Number(physicalDisplay?.raw_dpi_y ?? physicalDisplay?.dpi_y ?? 0);
  const width = Number(display.width ?? 1920);
  const height = Number(display.height ?? 1080);

  if (rawDpiX <= 0 || rawDpiY <= 0) {
    return {
      w: Math.max(96, Math.round(width * LAYOUT_SCALE)),
      h: Math.max(64, Math.round(height * LAYOUT_SCALE)),
      physical: false,
    };
  }

  return {
    w: Math.max(96, Math.round((width / rawDpiX) * displayScale)),
    h: Math.max(64, Math.round((height / rawDpiY) * displayScale)),
    physical: true,
  };
}

function displayGeometry(display, localDisplayInfo, displayScale) {
  const width = Number(display.width ?? 1920);
  const height = Number(display.height ?? 1080);
  const left = Number(display.x ?? 0);
  const top = Number(display.y ?? 0);
  return {
    display,
    displayId: display.display_id ?? "primary",
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
    size: physicalCanvasSize(display, localDisplayInfo, displayScale),
  };
}

function displayEdgeAligned(a, b) {
  return Math.abs(Number(a) - Number(b)) <= LAYOUT_COMMIT_SNAP_DISTANCE;
}

function physicalYRelativeToAnchor(target, anchor, anchorLayout) {
  if (displayEdgeAligned(target.bottom, anchor.bottom)) {
    return anchorLayout.y + anchorLayout.h - target.size.h;
  }
  if (displayEdgeAligned(target.top, anchor.top)) {
    return anchorLayout.y;
  }
  return anchorLayout.y + (target.top - anchor.top) * LAYOUT_SCALE;
}

function physicalXRelativeToAnchor(target, anchor, anchorLayout) {
  if (displayEdgeAligned(target.right, anchor.right)) {
    return anchorLayout.x + anchorLayout.w - target.size.w;
  }
  if (displayEdgeAligned(target.left, anchor.left)) {
    return anchorLayout.x;
  }
  return anchorLayout.x + (target.left - anchor.left) * LAYOUT_SCALE;
}

function localPhysicalDisplayCandidate(target, anchor, anchorLayout) {
  const candidates = [];
  if (rangesOverlap(target.top, target.bottom, anchor.top, anchor.bottom)) {
    if (target.left >= anchor.right) {
      const gap = target.left - anchor.right;
      candidates.push({
        x: anchorLayout.x + anchorLayout.w + gap * LAYOUT_SCALE,
        y: physicalYRelativeToAnchor(target, anchor, anchorLayout),
        score: gap,
      });
    }
    if (anchor.left >= target.right) {
      const gap = anchor.left - target.right;
      candidates.push({
        x: anchorLayout.x - target.size.w - gap * LAYOUT_SCALE,
        y: physicalYRelativeToAnchor(target, anchor, anchorLayout),
        score: gap,
      });
    }
  }

  if (rangesOverlap(target.left, target.right, anchor.left, anchor.right)) {
    if (target.top >= anchor.bottom) {
      const gap = target.top - anchor.bottom;
      candidates.push({
        x: physicalXRelativeToAnchor(target, anchor, anchorLayout),
        y: anchorLayout.y + anchorLayout.h + gap * LAYOUT_SCALE,
        score: gap,
      });
    }
    if (anchor.top >= target.bottom) {
      const gap = anchor.top - target.bottom;
      candidates.push({
        x: physicalXRelativeToAnchor(target, anchor, anchorLayout),
        y: anchorLayout.y - target.size.h - gap * LAYOUT_SCALE,
        score: gap,
      });
    }
  }

  return candidates.sort((left, right) => left.score - right.score)[0] ?? null;
}

function buildLocalPhysicalDisplayLayout(displays, localDisplayInfo, displayScale) {
  const entries = (displays ?? []).map((display) =>
    displayGeometry(display, localDisplayInfo, displayScale),
  );
  if (!entries.length) {
    return null;
  }

  const primary = entries.find((entry) => entry.display.primary) ?? entries[0];
  const placed = new Map();
  placed.set(primary.displayId, {
    entry: primary,
    x: CANVAS_ORIGIN_X + primary.right * LAYOUT_SCALE - primary.size.w,
    y: CANVAS_ORIGIN_Y + primary.bottom * LAYOUT_SCALE - primary.size.h,
    w: primary.size.w,
    h: primary.size.h,
  });

  let changed = true;
  while (placed.size < entries.length && changed) {
    changed = false;
    for (const target of entries) {
      if (placed.has(target.displayId)) {
        continue;
      }

      const bestCandidate = [...placed.values()]
        .map((anchorLayout) => ({
          anchor: anchorLayout,
          candidate: localPhysicalDisplayCandidate(target, anchorLayout.entry, anchorLayout),
        }))
        .filter((item) => item.candidate)
        .sort((left, right) => left.candidate.score - right.candidate.score)[0];

      if (bestCandidate) {
        placed.set(target.displayId, {
          entry: target,
          x: bestCandidate.candidate.x,
          y: bestCandidate.candidate.y,
          w: target.size.w,
          h: target.size.h,
        });
        changed = true;
      }
    }
  }

  if (placed.size !== entries.length) {
    return null;
  }

  return placed;
}

function layoutDisplayName(device, displayId, localDisplayNames) {
  if (device.kind === "local") {
    return localDisplayNames.get(displayId) ?? `${device.name} 显示器`;
  }

  return `${device.name} 屏幕`;
}

function buildLayoutFromVisibleGraph(visibleLayout, rememberedLayout, localDevice, remoteDevices, localControls) {
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

  const localDisplayNames = buildLocalDisplayNameLookup(localControls);
  const localDisplayInfo = buildLocalDisplayInfoLookup(localControls);
  const localPhysicalScale = physicalCanvasScale(localDisplayInfo);
  const visibleNodes = snapVisibleDeviceGroupsEdgeToEdge(
    visibleLayout.nodes.map((node) => ({
      ...node,
      displays: (node.displays ?? []).map((display) => ({ ...display })),
    })),
    new Set(visibleLayout.nodes.map((node) => node.device_id)),
  );

  const layoutMonitors = [];
  for (const node of visibleNodes) {
    const device = deviceLookup.get(node.device_id);
    if (!device) {
      continue;
    }
    const nodeDisplays =
      device.kind === "local"
        ? localNodeDisplays(node, localDisplayInfo)
        : node.displays ?? [];
    const localPhysicalLayout =
      device.kind === "local"
        ? buildLocalPhysicalDisplayLayout(
            nodeDisplays,
            localDisplayInfo,
            localPhysicalScale,
          )
        : null;

    for (const display of nodeDisplays) {
      const monitorIndex = layoutMonitors.length;
      const width = Number(display.width ?? 1920);
      const height = Number(display.height ?? 1080);
      const displayId = display.display_id ?? "primary";
      const localPhysicalDisplay = localPhysicalLayout?.get(displayId);
      const canvasSize =
        localPhysicalDisplay
          ? localPhysicalDisplay
          : device.kind === "local"
          ? physicalCanvasSize(display, localDisplayInfo, localPhysicalScale)
          : {
              w: Math.max(96, Math.round(width * LAYOUT_SCALE)),
              h: Math.max(64, Math.round(height * LAYOUT_SCALE)),
            };
      const canvasRight = CANVAS_ORIGIN_X + Number(display.x ?? 0) * LAYOUT_SCALE + width * LAYOUT_SCALE;
      const canvasBottom = CANVAS_ORIGIN_Y + Number(display.y ?? 0) * LAYOUT_SCALE + height * LAYOUT_SCALE;
      const canvasX = CANVAS_ORIGIN_X + Number(display.x ?? 0) * LAYOUT_SCALE;
      const canvasY = CANVAS_ORIGIN_Y + Number(display.y ?? 0) * LAYOUT_SCALE;
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
        name: layoutDisplayName(device, displayId, localDisplayNames),
        resWidth: width,
        resHeight: height,
        color: device.color,
        deviceKind: device.kind,
        x: localPhysicalDisplay
          ? localPhysicalDisplay.x
          : canvasSize.physical ? canvasRight - canvasSize.w : canvasX,
        y: localPhysicalDisplay
          ? localPhysicalDisplay.y
          : canvasSize.physical ? canvasBottom - canvasSize.h : canvasY,
        w: canvasSize.w,
        h: canvasSize.h,
        primary: Boolean(display.primary),
        enabled: true,
        orientation: display.orientation ?? null,
        scalePercent: display.scale_percent == null ? null : Number(display.scale_percent),
        refreshRateMillihz:
          display.refresh_rate_millihz == null
            ? null
            : Number(display.refresh_rate_millihz),
        writeCapabilities: {
          resolution: Boolean(display.write_capabilities?.resolution),
          refreshRate: Boolean(display.write_capabilities?.refresh_rate),
          orientation: Boolean(display.write_capabilities?.orientation),
          primary: Boolean(display.write_capabilities?.primary),
          position: Boolean(display.write_capabilities?.position),
          scale: Boolean(display.write_capabilities?.scale),
          capture: Boolean(display.write_capabilities?.capture),
        },
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

function nodeDisplayBounds(node) {
  const displays = node.displays ?? [];
  if (!displays.length) {
    return null;
  }

  return displays.reduce(
    (bounds, display) => {
      const x = Number(display.x ?? 0);
      const y = Number(display.y ?? 0);
      const width = Number(display.width ?? 0);
      const height = Number(display.height ?? 0);

      return {
        left: Math.min(bounds.left, x),
        top: Math.min(bounds.top, y),
        right: Math.max(bounds.right, x + width),
        bottom: Math.max(bounds.bottom, y + height),
      };
    },
    {
      left: Infinity,
      top: Infinity,
      right: -Infinity,
      bottom: -Infinity,
    },
  );
}

function translateNodeDisplays(node, dx, dy) {
  if (!dx && !dy) {
    return node;
  }

  return {
    ...node,
    displays: (node.displays ?? []).map((display) => ({
      ...display,
      x: Math.round(Number(display.x ?? 0) + dx),
      y: Math.round(Number(display.y ?? 0) + dy),
    })),
  };
}

function rangesOverlap(aStart, aEnd, bStart, bEnd) {
  return aStart < bEnd && aEnd > bStart;
}

function snapVisibleDeviceGroupsEdgeToEdge(nodes, visibleDeviceIds) {
  const visible = new Set(visibleDeviceIds);
  const sortedVisibleNodes = nodes
    .filter((node) => visible.has(node.device_id))
    .map((node) => ({ node, bounds: nodeDisplayBounds(node) }))
    .filter((entry) => entry.bounds)
    .sort((left, right) => left.bounds.left - right.bounds.left);

  if (sortedVisibleNodes.length < 2) {
    return nodes;
  }

  const offsets = new Map();
  for (let index = 0; index < sortedVisibleNodes.length - 1; index += 1) {
    const left = sortedVisibleNodes[index];
    const right = sortedVisibleNodes[index + 1];
    const horizontalGap = right.bounds.left - left.bounds.right;
    const verticallyAligned = rangesOverlap(
      left.bounds.top,
      left.bounds.bottom,
      right.bounds.top,
      right.bounds.bottom,
    );

    if (
      Math.abs(horizontalGap) <= LAYOUT_COMMIT_SNAP_DISTANCE &&
      verticallyAligned
    ) {
      const dx = -horizontalGap;
      offsets.set(right.node.device_id, {
        dx: (offsets.get(right.node.device_id)?.dx ?? 0) + dx,
        dy: 0,
      });
      right.bounds.left += dx;
      right.bounds.right += dx;
    }
  }

  if (!offsets.size) {
    return nodes;
  }

  return nodes.map((node) => {
    const offset = offsets.get(node.device_id);
    return offset ? translateNodeDisplays(node, offset.dx, offset.dy) : node;
  });
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
  const visibleDeviceIds = new Set((monitors ?? []).map((monitor) => monitor.deviceId));

  const nodes = snapVisibleDeviceGroupsEdgeToEdge(rememberedLayout.nodes.map((node) => ({
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
    })), visibleDeviceIds);

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

const CAPABILITY_LABELS = {
  Input: "输入",
  Clipboard: "剪贴板",
  Gamepad: "手柄",
  Audio: "音频",
  DisplayTopology: "显示拓扑",
  UsbHost: "USB 主机",
  UsbReceiver: "USB 接收",
  PrivilegedHelper: "特权助手",
  Diagnostics: "诊断",
};

const CAPABILITY_STATE_LABELS = {
  Available: "可用",
  Degraded: "降级",
  Unavailable: "不可用",
  Experimental: "实验",
};

export function buildCapabilityOverview(registry) {
  if (!registry || !Array.isArray(registry.devices)) {
    return {
      available: false,
      localDeviceId: null,
      generatedAtMs: null,
      devices: [],
    };
  }

  return {
    available: true,
    localDeviceId: registry.local_device_id ?? null,
    generatedAtMs: registry.generated_at_ms ?? null,
    devices: registry.devices.map((device) => ({
      id: device.device_id,
      name: device.device_name ?? "未知设备",
      hostname: device.hostname ?? "",
      connected: Boolean(device.connected),
      local: device.device_id === registry.local_device_id,
      capabilities: (device.capabilities ?? []).map((capability) => ({
        kind: capability.kind,
        label: CAPABILITY_LABELS[capability.kind] ?? capability.kind,
        state: capability.state,
        stateLabel: CAPABILITY_STATE_LABELS[capability.state] ?? capability.state,
        reason: capability.health_reason ?? null,
        permissionState: capability.permission_state ?? null,
        latencyMs: capability.latency_ms ?? null,
        transportState: capability.transport_state ?? null,
        details: capability.details ?? {},
      })),
    })),
  };
}

function capabilityDeviceFor(capabilities, deviceId) {
  return (capabilities?.devices ?? []).find(
    (device) => (device.id ?? device.device_id) === deviceId,
  );
}

function capabilityFor(deviceCapabilities, kind) {
  return (deviceCapabilities?.capabilities ?? []).find(
    (capability) => capability.kind === kind,
  );
}

function capabilityUsable(capability) {
  return Boolean(
    capability &&
      capability.state !== "Unavailable" &&
      capability.state !== "unavailable",
  );
}

function eventRemoteDeviceId(event) {
  return event?.device_id ?? event?.payload?.remote_device_id ?? null;
}

function eventsForRemote(snapshot, deviceId) {
  return asArray(snapshot?.recent_events)
    .filter((event) => eventRemoteDeviceId(event) === deviceId)
    .sort((left, right) => Number(left.sequence ?? 0) - Number(right.sequence ?? 0));
}

function visibleLayoutNodeFor(visibleLayout, deviceId) {
  return (visibleLayout?.nodes ?? []).find((node) => node.device_id === deviceId) ?? null;
}

function displayStateFromNode(node) {
  const displays = asArray(node?.displays).map((display, index) => ({
    display_id: display.display_id ?? (index === 0 ? "primary" : `display-${index + 1}`),
    x: Number(display.x ?? 0),
    y: Number(display.y ?? 0),
    width: Number(display.width ?? 1920),
    height: Number(display.height ?? 1080),
    primary: Boolean(display.primary ?? index === 0),
    active: display.active !== false,
    orientation: display.orientation ?? null,
    scale_percent: display.scale_percent ?? null,
    refresh_rate_millihz: display.refresh_rate_millihz ?? null,
    write_capabilities: display.write_capabilities ?? {},
  }));

  if (!displays.length) {
    return {
      display_count: 0,
      virtual_x: 0,
      virtual_y: 0,
      primary_width: 0,
      primary_height: 0,
      layout_width: 0,
      layout_height: 0,
      displays: [],
    };
  }

  const primary = displays.find((display) => display.primary) ?? displays[0];
  const bounds = displayBounds(displays);
  return {
    display_count: displays.length,
    virtual_x: bounds.minX,
    virtual_y: bounds.minY,
    primary_width: primary.width,
    primary_height: primary.height,
    layout_width: bounds.width,
    layout_height: bounds.height,
    displays,
  };
}

function latestEventPayload(events, kind) {
  const event = [...events].reverse().find((item) => item.device_kind === kind);
  return event?.payload ?? {};
}

function remoteInputDevice(device, kind, connected, eventCount) {
  return {
    id: `${device.id}-${kind}`,
    name: `${device.name ?? "远端设备"} ${kind === "keyboard" ? "键盘" : "鼠标"}`,
    source: "remote capability",
    connected,
    capture_path: "remote endpoint",
    event_count: eventCount,
    capabilities: ["remote", "endpoint"],
  };
}

export function buildRemoteControlSnapshot({
  baseSnapshot = null,
  device,
  capabilities = null,
  visibleLayout = null,
} = {}) {
  const deviceId = device?.id ?? device?.device_id ?? "";
  const remoteEvents = eventsForRemote(baseSnapshot, deviceId);
  const capabilityDevice = capabilityDeviceFor(capabilities, deviceId);
  const inputCapability = capabilityFor(capabilityDevice, "Input");
  const gamepadCapability = capabilityFor(capabilityDevice, "Gamepad");
  const audioCapability = capabilityFor(capabilityDevice, "Audio");
  const displayCapability = capabilityFor(capabilityDevice, "DisplayTopology");
  const inputDetected = capabilityUsable(inputCapability) || remoteEvents.length > 0;
  const keyboardEvents = remoteEvents.filter((event) => event.device_kind === "Keyboard");
  const mouseEvents = remoteEvents.filter((event) => event.device_kind === "Mouse");
  const latestKeyboard = latestEventPayload(remoteEvents, "Keyboard");
  const latestMouse = latestEventPayload(remoteEvents, "Mouse");
  const display = displayStateFromNode(visibleLayoutNodeFor(visibleLayout, deviceId));
  const gamepadAvailable = capabilityUsable(gamepadCapability);
  const audioAvailable = capabilityUsable(audioCapability);

  return {
    sequence: Number(baseSnapshot?.sequence ?? 0),
    keyboard: {
      detected: inputDetected,
      pressed_keys: latestKeyboard.state === "Pressed" && latestKeyboard.key ? [latestKeyboard.key] : [],
      last_key: latestKeyboard.key ?? null,
      event_count: keyboardEvents.length,
      capture_source: "remote endpoint",
    },
    mouse: {
      detected: inputDetected,
      x: Number(latestMouse.x ?? 0),
      y: Number(latestMouse.y ?? 0),
      pressed_buttons:
        latestMouse.state === "Pressed" && latestMouse.button ? [latestMouse.button] : [],
      wheel_delta_x: Number(latestMouse.delta_x ?? 0),
      wheel_delta_y: Number(latestMouse.delta_y ?? 0),
      event_count: mouseEvents.length,
      move_count: mouseEvents.filter((event) => event.event_kind === "move").length,
      button_event_count: mouseEvents.filter((event) => event.event_kind === "button").length,
      button_press_count: mouseEvents.filter((event) => event.payload?.state === "Pressed").length,
      button_release_count: mouseEvents.filter((event) => event.payload?.state === "Released").length,
      wheel_event_count: mouseEvents.filter((event) => event.event_kind === "wheel").length,
      display_relative_x: Number(latestMouse.display_relative_x ?? latestMouse.x ?? 0),
      display_relative_y: Number(latestMouse.display_relative_y ?? latestMouse.y ?? 0),
      current_display_id: latestMouse.display_id ?? null,
      current_display_index: latestMouse.display_index == null ? null : Number(latestMouse.display_index),
    },
    keyboard_devices: inputDetected
      ? [remoteInputDevice(device, "keyboard", Boolean(device?.connected), keyboardEvents.length)]
      : [],
    mouse_devices: inputDetected
      ? [remoteInputDevice(device, "mouse", Boolean(device?.connected), mouseEvents.length)]
      : [],
    gamepads: gamepadAvailable
      ? [
          {
            gamepad_id: 0,
            name: `${device?.name ?? "远端设备"} 手柄`,
            connected: true,
            event_count: remoteEvents.filter((event) => event.device_kind === "Gamepad").length,
            pressed_buttons: [],
          },
        ]
      : [],
    audio_inputs: [],
    audio_outputs: audioAvailable
      ? [
          {
            id: `${deviceId}-audio`,
            name: `${device?.name ?? "远端设备"} 音频`,
            source: "remote capability",
            connected: Boolean(device?.connected),
            default: true,
          },
        ]
      : [],
    display,
    capture_backend: {
      mode: "RemoteEndpoint",
      health: inputDetected ? "Healthy" : "Unavailable",
      active: inputDetected,
    },
    inject_backend: {
      mode: "RemoteEndpoint",
      health: Boolean(device?.connected) ? "Healthy" : "Unavailable",
      active: Boolean(device?.connected),
    },
    privilege_state: null,
    virtual_gamepad: {
      status: gamepadAvailable ? "remote_capability" : "not_available",
      detail: gamepadAvailable ? "Remote gamepad capability advertised." : "Remote gamepad capability unavailable.",
    },
    driver: baseSnapshot?.driver,
    recent_events: remoteEvents,
    last_error:
      capabilityUsable(displayCapability) || display.display_count > 0
        ? null
        : "Remote display topology has not been advertised yet.",
  };
}

function endpointPayloadData(payload) {
  if (!payload || typeof payload !== "object") {
    return {};
  }
  return payload.data && typeof payload.data === "object" ? payload.data : payload;
}

function endpointEventSource(source, direction) {
  if (direction === "InjectedLoopback") {
    return "InjectedLoopback";
  }
  if (direction === "Injected") {
    return "Injected";
  }
  switch (source) {
    case "Driver":
    case "Test":
      return "DriverTest";
    case "VirtualHid":
      return "VirtualDevice";
    case "SendInput":
      return "Injected";
    case "System":
      return "System";
    case "Hardware":
    case "UserModeHook":
    case "RemoteMirror":
    default:
      return "Hardware";
  }
}

function endpointDeviceKind(kind) {
  switch (kind) {
    case "Keyboard":
    case "Mouse":
    case "Gamepad":
    case "Display":
    case "Audio":
    case "Backend":
      return kind;
    default:
      return "Backend";
  }
}

function endpointEventKind(kind, payload) {
  const payloadKind = payload?.kind;
  if (kind === "Keyboard") {
    return "key";
  }
  if (payloadKind === "MouseMove") {
    return "move";
  }
  if (payloadKind === "MouseButton") {
    return "button";
  }
  if (payloadKind === "MouseWheel") {
    return "wheel";
  }
  return String(kind ?? "event").toLowerCase();
}

function endpointEventPayloadFields(event) {
  const payload = event?.payload;
  const data = endpointPayloadData(payload);
  const fields =
    data.fields && typeof data.fields === "object" && !Array.isArray(data.fields)
      ? data.fields
      : data;
  const result = {};
  for (const [key, value] of Object.entries(fields ?? {})) {
    if (value !== undefined && value !== null && typeof value !== "object") {
      result[key] = String(value);
    }
  }
  if (event?.device?.device_id) {
    result.device_id = String(event.device.device_id);
  }
  if (event?.device?.display_name) {
    result.device_display_name = String(event.device.display_name);
  }
  if (event?.endpoint_id) {
    result.endpoint_id = String(event.endpoint_id);
    result.remote_device_id = String(event.endpoint_id);
  }
  if (event?.origin_endpoint_id) {
    result.origin_endpoint_id = String(event.origin_endpoint_id);
  }
  if (event?.correlation_id) {
    result.correlation_id = String(event.correlation_id);
  }
  return result;
}

export function endpointEventToLocalControlEvent(event) {
  const payload = event?.payload;
  const fields = endpointEventPayloadFields(event);
  const deviceKind = endpointDeviceKind(event?.kind);
  const eventKind = endpointEventKind(event?.kind, payload);
  const summary =
    fields.summary ??
    (deviceKind === "Keyboard"
      ? `Key ${fields.key ?? "Unknown"} ${fields.state ?? ""}`.trim()
      : `${deviceKind} ${eventKind}`);

  return {
    sequence: Number(event?.sequence ?? event?.event_id ?? 0),
    timestamp_ms: Number(event?.timestamp_ms ?? 0),
    device_kind: deviceKind,
    event_kind: eventKind,
    summary,
    device_id: event?.endpoint_id ? String(event.endpoint_id) : null,
    device_instance_id:
      typeof event?.device?.instance_id === "string" ? event.device.instance_id : null,
    capture_path: event?.source ? `endpoint:${event.source}` : "endpoint",
    source: endpointEventSource(event?.source, event?.direction),
    payload: fields,
  };
}

function localControlEventDeviceId(event) {
  return event?.device_id ?? event?.payload?.remote_device_id ?? null;
}

function eventIsInjected(event) {
  return ["Injected", "InjectedLoopback", "VirtualDevice"].includes(event?.source);
}

export function buildEndpointAcceptance(snapshot, remoteDevices = [], inputTestResult = null) {
  const remoteIds = new Set((remoteDevices ?? []).map((device) => device.id));
  const connectedRemoteDevices = (remoteDevices ?? []).filter((device) => device.connected);
  const events = snapshot?.recent_events ?? [];
  const localEvents = events.filter((event) => {
    const sourceDeviceId = localControlEventDeviceId(event);
    return !sourceDeviceId || !remoteIds.has(sourceDeviceId);
  });
  const remoteEvents = events.filter((event) => remoteIds.has(localControlEventDeviceId(event)));
  const remoteInjectedEvents = remoteEvents.filter(eventIsInjected);
  const remoteInjectSucceeded =
    inputTestResult?.status === "Success" || remoteInjectedEvents.length > 0;
  const remoteInjectFailed =
    inputTestResult &&
    inputTestResult.status &&
    inputTestResult.status !== "Success";
  const captureActive = Boolean(snapshot?.capture_backend?.active);
  const injectActive = Boolean(snapshot?.inject_backend?.active);

  const checks = [
    {
      key: "local-events",
      label: "本机事件",
      state: statusCheck(localEvents.length > 0, Boolean(snapshot)),
      detail: localEvents.length
        ? `已捕获 ${localEvents.length} 条本机输入事件`
        : snapshot
          ? "等待键盘或鼠标事件"
          : "本机控制快照不可用",
    },
    {
      key: "remote-mirror",
      label: "远端镜像",
      state: statusCheck(remoteEvents.length > 0, connectedRemoteDevices.length > 0),
      detail: remoteEvents.length
        ? `已镜像 ${remoteEvents.length} 条远端端侧事件`
        : connectedRemoteDevices.length
          ? "已连接远端，等待远端输入事件"
          : "先连接局域网远端设备",
    },
    {
      key: "remote-inject",
      label: "远端注入",
      state: remoteInjectSucceeded
        ? "pass"
        : remoteInjectFailed
          ? "block"
          : connectedRemoteDevices.length
            ? "warn"
            : "block",
      detail: remoteInjectSucceeded
        ? "远端注入已返回成功或已观察到 loopback 事件"
        : remoteInjectFailed
          ? inputTestResult.message
          : connectedRemoteDevices.length
            ? "点击远端键盘或鼠标测试完成注入闭环"
            : "没有可注入的已连接远端",
    },
    {
      key: "endpoint-backend",
      label: "端侧后端",
      state: statusCheck(captureActive && injectActive, Boolean(snapshot)),
      detail:
        captureActive && injectActive
          ? "捕获与注入后端均处于 active"
          : snapshot
            ? `capture=${captureActive ? "active" : "inactive"} / inject=${injectActive ? "active" : "inactive"}`
            : "后端状态不可用",
    },
  ];

  return {
    ready: checks.every((check) => check.state === "pass"),
    localEventCount: localEvents.length,
    remoteEventCount: remoteEvents.length,
    remoteInjectedEventCount: remoteInjectedEvents.length,
    connectedRemoteCount: connectedRemoteDevices.length,
    checks,
  };
}

function endpointInjectStatusFromError(error) {
  if (error === "PermissionDenied") {
    return "PermissionDenied";
  }
  if (
    error === "BackendUnavailable" ||
    error === "BackendDegraded" ||
    error === "TargetDisconnected" ||
    error === "Timeout"
  ) {
    return "BackendUnavailable";
  }
  if (error === "UnsupportedEvent") {
    return "Unsupported";
  }
  return "Failed";
}

export function buildEndpointInjectSummary(results = [], context = {}) {
  const safeResults = Array.isArray(results) ? results : [];
  const totalCount = safeResults.length;
  const successCount = safeResults.filter((result) => result?.accepted).length;
  const latencies = safeResults
    .map((result) => Number(result?.elapsed_ms))
    .filter((value) => Number.isFinite(value));
  const averageElapsedMs = latencies.length
    ? Math.round(latencies.reduce((sum, value) => sum + value, 0) / latencies.length)
    : null;
  const maxElapsedMs = latencies.length ? Math.max(...latencies) : null;
  const failed = safeResults.find((result) => !result?.accepted);
  const status =
    totalCount > 0 && successCount === totalCount
      ? "Success"
      : endpointInjectStatusFromError(failed?.error ?? (totalCount ? "Failed" : "Timeout"));
  const latencyText =
    averageElapsedMs == null || maxElapsedMs == null
      ? ""
      : `，平均 ${averageElapsedMs} ms，最大 ${maxElapsedMs} ms`;
  const message =
    status === "Success"
      ? `Endpoint 注入完成：${successCount}/${totalCount} 成功${latencyText}`
      : `Endpoint 注入失败：${failed?.error ?? "没有收到注入结果"}（${successCount}/${totalCount} 成功${latencyText}）`;

  return {
    status,
    message,
    kind: context.kind ?? null,
    targetId: context.targetId ?? null,
    successCount,
    totalCount,
    averageElapsedMs,
    maxElapsedMs,
  };
}

function numberOrNull(value) {
  if (value == null || value === "") {
    return null;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

const LOCAL_FEEDBACK_LABELS = Object.freeze({
  keyboard: "键盘",
  mouse: "鼠标",
  gamepad: "手柄",
  transport: "QUIC",
});

function latencyStatusState(status) {
  switch (String(status ?? "").toLowerCase()) {
    case "healthy":
      return "pass";
    case "degraded":
    case "pending":
      return "warn";
    case "timeout":
    case "unavailable":
      return "block";
    case "idle":
    default:
      return "idle";
  }
}

function eventTimestampDetail(timestampMs, fallback) {
  const value = numberOrNull(timestampMs);
  if (value == null || value <= 0) {
    return fallback;
  }
  return `event @ ${value} ms`;
}

function timestampState(timestampMs, status) {
  const value = numberOrNull(timestampMs);
  return value == null || value <= 0 ? "idle" : latencyStatusState(status);
}

export function buildLocalLatencyFeedbackRows(feedback) {
  const local = feedback?.local_input ?? {};
  const transport = feedback?.transport ?? {};
  const keyboardState = timestampState(local.latest_keyboard_event_ms, local.status);
  const mouseState = timestampState(local.latest_mouse_event_ms, local.status);
  const gamepadState = timestampState(local.latest_gamepad_event_ms, local.status);
  const gamepadTimestamp = numberOrNull(local.latest_gamepad_event_ms);
  const transportStatus = transport.status ?? "Unavailable";
  const transportRttMs = numberOrNull(transport.rtt_ms);
  const gamepadParts = [
    local.latest_gamepad_id == null ? null : `gamepad ${local.latest_gamepad_id}`,
    local.latest_gamepad_event_kind,
    local.latest_gamepad_button,
    local.latest_gamepad_axis,
  ].filter(Boolean);
  const transportParts =
    feedback?.transport == null
      ? ["transport unavailable"]
      : [
          transport.transport ?? "quic",
          transportRttMs == null ? null : `${transportRttMs} ms RTT`,
          transport.datagram_available ? "datagram" : "no datagram",
        ].filter(Boolean);

  return [
    {
      key: "keyboard",
      label: LOCAL_FEEDBACK_LABELS.keyboard,
      state: keyboardState,
      metric: String(local.event_count ?? 0),
      detail: eventTimestampDetail(local.latest_keyboard_event_ms, "waiting for keyboard"),
    },
    {
      key: "mouse",
      label: LOCAL_FEEDBACK_LABELS.mouse,
      state: mouseState,
      metric: String(local.event_count ?? 0),
      detail: eventTimestampDetail(local.latest_mouse_event_ms, "waiting for mouse"),
    },
    {
      key: "gamepad",
      label: LOCAL_FEEDBACK_LABELS.gamepad,
      state: gamepadState,
      metric: String(local.event_count ?? 0),
      detail: gamepadTimestamp != null && gamepadTimestamp > 0 && gamepadParts.length
        ? gamepadParts.join(", ")
        : eventTimestampDetail(local.latest_gamepad_event_ms, "waiting for gamepad"),
    },
    {
      key: "transport",
      label: LOCAL_FEEDBACK_LABELS.transport,
      state: latencyStatusState(transportStatus),
      metric: transportStatus,
      detail: transportParts.join(", "),
    },
  ];
}

function remoteLatencyEventMatchesDevice(event, deviceId) {
  return [
    event?.payload?.target_device_id,
    event?.payload?.origin_device_id,
    event?.payload?.remote_device_id,
    event?.device_id,
  ].some((candidate) => candidate === deviceId);
}

function isRemoteLatencyAck(event) {
  return (
    event?.device_kind === "Backend" &&
    (event?.event_kind === "latency_probe_ack" ||
      event?.event_kind === "latency_endpoint_switch_ack")
  );
}

function isRemoteLatencySent(event) {
  return (
    event?.device_kind === "Backend" &&
    (event?.event_kind === "latency_probe_sent" ||
      event?.event_kind === "latency_endpoint_probe_sent" ||
      event?.event_kind === "latency_endpoint_switch_sent")
  );
}

function remoteLatencyProbeSequence(event) {
  const payload = event?.payload ?? {};
  if (
    event?.event_kind === "latency_endpoint_switch_ack" ||
    event?.event_kind === "latency_endpoint_switch_sent"
  ) {
    return numberOrNull(payload.origin_probe_sequence) ?? numberOrNull(payload.probe_sequence);
  }
  return numberOrNull(payload.probe_sequence) ?? numberOrNull(payload.origin_probe_sequence);
}

function sortNewestEvent(left, right) {
  const leftSequence = numberOrNull(left?.sequence);
  const rightSequence = numberOrNull(right?.sequence);
  if (leftSequence != null && rightSequence != null && leftSequence !== rightSequence) {
    return rightSequence - leftSequence;
  }

  const leftProbeSequence = remoteLatencyProbeSequence(left);
  const rightProbeSequence = remoteLatencyProbeSequence(right);
  if (
    leftProbeSequence != null &&
    rightProbeSequence != null &&
    leftProbeSequence !== rightProbeSequence
  ) {
    return rightProbeSequence - leftProbeSequence;
  }

  const leftTime = numberOrNull(left?.timestamp_ms) ?? 0;
  const rightTime = numberOrNull(right?.timestamp_ms) ?? 0;
  if (leftTime !== rightTime) {
    return rightTime - leftTime;
  }
  return 0;
}

function eventIsNewer(left, right) {
  if (!left) {
    return false;
  }
  if (!right) {
    return true;
  }
  return sortNewestEvent(left, right) < 0;
}

function maxTimestampMs(...values) {
  const timestamps = values
    .map(numberOrNull)
    .filter((value) => value != null);
  return timestamps.length ? Math.max(...timestamps) : null;
}

function buildDaemonRemoteLatencySummary(daemonFeedback) {
  const status = String(daemonFeedback.status ?? "").toLowerCase();
  const timestampMs = numberOrNull(daemonFeedback.last_ack_ms);
  const metrics = {
    networkRoundTripMs: numberOrNull(daemonFeedback.network_round_trip_ms),
    estimatedOneWayMs: numberOrNull(daemonFeedback.estimated_one_way_ms),
    rawRoundTripMs: numberOrNull(daemonFeedback.raw_round_trip_ms),
    remoteProcessingMs: numberOrNull(daemonFeedback.remote_processing_ms),
    direction: daemonFeedback.direction ?? null,
  };

  if (status === "healthy" || status === "degraded") {
    return {
      state: status === "healthy" ? "pass" : "warn",
      message: daemonFeedback.summary ?? "Latency ACK received",
      ...metrics,
      sequence: numberOrNull(daemonFeedback.latest_sequence),
      timestampMs,
    };
  }

  if (status === "pending") {
    return {
      state: "pending",
      message: "等待远端 latency ACK",
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: daemonFeedback.direction ?? null,
      sequence: numberOrNull(daemonFeedback.latest_sequence),
      timestampMs: numberOrNull(daemonFeedback.last_probe_sent_ms),
    };
  }

  if (status === "timeout") {
    const pendingDurationMs = numberOrNull(daemonFeedback.pending_duration_ms);
    return {
      state: "fail",
      message:
        pendingDurationMs == null
          ? "远端 latency ACK 超时"
          : `远端 latency ACK 超时：${pendingDurationMs} ms`,
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: daemonFeedback.direction ?? null,
      sequence: numberOrNull(daemonFeedback.latest_sequence),
      timestampMs: numberOrNull(daemonFeedback.last_probe_sent_ms),
    };
  }

  if (status === "unavailable") {
    return {
      state: "fail",
      message: "远端 latency 不可用",
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: daemonFeedback.direction ?? null,
      sequence: numberOrNull(daemonFeedback.latest_sequence),
      timestampMs: null,
    };
  }

  return {
    state: "idle",
    message: "尚未运行网络延时探测",
    networkRoundTripMs: null,
    estimatedOneWayMs: null,
    rawRoundTripMs: null,
    remoteProcessingMs: null,
    direction: daemonFeedback.direction ?? null,
    sequence: numberOrNull(daemonFeedback.latest_sequence),
    timestampMs: null,
  };
}

function eventSummaryIsNewerThanDaemon(eventSummary, daemonFeedback, snapshot) {
  if (!eventSummary || eventSummary.state === "idle") {
    return false;
  }
  const eventTimestampMs = numberOrNull(eventSummary.timestampMs);
  if (eventTimestampMs == null) {
    return false;
  }
  const eventSequence = numberOrNull(eventSummary.sequence);
  const daemonSequence = numberOrNull(daemonFeedback.latest_sequence);
  if (eventSequence != null && daemonSequence != null) {
    return eventSequence > daemonSequence;
  }
  const generatedAtMs = numberOrNull(snapshot?.latency_feedback?.generated_at_ms);
  const daemonTimestampMs =
    generatedAtMs != null && generatedAtMs > 0
      ? generatedAtMs
      : maxTimestampMs(daemonFeedback.last_ack_ms, daemonFeedback.last_probe_sent_ms);
  return daemonTimestampMs == null || eventTimestampMs >= daemonTimestampMs;
}

function buildRemoteLatencyEventSummary(snapshot, deviceId) {
  const events = [...(snapshot?.recent_events ?? [])].filter(
    (event) => remoteLatencyEventMatchesDevice(event, deviceId),
  );
  const ack = events.filter(isRemoteLatencyAck).sort(sortNewestEvent)[0];
  const sent = events.filter(isRemoteLatencySent).sort(sortNewestEvent)[0];
  if (eventIsNewer(sent, ack)) {
    return {
      state: "pending",
      message: "等待远端 latency ACK",
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: sent.payload?.direction ?? null,
      sequence: numberOrNull(sent.sequence),
      timestampMs: numberOrNull(sent.timestamp_ms),
    };
  }

  if (ack) {
    const payload = ack.payload ?? {};
    const networkRoundTripMs = numberOrNull(
      payload.network_round_trip_ms ?? payload.latency_ms,
    );
    const estimatedOneWayMs = numberOrNull(payload.estimated_one_way_ms);
    const rawRoundTripMs = numberOrNull(
      payload.raw_round_trip_ms ?? payload.raw_latency_ms,
    );
    const remoteProcessingMs = numberOrNull(payload.remote_processing_ms);
    return {
      state:
        networkRoundTripMs != null && networkRoundTripMs <= LATENCY_HEALTHY_RTT_MS
          ? "pass"
          : "warn",
      message: ack.summary ?? "Latency ACK received",
      networkRoundTripMs,
      estimatedOneWayMs,
      rawRoundTripMs,
      remoteProcessingMs,
      direction: payload.direction ?? null,
      sequence: numberOrNull(ack.sequence),
      timestampMs: numberOrNull(ack.timestamp_ms),
    };
  }

  if (sent) {
    return {
      state: "pending",
      message: "等待远端 latency ACK",
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: sent.payload?.direction ?? null,
      sequence: numberOrNull(sent.sequence),
      timestampMs: numberOrNull(sent.timestamp_ms),
    };
  }

  return {
    state: "idle",
    message: "尚未运行网络延时探测",
    networkRoundTripMs: null,
    estimatedOneWayMs: null,
    rawRoundTripMs: null,
    remoteProcessingMs: null,
    direction: null,
    sequence: null,
    timestampMs: null,
  };
}

export function buildRemoteLatencySummary(snapshot, deviceId) {
  const daemonFeedback = snapshot?.latency_feedback?.remote_latency?.devices?.find(
    (device) => device?.device_id === deviceId,
  );
  const eventSummary = buildRemoteLatencyEventSummary(snapshot, deviceId);
  if (!daemonFeedback) {
    return eventSummary;
  }

  if (eventSummaryIsNewerThanDaemon(eventSummary, daemonFeedback, snapshot)) {
    return eventSummary;
  }

  return buildDaemonRemoteLatencySummary(daemonFeedback);
}

export function buildDeviceTypeSummaries(counts = {}) {
  return [
    { kind: "keyboard", title: "键盘", detail: String(counts.keyboard ?? 0) },
    { kind: "mouse", title: "鼠标", detail: String(counts.mouse ?? 0) },
    { kind: "gamepad", title: "手柄", detail: String(counts.gamepad ?? 0) },
    { kind: "display", title: "显示", detail: String(counts.display ?? 0) },
    { kind: "audio", title: "音频", detail: String(counts.audio ?? 0) },
    { kind: "remote", title: "远端", detail: String(counts.remote ?? 0) },
];
}

function asArray(value) {
  return Array.isArray(value) ? value : [];
}

function nonEmptyText(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function localInputKindLabel(kind) {
  return kind === "mouse" ? "鼠标" : "键盘";
}

function isDriverLikeDeviceName(value, kind) {
  const name = nonEmptyText(value);
  if (!name) {
    return true;
  }
  const normalized = name.toLowerCase();
  const label = kind === "mouse" ? "mouse" : "keyboard";
  return (
    normalized === `driver ${label}` ||
    normalized === `raw input ${label}` ||
    normalized.startsWith(`raw input ${label} `) ||
    normalized.includes("rshare kmdf") ||
    normalized.includes("rshare-filter") ||
    normalized.includes("rshare-driver") ||
    normalized.includes("\\??\\") ||
    normalized.includes("hid\\") ||
    normalized.includes("root#") ||
    normalized.includes("vid_") ||
    normalized.includes("pid_")
  );
}

function friendlyLocalInputDeviceName(device, kind, index) {
  const name = nonEmptyText(device?.name);
  if (name && !isDriverLikeDeviceName(name, kind)) {
    return name;
  }
  return `${localInputKindLabel(kind)} ${index + 1}`;
}

function localInputDeviceDetail(device, fallback) {
  return (
    nonEmptyText(device?.source) ??
    nonEmptyText(device?.capture_path) ??
    fallback
  );
}

function friendlyGamepadName(gamepad, index) {
  const raw = nonEmptyText(gamepad?.name);
  if (!raw) {
    return `手柄 ${index + 1}`;
  }
  const cleaned = raw
    .replace(/\s*\((?:xinput\s+standard\s+gamepad|standard\s+gamepad)\)\s*/gi, "")
    .replace(/\s{2,}/g, " ")
    .trim();
  return cleaned || `手柄 ${index + 1}`;
}

function finiteNumber(value, fallback = 0) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function integerNumber(value, fallback = 0) {
  return Math.trunc(finiteNumber(value, fallback));
}

function normalizedGamepadButtonToken(value) {
  return String(value ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
}

function uniqueGamepadButtonNames(values) {
  const byToken = new Map();
  for (const value of asArray(values)) {
    const name = nonEmptyText(value);
    const token = normalizedGamepadButtonToken(name);
    if (name && token && !byToken.has(token)) {
      byToken.set(token, name);
    }
  }
  return Array.from(byToken.values());
}

function pressedGamepadButtonNames(gamepad) {
  const pressedButtons = uniqueGamepadButtonNames(gamepad?.pressed_buttons);
  if (pressedButtons.length) {
    return pressedButtons;
  }
  return uniqueGamepadButtonNames(
    asArray(gamepad?.buttons)
      .filter((button) => button?.pressed)
      .map((button) => button?.button),
  );
}

function gamepadButtonTokenSet(buttons) {
  return new Set(buttons.map(normalizedGamepadButtonToken));
}

function gamepadNumericPayload(gamepad, keys) {
  return Object.fromEntries(
    keys.map((key) => [key, String(integerNumber(gamepad?.[key], 0))]),
  );
}

function gamepadFieldsChanged(previousGamepad, nextGamepad, keys, threshold) {
  return keys.some(
    (key) =>
      Math.abs(integerNumber(previousGamepad?.[key], 0) - integerNumber(nextGamepad?.[key], 0)) >
      threshold,
  );
}

function gamepadFieldsActive(gamepad, keys, threshold) {
  return keys.some((key) => Math.abs(integerNumber(gamepad?.[key], 0)) > threshold);
}

function triggerPercent(value) {
  const normalized = Math.max(0, Math.min(1, finiteNumber(value, 0) / 65535));
  return Math.round(normalized * 100);
}

export function buildBrowserGamepadRecentEvents(
  previousGamepad = null,
  nextGamepad = null,
  options = {},
) {
  if (!nextGamepad) {
    return [];
  }

  const gamepadId = Math.max(
    0,
    integerNumber(nextGamepad?.gamepad_id ?? previousGamepad?.gamepad_id, 0),
  );
  const name = nonEmptyText(nextGamepad?.name) ?? nonEmptyText(previousGamepad?.name) ?? `Gamepad ${gamepadId}`;
  const timestampMs = integerNumber(options.timestampMs, Date.now());
  const sequenceBase = integerNumber(options.sequenceBase, 0);
  const connected = nextGamepad?.connected !== false;
  const wasConnected = Boolean(previousGamepad?.connected);
  const nextPressedButtons = pressedGamepadButtonNames(nextGamepad);
  const basePayload = {
    gamepad_id: String(gamepadId),
    name,
    pressed_buttons: nextPressedButtons.join(","),
    event_count: String(integerNumber(nextGamepad?.event_count, 0)),
    button_event_count: String(integerNumber(nextGamepad?.button_event_count, 0)),
    button_press_count: String(integerNumber(nextGamepad?.button_press_count, 0)),
    button_release_count: String(integerNumber(nextGamepad?.button_release_count, 0)),
    axis_event_count: String(integerNumber(nextGamepad?.axis_event_count, 0)),
    trigger_event_count: String(integerNumber(nextGamepad?.trigger_event_count, 0)),
  };
  const events = [];
  const pushEvent = (eventKind, summary, payload = {}) => {
    events.push({
      sequence: sequenceBase + events.length + 1,
      timestamp_ms: timestampMs,
      device_kind: "Gamepad",
      event_kind: eventKind,
      summary,
      device_id: `gamepad-${gamepadId}`,
      source: "Hardware",
      payload: {
        ...basePayload,
        ...payload,
      },
    });
  };

  if (connected && !wasConnected) {
    pushEvent("connected", `connected ${name}`);
  } else if (!connected && wasConnected) {
    pushEvent("disconnected", `disconnected ${name}`);
    return events;
  }

  if (!connected) {
    return events;
  }

  const previousPressedButtons = pressedGamepadButtonNames(previousGamepad);
  const previousButtonTokens = gamepadButtonTokenSet(previousPressedButtons);
  const nextButtonTokens = gamepadButtonTokenSet(nextPressedButtons);

  for (const button of nextPressedButtons) {
    if (!previousButtonTokens.has(normalizedGamepadButtonToken(button))) {
      pushEvent("button", `${button} Pressed`, {
        button,
        state: "Pressed",
        last_button: `${button} Pressed`,
      });
    }
  }

  for (const button of previousPressedButtons) {
    if (!nextButtonTokens.has(normalizedGamepadButtonToken(button))) {
      pushEvent("button", `${button} Released`, {
        button,
        state: "Released",
        last_button: `${button} Released`,
      });
    }
  }

  const stickKeys = ["left_stick_x", "left_stick_y", "right_stick_x", "right_stick_y"];
  const triggerKeys = ["left_trigger", "right_trigger"];
  const stickChanged = previousGamepad
    ? gamepadFieldsChanged(previousGamepad, nextGamepad, stickKeys, 512)
    : gamepadFieldsActive(nextGamepad, stickKeys, 512);
  const triggerChanged = previousGamepad
    ? gamepadFieldsChanged(previousGamepad, nextGamepad, triggerKeys, 512)
    : gamepadFieldsActive(nextGamepad, triggerKeys, 512);

  if (stickChanged) {
    pushEvent("axis", "stick", {
      last_axis: "stick",
      ...gamepadNumericPayload(nextGamepad, stickKeys),
    });
  }

  if (triggerChanged) {
    pushEvent(
      "trigger",
      `trigger LT ${triggerPercent(nextGamepad?.left_trigger)}% / RT ${triggerPercent(nextGamepad?.right_trigger)}%`,
      gamepadNumericPayload(nextGamepad, triggerKeys),
    );
  }

  return events;
}

const AUDIO_FORM_FACTOR_INFO = Object.freeze({
  RemoteNetworkDevice: { category: "network", label: "网络音频" },
  Speakers: { category: "speaker", label: "音箱" },
  LineLevel: { category: "line", label: "线路输出" },
  Headphones: { category: "headphones", label: "耳机" },
  Microphone: { category: "microphone", label: "麦克风" },
  Headset: { category: "headset", label: "耳麦" },
  Handset: { category: "handset", label: "听筒" },
  DigitalPassthrough: { category: "passthrough", label: "数字直通" },
  Spdif: { category: "spdif", label: "SPDIF" },
  Hdmi: { category: "display", label: "显示器音频" },
  Unknown: null,
});

function normalizeAudioText(value) {
  return String(value ?? "").toLowerCase();
}

function inferAudioEndpointInfo(device, direction) {
  if (device?.kind === "Loopback") {
    return { category: "loopback", label: "系统回环" };
  }

  const backendInfo = AUDIO_FORM_FACTOR_INFO[device?.form_factor];
  if (backendInfo) {
    return backendInfo;
  }

  const name = normalizeAudioText(`${device?.name ?? ""} ${device?.source ?? ""}`);
  if (/(headset|耳麦|hands-free|handsfree)/i.test(name)) {
    return { category: "headset", label: "耳麦" };
  }
  if (/(headphone|耳机|buds|earbuds|wh-|\bairpods?\b)/i.test(name)) {
    return { category: "headphones", label: "耳机" };
  }
  if (/(speaker|speakers|扬声器|音箱)/i.test(name)) {
    return { category: "speaker", label: "音箱" };
  }
  if (/(hdmi|displayport|\bdp\b|monitor|显示器)/i.test(name)) {
    return { category: "display", label: "显示器音频" };
  }
  if (/(spdif|s\/pdif|optical|光纤)/i.test(name)) {
    return { category: "spdif", label: "SPDIF" };
  }
  if (/(mic|microphone|麦克风|阵列麦)/i.test(name) || direction === "input") {
    return { category: "microphone", label: "麦克风" };
  }
  if (/(bluetooth|蓝牙)/i.test(name)) {
    return { category: "bluetooth", label: "蓝牙音频" };
  }
  return { category: "audio", label: direction === "input" ? "音频输入" : "音频输出" };
}

export function describeAudioEndpoint(device = {}, direction = "output") {
  const info = inferAudioEndpointInfo(device, direction);
  const source = nonEmptyText(device?.source) ?? "audio";
  const parts = [info.label];
  const formFactorInfo = AUDIO_FORM_FACTOR_INFO[device?.form_factor];
  if (device?.kind === "Loopback" && formFactorInfo?.label) {
    parts.push(formFactorInfo.label);
  }
  parts.push(source);
  if (device?.default) {
    parts.push("default");
  }
  return {
    category: info.category,
    label: info.label,
    detail: parts.join(" / "),
  };
}

export function buildLocalDeviceSelectItems(snapshot = null, kind, audioOutputs = []) {
  if (kind === "keyboard" || kind === "mouse") {
    const devices = asArray(kind === "keyboard" ? snapshot?.keyboard_devices : snapshot?.mouse_devices);
    const state = kind === "keyboard" ? snapshot?.keyboard : snapshot?.mouse;
    const label = localInputKindLabel(kind);
    const aggregateItem = {
      id: `${kind}-default`,
      name: `综合${label}`,
      detail: devices.length
        ? `${devices.length} 个${label}合并输出`
        : nonEmptyText(state?.capture_source) ?? "等待输入事件",
      live: Boolean(state?.detected) || devices.some((device) => device?.connected !== false),
      active: true,
    };

    return [
      aggregateItem,
      ...devices.map((device, index) => ({
        id: device?.id || `${kind}-${index}`,
        name: friendlyLocalInputDeviceName(device, kind, index),
        detail: localInputDeviceDetail(device, kind),
        live: device?.connected !== false,
        active: false,
      })),
    ];
  }

  if (kind === "gamepad") {
    const gamepads = asArray(snapshot?.gamepads);
    const aggregateLive = gamepads.some((gamepad) => gamepad?.connected);
    return [
      {
        id: "gamepad-default",
        name: "综合手柄",
        detail: gamepads.length ? `${gamepads.length} 个手柄合并输出` : "未连接",
        live: aggregateLive,
        active: true,
      },
      ...gamepads.map((gamepad, index) => ({
        id: `gamepad-${gamepad?.gamepad_id ?? index}`,
        name: friendlyGamepadName(gamepad, index),
        detail: `事件 ${gamepad?.event_count ?? 0}`,
        live: Boolean(gamepad?.connected),
        active: false,
      })),
    ];
  }

  if (kind === "display") {
    const displays = asArray(snapshot?.display?.displays);
    if (displays.length) {
      return displays.map((display, index) => ({
        id: display?.display_id || `display-${index}`,
        name: display?.primary ? "主显示器" : `显示器 ${index + 1}`,
        detail: `${display?.width} x ${display?.height}`,
        live: true,
        active: Boolean(display?.primary) || index === 0,
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

  const audioInputs = asArray(snapshot?.audio_inputs).map((device, index) => {
    const endpoint = describeAudioEndpoint(device, "input");
    return {
      id: device?.id || `audio-input-${index}`,
      name: nonEmptyText(device?.name) ?? `音频输入 ${index + 1}`,
      detail: endpoint.detail,
      live: device?.connected !== false,
      active: Boolean(device?.default),
    };
  });
  const outputs = asArray(audioOutputs).map((device, index) => {
    const endpoint = describeAudioEndpoint(device, "output");
    return {
      id: device?.id || `audio-output-${index}`,
      name: nonEmptyText(device?.name) ?? `音频输出 ${index + 1}`,
      detail: endpoint.detail,
      live: device?.connected !== false,
      active: Boolean(device?.default),
    };
  });
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

export const HARDWARE_RIG_ASSETS = Object.freeze({
  keyboard: {
    manifest: "/assets/hardware/live2d/keyboard/manifest.json",
    base: "/assets/hardware/live2d/keyboard/base.png",
  },
  mouse: {
    manifest: "/assets/hardware/live2d/mouse/manifest.json",
    base: "/assets/hardware/live2d/mouse/base.png",
  },
  gamepad: {
    manifest: "/assets/hardware/live2d/gamepad/manifest.json",
    base: "/assets/hardware/live2d/gamepad/base.png",
  },
});

const RECENT_BUTTON_EVENT_WINDOW_MS = 900;
const MOUSE_BUTTON_ALIASES = Object.freeze({
  Left: ["Left", "button0", "button1", "primary", "mouseleft"],
  Right: ["Right", "button2", "button3", "secondary", "mouseright"],
  Middle: ["Middle", "middlebutton", "button1", "button3", "wheel", "wheelbutton", "auxiliary", "mousemiddle"],
  Back: ["Back", "x1", "xbutton1", "button4", "button8", "browserback", "side1", "other1", "other4", "other8", "unknown1", "unknown4", "unknown8"],
  Forward: ["Forward", "x2", "xbutton2", "button5", "button9", "browserforward", "side2", "other2", "other5", "other9", "unknown2", "unknown5", "unknown9"],
});

function hardwareRigForKind(kind) {
  return HARDWARE_RIG_ASSETS[kind] ? kind : null;
}

function hardwareRigVariantForKind(kind) {
  return HARDWARE_RIG_ASSETS[kind] ? "default" : null;
}

function recentKeyboardEvents(snapshot) {
  return (snapshot?.recent_events ?? [])
    .filter((event) => event?.device_kind === "Keyboard")
    .slice(-12);
}

function eventPayloadTokens(event, keys) {
  return keys
    .flatMap((key) => {
      const value = event?.payload?.[key];
      if (Array.isArray(value)) {
        return value;
      }
      if (value === null || value === undefined) {
        return [];
      }
      return String(value).split(/[,\s/]+/);
    })
    .filter(Boolean);
}

function normalizeButtonToken(value) {
  return String(value ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
}

function mouseButtonAliases(name) {
  return MOUSE_BUTTON_ALIASES[name] ?? [name];
}

function mouseButtonEventTokens(event) {
  return [
    ...eventPayloadTokens(event, ["button", "button_name", "name", "pressed_buttons"]),
    event?.summary,
  ]
    .filter(Boolean)
    .flatMap((value) => String(value).split(/[,\s/]+/).filter(Boolean));
}

function mouseEventState(event) {
  return String(event?.payload?.state ?? event?.summary ?? "");
}

function mouseEventIsPressed(event) {
  return /\b(pressed|down)\b/i.test(mouseEventState(event));
}

function mouseEventIsReleased(event) {
  return /\b(released|up)\b/i.test(mouseEventState(event));
}

export function mouseButtonRecentlyDown(
  events = [],
  buttonName,
  windowMs = RECENT_BUTTON_EVENT_WINDOW_MS,
) {
  const wanted = new Set(mouseButtonAliases(buttonName).map(normalizeButtonToken));
  const latestTimestamp = events.reduce(
    (latest, event) => Math.max(latest, Number(event?.timestamp_ms ?? 0)),
    0,
  );
  let latestMatchingEvent = null;

  for (const event of events) {
    if (event?.device_kind !== "Mouse" || event?.event_kind !== "button") {
      continue;
    }
    if (latestTimestamp) {
      const timestamp = Number(event?.timestamp_ms ?? 0);
      if (latestTimestamp - timestamp > windowMs) {
        continue;
      }
    }
    const matchesButton = mouseButtonEventTokens(event).some((token) =>
      wanted.has(normalizeButtonToken(token)),
    );
    if (matchesButton && (mouseEventIsPressed(event) || mouseEventIsReleased(event))) {
      latestMatchingEvent = event;
    }
  }

  return latestMatchingEvent ? mouseEventIsPressed(latestMatchingEvent) : false;
}

function recentMouseButtons(snapshot) {
  const events = snapshot?.recent_events ?? [];
  return Object.keys(MOUSE_BUTTON_ALIASES).filter((button) =>
    mouseButtonRecentlyDown(events, button),
  );
}

function galleryNode(index, item) {
  const physicalSlots = {
    display: { x: 620, y: 260, w: 460, h: 270, shape: "monitor" },
    keyboard: { x: 520, y: 575, w: 560, h: 170, shape: "keyboard" },
    mouse: { x: 1135, y: 300, w: 220, h: 260, shape: "mouse" },
    gamepad: { x: 250, y: 350, w: 300, h: 220, shape: "gamepad" },
    audio: { x: 1110, y: 60, w: 300, h: 190, shape: "speaker" },
    remote: { x: 80, y: 90, w: 310, h: 210, shape: "computer" },
  };
  const fallbackSlots = [
    { x: 90, y: 790 },
    { x: 450, y: 810 },
    { x: 820, y: 800 },
    { x: 1190, y: 790 },
  ];
  const slot = physicalSlots[item.kind];
  const fallback = fallbackSlots[index % fallbackSlots.length];
  const row = Math.floor(index / fallbackSlots.length);
  const point = slot ?? {
    x: fallback.x,
    y: fallback.y + row * 260,
    w: 280,
    h: 170,
    shape: "device",
  };
  return {
    x: point.x,
    y: point.y,
    w: point.w,
    h: point.h,
    shape: point.shape,
    rigKind: hardwareRigForKind(item.kind),
    rigVariant: hardwareRigVariantForKind(item.kind),
    ...item,
  };
}

export function buildDeviceGalleryItems(snapshot, audioOutputs = [], remoteDevices = []) {
  const keyboardDevices = snapshot?.keyboard_devices ?? [];
  const mouseDevices = snapshot?.mouse_devices ?? [];
  const gamepads = snapshot?.gamepads ?? [];
  const displays = snapshot?.display?.displays?.length
    ? snapshot.display.displays
    : snapshot?.display?.display_count
      ? [
          {
            display_id: "primary",
            width: snapshot.display.primary_width ?? 1920,
            height: snapshot.display.primary_height ?? 1080,
            primary: true,
          },
        ]
      : [];
  const audioInputs = snapshot?.audio_inputs ?? [];
  const allAudioOutputs = snapshot?.audio_outputs?.length ? snapshot.audio_outputs : audioOutputs;
  const items = [];

  if (keyboardDevices.length || snapshot?.keyboard?.detected) {
    items.push({
      id: "gallery-keyboard",
      kind: "keyboard",
      title: "综合键盘",
      detail: keyboardDevices.length
        ? `${keyboardDevices.length} 台键盘`
        : "默认键盘",
      metric: `${Number(snapshot?.keyboard?.event_count ?? 0)} 次`,
      activity: {
        pressedKeys: snapshot?.keyboard?.pressed_keys ?? [],
        lastKey: snapshot?.keyboard?.last_key ?? null,
        keyboardEvents: recentKeyboardEvents(snapshot),
      },
      live: Boolean(snapshot?.keyboard?.detected || keyboardDevices.some((device) => device.connected !== false)),
    });
  }

  if (mouseDevices.length || snapshot?.mouse?.detected) {
    items.push({
      id: "gallery-mouse",
      kind: "mouse",
      title: "综合鼠标",
      detail: mouseDevices.length ? `${mouseDevices.length} 台鼠标` : "默认鼠标",
      metric: `${Number(snapshot?.mouse?.event_count ?? 0)} 次`,
      activity: {
        pressedButtons: snapshot?.mouse?.pressed_buttons ?? [],
        recentButtons: recentMouseButtons(snapshot),
        x: Number(snapshot?.mouse?.x ?? 0),
        y: Number(snapshot?.mouse?.y ?? 0),
        wheelDeltaX: Number(snapshot?.mouse?.wheel_delta_x ?? 0),
        wheelDeltaY: Number(snapshot?.mouse?.wheel_delta_y ?? 0),
      },
      live: Boolean(snapshot?.mouse?.detected || mouseDevices.some((device) => device.connected !== false)),
    });
  }

  for (const gamepad of gamepads) {
    items.push({
      id: `gallery-gamepad-${gamepad.gamepad_id}`,
      kind: "gamepad",
      title: gamepad.name || `手柄 ${gamepad.gamepad_id}`,
      detail: "手柄",
      metric: `${Number(gamepad.event_count ?? 0)} 次`,
      activity: {
        pressedButtons: gamepad.pressed_buttons ?? [],
        leftStickX: Number(gamepad.left_stick_x ?? 0),
        leftStickY: Number(gamepad.left_stick_y ?? 0),
        rightStickX: Number(gamepad.right_stick_x ?? 0),
        rightStickY: Number(gamepad.right_stick_y ?? 0),
        leftTrigger: Number(gamepad.left_trigger ?? 0),
        rightTrigger: Number(gamepad.right_trigger ?? 0),
      },
      live: Boolean(gamepad.connected),
    });
  }

  for (const display of displays) {
    const width = Number(display.width ?? snapshot?.display?.primary_width ?? 1920);
    const height = Number(display.height ?? snapshot?.display?.primary_height ?? 1080);
    const displayId = display.display_id ?? "primary";
    const currentDisplayId = snapshot?.mouse?.current_display_id ?? null;
    const hasCurrentDisplayIndex =
      snapshot?.mouse?.current_display_index !== undefined &&
      snapshot?.mouse?.current_display_index !== null;
    const currentDisplayIndex = Number(snapshot?.mouse?.current_display_index ?? -1);
    const displayIndex = displays.indexOf(display);
    const pointerOnDisplay = Boolean(
      snapshot?.mouse?.detected &&
        (currentDisplayId
          ? currentDisplayId === displayId
          : hasCurrentDisplayIndex
            ? currentDisplayIndex === displayIndex
            : display.primary),
    );
    items.push({
      id: `gallery-display-${displayId}`,
      kind: "display",
      title: display.primary ? "主显示" : "显示",
      detail: `${width} x ${height}`,
      metric: display.primary ? "Primary" : "Display",
      activity: {
        pointerVisible: pointerOnDisplay,
        pointerX: Number(snapshot?.mouse?.display_relative_x ?? snapshot?.mouse?.x ?? 0),
        pointerY: Number(snapshot?.mouse?.display_relative_y ?? snapshot?.mouse?.y ?? 0),
        width,
        height,
      },
      live: true,
    });
  }

  const audioCount = audioInputs.length + allAudioOutputs.length;
  if (audioCount) {
    items.push({
      id: "gallery-audio",
      kind: "audio",
      title: "音频矩阵",
      detail: `${audioCount} 个端点`,
      metric: `${audioInputs.length} in / ${allAudioOutputs.length} out`,
      activity: {
        inputs: audioInputs.length,
        outputs: allAudioOutputs.length,
      },
      live: true,
    });
  }

  for (const device of remoteDevices) {
    items.push({
      id: `gallery-remote-${device.id}`,
      kind: "remote",
      title: device.name,
      detail: device.connected ? "已连接" : "已发现",
      metric: device.ipAddress ?? device.address ?? device.hostname ?? "",
      live: Boolean(device.connected),
    });
  }

  return items.map((item, index) => galleryNode(index, item));
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
  const keyboardEventCount = Number(keyboard.event_count ?? 0);
  const mouseEventCount = Number(mouse.event_count ?? 0);
  const gamepadEventCount = gamepads.reduce(
    (sum, item) => sum + Number(item?.event_count ?? 0),
    0,
  );
  const displayCount = Number(display.display_count ?? 0);
  const audioDeviceCount =
    (snapshot?.audio_inputs?.length ?? 0) + (snapshot?.audio_outputs?.length ?? 0);

  return {
    available: Boolean(snapshot && !error),
    error,
    composite: {
      label: "综合",
      live: Boolean(
        keyboard.detected ||
          mouse.detected ||
          gamepads.some((item) => item.connected) ||
          displayCount > 0 ||
          audioDeviceCount > 0,
      ),
      eventCount: keyboardEventCount + mouseEventCount + gamepadEventCount,
      deviceCount:
        (snapshot?.keyboard_devices?.length ?? (keyboard.detected ? 1 : 0)) +
        (snapshot?.mouse_devices?.length ?? (mouse.detected ? 1 : 0)) +
        gamepads.length +
        displayCount +
        audioDeviceCount,
    },
    keyboard: {
      status: keyboard.detected ? "capturing" : "missing",
      lastKey: keyboard.last_key ?? null,
      pressedKeys: keyboard.pressed_keys ?? [],
      eventCount: keyboardEventCount,
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
      eventCount: mouseEventCount,
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
      count: displayCount,
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
      key: "local",
      label: "本机能力",
      state: statusCheck(acceptance.localReady),
      detail: acceptance.localReady
        ? `本机后台、输入后端和 ${acceptance.localDisplayCount} 块显示器已就绪`
        : "本机后台、输入后端或显示器布局未就绪",
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
      state: statusCheck(acceptance.localDisplayCount > 0, acceptance.daemonOnline),
      detail: `本机显示器 ${acceptance.localDisplayCount} 块，Layout 当前显示 ${acceptance.visibleLayoutDevices} 个在线节点`,
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
  const connectedRemoteDevices = remoteDevices.filter((device) => device.connected);
  const backgroundReady =
    daemonOnline &&
    (status?.background_owner ?? "Daemon") === "Daemon" &&
    (status?.background_mode ?? "BackgroundProcess") === "BackgroundProcess";
  const trayOwnedByDaemon = daemonOnline && (status?.tray_owner ?? "Daemon") === "Daemon";
  const trayState = status?.tray_state ?? "Unavailable";
  const visibleLayoutDevices = payload?.visible_layout?.nodes?.length ?? layout.devices.length;
  const inputReady = daemonOnline && Boolean(status?.input_mode) && inputMode.health === "Healthy";
  const localDisplayCount =
    payload?.visible_layout?.nodes
      ?.find((node) => node.device_id === status?.device_id)
      ?.displays?.length ?? (layout.monitors.filter((monitor) => monitor.deviceId === status?.device_id).length || 0);
  const localReady =
    backgroundReady &&
    inputReady &&
    localDisplayCount > 0 &&
    !payload?.layout_error;
  const dualMachineReady =
    backgroundReady &&
    inputReady &&
    connectedRemoteDevices.length > 0 &&
    visibleLayoutDevices > 1 &&
    !payload?.layout_error;

  let nextStep = "启动守护进程后进行双机实机验收";
  if (daemonOnline && !inputReady) {
    nextStep = "检查输入后端权限或降级原因";
  } else if (daemonOnline && localReady && remoteDevices.length === 0) {
    nextStep = "本机能力已就绪，可以进行本机设备监控；双机验收等待局域网发现";
  } else if (daemonOnline && remoteDevices.length === 0) {
    nextStep = "打开另一台机器并保持同一局域网，等待自动发现";
  } else if (daemonOnline && connectedRemoteDevices.length === 0) {
    nextStep = "已发现远端设备；连接一台在线设备后开始边缘切换验收";
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
    connectedDevices: connectedRemoteDevices.length,
    visibleLayoutDevices,
    localDisplayCount,
    localReady,
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
        localDisplayCount: Number(raw.local_display_count ?? 0),
        localReady: Boolean(raw.local_ready),
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

export function buildDesktopViewModel(payload, localControls = null) {
  const status = payload?.status ?? null;
  const capabilities = buildCapabilityOverview(payload?.capabilities ?? null);
  const localDevice = buildLocalDevice(status);
  const discoveredRemoteDevices = (payload?.devices ?? []).map(buildRemoteDevice);
  const remoteDevices = discoveredRemoteDevices;
  const daemonLayout = buildLayoutFromVisibleGraph(
    payload?.visible_layout,
    payload?.layout,
    localDevice,
    remoteDevices,
    localControls,
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
    discoveredDevices: status?.discovered_devices ?? discoveredRemoteDevices.length,
    connectedDevices:
      status?.connected_devices ?? remoteDevices.filter((device) => device.connected).length,
    autoStarted: Boolean(payload?.auto_started ?? status?.started_by_desktop),
  };

  return {
    service,
    layout,
    devices: remoteDevices,
    latencyFeedback: status?.latency_feedback ?? null,
    capabilities,
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
    acceptance: buildAcceptance(payload, status, discoveredRemoteDevices, layout, inputMode),
  };
}
