import test from 'node:test';
import assert from 'node:assert/strict';

import { buildScreenLayout, buildStatusBanner } from './layout.mjs';

test('buildScreenLayout keeps the local screen first and places discovered devices around it', () => {
  const layout = buildScreenLayout(
    [
      { id: 'b', name: 'MacBook', connected: false, hostname: 'macbook.local', last_seen_secs: 6 },
      { id: 'a', name: 'Studio PC', connected: true, hostname: 'studio.local', last_seen_secs: 1 },
    ],
    {
      device_id: 'local',
      device_name: 'This PC',
      bind_address: '0.0.0.0:27431',
      discovery_port: 27432,
    },
  );

  assert.equal(layout.length, 3);
  assert.equal(layout[0].id, 'local');
  assert.equal(layout[0].kind, 'local');
  assert.equal(layout[1].id, 'a');
  assert.equal(layout[1].x > layout[0].x, true);
  assert.equal(layout[2].id, 'b');
});

test('buildScreenLayout still returns a local screen when the daemon is offline', () => {
  const layout = buildScreenLayout([], null);

  assert.equal(layout.length, 1);
  assert.equal(layout[0].label, 'This PC');
  assert.equal(layout[0].status, 'Offline');
});

test('buildStatusBanner prefers daemon details when available', () => {
  const banner = buildStatusBanner(
    { device_name: 'Desktop', bind_address: '0.0.0.0:27431', discovery_port: 27432 },
    [{ connected: true }, { connected: false }],
  );

  assert.deepEqual(banner, {
    title: 'Desktop - 1 connected / 2 discovered',
    detail: 'Listening on 0.0.0.0:27431 - discovery UDP 27432',
    actionLabel: 'Stop Service',
  });
});

test('buildStatusBanner falls back to offline copy when the daemon is unavailable', () => {
  const banner = buildStatusBanner(null, []);

  assert.deepEqual(banner, {
    title: 'Daemon offline',
    detail: 'Start the service to discover devices and simulate their screen positions.',
    actionLabel: 'Start Service',
  });
});

test('builders tolerate null device lists from older daemon responses', () => {
  const layout = buildScreenLayout(null, null);
  const banner = buildStatusBanner(
    { device_name: 'Desktop', bind_address: '0.0.0.0:27431', discovery_port: 27432 },
    null,
  );

  assert.equal(layout.length, 1);
  assert.equal(banner.title, 'Desktop - 0 connected / 0 discovered');
});
