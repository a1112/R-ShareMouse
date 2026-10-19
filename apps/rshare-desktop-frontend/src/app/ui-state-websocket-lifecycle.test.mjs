import assert from "node:assert/strict";
import { getEventListeners } from "node:events";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { createWebSocketUiStateConnector } from "./ui-state-client.mjs";

function harness() {
  const sockets = [];
  class FakeWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;

    constructor() {
      this.readyState = FakeWebSocket.CONNECTING;
      this.listeners = new Map();
      this.sent = [];
      this.closeCalls = 0;
      this.sendError = null;
      sockets.push(this);
    }

    addEventListener(type, listener) {
      if (!this.listeners.has(type)) this.listeners.set(type, new Set());
      this.listeners.get(type).add(listener);
    }

    removeEventListener(type, listener) {
      this.listeners.get(type)?.delete(listener);
    }

    emit(type, event = {}) {
      for (const listener of [...(this.listeners.get(type) ?? [])]) listener(event);
    }

    open() {
      this.readyState = FakeWebSocket.OPEN;
      this.emit("open");
    }

    send(data) {
      if (this.sendError) throw this.sendError;
      this.sent.push(JSON.parse(data));
    }

    close(code) {
      this.closeCalls += 1;
      this.readyState = FakeWebSocket.CLOSING;
      // The platform may complete closing later; no real transport is used.
      this.closeCode = code;
    }

    listenerCount() {
      return [...this.listeners.values()].reduce((count, items) => count + items.size, 0);
    }
  }
  return {
    sockets,
    connect: createWebSocketUiStateConnector({
      WebSocket: FakeWebSocket,
      url: "ws://synthetic.invalid/ui-state",
    }),
  };
}

const options = (extra = {}) => ({
  cursor: null,
  onEnvelope() {},
  onDisconnect() {},
  ...extra,
});

test("live WebSocket preserves subscribe, envelopes and full resync", async () => {
  const { connect, sockets } = harness();
  const delivered = [];
  const cursor = { boot_id: "synthetic", revision: 7 };
  const pending = connect(options({ cursor, onEnvelope: (value) => delivered.push(value) }));
  const socket = sockets[0];
  socket.open();
  const connection = await pending;
  socket.emit("message", { data: JSON.stringify({ type: "heartbeat" }) });
  connection.requestFullResync();
  assert.deepEqual(socket.sent, [{ type: "subscribe", cursor }, { type: "resync" }]);
  assert.deepEqual(delivered, [{ type: "heartbeat" }]);
  assert.equal(socket.listenerCount(), 4);
  assert.equal(socket.closeCalls, 0);
  connection.close();
});

test("close removes listeners, fences queued messages and remains idempotent", async () => {
  const { connect, sockets } = harness();
  const delivered = [];
  const disconnected = [];
  const pending = connect(options({
    onEnvelope: (value) => delivered.push(value),
    onDisconnect: (error) => disconnected.push(error),
  }));
  const socket = sockets[0];
  socket.open();
  const connection = await pending;
  const queuedMessages = [...socket.listeners.get("message")];
  connection.close();
  connection.close();
  connection.requestFullResync();
  for (let i = 0; i < 100; i += 1) queuedMessages[0]({ data: "{}" });
  socket.emit("error");
  socket.emit("close", { code: 1006 });
  assert.deepEqual(delivered, []);
  assert.deepEqual(disconnected, []);
  assert.equal(socket.listenerCount(), 0);
  assert.equal(socket.closeCalls, 1);
  assert.equal(socket.sent.length, 1);
});

test("abort while connecting rejects and detaches all socket listeners", async () => {
  const { connect, sockets } = harness();
  const controller = new AbortController();
  const pending = connect(options({ signal: controller.signal }));
  controller.abort();
  await assert.rejects(pending, { name: "AbortError" });
  assert.equal(sockets[0].listenerCount(), 0);
  assert.equal(sockets[0].closeCalls, 1);
});

test("an already aborted attempt rejects without creating a WebSocket", async () => {
  const { connect, sockets } = harness();
  const controller = new AbortController();
  controller.abort();
  const pending = connect(options({ signal: controller.signal }));
  // Old adapters only observe the abort after open; keep the baseline finite.
  sockets[0]?.open();
  await assert.rejects(pending, { name: "AbortError" });
  assert.equal(sockets.length, 0);
});

test("error before open rejects, closes the socket and releases listeners", async () => {
  const { connect, sockets } = harness();
  let disconnected = 0;
  const pending = connect(options({ onDisconnect: () => { disconnected += 1; } }));
  sockets[0].emit("error");
  await assert.rejects(pending, /WebSocket is unavailable/);
  assert.equal(sockets[0].closeCalls, 1);
  assert.equal(sockets[0].listenerCount(), 0);
  assert.equal(disconnected, 0);
});

test("remote close before open rejects and releases listeners", async () => {
  const { connect, sockets } = harness();
  const pending = connect(options());
  sockets[0].readyState = 3;
  sockets[0].emit("close", { code: 1006, reason: "synthetic close" });
  await assert.rejects(pending, /synthetic close/);
  assert.equal(sockets[0].listenerCount(), 0);
  assert.equal(sockets[0].closeCalls, 0);
});

for (const failure of ["error", "close", "malformed", "non-text"]) {
  test(`${failure} after open reports once and releases the transport`, async () => {
    const { connect, sockets } = harness();
    const delivered = [];
    const disconnected = [];
    const controller = new AbortController();
    const pending = connect(options({
      signal: controller.signal,
      onEnvelope: (value) => delivered.push(value),
      onDisconnect: (error) => disconnected.push(error),
    }));
    const socket = sockets[0];
    socket.open();
    const connection = await pending;
    const queuedMessage = [...socket.listeners.get("message")][0];
    if (failure === "close") socket.readyState = 3;
    if (failure === "malformed") socket.emit("message", { data: "{" });
    else if (failure === "non-text") socket.emit("message", { data: new Uint8Array(2) });
    else socket.emit(failure, { code: 1006, reason: "synthetic close" });
    queuedMessage({ data: "{}" });
    assert.equal(getEventListeners(controller.signal, "abort").length, 0);
    controller.abort();
    connection.close();
    assert.equal(disconnected.length, 1);
    assert.deepEqual(delivered, []);
    assert.equal(socket.listenerCount(), 0);
    assert.equal(socket.closeCalls, failure === "close" ? 0 : 1);
  });
}

test("subscribe send failure rejects the pending connection and cleans up", async () => {
  const { connect, sockets } = harness();
  const controller = new AbortController();
  const pending = connect(options({ signal: controller.signal }));
  const settled = pending.then((value) => ({ value }), (error) => ({ error }));
  sockets[0].sendError = new Error("synthetic send failure");
  let thrown;
  try {
    try { sockets[0].open(); } catch (error) { thrown = error; }
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(thrown, undefined);
    const result = await Promise.race([settled, Promise.resolve({ pending: true })]);
    assert.match(result.error?.message ?? "connection still pending", /synthetic send failure/);
    assert.equal(sockets[0].listenerCount(), 0);
    assert.equal(sockets[0].closeCalls, 1);
  } finally {
    // Cancel the synthetic transport even if the old promise remains pending.
    controller.abort();
  }
});

test("forty retired connections leave no registered socket callbacks", async () => {
  const { connect, sockets } = harness();
  for (let i = 0; i < 40; i += 1) {
    const pending = connect(options());
    sockets.at(-1).open();
    const connection = await pending;
    connection.close();
  }
  const retained = sockets.reduce((count, socket) => count + socket.listenerCount(), 0);
  assert.equal(retained, 0);
  assert.equal(sockets.filter((socket) => socket.closeCalls === 1).length, 40);
});

test("queued platform callbacks release retired consumers and preserve the live owner", () => {
  const result = spawnSync(process.execPath, [
    "--max-old-space-size=128",
    "--expose-gc",
    fileURLToPath(new URL("./ui-state-websocket-ownership.node.mjs", import.meta.url)),
  ], { encoding: "utf8", timeout: 10000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const metrics = JSON.parse(result.stdout);
  assert.equal(metrics.deliberatelyHeldSocketObjects, 41);
  assert.equal(metrics.deliberatelyHeldQueuedMessageCallbacks, 40);
  assert.equal(metrics.retiredRetainedOwners, 0);
  assert.equal(metrics.activeControlRetainedOwners, 1);
});
