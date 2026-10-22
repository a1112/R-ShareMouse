// Shared pure helpers are tested with node --test; browser startup is explicit.
export function imagePoint(clientX, clientY, rect, width, height, clamp = false) {
  if (!(width > 0 && height > 0 && rect.width > 0 && rect.height > 0)) return null;
  const scale = Math.min(rect.width / width, rect.height / height);
  const w = width * scale, h = height * scale;
  const x = (clientX - rect.left - (rect.width - w) / 2) / w;
  const y = (clientY - rect.top - (rect.height - h) / 2) / h;
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  if (!clamp && (x < 0 || x > 1 || y < 0 || y > 1)) return null;
  return { x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)) };
}

export class ContactTracker {
  contacts = new Map();
  down(pointer, point, pressure = .5) {
    if (!point || this.contacts.size >= 10 || this.contacts.has(pointer)) return false;
    const used = new Set(this.snapshot().map(p => p.id));
    let id = 1; while (used.has(id)) id++;
    this.contacts.set(pointer, { id, ...point, pressure: Math.round(Math.max(0, Math.min(1, pressure || .5)) * 1024) });
    return true;
  }
  move(pointer, point, pressure = .5) {
    const previous = this.contacts.get(pointer);
    if (previous && point) this.contacts.set(pointer, { ...previous, ...point, pressure: Math.round(Math.max(0, Math.min(1, pressure || .5)) * 1024) });
  }
  up(pointer) { this.contacts.delete(pointer); }
  clear() { this.contacts.clear(); }
  snapshot() { return [...this.contacts.values()].map(p => ({ ...p })); }
}

export function preferH264(codecs) {
  return [...codecs].sort((a, b) => Number(b.mimeType.toLowerCase() === 'video/h264') - Number(a.mimeType.toLowerCase() === 'video/h264'));
}
export function selectedPath(stats) {
  const transport = [...stats.values()].find(s => s.type === 'transport' && s.selectedCandidatePairId);
  const pair = transport && stats.get(transport.selectedCandidatePairId);
  if (!pair) return null;
  const local = stats.get(pair.localCandidateId), remote = stats.get(pair.remoteCandidateId);
  return { local: local?.address || '隐藏', remote: remote?.address || '隐藏', protocol: local?.protocol || '未知', rttMs: Number.isFinite(pair.currentRoundTripTime) ? Math.round(pair.currentRoundTripTime * 1000) : null };
}

export function isVideoPlaying(video) {
  // WebKit can resolve play() on a remote track before ICE or its first frame.
  return !!video.srcObject && !video.paused && !video.ended && video.readyState >= 2
    && video.videoWidth > 0 && video.videoHeight > 0;
}

// Browsers may deny this on HTTP; native receivers implement the same lifecycle.
export class ScreenWakeLock {
  constructor(api) { this.api = api; this.active = false; this.lock = null; this.pending = false; }
  async setActive(active) {
    this.active = active;
    if (!active) {
      const lock = this.lock; this.lock = null;
      try { await lock?.release(); } catch {}
      return;
    }
    if (!this.api || this.lock || this.pending) return;
    this.pending = true;
    try {
      const lock = await this.api.request('screen');
      if (!this.active) { await lock.release(); return; }
      this.lock = lock;
      lock.addEventListener('release', () => { if (this.lock === lock) this.lock = null; });
    } catch {} finally { this.pending = false; }
  }
}

export function startPage() {
  const el = id => document.getElementById(id);
  const host = new URL(location.href).searchParams.get('role') === 'host';
  const video = el('video'), contacts = new ContactTracker(), wakeLock = new ScreenWakeLock(navigator.wakeLock);
  let socket, pc, stream, screen, receiver = false, sequence = 0, touchEnabled = false;
  let makingOffer = false, stopped = false, signalChain = Promise.resolve(), touchPending = false;
  let heartbeat, statsTimer, retryTimer = null, retries = 0, resumeOnShow = false, queuedIce = [];
  let previousBytes = 0, previousTime = 0;
  const status = text => { el('status').textContent = text; };
  el('host-controls').hidden = !host;
  el('receiver-controls').hidden = host;
  el('title').textContent = host ? '扩展显示器 · 主机' : '扩展显示器 · 接收端';
  history.replaceState(null, '', `${location.pathname}${host ? '?role=host' : ''}`);
  function presentation() {
    const playing = !host && !stopped && !document.hidden && pc?.connectionState === 'connected' && isVideoPlaying(video);
    const immersive = playing && document.body.classList.contains('immersive');
    void wakeLock.setActive(playing);
    window.dispatchEvent(new CustomEvent('rshare-display-state', { detail: { playing, immersive } }));
  }
  for (const event of ['playing', 'pause', 'ended', 'emptied', 'loadeddata', 'resize', 'waiting']) video.addEventListener(event, presentation);
  video.addEventListener('playing', () => { retries = 0; });
  function send(message) {
    if (socket?.readyState !== WebSocket.OPEN) return false;
    if (socket.bufferedAmount > 64 * 1024) { stop('控制通道拥塞，已停止连接'); return false; }
    socket.send(JSON.stringify(message)); return true;
  }
  function release() { contacts.clear(); sendTouch(); }
  function resetTouch() {
    touchEnabled = false; touchPending = false; contacts.clear();
    el('touch').checked = false; el('touch-status').textContent = '触摸未启用';
  }
  function sendTouch() {
    if (host || !touchEnabled) return;
    touchPending = false;
    send({ type: 'touch', sequence: ++sequence, contacts: contacts.snapshot() });
  }
  function closePeer() {
    release(); pc?.close(); pc = null;
    if (!host) video.srcObject = null;
    presentation();
  }
  function stop(reason = '连接已结束') {
    if (stopped) return;
    stopped = true;
    clearTimeout(retryTimer); retryTimer = null; resumeOnShow = false;
    release(); stream?.getTracks().forEach(t => t.stop()); stream = null;
    document.body.classList.remove('immersive');
    if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
    closePeer(); resetTouch(); queuedIce = [];
    const previous = socket; socket = null; previous?.close();
    clearInterval(heartbeat); clearInterval(statsTimer);
    status(reason);
    el('share').disabled = true; el('create').disabled = true; el('touch').disabled = true;
    el('play').disabled = true; el('reconnect').hidden = host;
    presentation();
  }
  function interrupted(reason) {
    stop(reason);
    if (host || document.hidden || retries >= 5) return;
    const delay = Math.min(500 * 2 ** retries++, 5000);
    status(`${reason}，正在重连（${retries}/5）…`);
    retryTimer = setTimeout(() => { retryTimer = null; connect(); }, delay);
  }
  function suspend() {
    if (host || (stopped && retryTimer === null)) return;
    clearTimeout(retryTimer); retryTimer = null;
    stop('附屏已暂停'); resumeOnShow = true;
  }
  function resume() {
    if (resumeOnShow && !document.hidden) { resumeOnShow = false; retries = 0; connect(); }
    presentation();
  }
  function newPeer() {
    closePeer();
    previousBytes = 0; previousTime = 0;
    const next = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' });
    pc = next;
    next.onicecandidate = e => { if (e.candidate && pc === next) send({ type: 'ice', candidate: e.candidate.toJSON() }); };
    next.onconnectionstatechange = () => {
      if (pc !== next) return;
      const labels = { new: '等待视频连接', connecting: '正在连接视频', connected: '视频已连接', disconnected: '视频连接中断', failed: '视频连接失败', closed: '视频已关闭' };
      status(labels[next.connectionState] || '等待视频连接');
      presentation();
      if (next.connectionState === 'failed' || next.connectionState === 'disconnected') release();
      if (!host && next.connectionState === 'failed') interrupted('视频连接中断');
    };
    next.ontrack = e => {
      if (pc !== next || stopped) return;
      video.srcObject = e.streams[0] ?? new MediaStream([e.track]);
      // Browser support varies. Targets are hints, never a measured latency claim.
      if ('jitterBufferTarget' in e.receiver) { try { e.receiver.jitterBufferTarget = 0; } catch {} }
      if ('playoutDelayHint' in e.receiver) { try { e.receiver.playoutDelayHint = 0; } catch {} }
      el('play').disabled = false;
      video.play().then(presentation).catch(() => status('请点击“播放 / 全屏”开始播放'));
    };
    return next;
  }
  async function offer() {
    if (stopped || !receiver || !stream || makingOffer) return;
    makingOffer = true;
    try {
      const next = newPeer();
      const track = stream.getVideoTracks()[0];
      track.contentHint = 'motion';
      const sender = next.addTrack(track, stream);
      const transceiver = next.getTransceivers()[0];
      const codecs = RTCRtpSender.getCapabilities?.('video')?.codecs;
      if (codecs && transceiver.setCodecPreferences) transceiver.setCodecPreferences(preferH264(codecs));
      await next.setLocalDescription(await next.createOffer());
      if (pc !== next || stopped) return;
      send({ type: 'offer', sdp: next.localDescription.sdp });
      const parameters = sender.getParameters();
      if (parameters.encodings?.length) {
        parameters.encodings[0].maxBitrate = 12_000_000;
        parameters.encodings[0].maxFramerate = 60;
        parameters.degradationPreference = 'maintain-framerate';
        await sender.setParameters(parameters).catch(() => {});
      }
    } finally { makingOffer = false; }
  }
  async function receive(message) {
    switch (message.type) {
      case 'error': status(message.message); break;
      case 'hello': status(host ? '先创建扩展屏，再点击选择屏幕' : '等待主机分享扩展屏'); break;
      case 'display':
        screen = message.display;
        el('screen').textContent = `${screen.friendly_name || screen.display_id} · ${screen.width} × ${screen.height}`;
        el('share').disabled = false; el('create').disabled = true;
        status(host ? '扩展屏已被系统识别；请在屏幕选择器中选中它' : '扩展屏已就绪，等待主机分享画面'); break;
      case 'receiver_ready': receiver = true; await offer(); break;
      case 'receiver_left': receiver = false; closePeer(); resetTouch(); queuedIce = []; status('接收端已断开，等待重新连接'); break;
      case 'host_left': stop('主机已断开'); break;
      case 'session_ended': stop('扩展屏会话已结束'); break;
      case 'touch_enabled': touchEnabled = message.enabled; el('touch').checked = touchEnabled; if (!touchEnabled) contacts.clear(); el('touch-status').textContent = touchEnabled ? 'Windows 多点触摸已启用' : '触摸未启用'; break;
      case 'offer': {
        const next = newPeer();
        await next.setRemoteDescription({ type: 'offer', sdp: message.sdp });
        if (pc !== next || stopped) return;
        for (const candidate of queuedIce.splice(0)) await next.addIceCandidate(candidate);
        await next.setLocalDescription(await next.createAnswer());
        if (pc !== next || stopped) return;
        send({ type: 'answer', sdp: next.localDescription.sdp }); break;
      }
      case 'answer': {
        const next = pc;
        if (next) {
          await next.setRemoteDescription({ type: 'answer', sdp: message.sdp });
          if (pc === next) for (const candidate of queuedIce.splice(0)) await next.addIceCandidate(candidate);
        }
        break;
      }
      case 'ice':
        if (pc?.remoteDescription) await pc.addIceCandidate(message.candidate);
        else if (queuedIce.length < 64) queuedIce.push(message.candidate);
        else interrupted('视频协商数据过多');
        break;
    }
  }
  el('create').onclick = () => send({ type: 'create', width: 1920, height: 1080 });
  el('share').onclick = async () => {
    try {
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error('请在本机 Edge / Chrome 中打开 localhost 主机链接');
      // Changing capture invalidates the user's screen-to-touch confirmation,
      // including when the picker is cancelled. Revoke before opening it.
      send({ type: 'enable_touch', enabled: false });
      touchEnabled = false; el('touch').checked = false; el('touch').disabled = true;
      closePeer();
      stream?.getTracks().forEach(t => { t.onended = null; t.stop(); });
      stream = null;
      // Call directly from the click gesture, without preceding asynchronous work.
      stream = await navigator.mediaDevices.getDisplayMedia({ video: { displaySurface: 'monitor', frameRate: { ideal: 60, max: 60 }, width: { ideal: 1920 }, height: { ideal: 1080 } }, audio: false });
      if (stopped) { stream.getTracks().forEach(t => t.stop()); stream = null; return; }
      const track = stream.getVideoTracks()[0];
      if (track.getSettings().displaySurface && track.getSettings().displaySurface !== 'monitor') {
        stream.getTracks().forEach(t => t.stop()); stream = null;
        throw new Error('请选择整个扩展显示器，不要选择窗口或浏览器标签页');
      }
      video.srcObject = stream; video.play().catch(() => {});
      track.onended = () => stop('屏幕分享已结束');
      el('touch').disabled = false;
      status('请核对画面确实来自上方扩展屏，再启用触摸');
      await offer();
    } catch (error) { status(error.message); }
  };
  el('touch').onchange = e => send({ type: 'enable_touch', enabled: e.target.checked });
  el('stop').onclick = () => { clearTimeout(retryTimer); retryTimer = null; resumeOnShow = false; stop(); status('连接已结束'); };
  el('reconnect').onclick = () => { retries = 0; resumeOnShow = false; connect(); };
  el('play').onclick = () => {
    video.play().catch(() => {});
    document.body.classList.add('immersive');
    // Fullscreen the DOM container, not Safari's native video player: the video
    // must remain a Pointer Events target. CSS covers browsers without the API.
    if (!window.rshareNativeDisplay) el('viewer').requestFullscreen?.().catch(() => {});
    presentation();
  };
  el('leave-fullscreen').onclick = () => {
    release(); document.body.classList.remove('immersive');
    if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
    presentation();
  };
  document.addEventListener('fullscreenchange', () => {
    release();
    if (!document.fullscreenElement) document.body.classList.remove('immersive');
    presentation();
  });
  for (const name of ['pointerdown', 'pointermove', 'pointerup', 'pointercancel', 'lostpointercapture']) {
    video.addEventListener(name, event => {
      if (host || !touchEnabled || !pc || pc.connectionState !== 'connected') return;
      event.preventDefault();
      const point = imagePoint(event.clientX, event.clientY, video.getBoundingClientRect(), video.videoWidth, video.videoHeight, name !== 'pointerdown');
      if (name === 'pointerdown') {
        if (!contacts.down(event.pointerId, point, event.pressure)) return;
        video.setPointerCapture(event.pointerId); sendTouch();
      } else if (name === 'pointermove') {
        contacts.move(event.pointerId, point, event.pressure);
        if (!touchPending) { touchPending = true; requestAnimationFrame(() => { if (touchPending) sendTouch(); }); }
      } else { contacts.up(event.pointerId); sendTouch(); }
    });
  }
  document.addEventListener('visibilitychange', () => { if (document.hidden) { release(); suspend(); } else resume(); });
  window.addEventListener('pagehide', () => { if (host) stop(); else suspend(); });
  window.addEventListener('pageshow', resume);
  window.addEventListener('resize', release);
  async function updateStats() {
    const peer = pc;
    if (!peer) return;
    try {
      const report = await peer.getStats();
      if (pc !== peer) return;
      const path = selectedPath(report);
      const media = [...report.values()].find(s => s.type === (host ? 'outbound-rtp' : 'inbound-rtp') && s.kind === 'video');
      const bytes = media?.bytesSent ?? media?.bytesReceived ?? 0;
      const elapsed = (media?.timestamp ?? 0) - previousTime;
      const mbps = elapsed > 0 ? (bytes - previousBytes) * 8 / elapsed / 1000 : 0;
      previousTime = media?.timestamp ?? 0; previousBytes = bytes;
      const codec = media && report.get(media.codecId);
      el('stats').textContent = `${media?.framesPerSecond ?? '?'} fps · ${Math.max(0, mbps).toFixed(1)} Mbps · ${codec?.mimeType ?? ''}${path ? ` · RTT ${path.rttMs ?? '?'} ms · ${path.local} → ${path.remote} (${path.protocol})` : ''}`;
    } catch {}
  }
  function connect() {
    clearTimeout(retryTimer); retryTimer = null;
    if (socket) return;
    stopped = false; sequence = 0; signalChain = Promise.resolve(); queuedIce = [];
    resetTouch(); el('reconnect').hidden = true; el('play').disabled = true;
    status('正在连接…');
    heartbeat = setInterval(() => { send({ type: 'ping' }); if (contacts.contacts.size) sendTouch(); }, 250);
    statsTimer = setInterval(updateStats, 1000);
    try {
      if (typeof RTCPeerConnection !== 'function') throw new Error('此浏览器环境未提供 WebRTC，请使用支持 WebRTC 的浏览器');
      const url = new URL('/api/display/socket', location.href);
      url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      url.searchParams.set('role', host ? 'host' : 'receiver');
      const next = new WebSocket(url); socket = next;
      next.onmessage = event => {
        signalChain = signalChain.then(() => { if (socket === next) return receive(JSON.parse(event.data)); }).catch(error => { if (socket === next) status(error.message); });
      };
      next.onclose = () => { if (socket === next) interrupted('连接已关闭'); };
      next.onerror = () => { if (socket === next) status('连接失败，请检查地址、配对令牌和主机网关'); };
    } catch (error) { stop(error.message); }
  }
  connect();
}
