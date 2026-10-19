import assert from "node:assert/strict";
import { createWebSocketUiStateConnector } from "./ui-state-client.mjs";

assert.equal(typeof global.gc, "function", "run with node --expose-gc");
const sockets = [];
const queuedMessages = [];
const owners = [];
let delivered = 0;

class FakeWebSocket {
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  constructor() {
    this.readyState = 0;
    this.listeners = new Map();
    sockets.push(this);
  }

  addEventListener(type, listener) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type).add(listener);
  }

  removeEventListener(type, listener) { this.listeners.get(type)?.delete(listener); }
  send() {}
  close() { this.readyState = 2; }
  open() {
    this.readyState = 1;
    for (const listener of this.listeners.get("open")) listener({});
  }
}

const connect = createWebSocketUiStateConnector({
  WebSocket: FakeWebSocket,
  url: "ws://synthetic.invalid/ui-state",
});

async function createOwner(retired) {
  const owner = { payload: new Uint8Array(1024), delivered: 0 };
  owners.push({ reference: new WeakRef(owner), retired });
  const pending = connect({
    cursor: null,
    onEnvelope() { owner.delivered += 1; delivered += 1; },
    onDisconnect() { owner.delivered += 1; },
  });
  const socket = sockets.at(-1);
  socket.open();
  const connection = await pending;
  if (retired) {
    queuedMessages.push(...socket.listeners.get("message"));
    connection.close();
  }
  return retired ? undefined : connection;
}

for (let i = 0; i < 40; i += 1) await createOwner(true);
const active = await createOwner(false);
for (let i = 0; i < 8; i += 1) {
  await new Promise((resolve) => setImmediate(resolve));
  global.gc();
}

const retiredRetained = owners.filter(({ reference, retired }) => retired && reference.deref()).length;
const activeRetained = owners.filter(({ reference, retired }) => !retired && reference.deref()).length;
for (const listener of queuedMessages) listener({ data: "{}" });
const listenerCount = (socket) => [...socket.listeners.values()]
  .reduce((sum, callbacks) => sum + callbacks.size, 0);
const metrics = {
  retiredConnections: 40,
  deliberatelyHeldSocketObjects: sockets.length,
  deliberatelyHeldQueuedMessageCallbacks: queuedMessages.length,
  retiredListeners: sockets.slice(0, 40).reduce((total, socket) => total + listenerCount(socket), 0),
  retiredRetainedOwners: retiredRetained,
  activeControlRetainedOwners: activeRetained,
  activeControlListeners: listenerCount(sockets.at(-1)),
  lateDeliveries: delivered,
};
active.close();
console.log(JSON.stringify(metrics));
assert.equal(metrics.retiredListeners, 0);
assert.equal(metrics.retiredRetainedOwners, 0);
assert.equal(metrics.lateDeliveries, 0);
assert.equal(metrics.activeControlRetainedOwners, 1);
assert.equal(metrics.activeControlListeners, 4);
