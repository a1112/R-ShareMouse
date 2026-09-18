import { useCallback, useEffect, useState } from "react";
import { audioMetric, audioDeviceState, canGrantAudio, audioErrorMessage } from "./network-audio-model.mjs";
type Endpoint = { id: string; name: string; direction: "Input" | "Output"; channels: number; sample_rates: number[]; virtual_device: boolean; available: boolean };
type Grant = { peer: string; endpoint: string; direction: "Input" | "Output" };
type Config = { enabled: boolean; auto_register: boolean; media_port: number; buffer_ms: number; grants: Grant[] };
type Snapshot = {
  config: Config; local_endpoints: Endpoint[];
  devices: { id: string; name: string; status: string; error?: string }[];
  sessions: { id: string; format: {sample_rate:number;channels:number}; channel_map: number[] }[];
  backend: string; backend_ready: boolean; last_error?: string;
  diagnostics: { buffer_depth_ms: number; network_rtt_ms?: number; measured_one_way_p95_ms?: number; drift_ppm: number; lost: number; underruns: number };
};
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export function NetworkAudioPanel({ invoke }: {invoke:Invoke}) {
  const [snapshot,setSnapshot]=useState<Snapshot|null>(null);
  const [error,setError]=useState("");const [peer,setPeer]=useState("");const [busy,setBusy]=useState(false);
  const refresh=useCallback(async()=>{try{setSnapshot(await invoke<Snapshot>("network_audio",{command:"Status"}));setError("");}catch(e){setError(audioErrorMessage(e));}},[invoke]);
  useEffect(()=>{let active=true;let timer:ReturnType<typeof setTimeout>;const poll=async()=>{try{const value=await invoke<Snapshot>("network_audio",{command:"Status"});if(active){setSnapshot(value);setError("");}}catch(e){if(active)setError(audioErrorMessage(e));}finally{if(active)timer=setTimeout(poll,3000);}};void poll();return()=>{active=false;clearTimeout(timer);};},[invoke]);
  const command=async(value:unknown)=>{setBusy(true);try{setSnapshot(await invoke<Snapshot>("network_audio",{command:value}));setError("");}catch(e){setError(audioErrorMessage(e));}finally{setBusy(false);}};
  const configure=(patch:Partial<Config>)=>snapshot&&command({Configure:{...snapshot.config,...patch}});
  return <section className="mb-4 rounded-md border border-white/15 p-4 text-xs" aria-label="网络音频设备">
    <div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold">网络音频设备</h3><button type="button" onClick={()=>void refresh()}>刷新</button></div>
    {error&&<p role="alert" className="mb-3 break-words text-amber-300">{error}</p>}
    {snapshot&&<>
      <p className="mb-3">{snapshot.backend} · {snapshot.backend_ready?"可用":"尚未就绪"}</p>
      {snapshot.last_error&&<p className="mb-3 break-words text-amber-300">{audioErrorMessage(snapshot.last_error)}</p>}
      <div className="mb-4 flex flex-wrap items-center gap-4">
        <label><input type="checkbox" checked={snapshot.config.enabled} disabled={busy} onChange={e=>void configure({enabled:e.target.checked})}/> 启用网络音频</label>
        <label><input type="checkbox" checked={snapshot.config.auto_register} disabled={busy} onChange={e=>void configure({auto_register:e.target.checked})}/> 自动注册已授权端点</label>
        <label>接收缓冲 <select aria-label="接收缓冲" value={snapshot.config.buffer_ms} disabled={busy} onChange={e=>void configure({buffer_ms:Number(e.target.value)})} className="bg-neutral-800 p-1">{Array.from({length:19},(_,i)=>i+2).map(n=><option key={n} value={n}>{n} ms</option>)}</select></label>
      </div>
      <dl className="mb-4 grid grid-cols-2 gap-2 lg:grid-cols-3">
        <div><dt>缓冲深度</dt><dd>{audioMetric(snapshot.diagnostics.buffer_depth_ms)}</dd></div>
        <div><dt>网络 RTT</dt><dd>{audioMetric(snapshot.diagnostics.network_rtt_ms)}</dd></div>
        <div><dt>实测单向 P95</dt><dd>{audioMetric(snapshot.diagnostics.measured_one_way_p95_ms)}</dd></div>
        <div><dt>时钟漂移</dt><dd>{snapshot.diagnostics.drift_ppm.toFixed(1)} ppm</dd></div>
        <div><dt>丢包 / 欠载</dt><dd>{snapshot.diagnostics.lost} / {snapshot.diagnostics.underruns}</dd></div>
      </dl>
      <h4 className="mb-2 font-semibold">允许对端访问本机音频</h4>
      <input aria-label="已批准对端的设备 ID" value={peer} onChange={e=>setPeer(e.target.value.trim())} placeholder="已批准对端的设备 ID" className="mb-3 w-full rounded border border-white/20 bg-transparent p-2"/>
      <div className="mb-4 space-y-2">{snapshot.local_endpoints.map(endpoint=><div key={`${endpoint.direction}:${endpoint.id}`} className="flex items-center justify-between gap-3"><span>{endpoint.name} · {endpoint.direction==="Input"?"输入":"输出"} · {endpoint.channels} ch · {endpoint.sample_rates.join(" / ")} Hz</span><button type="button" disabled={busy||!canGrantAudio(peer,endpoint)} className="shrink-0 disabled:opacity-40" onClick={()=>void command({Grant:{peer,endpoint:endpoint.id,direction:endpoint.direction}})}>授权</button></div>)}{!snapshot.local_endpoints.length&&<p>未发现可用原生音频端点</p>}</div>
      {snapshot.config.grants.map(grant=><div className="mb-2 flex gap-3" key={`${grant.peer}:${grant.direction}:${grant.endpoint}`}><span className="min-w-0 flex-1 break-all">{grant.peer} → {grant.endpoint} · {grant.direction}</span><button type="button" disabled={busy} onClick={()=>void command({Revoke:grant})}>撤销</button></div>)}
      <h4 className="mb-2 mt-4 font-semibold">本机虚拟设备</h4>
      {snapshot.devices.length?snapshot.devices.map(device=><div className="mb-2" key={device.id}><span>{device.name} · {audioDeviceState(device)}</span>{device.error&&<p className="text-amber-300">{device.error}</p>}</div>):<p>尚无已注册的远端音频设备</p>}
      {snapshot.sessions.map(session=><div className="mt-2 flex gap-3" key={session.id}><span>{session.format.sample_rate} Hz · {session.format.channels} ch · 通道 {session.channel_map.map(c=>c+1).join(", ")}</span><button type="button" disabled={busy} onClick={()=>void command({Close:{session:session.id}})}>停止</button></div>)}
    </>}
  </section>;
}
