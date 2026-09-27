import test from 'node:test';
import assert from 'node:assert/strict';
import { imagePoint, ContactTracker, preferH264, selectedPath } from '../../../rshare-daemon/src/extended-display/client.mjs';

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
