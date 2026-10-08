export const MACOS_PERMISSION_ITEMS = Object.freeze([
  {
    key: "input_monitoring",
    label: "输入监听能力",
    description: "读取本机键盘和鼠标事件。辅助功能授权也可提供此能力，输入监控列表可能没有独立记录。",
    settingsLabel: "打开输入监控",
  },
  {
    key: "accessibility",
    label: "输入注入能力",
    description: "将远端键盘和鼠标事件注入本机；需要辅助功能授权。",
    settingsLabel: "打开辅助功能",
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
  return `${missing.map((item) => item.label).join("、")}不可用`;
}

// A daemon started before an Accessibility grant can keep reporting stale
// preflight results. Restart must remain available so the new process can read
// those grants, even when the old process reports missing input capabilities.
export function canRestartMacosInputService(value, {
  restartRequired = false,
  daemonInputReady = true,
} = {}) {
  return Boolean(normalizeMacosInputPermissions(value) &&
    (restartRequired || !daemonInputReady));
}

/** Decide recovery from freshly read daemon values, never a render closure. */
export function assessMacosInputRecovery(value, status) {
  const permissions = normalizeMacosInputPermissions(value);
  if (!permissions || !status || typeof status !== "object") {
    return {
      ready: false,
      reason: "无法确认重启后的守护进程输入状态，请重新检测。",
    };
  }
  if (!permissions.ready) {
    return {
      ready: false,
      reason: `${macosInputPermissionSummary(permissions)}。请确认当前 R-ShareMouse.app 的辅助功能授权后重启服务。`,
    };
  }
  if (!status.input_mode || status.backend_health !== "Healthy") {
    const reason = status.last_backend_error ?? status.backend_health?.Degraded?.reason ??
      (typeof status.backend_health === "string" ? status.backend_health : null) ?? "未报告具体原因";
    return { ready: false, reason: `守护进程输入后端未就绪：${reason}` };
  }
  return { ready: true, reason: null };
}

/**
 * Build the single macOS input warning shown by desktop UI.
 *
 * Preflight runs in the daemon that owns the actual event tap. It reports listen
 * and post capabilities, not individual TCC pane grants. Keep that snapshot
 * separate from input-backend health.
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
    label: permissionIssue ? "输入能力不足⚠️" : "输入异常⚠️",
    summary: [permissionIssue, runtimeSummary].filter(Boolean).join("；"),
    runtimeIssue,
  };
}
