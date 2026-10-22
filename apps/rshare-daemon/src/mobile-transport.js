// One ordered connection; at most four unacknowledged events. Callers can wait
// for `sent` for motion, but must wait for `result` before confirming a key state.
function createMobileInputTransport(token, post) {
  let socket = null;
  let opening = null;
  let retryAt = 0;
  let revision = 0;
  let heartbeat = 0;
  const pending = new Map();
  const waiting = [];
  const key = envelope => `${envelope.client_id || ''}:${envelope.sequence}`;
  function fail(error) {
    for (const item of pending.values()) { clearTimeout(item.timer); item.reject(error); }
    pending.clear();
    while (waiting.length) waiting.shift().reject(error);
  }
  function close() {
    revision++;
    clearTimeout(heartbeat);
    const previous = socket;
    socket = null;
    opening = null;
    fail(new Error('输入连接已中断，请重试'));
    previous?.close();
  }
  function keepAlive() {
    if (socket?.readyState !== 1) return;
    try { socket.send(JSON.stringify({ type: 'ping' })); } catch { close(); return; }
    heartbeat = setTimeout(keepAlive, 20000);
  }
  function drain() {
    while (socket?.readyState === 1 && pending.size < 4 && waiting.length) {
      const item = waiting.shift();
      if (socket.bufferedAmount > 65536) { item.reject(new Error('输入连接拥塞')); close(); return; }
      pending.set(key(item.envelope), item);
      item.timer = setTimeout(() => close(), 2000);
      try {
        socket.send(JSON.stringify(item.envelope));
        item.sent();
      } catch { close(); }
    }
  }
  function connect() {
    if (socket?.readyState === 1) return Promise.resolve(true);
    if (opening) return opening;
    if (Date.now() < retryAt || typeof WebSocket === 'undefined') return Promise.resolve(false);
    const url = new URL('/api/input/socket', location.href);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.searchParams.set('t', token);
    let ws;
    try { ws = new WebSocket(url); } catch { retryAt = Date.now() + 5000; return Promise.resolve(false); }
    socket = ws;
    opening = new Promise(resolve => {
      const timer = setTimeout(() => { ws.close(); resolve(false); }, 1500);
      const ended = () => {
        clearTimeout(timer);
        if (socket === ws) {
          clearTimeout(heartbeat);
          socket = null; opening = null; retryAt = Date.now() + 5000;
          fail(new Error('输入连接已中断，请重试'));
        }
        resolve(false);
      };
      ws.onopen = () => {
        if (socket !== ws) { ws.close(); resolve(false); return; }
        clearTimeout(timer); opening = null; resolve(true); drain();
        heartbeat = setTimeout(keepAlive, 20000);
      };
      ws.onclose = ended;
      ws.onerror = ended;
      ws.onmessage = event => {
        if (socket !== ws) return;
        let response;
        try { response = JSON.parse(event.data); } catch { close(); return; }
        const id = key(response);
        const item = pending.get(id);
        if (!item) return;
        pending.delete(id); clearTimeout(item.timer);
        if (response.error) item.reject(new Error(response.error));
        else item.resolve(response.result);
        drain();
      };
    });
    return opening;
  }
  function submit(envelope) {
    const generation = revision;
    let sent, rejectSent, resolve, rejectResult;
    const sentPromise = new Promise((yes, no) => { sent = yes; rejectSent = no; });
    const result = new Promise((yes, no) => { resolve = yes; rejectResult = no; });
    // A caller may await only the other promise before registering its handler.
    sentPromise.catch(() => {}); result.catch(() => {});
    const reject = error => { rejectSent(error); rejectResult(error); };
    void (async () => {
      const connected = await connect();
      if (generation !== revision) throw new Error('输入连接已中断');
      if (connected && socket?.readyState === 1) {
        waiting.push({ envelope, sent, resolve, reject }); drain();
      } else {
        // Old hosts still work. Never replay a frame whose WS delivery is unknown.
        const response = await post(envelope);
        sent(); resolve(response);
      }
    })().catch(reject);
    return { sent: sentPromise, result };
  }
  return { connect, submit, close };
}

function createPointerDeltaAccumulator() {
  let x = 0, y = 0;
  return {
    take(dx, dy) {
      x += dx; y += dy;
      const wholeX = Math.round(x), wholeY = Math.round(y);
      x -= wholeX; y -= wholeY;
      return { x: wholeX, y: wholeY };
    },
    reset() { x = 0; y = 0; }
  };
}
