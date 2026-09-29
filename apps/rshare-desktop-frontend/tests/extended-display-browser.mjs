// Real Chromium WebRTC with a generated source; OS display/signaling are simulated.
import { chromium } from '@playwright/test';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';
const assets = new URL('../../rshare-daemon/src/extended-display/', import.meta.url);
const server = createServer(async (req, res) => {
  const js = req.url.startsWith('/display/client.mjs');
  res.setHeader('Content-Type', js ? 'text/javascript' : 'text/html');
  res.end(await readFile(new URL(js ? 'client.mjs' : 'page.html', assets)));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
let browser, host, receiver;
try {
  browser = await chromium.launch({ headless: true, ...(process.env.RSHARE_BROWSER_CHANNEL ? { channel: process.env.RSHARE_BROWSER_CHANNEL } : {}) });
  console.log('Chromium launched');
  const context = await browser.newContext({ viewport: { width: 1024, height: 900 } });
  const peers = new Map(), touches = [], errors = [];
  await context.routeWebSocket('**/api/display/socket**', ws => {
    const role = new URL(ws.url()).searchParams.get('role'); peers.set(role, ws);
    ws.send(JSON.stringify({ type: 'hello' }));
    if (peers.has('host') && peers.has('receiver')) peers.get('host').send(JSON.stringify({ type: 'receiver_ready' }));
    ws.onMessage(raw => {
      const message = JSON.parse(raw), other = peers.get(role === 'host' ? 'receiver' : 'host');
      if (message.type !== 'ping') console.log('signal', role, message.type);
      if (message.type === 'create') ws.send(JSON.stringify({ type: 'display', display: { display_id: 'test-screen', width: 1920, height: 1080 } }));
      else if (message.type === 'enable_touch') {
        for (const peer of peers.values()) peer.send(JSON.stringify({ type: 'touch_enabled', enabled: message.enabled }));
      } else if (message.type === 'touch') touches.push(message);
      else if (['offer', 'answer', 'ice'].includes(message.type)) other?.send(raw);
    });
    ws.onClose(() => {
      peers.delete(role);
      peers.get(role === 'host' ? 'receiver' : 'host')?.send(JSON.stringify({ type: role === 'host' ? 'host_left' : 'receiver_left' }));
    });
  });
  host = await context.newPage(); receiver = await context.newPage();
  for (const page of [host, receiver]) page.on('pageerror', e => errors.push(e.message));
  await host.addInitScript(() => {
    navigator.mediaDevices.getDisplayMedia = async () => {
      const canvas = document.createElement('canvas'); canvas.width = 1920; canvas.height = 1080;
      const ctx = canvas.getContext('2d'); let frame = 0;
      setInterval(() => {
        ctx.fillStyle = '#163b2a'; ctx.fillRect(0, 0, 1920, 1080);
        ctx.fillStyle = '#bcf4d2'; ctx.font = '64px sans-serif'; ctx.fillText(`R-ShareMouse WebRTC ${frame++}`, 120, 300);
      }, 33);
      return canvas.captureStream(30);
    };
  });
  await host.goto(`${base}/display?role=host&t=test`);
  await receiver.goto(`${base}/display?t=test`);
  console.log('Sender and receiver pages loaded');
  await host.getByRole('button', { name: '1. 创建 1080p 扩展屏' }).click();
  await host.getByRole('button', { name: '2. 选择扩展屏并分享' }).click();
  // WebRTC may reduce dimensions under load; verify actual playback, not a target.
  await receiver.waitForFunction(() => document.querySelector('video').videoWidth > 0 && document.querySelector('video').currentTime > 0, null, { timeout: 15_000 });
  console.log('Receiver decoded video');
  await receiver.locator('#play').click();
  assert.equal(await receiver.locator('body').evaluate(node => node.classList.contains('immersive')), true, 'immersive mode keeps touch in the page');
  await receiver.locator('#leave-fullscreen').click();
  await host.locator('#touch').check();
  await receiver.waitForFunction(() => document.getElementById('touch-status').textContent.includes('已启用'));
  // Real browser pointer capture; mouse pointing on the video also maps to touch.
  await receiver.mouse.move(500, 300); await receiver.mouse.down(); await receiver.mouse.move(520, 310); await receiver.mouse.up();
  await receiver.waitForTimeout(200);
  assert(touches.some(m => m.contacts.length > 0), 'a DOWN snapshot must reach the host');
  assert(touches.some(m => m.contacts.length === 0), 'an UP snapshot must release all contacts');
  assert(touches.every((m, i) => i === 0 || m.sequence > touches[i - 1].sequence));
  await mkdir(new URL('../../../target/extended-display-browser/', import.meta.url), { recursive: true });
  await receiver.screenshot({ path: new URL('../../../target/extended-display-browser/receiver.png', import.meta.url).pathname.replace(/^\/(\w:)/, '$1'), fullPage: true });
  await host.evaluate(() => { navigator.mediaDevices.getDisplayMedia = async () => { throw new DOMException('Test: picker cancelled', 'NotAllowedError'); }; });
  await host.getByRole('button', { name: '2. 选择扩展屏并分享' }).click();
  await receiver.waitForFunction(() => document.getElementById('touch-status').textContent === '触摸未启用', null, { timeout: 3000 });
  assert.equal(await host.locator('#touch').isChecked(), false, 'recapture requires new touch confirmation');
  assert.equal(await host.locator('#touch').isDisabled(), true, 'cancelled capture cannot enable touch');
  await receiver.locator('#play').click();
  await host.getByRole('button', { name: '断开', exact: true }).click();
  await receiver.waitForFunction(() => document.getElementById('status').textContent.includes('主机已断开'));
  assert.equal(await receiver.locator('body').evaluate(node => node.classList.contains('immersive')), false, 'disconnect exposes connection status');
  assert.deepEqual(errors, []);
  console.log('PASS: real browser video decode, touch snapshots and peer disconnect (simulated OS capture/signaling).');
} catch (error) {
  for (const page of [host, receiver]) if (page) console.error(await page.locator('body').innerText());
  console.error(error);
  throw error;
} finally {
  await browser?.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
}
