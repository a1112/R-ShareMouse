import { useEffect, useRef, useState } from "react";
import { dispatchFileDrop, fileDropTarget, formatTransferBytes, transferIsActive, transferPercent, TRANSFER_STATUS } from "../file-transfer.mjs";

type Peer = { id: string; name: string; connected: boolean };
type Transfer = { id: string; peer_id: string; incoming: boolean; status: string; entries: {path: string}[]; entry_count: number; total_bytes: number; transferred_bytes: number; destination?: string; error?: string };
type Props = {
  devices: Peer[]; theme: any;
  invoke: (command: string, args?: Record<string, unknown>) => Promise<any>;
  listen: (event: string, handler: (payload: any) => void) => Promise<null | (() => void)>;
};

export default function FileTransfersPanel({ devices, theme, invoke, listen }: Props) {
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [hover, setHover] = useState<string | null>(null);
  const peers = useRef(devices);
  peers.current = devices;
  const native = Boolean((window as any).__TAURI__?.core?.invoke);

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const value = await invoke("file_transfers");
        if (!stopped) { setTransfers(value); setLoadError(""); }
      } catch (err) {
        if (!stopped) setLoadError(`文件传输服务不可用：${String(err)}`);
      } finally {
        if (!stopped) timer = setTimeout(refresh, 1000);
      }
    }
    void refresh();
    return () => { stopped = true; clearTimeout(timer); };
  }, [invoke]);

  useEffect(() => {
    let stopped = false;
    const cleanups: (() => void)[] = [];
    const subscribe = async (event: string, callback: (payload: any) => void) => {
      try {
        const cleanup = await listen(event, (payload) => { if (!stopped) callback(payload); });
        if (cleanup) { if (stopped) cleanup(); else cleanups.push(cleanup); }
      } catch (err) { if (!stopped) setError(`拖拽监听失败：${String(err)}`); }
    };
    const point = (payload: any) => setHover(fileDropTarget(payload, peers.current, document, window.devicePixelRatio));
    void subscribe("tauri://drag-enter", point);
    void subscribe("tauri://drag-over", point);
    void subscribe("tauri://drag-leave", () => setHover(null));
    void subscribe("tauri://drag-drop", (payload) => {
      setHover(null); setError("");
      void dispatchFileDrop(payload, peers.current, document, window.devicePixelRatio, invoke)
        .then((value: Transfer) => { if (!stopped) setTransfers((old) => [value, ...old.filter((t) => t.id !== value.id)]); })
        .catch((err: unknown) => { if (!stopped) setError(String(err)); });
    });
    return () => { stopped = true; cleanups.forEach((cleanup) => cleanup()); };
  }, [invoke, listen]);

  const act = (command: string, transfer: Transfer) => {
    setError("");
    void invoke(command, { transferId: transfer.id }).catch((err) => setError(String(err)));
  };

  return <section aria-label="跨设备文件拖拽" className="shrink-0 border-b p-3" style={{ borderColor: theme.border, background: theme.sidebar }}>
    <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
      <h2 className="text-sm font-semibold">文件传输</h2>
      <span className="text-xs" style={{ color: theme.textMuted }}>{native ? "可跨屏拖入 Finder／资源管理器文件夹；拖到下方设备则接收到下载/R-ShareMouse" : "请在桌面应用中拖入文件；此处可查看传输进度"}</span>
    </div>
    <div className="mt-2 flex gap-2 overflow-x-auto pb-1">
      {devices.map((peer) => <div key={peer.id} data-file-drop-peer={peer.id}
        aria-label={`发送文件到 ${peer.name}`} aria-disabled={!peer.connected}
        className="min-w-[150px] rounded-md border border-dashed px-3 py-2 text-xs"
        style={{ borderColor: hover === peer.id ? theme.accent : theme.border, background: hover === peer.id ? theme.accentSoft : "transparent", opacity: peer.connected ? 1 : 0.5 }}>
        <div className="font-semibold">{peer.name}</div>
        <div className="mt-1" style={{ color: theme.textMuted }}>{peer.connected ? (hover === peer.id ? "松开发送文件" : "拖入文件或文件夹") : "连接后可发送"}</div>
      </div>)}
      {!devices.length && <span className="py-1 text-xs" style={{ color: theme.textMuted }}>连接另一台运行 R-ShareMouse 的电脑后即可发送。</span>}
    </div>
    {(error || loadError) && <p role="alert" className="mt-1 text-xs" style={{ color: theme.statusDangerText }}>{error || loadError}</p>}
    {!!transfers.length && <div className="mt-2 max-h-36 overflow-y-auto" aria-label="传输记录">
      {transfers.map((transfer) => <div key={transfer.id} className="border-t py-2 text-xs" style={{ borderColor: theme.border }}>
        <div className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate" title={transfer.entries.map((e) => e.path).join("\n")}>
            {transfer.incoming ? "接收" : "发送"} · {devices.find((p) => p.id === transfer.peer_id)?.name ?? transfer.peer_id.slice(0, 8)} · {transfer.entries[0]?.path ?? "准备文件"}{transfer.entry_count > 1 ? ` 等 ${transfer.entry_count} 项` : ""}
          </span>
          <span>{TRANSFER_STATUS[transfer.status] ?? transfer.status} · {transferPercent(transfer)}%</span>
          {transferIsActive(transfer.status) && <button type="button" className="rounded border px-2 py-1" onClick={() => act("cancel_file_transfer", transfer)}>取消</button>}
          {native && transfer.incoming && !transferIsActive(transfer.status) && transfer.destination && <button type="button" className="rounded border px-2 py-1" onClick={() => act("open_received_files", transfer)}>打开文件夹</button>}
        </div>
        <div className="mt-1" style={{ color: transfer.error ? theme.statusDangerText : theme.textMuted }}>
          {transfer.error ?? transfer.destination ?? `${formatTransferBytes(transfer.transferred_bytes)} / ${formatTransferBytes(transfer.total_bytes)}`}
        </div>
        {transferIsActive(transfer.status) && <progress aria-label="传输进度" max={100} value={transferPercent(transfer)} className="mt-1 h-1 w-full" style={{ accentColor: theme.accent }} />}
      </div>)}
    </div>}
  </section>;
}
