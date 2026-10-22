// Physical iOS diagnostic over an existing Apple development USB tunnel.
// Uses generated video and fixture signaling: NOT a system extended-display test.
// Install the current Debug app first. Never allows Wi-Fi/LAN ICE fallback.
import { chromium } from '@playwright/test';
import { wsServer as WebSocketServer } from 'playwright-core/lib/utilsBundle';
import { createServer } from 'node:http';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { isIP } from 'node:net';
import assert from 'node:assert/strict';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const assets = new URL('../../rshare-daemon/src/extended-display/', import.meta.url);
const run = promisify(execFile);
const device = process.env.RSHARE_IOS_DEVICE;
const usbHost = process.env.RSHARE_USB_HOST;
const usbPeer = process.env.RSHARE_USB_PEER;
assert(device && isIP(usbHost) === 6 && isIP(usbPeer) === 6,
  'Set RSHARE_IOS_DEVICE, RSHARE_USB_HOST and RSHARE_USB_PEER to the wired device and its development-tunnel IPv6 addresses');
const canonical = address => new URL(`http://[${address}]`).hostname;
const token = randomBytes(24).toString('hex');
const output = `${root}/target/usb-display-validation`;
await mkdir(output, { recursive: true });
const evidence = { startedAt: new Date().toISOString(), source: 'generated 1280x720 video; fixture signaling',
  usbHost, usbPeer, requests: [], signaling: [], candidates: [], reports: [], errors: [] };
const peers = new Map();
const sockets = new WebSocketServer({ noServer: true });
const send = (peer, message) => { if (peer?.readyState === 1) peer.send(JSON.stringify(message)); };
const display = { type: 'display', display: { display_id: 'fixture', friendly_name: 'USB 视频测试（非系统扩展屏）', width: 1280, height: 720 } };
function candidateAllowed(role, candidate) {
  const fields = candidate.replace(/^a=/, '').split(' ');
  const address = fields[4];
  const allowed = isIP(address) === 6 && canonical(address) === canonical(role === 'host' ? usbHost : usbPeer);
  evidence.candidates.push({ role, protocol: fields[2], address, port: fields[5], type: fields[7], allowed });
  return allowed;
}
function instrument() {
  const original = window.RTCPeerConnection;
  const connections = [];
  let presentation;
  window.addEventListener('rshare-display-state', event => { presentation = event.detail; });
  window.RTCPeerConnection = class extends original {
    constructor(...args) { super(...args); connections.push(this); }
  };
  window.usbProbe = async () => {
    const pc = connections.at(-1), video = document.querySelector('video');
    const report = { status: document.querySelector('#status')?.textContent, presentation,
      connectionState: pc?.connectionState, iceState: pc?.iceConnectionState,
      videoWidth: video?.videoWidth, videoHeight: video?.videoHeight, paused: video?.paused };
    if (pc) {
      const stats = await pc.getStats();
      for (const item of stats.values()) {
        if (item.type === 'inbound-rtp' && item.kind === 'video') {
          report.framesDecoded = item.framesDecoded; report.bytesReceived = item.bytesReceived;
        }
        if (item.type === 'transport' && item.selectedCandidatePairId) {
          const pair = stats.get(item.selectedCandidatePairId);
          report.selected = { local: stats.get(pair.localCandidateId), remote: stats.get(pair.remoteCandidateId), bytesReceived: pair.bytesReceived };
        }
      }
    }
    return report;
  };
}
const handler = async (req, res) => {
  const url = new URL(req.url, 'http://localhost');
  evidence.requests.push({ path: url.pathname, remote: req.socket.remoteAddress });
  if (url.pathname === '/probe' && req.method === 'POST' && url.searchParams.get('t') === token) {
    let body = '';
    for await (const chunk of req) { body += chunk; if (body.length > 65536) { req.destroy(); return; } }
    try { evidence.reports.push({ at: Date.now(), remote: req.socket.remoteAddress, ...JSON.parse(body) }); }
    catch { res.writeHead(400); res.end(); return; }
    res.end('ok'); return;
  }
  const script = url.pathname === '/display/client.mjs';
  if (!script && (url.pathname !== '/display' || url.searchParams.get('t') !== token)) {
    res.writeHead(401); res.end(); return;
  }
  res.setHeader('Content-Type', script ? 'text/javascript' : 'text/html');
  let body = await readFile(new URL(script ? 'client.mjs' : 'page.html', assets), 'utf8');
  if (!script) body = body.replace('<head>', `<head><script>(${instrument})();setInterval(async()=>{try{await fetch('/probe?t=${token}',{method:'POST',body:JSON.stringify(await window.usbProbe())})}catch{}},1000)</script>`);
  res.end(body);
};
const servers = [createServer(handler), createServer(handler)];
for (const server of servers) server.on('upgrade', (request, socket, head) => {
  const url = new URL(request.url, 'http://localhost');
  const role = url.searchParams.get('role');
  if (url.pathname !== '/api/display/socket' || url.searchParams.get('t') !== token ||
      !['host', 'receiver'].includes(role) || peers.has(role)) { socket.destroy(); return; }
  sockets.handleUpgrade(request, socket, head, peer => {
    peers.set(role, peer);
    send(peer, { type: 'hello' });
    if (role === 'receiver') send(peer, display);
    if (peers.has('host') && peers.has('receiver')) send(peers.get('host'), { type: 'receiver_ready' });
    peer.on('message', data => {
      const message = JSON.parse(data.toString());
      evidence.signaling.push({ role, type: message.type });
      if (message.type === 'create') send(peer, display);
      if (message.type === 'ice' && !candidateAllowed(role, message.candidate.candidate)) return;
      if (['offer', 'answer'].includes(message.type)) message.sdp = message.sdp.split('\r\n')
        .filter(line => !line.startsWith('a=candidate:') || candidateAllowed(role, line)).join('\r\n');
      if (['offer', 'answer', 'ice'].includes(message.type)) send(peers.get(role === 'host' ? 'receiver' : 'host'), message);
    });
    peer.on('close', () => { if (peers.get(role) === peer) peers.delete(role); });
  });
});
let browser;
try {
  for (const [i, server] of servers.entries()) await new Promise((resolve, reject) => {
    server.once('error', reject); server.listen(0, i ? usbHost : '127.0.0.1', resolve);
  });
  browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
  const host = await browser.newPage();
  host.on('pageerror', error => evidence.errors.push(error.message));
  await host.addInitScript(() => {
    navigator.mediaDevices.getDisplayMedia = async () => {
      const canvas = document.createElement('canvas'); canvas.width = 1280; canvas.height = 720;
      const context = canvas.getContext('2d'); let frame = 0;
      setInterval(() => {
        context.fillStyle = '#123d2d'; context.fillRect(0, 0, 1280, 720);
        context.fillStyle = '#b4f3c8'; context.font = '48px sans-serif';
        context.fillText('USB transport probe - generated video', 50, 280);
        context.fillText(`Frame ${frame++}`, 50, 380);
      }, 33);
      return canvas.captureStream(30);
    };
  });
  await host.goto(`http://127.0.0.1:${servers[0].address().port}/display?role=host&t=${token}`);
  await host.locator('#create').click(); await host.locator('#share').click();
  const url = `http://[${usbHost}]:${servers[1].address().port}/display?t=${token}`;
  await run('xcrun', ['devicectl', 'device', 'process', 'launch', '--device', device, '--terminate-existing',
    '--environment-variables', JSON.stringify({ RSHARE_PAIRING_URL: url }), 'com.rsharemouse.ios', '--ui-testing'], { timeout: 20000 });
  // Let gathering/negotiation and native playback finish; this is a finite probe.
  await new Promise(resolve => setTimeout(resolve, 25000));
  evidence.host = await host.evaluate(() => window.usbProbe());
  await run('xcrun', ['devicectl', 'device', 'capture', 'screenshot', '--device', device,
    '--destination', `${output}/phone-usb-probe.png`], { timeout: 15000 });
  const phoneReports = evidence.reports.filter(report => isIP(report.remote) === 6 && canonical(report.remote) === canonical(usbPeer));
  evidence.usbHttpPassed = evidence.requests.some(request => isIP(request.remote) === 6 && canonical(request.remote) === canonical(usbPeer));
  evidence.usbVideoPassed = phoneReports.some(report => report.framesDecoded > 0 &&
    report.connectionState === 'connected' && report.selected &&
    isIP(report.selected.remote?.address) === 6 && canonical(report.selected.remote.address) === canonical(usbHost));
  evidence.systemExtendedDisplayPassed = false;
  assert(phoneReports.every(report => report.videoWidth > 0 || report.presentation?.playing !== true),
    'The native playback badge must stay off until the first decoded video frame');
  console.log(JSON.stringify({ usbHttpPassed: evidence.usbHttpPassed, usbVideoPassed: evidence.usbVideoPassed,
    candidates: evidence.candidates, host: evidence.host, phone: phoneReports.at(-1), output }, null, 2));
} finally {
  await writeFile(`${output}/usb-probe.json`, JSON.stringify(evidence, null, 2));
  for (const peer of sockets.clients) peer.terminate(); sockets.close();
  for (const server of servers) { server.closeAllConnections(); server.close(); }
  await browser?.close();
}
