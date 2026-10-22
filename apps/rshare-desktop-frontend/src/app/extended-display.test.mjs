import test from 'node:test';
import assert from 'node:assert/strict';
import { imagePoint, ContactTracker, preferH264, selectedPath, ScreenWakeLock, isVideoPlaying } from '../../../rshare-daemon/src/extended-display/client.mjs';

test('mobile playback waits for decoded video, not just an unpaused MediaStream', () => {
  const waiting = { srcObject: {}, paused: false, ended: false, readyState: 0, videoWidth: 0, videoHeight: 0 };
  assert.equal(isVideoPlaying(waiting), false);
  assert.equal(isVideoPlaying({ ...waiting, readyState: 1 }), false);
  assert.equal(isVideoPlaying({ ...waiting, readyState: 2 }), false);
  const playing = { ...waiting, readyState: 2, videoWidth: 1280, videoHeight: 720 };
  assert.equal(isVideoPlaying(playing), true);
  for (const stopped of [{ srcObject: null }, { paused: true }, { ended: true }, { readyState: 1 }]) {
    assert.equal(isVideoPlaying({ ...playing, ...stopped }), false);
  }
});

test('extended display maps aspect-fit video and excludes black bars', () => {
  const rect = { left: 10, top: 20, width: 1000, height: 1000 };
  assert.equal(imagePoint(510, 40, rect, 1920, 1080), null);
  assert.deepEqual(imagePoint(510, 520, rect, 1920, 1080), { x: 0.5, y: 0.5 });
  assert.deepEqual(imagePoint(-20, 40, rect, 1920, 1080, true), { x: 0, y: 0 });
  assert.equal(imagePoint(1, 1, rect, 0, 0), null);
});
test('extended display uses bounded contact ids and whole snapshots', () => {
  const tracker = new ContactTracker();
  for (let i = 0; i < 10; i++) assert.equal(tracker.down(100 + i, { x: .2, y: .4 }, .5), true);
  assert.equal(tracker.down(999, { x: 0, y: 0 }, .5), false);
  assert.equal(tracker.snapshot().length, 10);
  tracker.up(103);
  assert.equal(tracker.snapshot().length, 9);
  assert.equal(tracker.down(200, { x: 1, y: 1 }, 1), true);
  assert.equal(new Set(tracker.snapshot().map(p => p.id)).size, 10);
  tracker.clear();
  assert.deepEqual(tracker.snapshot(), []);
});
test('extended display prefers H264 while retaining negotiated fallback', () => {
  const codecs = [{ mimeType: 'video/VP8' }, { mimeType: 'video/H264' }, { mimeType: 'video/rtx' }];
  assert.equal(preferH264(codecs)[0].mimeType, 'video/H264');
  assert.equal(preferH264(codecs).length, 3);
  assert.equal(codecs[0].mimeType, 'video/VP8');
});
test('extended display reports actual selected ICE pair without inferring USB', () => {
  const report = new Map([
    ['t', { type: 'transport', selectedCandidatePairId: 'pair' }],
    ['pair', { type: 'candidate-pair', localCandidateId: 'local', remoteCandidateId: 'remote', currentRoundTripTime: .012 }],
    ['local', { address: '192.168.42.1', protocol: 'udp', candidateType: 'host' }],
    ['remote', { address: '192.168.42.2', protocol: 'udp', candidateType: 'host' }],
  ]);
  assert.deepEqual(selectedPath(report), { local: '192.168.42.1', remote: '192.168.42.2', protocol: 'udp', rttMs: 12 });
  assert.equal(selectedPath(new Map()), null);
});

test('extended display never reports missing network measurements as zero', () => {
  const report = new Map([
    ['t', { type: 'transport', selectedCandidatePairId: 'pair' }],
    ['pair', { type: 'candidate-pair', localCandidateId: 'local' }],
    ['local', { address: '', protocol: 'udp' }],
  ]);
  assert.deepEqual(selectedPath(report), { local: '隐藏', remote: '隐藏', protocol: 'udp', rttMs: null });
});

test('mobile display releases a wake lock acquired after suspension', async () => {
  let resolve, released = 0, requests = 0;
  const lock = new ScreenWakeLock({ request: () => { requests++; return new Promise(r => { resolve = r; }); } });
  const pending = lock.setActive(true);
  await lock.setActive(true);
  await lock.setActive(false);
  resolve({ release: async () => { released++; } });
  await pending;
  assert.equal(requests, 1);
  assert.equal(released, 1);
  assert.equal(lock.lock, null);
});

test('mobile display tolerates unavailable wake locks and releases on disconnect', async () => {
  await new ScreenWakeLock(undefined).setActive(true);
  await new ScreenWakeLock({ request: async () => { throw new Error('HTTP not secure'); } }).setActive(true);
  let released = 0;
  const lock = new ScreenWakeLock({ request: async () => ({ addEventListener() {}, release: async () => { released++; } }) });
  await lock.setActive(true);
  await lock.setActive(false);
  assert.equal(released, 1);
});
