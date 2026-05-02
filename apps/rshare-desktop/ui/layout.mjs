const SLOT_POSITIONS = [
  { x: 420, y: 176 },
  { x: 52, y: 176 },
  { x: 236, y: 24 },
  { x: 236, y: 332 },
  { x: 604, y: 64 },
  { x: 604, y: 288 },
];

function normalizeDevice(device, index) {
  const slot = SLOT_POSITIONS[index] ?? {
    x: 236 + ((index % 3) * 184),
    y: 24 + (Math.floor(index / 3) * 164),
  };

  return {
    id: String(device.id),
    label: device.name || 'Remote Device',
    subtitle: device.hostname || device.addresses?.[0] || 'unknown host',
    status: device.connected ? 'Connected' : 'Discovered',
    kind: 'remote',
    connected: Boolean(device.connected),
    x: slot.x,
    y: slot.y,
    width: 252,
    height: 156,
    lastSeen: device.last_seen_secs == null ? 'recently' : `${device.last_seen_secs}s ago`,
  };
}

export function buildScreenLayout(devices = [], status = null) {
  const deviceList = Array.isArray(devices) ? devices : [];
  const localScreen = {
    id: status?.device_id ? String(status.device_id) : 'local',
    label: status?.device_name || 'This PC',
    subtitle: status?.bind_address || 'Local device',
    status: status ? 'Ready' : 'Offline',
    kind: 'local',
    connected: Boolean(status),
    x: 236,
    y: 160,
    width: 320,
    height: 196,
    lastSeen: 'now',
  };

  const remoteScreens = [...deviceList]
    .sort((left, right) => {
      if (left.connected !== right.connected) {
        return left.connected ? -1 : 1;
      }
      return String(left.name).localeCompare(String(right.name));
    })
    .map((device, index) => normalizeDevice(device, index));

  return [localScreen, ...remoteScreens];
}

export function buildStatusBanner(status, devices = []) {
  const deviceList = Array.isArray(devices) ? devices : [];
  if (!status) {
    return {
      title: 'Daemon offline',
      detail: 'Start the service to discover devices and simulate their screen positions.',
      actionLabel: 'Start Service',
    };
  }

  const connectedCount = deviceList.filter((device) => device.connected).length;
  return {
    title: `${status.device_name} - ${connectedCount} connected / ${deviceList.length} discovered`,
    detail: `Listening on ${status.bind_address} - discovery UDP ${status.discovery_port}`,
    actionLabel: 'Stop Service',
  };
}
