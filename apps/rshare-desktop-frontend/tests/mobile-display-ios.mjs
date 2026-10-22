// Real Chrome encoder -> iOS WKWebView decoder. Only the OS screen and daemon
// signaling are simulated. Never substitute this for Windows driver acceptance.
import { chromium } from '@playwright/test';
import { wsServer as WebSocketServer, PNG } from 'playwright-core/lib/utilsBundle';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const assets = new URL('../../rshare-daemon/src/extended-display/', import.meta.url);
const server = createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost');
  const script = url.pathname === '/display/client.mjs';
  if (!script && (url.pathname !== '/display' || url.searchParams.get('t') !== 'test')) {
    res.writeHead(401); res.end(); return;
  }
  res.setHeader('Content-Type', script ? 'text/javascript' : 'text/html');
  res.end(await readFile(new URL(script ? 'client.mjs' : 'page.html', assets)));
});
const sockets = new WebSocketServer({ noServer: true });
const peers = new Map();
let receiverCount = 0;
const display = { type: 'display', display: { display_id: 'fixture', friendly_name: 'WebRTC 测试画面', width: 1280, height: 720 } };
const send = (peer, message) => { if (peer?.readyState === 1) peer.send(JSON.stringify(message)); };
server.on('upgrade', (request, socket, head) => {
  const url = new URL(request.url, 'http://localhost');
  if (url.pathname !== '/api/display/socket' || url.searchParams.get('t') !== 'test') { socket.destroy(); return; }
  const role = url.searchParams.get('role');
  if (!['host', 'receiver'].includes(role) || peers.has(role)) { socket.destroy(); return; }
  sockets.handleUpgrade(request, socket, head, peer => {
    peers.set(role, peer);
    if (role === 'receiver') receiverCount++;
    send(peer, { type: 'hello' });
    if (role === 'receiver') send(peer, display);
    if (peers.has('host') && peers.has('receiver')) send(peers.get('host'), { type: 'receiver_ready' });
    peer.on('message', data => {
      const message = JSON.parse(data.toString());
      if (message.type === 'create') send(peer, display);
      else if (message.type === 'enable_touch') for (const target of peers.values()) send(target, { type: 'touch_enabled', enabled: message.enabled });
      else if (['offer', 'answer', 'ice'].includes(message.type)) send(peers.get(role === 'host' ? 'receiver' : 'host'), message);
    });
    peer.on('close', () => {
      if (peers.get(role) !== peer) return;
      peers.delete(role);
      send(peers.get(role === 'host' ? 'receiver' : 'host'), { type: role === 'host' ? 'host_left' : 'receiver_left' });
    });
  });
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ headless: true, channel: process.env.RSHARE_BROWSER_CHANNEL || 'chrome' });
  const host = await browser.newPage();
  const errors = [];
  host.on('pageerror', error => errors.push(error.message));
  await host.addInitScript(() => {
    navigator.mediaDevices.getDisplayMedia = async () => {
      const canvas = document.createElement('canvas'); canvas.width = 1280; canvas.height = 720;
      const ctx = canvas.getContext('2d'); let frame = 0;
      setInterval(() => {
        ctx.fillStyle = '#123d2d'; ctx.fillRect(0, 0, 1280, 720);
        ctx.fillStyle = '#b4f3c8'; ctx.font = '52px sans-serif';
        ctx.fillText('R-ShareMouse mobile display', 80, 280);
        ctx.fillText(`WebRTC frame ${frame++}`, 80, 380);
      }, 33);
      return canvas.captureStream(30);
    };
  });
  await host.goto(`${base}/display?role=host&t=test`);
  await host.locator('#create').click(); await host.locator('#share').click();
  await mkdir(`${root}/target/mobile-display-validation`, { recursive: true });
  const destination = process.env.RSHARE_IOS_SIMULATOR;
  assert(destination, 'Set RSHARE_IOS_SIMULATOR to a booted iPhone/iPad simulator UDID');
  const resultPath = `${root}/target/mobile-display-validation/ios-${Date.now()}.xcresult`;
  let output = '';
  const child = spawn('xcodebuild', [
    '-project', 'apps/rshare-ios/RShareMouse.xcodeproj', '-scheme', 'RShareMouse',
    '-destination', `platform=iOS Simulator,id=${destination}`,
    '-derivedDataPath', 'target/mobile-display-ios', '-resultBundlePath', resultPath,
    '-only-testing:RShareMouseUITests/LiveClientUITests/testMobileDisplayPlaybackFullscreenRotationAndRecovery', 'test'
  ], { cwd: root, timeout: 180_000, env: { ...process.env, RSHARE_DISPLAY_TEST_URL: `${base}/display?t=test` } });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', data => { output += data; process.stdout.write(data); });
  const code = await new Promise((resolve, reject) => { child.on('error', reject); child.on('exit', resolve); });
  assert.equal(code, 0, 'iOS UI test must pass');
  assert(receiverCount >= 3, `Expected initial playback, reconnect and foreground recovery, got ${receiverCount} connections`);
  assert(!output.includes('Test skipped'), 'The stream test must execute');
  assert.deepEqual(errors, []);
  const attachments = resultPath.replace(/\.xcresult$/, '-attachments');
  execFileSync('xcrun', ['xcresulttool', 'export', 'attachments', '--path', resultPath, '--output-path', attachments]);
  const manifest = JSON.parse(await readFile(`${attachments}/manifest.json`, 'utf8'));
  const landscape = manifest.flatMap(test => test.attachments).find(item => item.suggestedHumanReadableName.startsWith('Mobile display landscape fullscreen'));
  assert(landscape, 'Landscape screenshot must be retained');
  const png = PNG.sync.read(await readFile(`${attachments}/${landscape.exportedFileName}`));
  let green = 0, pixels = 0;
  for (let i = 0; i < png.data.length; i += 16) {
    const [r, g, b] = png.data.subarray(i, i + 3); pixels++;
    if (g > r * 1.5 && g > b * 1.1 && g > 20) green++;
  }
  // XCUIScreen retains orientation metadata; raw PNG width/height may still be
  // portrait. XCTest checks the landscape frame, pixels check actual coverage.
  assert(green / pixels > .6,
    `Landscape must show a fitted video, not a cropped portrait surface (${Math.round(green / pixels * 100)}% test image)`);
  console.log(`PASS: iOS WebRTC playback, native fullscreen, rotation and recovery. Evidence: ${resultPath}`);
} finally {
  // Iterate the server's complete set, including sockets already superseded in
  // the role map; otherwise failed UI runs can leave cleanup waiting forever.
  for (const peer of sockets.clients) peer.terminate();
  sockets.close(); server.closeAllConnections(); server.close();
  await browser?.close();
}
