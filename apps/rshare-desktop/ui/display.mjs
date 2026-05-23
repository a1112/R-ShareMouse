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

function formatOrientation(orientation) {
  return String(orientation || 'Landscape').replace(/([a-z])([A-Z])/g, '$1 $2');
}

function unsupportedControls(display) {
  const capabilities = display?.write_capabilities ?? {};
  return Object.entries(CONTROL_LABELS)
    .filter(([key]) => capabilities[key] === false)
    .map(([, label]) => label);
}

function normalizedModes(display) {
  const modes = Array.isArray(display?.modes) ? display.modes : [];
  return modes
    .map((mode) => ({
      width: Number(mode?.width),
      height: Number(mode?.height),
      refreshRateMillihz: Number(mode?.refresh_rate_millihz),
      orientation: mode?.orientation || 'Landscape',
    }))
    .filter(
      (mode) =>
        mode.width > 0 &&
        mode.height > 0 &&
        Number.isFinite(mode.refreshRateMillihz) &&
        mode.refreshRateMillihz > 0,
    );
}

function uniqueByValue(options) {
  const seen = new Set();
  return options.filter((option) => {
    if (seen.has(option.value)) {
      return false;
    }
    seen.add(option.value);
    return true;
  });
}

function parseResolution(value) {
  const match = String(value || '').match(/^(\d+)x(\d+)$/);
  if (!match) {
    return null;
  }
  return { width: Number(match[1]), height: Number(match[2]) };
}

function displaySupportsModeWrites(display, controlsAvailable) {
  const capabilities = display?.write_capabilities ?? {};
  if (controlsAvailable === false) {
    return { supported: false, reason: 'Daemon offline' };
  }
  if (display?.active === false) {
    return { supported: false, reason: 'Display offline' };
  }
  if (
    capabilities.resolution !== true ||
    capabilities.refresh_rate !== true ||
    capabilities.orientation !== true
  ) {
    return { supported: false, reason: 'Mode writes unsupported' };
  }
  if (normalizedModes(display).length === 0) {
    return { supported: false, reason: 'No writable modes reported' };
  }
  return { supported: true, reason: '' };
}

export function buildDisplayModeControls(display = {}, options = {}) {
  const modes = normalizedModes(display);
  const support = displaySupportsModeWrites(display, options.controlsAvailable !== false);
  const currentResolution =
    Number(display.width) > 0 && Number(display.height) > 0
      ? `${Number(display.width)}x${Number(display.height)}`
      : `${modes[0]?.width ?? 0}x${modes[0]?.height ?? 0}`;
  const currentRefresh = Number(display.refresh_rate_millihz || modes[0]?.refreshRateMillihz || 0);
  const currentOrientation = display.orientation || modes[0]?.orientation || 'Landscape';

  return {
    disabled: !support.supported,
    disabledReason: support.reason,
    canApply: support.supported,
    selected: {
      resolution: currentResolution,
      refreshRateMillihz: String(currentRefresh),
      orientation: currentOrientation,
    },
    resolutionOptions: uniqueByValue(
      modes.map((mode) => ({
        value: `${mode.width}x${mode.height}`,
        label: `${mode.width} x ${mode.height}`,
        width: mode.width,
        height: mode.height,
      })),
    ),
    refreshRateOptions: uniqueByValue(
      modes.map((mode) => ({
        value: String(mode.refreshRateMillihz),
        label: formatRefreshRate(mode.refreshRateMillihz),
        refreshRateMillihz: mode.refreshRateMillihz,
      })),
    ),
    orientationOptions: uniqueByValue(
      modes.map((mode) => ({
        value: mode.orientation,
        label: formatOrientation(mode.orientation),
      })),
    ),
  };
}

export function buildDisplaySettingsRequest(display = {}, selections = {}) {
  const support = displaySupportsModeWrites(display, true);
  if (!support.supported) {
    return { request: null, error: support.reason };
  }

  const resolution = parseResolution(selections.resolution);
  const refreshRateMillihz = Number(selections.refreshRateMillihz);
  const orientation = selections.orientation || display.orientation || 'Landscape';

  if (!resolution || !Number.isFinite(refreshRateMillihz) || refreshRateMillihz <= 0) {
    return { request: null, error: 'Select a supported display mode.' };
  }

  const mode = normalizedModes(display).find(
    (candidate) =>
      candidate.width === resolution.width &&
      candidate.height === resolution.height &&
      candidate.refreshRateMillihz === refreshRateMillihz &&
      candidate.orientation === orientation,
  );
  if (!mode) {
    return { request: null, error: 'Select a supported display mode.' };
  }

  return {
    request: {
      display_id: display.display_id,
      width: mode.width,
      height: mode.height,
      refresh_rate_millihz: mode.refreshRateMillihz,
      orientation: mode.orientation,
    },
    error: null,
  };
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
    modeControls: buildDisplayModeControls(display, { controlsAvailable: options.controlsAvailable }),
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

function normalizeCaptureBytes(bytes) {
  if (bytes instanceof Uint8Array) {
    return bytes;
  }
  if (Array.isArray(bytes)) {
    return Uint8Array.from(bytes);
  }
  if (bytes instanceof ArrayBuffer) {
    return new Uint8Array(bytes);
  }
  return new Uint8Array();
}

export function updateDisplayPreviewState(previews = new Map(), result = {}, adapters = {}) {
  if (result?.status !== 'Success' || !result?.display_id) {
    return previews;
  }

  const bytes = normalizeCaptureBytes(result.bytes);
  if (bytes.length === 0) {
    return previews;
  }

  const BlobCtor = adapters.Blob ?? globalThis.Blob;
  const createObjectURL =
    adapters.createObjectURL ?? globalThis.URL?.createObjectURL?.bind(globalThis.URL);
  const revokeObjectURL =
    adapters.revokeObjectURL ?? globalThis.URL?.revokeObjectURL?.bind(globalThis.URL);

  if (!BlobCtor || !createObjectURL) {
    return previews;
  }

  const mimeType = result.mime_type || 'image/png';
  const blob = new BlobCtor([bytes], { type: mimeType });
  const url = createObjectURL(blob);
  const next = new Map(previews);
  const previous = next.get(result.display_id);
  if (previous?.url && revokeObjectURL) {
    revokeObjectURL(previous.url);
  }
  next.set(result.display_id, {
    displayId: result.display_id,
    url,
    mimeType,
    width: result.width ?? null,
    height: result.height ?? null,
  });
  return next;
}
