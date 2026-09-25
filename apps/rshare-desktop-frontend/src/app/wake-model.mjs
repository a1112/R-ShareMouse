export function buildWakeRows(targets, devices, attempts) {
  const peers = new Map(devices.map((device) => [device.id, device]));
  return targets.map((target) => {
    const peer = target.peer_id ? peers.get(target.peer_id) : null;
    return {
      ...target,
      online: Boolean(peer?.connected || peer?.online),
      attempt: attempts[target.id] ?? null,
    };
  });
}

export function suggestedWakeIpv4(address) {
  if (typeof address !== "string") return "";
  const parts = address.split(".");
  return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
    ? address
    : "";
}

export function availableWakePeers(devices, targets, selectedPeerId, selectedName) {
  const saved = new Set(targets.map((target) => target.peer_id).filter(Boolean));
  const options = devices.filter((device) =>
    (device.online || device.connected || device.id === selectedPeerId) &&
    (!saved.has(device.id) || device.id === selectedPeerId))
    .map((device) => ({ id: device.id, name: device.name }));
  if (selectedPeerId && !options.some((device) => device.id === selectedPeerId)) {
    options.unshift({ id: selectedPeerId, name: `${selectedName}（离线）` });
  }
  return options;
}
