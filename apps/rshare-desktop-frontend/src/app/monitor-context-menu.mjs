export function buildMonitorContextMenuModel(monitor) {
  const displayId = monitor?.displayId ?? monitor?.id ?? "";
  const name = monitor?.name ?? "";
  const resolution =
    Number(monitor?.resWidth) > 0 && Number(monitor?.resHeight) > 0
      ? `${Number(monitor.resWidth)}x${Number(monitor.resHeight)}`
      : "";
  const local = monitor?.deviceKind === "local";
  const localOnlyReason = "仅本机显示器支持";

  return [
    {
      id: "copy-name",
      label: "复制显示器名称",
      enabled: Boolean(name),
      copyText: name,
      group: "copy",
    },
    {
      id: "copy-id",
      label: "复制显示器 ID",
      enabled: Boolean(displayId),
      copyText: displayId,
      group: "copy",
    },
    {
      id: "copy-resolution",
      label: "复制分辨率",
      enabled: Boolean(resolution),
      copyText: resolution,
      group: "copy",
    },
    {
      id: "open-display-settings",
      label: "打开系统显示设置",
      enabled: local,
      reason: local ? null : localOnlyReason,
      group: "settings",
    },
    {
      id: "change-resolution",
      label: "更改分辨率",
      enabled: local,
      reason: local ? null : localOnlyReason,
      group: "settings",
    },
    {
      id: "change-orientation",
      label: "更改方向",
      enabled: local,
      reason: local ? null : localOnlyReason,
      group: "settings",
    },
    {
      id: "change-scale",
      label: "更改缩放",
      enabled: local,
      reason: local ? null : localOnlyReason,
      group: "settings",
    },
    {
      id: "edit-position",
      label: "编辑显示器位置",
      enabled: Boolean(monitor?.id),
      reason: null,
      group: "layout",
    },
  ];
}
