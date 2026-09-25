import { useEffect, useState } from "react";
import { availableWakePeers, buildWakeRows, suggestedWakeIpv4 } from "./wake-model.mjs";

type WakeTarget = {
  id: string;
  name: string;
  mac: string;
  peer_id: string | null;
  ipv4: string | null;
};
type WakeAttempt = {
  id: string;
  target_id: string;
  status: "Waiting" | "Confirmed" | "Unconfirmed" | "Failed";
  message: string;
};
type Device = { id: string; name: string; connected: boolean; online: boolean; ipAddress?: string };
type Form = { id: string | null; name: string; mac: string; peer_id: string | null; ipv4: string };
const emptyForm: Form = { id: null, name: "", mac: "", peer_id: null, ipv4: "" };

export function WakePanel({ devices, command, theme }: {
  devices: Device[];
  command: <T = unknown>(name: string, args?: Record<string, unknown>) => Promise<T>;
  theme: { sidebar: string; border: string; text: string; textMuted: string; accent: string; accentSoft: string };
}) {
  const [targets, setTargets] = useState<WakeTarget[]>([]);
  const [attempts, setAttempts] = useState<Record<string, WakeAttempt>>({});
  const [form, setForm] = useState<Form>(emptyForm);
  const [mode, setMode] = useState<"standalone" | "peer">("standalone");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function refresh() {
    setTargets(await command<WakeTarget[]>("wake_targets"));
  }

  useEffect(() => {
    let active = true;
    Promise.all([command<WakeTarget[]>("wake_targets"), command<WakeAttempt[]>("wake_attempts")])
      .then(([saved, recent]) => {
        if (!active) return;
        setTargets(saved);
        const byTarget: Record<string, WakeAttempt> = {};
        for (const attempt of recent) {
          if (!byTarget[attempt.target_id]) byTarget[attempt.target_id] = attempt;
        }
        setAttempts(byTarget);
      })
      .catch((failure) => { if (active) setError(String(failure)); });
    return () => { active = false; };
  }, [command]);

  useEffect(() => {
    const waiting = Object.values(attempts).filter((attempt) => attempt.status === "Waiting");
    if (!waiting.length) return;
    const timer = window.setInterval(() => {
      for (const attempt of waiting) {
        void command<WakeAttempt>("wake_attempt", { attempt_id: attempt.id })
          .then((next) => setAttempts((current) => ({ ...current, [next.target_id]: next })))
          .catch((failure) => setError(String(failure)));
      }
    }, 2000);
    return () => window.clearInterval(timer);
  }, [attempts, command]);

  const rows = buildWakeRows(targets, devices, attempts) as Array<WakeTarget & { online: boolean; attempt: WakeAttempt | null }>;
  const availablePeers = availableWakePeers(devices, targets, form.peer_id, form.name) as Array<{ id: string; name: string }>;

  async function save() {
    setBusy(true); setError("");
    try {
      await command("save_wake_target", { target: {
        id: form.id, name: form.name, mac: form.mac,
        peer_id: mode === "peer" ? form.peer_id : null,
        ipv4: form.ipv4.trim() || null,
      } });
      await refresh();
      setForm(emptyForm);
      setMode("standalone");
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  }

  async function remove(id: string) {
    setBusy(true); setError("");
    try {
      await command("delete_wake_target", { target_id: id });
      await refresh();
      if (form.id === id) setForm(emptyForm);
      setAttempts((current) => { const next = { ...current }; delete next[id]; return next; });
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  }

  async function wake(id: string) {
    setBusy(true); setError("");
    try {
      const attempt = await command<WakeAttempt>("wake_target", { target_id: id });
      setAttempts((current) => ({ ...current, [id]: attempt }));
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  }

  function edit(target: WakeTarget) {
    setMode(target.peer_id ? "peer" : "standalone");
    setForm({ id: target.id, name: target.name, mac: target.mac, peer_id: target.peer_id, ipv4: target.ipv4 ?? "" });
  }

  const inputStyle = { background: "rgba(255,255,255,0.04)", color: theme.text, border: `1px solid ${theme.border}` };
  return (
    <section className="shrink-0 border-b px-4 py-3" style={{ background: theme.sidebar, borderColor: theme.border }} aria-label="局域网唤醒">
      <div className="mb-2 flex items-center justify-between gap-3">
        <div><h2 className="text-sm font-semibold">局域网唤醒</h2><p className="text-xs" style={{ color: theme.textMuted }}>目标需启用网卡和固件 WoL，双方位于同一子网。发包成功不代表已开机。</p></div>
        <button type="button" onClick={() => void refresh().catch((failure) => setError(String(failure)))} className="rounded px-2 py-1 text-xs" style={inputStyle}>刷新</button>
      </div>
      <div className="mb-3 grid gap-2 md:grid-cols-6">
        <select aria-label="目标类型" value={mode} disabled={Boolean(form.id)} onChange={(event) => { setMode(event.target.value as "peer" | "standalone"); setForm(emptyForm); }} className="rounded px-2 py-1.5 text-xs" style={inputStyle}>
          <option value="standalone">独立设备</option><option value="peer">R-ShareMouse 对端</option>
        </select>
        {mode === "peer" ? (
          <select aria-label="选择对端" value={form.peer_id ?? ""} onChange={(event) => {
            const peer = devices.find((device) => device.id === event.target.value);
            setForm((current) => ({ ...current, peer_id: peer?.id ?? null, name: peer?.name ?? current.name, ipv4: suggestedWakeIpv4(peer?.ipAddress) }));
          }} className="rounded px-2 py-1.5 text-xs" style={inputStyle}>
            <option value="">选择当前发现的对端</option>
            {availablePeers.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
          </select>
        ) : null}
        <input aria-label="目标名称" placeholder="设备名称" value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} className="rounded px-2 py-1.5 text-xs" style={inputStyle} />
        <input aria-label="MAC 地址" placeholder="MAC，例如 02:11:22:33:44:55" value={form.mac} onChange={(event) => setForm({ ...form, mac: event.target.value })} className="rounded px-2 py-1.5 text-xs" style={inputStyle} />
        <input aria-label="IPv4 地址" placeholder={mode === "peer" ? "IPv4（可选）" : "IPv4（必填，用于确认）"} value={form.ipv4} onChange={(event) => setForm({ ...form, ipv4: event.target.value })} className="rounded px-2 py-1.5 text-xs" style={inputStyle} />
        <button type="button" disabled={busy || !form.name.trim() || !form.mac.trim() || (mode === "peer" ? !form.peer_id : !form.ipv4.trim())} onClick={() => void save()} className="rounded px-3 py-1.5 text-xs disabled:opacity-50" style={{ background: theme.accentSoft, color: theme.accent }}>{form.id ? "保存" : "添加"}</button>
      </div>
      {form.id ? <button type="button" onClick={() => { setForm(emptyForm); setMode("standalone"); }} className="mb-2 text-xs" style={{ color: theme.textMuted }}>取消编辑</button> : null}
      {error ? <p role="alert" className="mb-2 text-xs text-red-300">{error}</p> : null}
      <div className="flex max-h-28 flex-wrap gap-2 overflow-auto">
        {rows.length ? rows.map((target) => (
          <div key={target.id} className="flex min-w-[280px] items-center gap-2 rounded px-2 py-1.5 text-xs" style={inputStyle}>
            <div className="min-w-0 flex-1"><div className="truncate font-medium">{target.name} · {target.online ? "在线" : "离线"}</div><div className="truncate" style={{ color: theme.textMuted }}>{target.mac}{target.ipv4 ? ` · ${target.ipv4}` : ""}</div>{target.attempt ? <div role="status" className="truncate" title={target.attempt.message}>{target.attempt.status === "Waiting" ? "等待确认：" : target.attempt.status === "Confirmed" ? "已响应：" : target.attempt.status === "Failed" ? "发送失败：" : "未确认："}{target.attempt.message}</div> : null}</div>
            <button type="button" disabled={busy || target.online || target.attempt?.status === "Waiting"} onClick={() => void wake(target.id)} className="disabled:opacity-40" style={{ color: theme.accent }}>唤醒</button>
            <button type="button" disabled={busy} onClick={() => edit(target)}>编辑</button>
            <button type="button" disabled={busy} onClick={() => void remove(target.id)}>删除</button>
          </div>
        )) : <p className="text-xs" style={{ color: theme.textMuted }}>暂无唤醒目标。对端需先在线以建立绑定；独立设备可直接添加。</p>}
      </div>
    </section>
  );
}
