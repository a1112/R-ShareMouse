import test from 'node:test';
import assert from 'node:assert/strict';

import {
  buildDisplayModeControls,
  buildDisplayOperationStatus,
  buildDisplaySettingsRequest,
  buildDisplayView,
  buildScaleControl,
  formatDisplayRow,
  updateDisplayPreviewState,
} from './display.mjs';

const PRIMARY_DISPLAY = {
  display_id: 'DISPLAY1',
  device_name: '\\\\.\\DISPLAY1',
  friendly_name: 'Studio Display',
  width: 3840,
  height: 2160,
  refresh_rate_millihz: 60000,
  orientation: 'Landscape',
  scale_percent: 150,
  primary: true,
  active: true,
  modes: [
    {
      width: 3840,
      height: 2160,
      refresh_rate_millihz: 60000,
      orientation: 'Landscape',
    },
    {
      width: 2560,
      height: 1440,
      refresh_rate_millihz: 144000,
      orientation: 'Landscape',
    },
    {
      width: 2160,
      height: 3840,
      refresh_rate_millihz: 60000,
      orientation: 'Portrait',
    },
  ],
  write_capabilities: {
    resolution: true,
    refresh_rate: true,
    orientation: true,
    primary: false,
    position: false,
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
  assert.deepEqual(unsupported.rows[0].unsupportedControls, ['Primary', 'Position', 'Scale']);
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

test('updateDisplayPreviewState retains per-display object URLs and revokes replacements', () => {
  const blobs = [];
  const revoked = [];
  class FakeBlob {
    constructor(parts, options) {
      this.parts = parts;
      this.type = options.type;
      blobs.push(this);
    }
  }
  let nextUrl = 1;
  const adapters = {
    Blob: FakeBlob,
    createObjectURL: () => `blob:preview-${nextUrl++}`,
    revokeObjectURL: (url) => revoked.push(url),
  };

  let previews = updateDisplayPreviewState(
    new Map(),
    {
      status: 'Success',
      display_id: 'DISPLAY1',
      mime_type: 'image/png',
      width: 320,
      height: 180,
      bytes: [137, 80, 78, 71],
    },
    adapters,
  );
  previews = updateDisplayPreviewState(
    previews,
    {
      status: 'Success',
      display_id: 'DISPLAY2',
      mime_type: 'image/jpeg',
      width: 200,
      height: 100,
      bytes: [255, 216, 255],
    },
    adapters,
  );
  previews = updateDisplayPreviewState(
    previews,
    {
      status: 'Success',
      display_id: 'DISPLAY1',
      mime_type: 'image/png',
      width: 640,
      height: 360,
      bytes: [1, 2, 3],
    },
    adapters,
  );

  assert.equal(previews.get('DISPLAY1').url, 'blob:preview-3');
  assert.equal(previews.get('DISPLAY1').width, 640);
  assert.equal(previews.get('DISPLAY2').url, 'blob:preview-2');
  assert.deepEqual(revoked, ['blob:preview-1']);
  assert.equal(blobs[0].parts[0] instanceof Uint8Array, true);
  assert.deepEqual([...blobs[0].parts[0]], [137, 80, 78, 71]);
});

test('buildDisplayModeControls exposes supported mode controls and explicit update request', () => {
  const controls = buildDisplayModeControls(PRIMARY_DISPLAY);

  assert.equal(controls.disabled, false);
  assert.deepEqual(
    controls.resolutionOptions.map((option) => option.value),
    ['3840x2160', '2560x1440', '2160x3840'],
  );
  assert.deepEqual(
    controls.refreshRateOptions.map((option) => option.value),
    ['60000', '144000'],
  );
  assert.deepEqual(
    controls.orientationOptions.map((option) => option.value),
    ['Landscape', 'Portrait'],
  );

  const update = buildDisplaySettingsRequest(PRIMARY_DISPLAY, {
    resolution: '2560x1440',
    refreshRateMillihz: '144000',
    orientation: 'Landscape',
  });

  assert.equal(update.error, null);
  assert.deepEqual(update.request, {
    display_id: 'DISPLAY1',
    width: 2560,
    height: 1440,
    refresh_rate_millihz: 144000,
    orientation: 'Landscape',
  });
});

test('buildDisplayModeControls disables writes when offline or unsupported', () => {
  const offline = buildDisplayModeControls(PRIMARY_DISPLAY, { controlsAvailable: false });
  const unsupported = buildDisplayModeControls({
    ...PRIMARY_DISPLAY,
    write_capabilities: {
      ...PRIMARY_DISPLAY.write_capabilities,
      refresh_rate: false,
    },
  });

  assert.equal(offline.disabled, true);
  assert.equal(offline.disabledReason, 'Daemon offline');
  assert.equal(unsupported.disabled, true);
  assert.equal(unsupported.disabledReason, 'Mode writes unsupported');
});
