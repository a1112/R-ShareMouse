import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../../../rshare-daemon/src/mobile-transport.js', import.meta.url), 'utf8');
function fixture() {
  const sockets = [];
  class Socket {
    readyState = 0; bufferedAmount = 0; messages = [];
    constructor() { sockets.push(this); }
    send(value) { this.messages.push(JSON.parse(value)); }
    close() { this.readyState = 3; this.onclose?.(); }
    open() { this.readyState = 1; this.onopen(); }
    ack(sequence) { this.onmessage({ data: JSON.stringify({ sequence, result: { accepted: true } }) }); }
  }
  const context = vm.createContext({ WebSocket: Socket, URL, setTimeout, clearTimeout, location: { href: 'http://192.168.1.2:27437/mobile' } });
  vm.runInContext(source, context);
  let posts = 0;
  const transport = context.createMobileInputTransport('test', async () => { posts++; return { accepted: true }; });
  transport.connect(); sockets[0].open();
  return { context, transport, socket: sockets[0], posts: () => posts };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test('movement pipelines without waiting for RTT but bounds outstanding input', async () => {
  const { transport, socket, posts } = fixture();
  const tickets = Array.from({ length: 5 }, (_, i) => transport.submit({ sequence: i + 1 }));
  let confirmed = false;
  tickets[0].result.then(() => { confirmed = true; });
  await Promise.all(tickets.slice(0, 4).map(t => t.sent));
  assert.equal(socket.messages.length, 4);
  assert.equal(confirmed, false, 'sending a frame must not claim injection success');
  socket.ack(1); await tickets[4].sent;
  assert.deepEqual(socket.messages.map(m => m.sequence), [1, 2, 3, 4, 5]);
  for (let i = 2; i <= 5; i++) socket.ack(i);
  await Promise.all(tickets.map(t => t.result));
  assert.equal(posts(), 0);
  transport.close();
});

test('old hosts fall back to HTTP and wait for its real response', async () => {
  const context = vm.createContext({ URL, setTimeout, clearTimeout, location: { href: 'http://192.168.1.2/mobile' } });
  vm.runInContext(source, context);
  let complete;
  const transport = context.createMobileInputTransport('test', () => new Promise(resolve => { complete = resolve; }));
  const ticket = transport.submit({ sequence: 1 });
  let sent = false; ticket.sent.then(() => { sent = true; });
  await tick(); assert.equal(sent, false);
  complete({ accepted: true });
  assert.equal((await ticket.result).accepted, true);
  await ticket.sent; assert.equal(sent, true);
  transport.close();
});

test('connection loss rejects ambiguous input without replaying a click over HTTP', async () => {
  const { transport, socket, posts } = fixture();
  const ticket = transport.submit({ sequence: 1, request: 'click' });
  const rejected = assert.rejects(ticket.result, /连接/);
  await ticket.sent; socket.close(); await rejected;
  assert.equal(posts(), 0);
  transport.close();
});

test('suspending cancels unsent input and bounded waiters', async () => {
  const { transport, socket, posts } = fixture();
  const tickets = Array.from({ length: 5 }, (_, i) => transport.submit({ sequence: i + 1 }));
  const settled = Promise.allSettled(tickets.flatMap(t => [t.sent, t.result]));
  await tick(); transport.close(); await settled;
  assert.equal(socket.messages.length, 4);
  assert.equal(posts(), 0);
});

test('subpixel touch movement accumulates symmetrically instead of disappearing', () => {
  const { context, transport } = fixture();
  const delta = context.createPointerDeltaAccumulator();
  let x = 0; let y = 0;
  for (let i = 0; i < 100; i++) { const d = delta.take(0.2, -0.2); x += d.x; y += d.y; }
  assert.equal(x, 20); assert.equal(y, -20);
  delta.reset(); assert.equal(delta.take(0.2, 0).x, 0);
  transport.close();
});
