export function getPageLabels() {
  return [
    { key: "layout", label: "布局" },
    { key: "devices", label: "设备" },
    { key: "settings", label: "设置" },
  ];
}

export function getThemeModeOptions() {
  return [
    { key: "light", label: "浅色" },
    { key: "dark", label: "深色" },
    { key: "system", label: "系统" },
  ];
}

export function getHeaderMetrics() {
  return {
    headerPaddingX: 12,
    brandGap: 8,
    navGap: 4,
    navButtonPaddingX: 10,
    navButtonPaddingY: 4,
    actionGap: 6,
    actionButtonPaddingX: 10,
    actionButtonPaddingY: 4,
    windowGap: 0,
    windowButtonSize: 32,
  };
}

export function buildFooterStatus(model) {
  if (!model.service.online) {
    return {
      summary: "守护进程离线，当前显示本机屏幕",
      endpoint: model.settings.localDevice.bindAddress,
    };
  }

  return {
    summary: `${model.settings.localDevice.name} · 已连接 ${model.service.connectedDevices} 台，已发现 ${model.service.discoveredDevices} 台`,
    endpoint: model.settings.localDevice.bindAddress,
  };
}
