import test from 'node:test';
import assert from 'node:assert/strict';

import {
  buildDisplayOperationStatus,
  buildDisplayView,
  buildScaleControl,
  formatDisplayRow,
} from './display.mjs';

const PRIMARY_DISPLAY = {
  display_id: 'DISPLAY1',
  device_name: '\\\\.\\DISPLAY1',
  friendly_name: 'Studio Display',
  width: 3840,
  height: 2160,
  refresh_rate_millihz: 60000,
  scale_percent: 150,
  primary: true,
  active: true,
  write_capabilities: {
    resolution: true,
    refresh_rate: true,
    orientation: true,
    primary: true,
    position: true,
    scale: false,
  },
};

test('formatDisplayRow formats monitor identity, mode, scale, and primary marker', () => {
  const row = formatDisplayRow(PRIMARY_DISPLAY, 0);

  assert.equal(row.title, 'Studio Display');
  assert.equal(row.friendlyName, '\\\\.\\DISPLAY1');
  assert.equal(row.resolution, '3840 x 2160');
  assert.equal(row.refreshRate, '60 Hz');
  assert.equal(row.scale, '150%');
  assert.deepEqual(row.markers, ['Primary', 'Active']);
});

test('buildScaleControl routes unsupported scale writes to system settings', () => {
  const control = buildScaleControl(PRIMARY_DISPLAY);

  assert.equal(control.label, 'Scale');
  assert.equal(control.value, '150%');
  assert.equal(control.disabled, true);
  assert.equal(control.action, 'open-system-settings');
  assert.equal(control.reason, 'Use system display settings');
});

test('buildScaleControl stays read-only even when scale write capability appears available', () => {
  const control = buildScaleControl({
    ...PRIMARY_DISPLAY,
    write_capabilities: {
      ...PRIMARY_DISPLAY.write_capabilities,
      scale: true,
    },
  });

  assert.equal(control.disabled, true);
  assert.equal(control.action, 'open-system-settings');
  assert.equal(control.reason, 'Use system display settings');
});

test('buildDisplayView exposes unsupported capability labels and empty display state', () => {
  const view = buildDisplayView({
    display_count: 0,
    displays: [],
  });
  const unsupported = buildDisplayView({
    display_count: 1,
    displays: [PRIMARY_DISPLAY],
  });

  assert.equal(view.empty, true);
  assert.equal(view.emptyMessage, 'No local displays reported yet.');
  assert.deepEqual(unsupported.rows[0].unsupportedControls, ['Scale']);
});

test('buildDisplayView marks daemon-backed display controls disabled while offline', () => {
  const view = buildDisplayView(
    {
      display_count: 1,
      displays: [PRIMARY_DISPLAY],
    },
    { controlsAvailable: false },
  );

  assert.equal(view.controlsAvailable, false);
  assert.equal(view.rows[0].controlsDisabled, true);
  assert.equal(view.rows[0].controlsDisabledReason, 'Daemon offline');
});

test('buildDisplayOperationStatus only treats Success as ok', () => {
  assert.deepEqual(buildDisplayOperationStatus({ status: 'Success' }, 'Capture'), {
    text: 'Capture: Success',
    tone: 'ok',
  });
  assert.deepEqual(buildDisplayOperationStatus({ status: 'Unsupported' }, 'Capture'), {
    text: 'Capture: Unsupported',
    tone: 'warning',
  });
  assert.deepEqual(
    buildDisplayOperationStatus(
      { status: 'ApplyFailed', message: 'display capture is not implemented' },
      'Capture',
    ),
    {
      text: 'display capture is not implemented',
      tone: 'error',
    },
  );
});
