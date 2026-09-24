export const MACOS_PERMISSION_ITEMS = Object.freeze([
  {
    key: "input_monitoring",
    label: "输入监控",
    description: "允许 R-ShareMouse 读取本机键盘和鼠标事件，用本机控制另一台电脑。",
    settingsLabel: "检查输入监控",
  },
  {
    key: "accessibility",
    label: "辅助功能",
    description: "允许 R-ShareMouse 将远端键盘和鼠标事件注入本机。",
    settingsLabel: "检查辅助功能",
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

export function shouldPromptForMacosInputPermissions(checked, value) {
  return checked && !normalizeMacosInputPermissions(value)?.ready;
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
  return `当前版本未获得${missing.map((item) => item.label).join("、")}`;
}

/**
 * Build the single macOS input warning shown by desktop UI.
 *
 * TCC preflight runs in the daemon that owns the actual event tap. Keep its
 * permission snapshot separate from input-backend health.
 */
export function buildMacosInputWarning(value, {
  permissionCheckFailed = false,
  runtimeDegraded = false,
  runtimeReason = null,
} = {}) {
  const permissions = normalizeMacosInputPermissions(value);
  const permissionMissing = Boolean(permissions && !permissions.ready);
  const permissionIssue = permissionMissing
    ? macosInputPermissionSummary(permissions)
    : permissionCheckFailed
      ? "无法读取权限状态"
      : null;
  const runtimeIssue = runtimeDegraded
    ? runtimeReason || "未报告具体原因"
    : null;
  const runtimeSummary = runtimeIssue
    ? `守护进程输入后端未就绪：${runtimeIssue}`
    : null;

  if (!permissionIssue && !runtimeSummary) {
    return null;
  }

  return {
    label: permissionIssue ? "权限不足⚠️" : "输入异常⚠️",
    summary: [permissionIssue, runtimeSummary].filter(Boolean).join("；"),
    runtimeIssue,
  };
}
