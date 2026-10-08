// This tracker observes daemon truth only; discovery never grants trust.
export function deviceNotificationSnapshot(state) {
  const connections = state.connections;
  return {
    ready: connections.status !== null,
    // Dashboard fallback can retain an old stream boot ID; PID still changes
    // when the daemon restarts, so neither transport masks a fresh baseline.
    bootId: `${state.bootId ?? "dashboard"}:pid:${connections.status?.pid ?? "unknown"}`,
    devices: connections.devices,
  };
}

export function createDeviceNotificationTracker() {
  let initialized = false;
  let observedBootId;
  let peers = new Map();

  return {
    observe(snapshot, { enabled = true } = {}) {
      if (!snapshot?.ready) return [];
      const devices = new Map();
      for (const device of snapshot.devices ?? []) {
        if (typeof device?.id === "string" && device.id) devices.set(device.id, device);
      }
      const baseline = !initialized || snapshot.bootId !== observedBootId;
      if (baseline) peers = new Map();
      const events = [];
      for (const [id, device] of devices) {
        const previous = peers.get(id);
        const connected = device.connected === true;
        const name = String(device.name || device.hostname || id);
        if (!baseline) {
          if (!previous) {
            events.push({ deviceId: id, name, kind: connected ? "joined" : "discovered" });
          } else if (connected && !previous.connected) {
            events.push({ deviceId: id, name, kind: previous.everConnected ? "reconnected" : "joined" });
          }
        }
        peers.set(id, { connected, everConnected: connected || previous?.everConnected === true });
      }
      for (const [id, peer] of peers) {
        if (!devices.has(id)) peers.set(id, { ...peer, connected: false });
      }
      initialized = true;
      observedBootId = snapshot.bootId;
      return enabled ? events : [];
    },
  };
}

export function describeDeviceNotification(event) {
  switch (event.kind) {
    case "discovered":
      return { title: "发现附近设备", description: `${event.name} 已被发现，请在设备页确认连接。` };
    case "reconnected":
      return { title: "设备恢复连接", description: `${event.name} 已重新连接。` };
    default:
      return { title: "设备已加入", description: `${event.name} 已通过认证并连接。` };
  }
}
