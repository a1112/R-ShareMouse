export const MACOS_PERMISSION_ITEMS = Object.freeze([
  {
    key: "input_monitoring",
    label: "输入监控",
    description: "允许 R-ShareMouse 读取本机键盘和鼠标事件，用本机控制另一台电脑。",
    settingsLabel: "开启输入监控",
  },
  {
    key: "accessibility",
    label: "辅助功能",
    description: "允许 R-ShareMouse 将远端键盘和鼠标事件注入本机。",
    settingsLabel: "开启辅助功能",
  },
]);

export function normalizeMacosInputPermissions(value) {
  if (!value || value.supported !== true) {
    return null;
  }

  const inputMonitoring = Boolean(value.input_monitoring);
  const accessibility = Boolean(value.accessibility);
  return {
    supported: true,
    input_monitoring: inputMonitoring,
    accessibility,
    ready: inputMonitoring && accessibility,
  };
}

export function missingMacosInputPermissions(value) {
  const permissions = normalizeMacosInputPermissions(value) ?? value;
  if (!permissions || permissions.supported !== true) {
    return [];
  }

  return MACOS_PERMISSION_ITEMS.filter((item) => !permissions[item.key]);
}

export function macosInputPermissionSummary(value) {
  const permissions = normalizeMacosInputPermissions(value);
  if (!permissions) {
    return "未检测";
  }
  if (permissions.ready) {
    return "已就绪";
  }

  const missing = missingMacosInputPermissions(permissions);
  return `缺少${missing.map((item) => item.label).join("、")}`;
}
