export function getPageLabels() {
  return [
    { key: "layout", label: "布局" },
    { key: "devices", label: "设备" },
    { key: "logs", label: "日志" },
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

export function getHardwareAssetPresetOptions() {
  return [
    { key: "office", label: "办公" },
    { key: "gaming", label: "游戏" },
  ];
}

export function getDeviceConsoleSections() {
  return {
    endpointAcceptance: true,
    localLatencyFeedback: false,
  };
}

export function getDeviceSimulatorChrome() {
  return {
    textureFirst: true,
    deviceFrames: false,
    annotationFrames: false,
    frontFacingDisplays: true,
    displayWindowTexture: true,
  };
}

export function getSettingsLayoutSections() {
  return [
    { key: "local", label: "本机信息", description: "设备名称、主机与监听端口" },
    { key: "service", label: "服务状态", description: "守护进程运行状态" },
    { key: "hardware", label: "硬件资产", description: "贴图和导入包" },
    { key: "input", label: "输入后端", description: "捕获模式与健康度" },
    { key: "appearance", label: "界面风格", description: "主题外观" },
    { key: "acceptance", label: "实机验收", description: "联机前检查项" },
  ];
}

export function getHeaderMetrics() {
  return {
    headerHeight: 40,
    headerPaddingX: 10,
    navGap: 4,
    navButtonPaddingX: 10,
    navButtonPaddingY: 3,
    actionGap: 6,
    actionButtonPaddingX: 10,
    actionButtonPaddingY: 3,
    windowGap: 0,
    windowButtonSize: 16,
    windowButtonHitSize: 46,
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
