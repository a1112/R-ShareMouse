export const TRANSFER_STATUS = {
  Preparing: "整理文件", Waiting: "等待接收", Transferring: "传输中",
  Completed: "已完成", Cancelled: "已取消", Failed: "失败",
};

export function transferIsActive(status) {
  return ["Preparing", "Waiting", "Transferring"].includes(status);
}

export function transferPercent(transfer) {
  if (transfer.status === "Completed") return 100;
  if (!transfer.total_bytes) return 0;
  // Only the receiver's completion acknowledgement may show 100%.
  return Math.min(99, Math.max(0, Math.floor(100 * transfer.transferred_bytes / transfer.total_bytes)));
}

export function formatTransferBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GiB`;
}

// Tauri drag positions use physical pixels; DOM hit testing uses CSS pixels.
export function fileDropTarget(payload, devices, document, scale = 1) {
  const { x, y } = payload?.position ?? {};
  if (!Number.isFinite(x) || !Number.isFinite(y) || !(scale > 0)) return null;
  const element = document.elementFromPoint(x / scale, y / scale)?.closest("[data-file-drop-peer]");
  const id = element?.getAttribute("data-file-drop-peer");
  return devices.find((device) => device.id === id && device.connected)?.id ?? null;
}

export async function dispatchFileDrop(payload, devices, document, scale, send) {
  const id = fileDropTarget(payload, devices, document, scale);
  if (!id) throw new Error("请将文件拖到已连接设备的文件接收卡片");
  const paths = payload?.paths;
  if (!Array.isArray(paths) || !paths.length || paths.length > 1024
      || paths.some((path) => typeof path !== "string" || !path)) {
    throw new Error("请拖入 1–1024 个本地文件或目录");
  }
  return send("send_files", { deviceId: id, paths });
}
