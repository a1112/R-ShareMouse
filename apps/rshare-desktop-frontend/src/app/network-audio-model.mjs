export function audioMetric(value, unit = "ms") {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? `${value.toFixed(2)} ${unit}` : "未测量";
}
export function audioDeviceState(device) {
  return ({ Pending: "等待系统注册", Registered: "已注册", Offline: "离线 · 静音", Unavailable: "不可用" })[device?.status] ?? "未知";
}
export function canGrantAudio(peer, endpoint) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(peer)
    && Boolean(endpoint?.id) && !endpoint?.virtual_device;
}
export function audioErrorMessage(error) {
  const message = String(error ?? "");
  if (message.includes("network_audio") && message.includes("Tauri")) {
    return "请在更新后的桌面应用中管理网络音频设备。";
  }
  if (message.includes("unknown variant") && message.includes("NetworkAudio")) {
    return "正在运行的后台服务版本较旧，请更新并重启服务。";
  }
  if (message.includes("Media orchestration")) {
    return "当前版本尚未接通网络音频设备，暂时不能使用远端输入输出。";
  }
  if (message.includes("HAL plug-in is not loaded")) {
    return "尚未加载 RShare 音频驱动。";
  }
  return message;
}
