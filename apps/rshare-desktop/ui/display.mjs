const CONTROL_LABELS = {
  resolution: 'Resolution',
  refresh_rate: 'Refresh rate',
  orientation: 'Orientation',
  primary: 'Primary',
  position: 'Position',
  scale: 'Scale',
};

function hasSize(display) {
  return Number(display?.width) > 0 && Number(display?.height) > 0;
}

function formatRefreshRate(millihz) {
  if (millihz == null || Number(millihz) <= 0) {
    return 'Unknown';
  }

  const hz = Number(millihz) / 1000;
  return `${Number.isInteger(hz) ? hz : hz.toFixed(1)} Hz`;
}

function formatScale(percent) {
  return percent == null || Number(percent) <= 0 ? 'System' : `${Number(percent)}%`;
}

function unsupportedControls(display) {
  const capabilities = display?.write_capabilities ?? {};
  return Object.entries(CONTROL_LABELS)
    .filter(([key]) => capabilities[key] === false)
    .map(([, label]) => label);
}

export function buildScaleControl(display) {
  return {
    label: 'Scale',
    value: formatScale(display?.scale_percent),
    disabled: true,
    action: 'open-system-settings',
    reason: 'Use system display settings',
  };
}

export function formatDisplayRow(display = {}, index = 0, options = {}) {
  const title =
    display.friendly_name || display.device_name || display.display_id || `Display ${index + 1}`;
  const friendlyName =
    title === display.friendly_name
      ? display.device_name || display.display_id || ''
      : display.friendly_name || display.display_id || '';
  const markers = [];

  if (display.primary) {
    markers.push('Primary');
  }
  if (display.active) {
    markers.push('Active');
  }

  return {
    id: display.display_id || `display-${index + 1}`,
    title,
    friendlyName,
    resolution: hasSize(display) ? `${display.width} x ${display.height}` : 'Unknown',
    refreshRate: formatRefreshRate(display.refresh_rate_millihz),
    scale: formatScale(display.scale_percent),
    primary: Boolean(display.primary),
    active: Boolean(display.active),
    markers,
    controlsDisabled: options.controlsAvailable === false,
    controlsDisabledReason: options.controlsAvailable === false ? 'Daemon offline' : '',
    scaleControl: buildScaleControl(display),
    unsupportedControls: unsupportedControls(display),
    source: display,
  };
}

export function buildDisplayView(displayState = {}, options = {}) {
  const displays = Array.isArray(displayState?.displays) ? displayState.displays : [];
  const controlsAvailable = options.controlsAvailable !== false;
  const rows = displays.map((display, index) =>
    formatDisplayRow(display, index, { controlsAvailable }),
  );

  return {
    empty: rows.length === 0,
    emptyMessage: 'No local displays reported yet.',
    countLabel: `${rows.length} display${rows.length === 1 ? '' : 's'}`,
    controlsAvailable,
    rows,
  };
}

export function buildDisplayOperationStatus(result, operationLabel) {
  const status = result?.status || 'ApplyFailed';
  const message = result?.message || `${operationLabel}: ${status}`;
  const tone =
    status === 'Success'
      ? 'ok'
      : status === 'Unsupported' || status === 'RequiresSystemSettings'
        ? 'warning'
        : 'error';

  return { text: message, tone };
}
