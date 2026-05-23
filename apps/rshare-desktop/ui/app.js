import { buildScreenLayout, buildStatusBanner } from './layout.mjs';
import {
  buildDisplayOperationStatus,
  buildDisplaySettingsRequest,
  buildDisplayView,
  updateDisplayPreviewState,
} from './display.mjs';

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;

const DEFAULT_CONFIG = {
  network: {
    port: 27431,
    bind_address: '0.0.0.0',
    mdns_enabled: true,
  },
  gui: {
    minimize_to_tray: true,
    show_notifications: true,
    start_minimized: false,
    show_tray_icon: true,
    screen_layout: [],
  },
  input: {
    clipboard_sync: true,
    edge_threshold: 10,
    mouse_wheel_sync: true,
    key_delay_ms: 0,
  },
  gamepad: {
    enabled: false,
    routing_mode: 'Disabled',
    deadzone_basis_points: 800,
    max_update_hz: 120,
    vibration: false,
  },
  features: {
    suppress_local_shortcuts_when_remote: true,
    auto_endpoint_latency_probe: true,
    audio_capture: true,
    audio_forwarding: true,
    usb_forwarding_experimental: false,
    usb_device_advertising: true,
    usb_descriptor_probe: true,
  },
  security: {
    password_required: false,
    encryption: true,
    password_hash: null,
    trusted_devices: [],
    lan_only: true,
  },
  known_devices: [],
};

const SETTINGS_GROUPS = [
  {
    title: 'Window',
    items: [
      ['gui.minimize_to_tray', 'Minimize to tray'],
      ['gui.show_notifications', 'Show notifications'],
      ['gui.start_minimized', 'Start minimized'],
      ['gui.show_tray_icon', 'Show tray icon'],
    ],
  },
  {
    title: 'Input',
    items: [
      ['input.clipboard_sync', 'Clipboard sync'],
      ['input.mouse_wheel_sync', 'Mouse wheel sync'],
      ['features.suppress_local_shortcuts_when_remote', 'Block local shortcuts while remote'],
      ['features.auto_endpoint_latency_probe', 'Auto endpoint latency probe'],
    ],
  },
  {
    title: 'Audio',
    items: [
      ['features.audio_capture', 'Audio capture'],
      ['features.audio_forwarding', 'Audio forwarding'],
    ],
  },
  {
    title: 'Experimental USB',
    items: [
      ['features.usb_forwarding_experimental', 'USB forwarding'],
      ['features.usb_device_advertising', 'Advertise local USB devices'],
      ['features.usb_descriptor_probe', 'Remote USB descriptor probe'],
    ],
  },
  {
    title: 'Network',
    items: [
      ['network.mdns_enabled', 'mDNS discovery'],
      ['security.lan_only', 'LAN only'],
      ['security.encryption', 'Encrypted transport'],
    ],
  },
];

const state = {
  serviceRunning: false,
  status: null,
  devices: [],
  config: null,
  localControls: null,
  displayControlsAvailable: false,
  displayStatus: 'Display state not loaded.',
  displayPreviews: new Map(),
};

const heroTitle = document.getElementById('heroTitle');
const heroSubtitle = document.getElementById('heroSubtitle');
const serviceMetric = document.getElementById('serviceMetric');
const serviceDetail = document.getElementById('serviceDetail');
const networkMetric = document.getElementById('networkMetric');
const networkDetail = document.getElementById('networkDetail');
const devicesMetric = document.getElementById('devicesMetric');
const devicesDetail = document.getElementById('devicesDetail');
const screenStage = document.getElementById('screenStage');
const displayList = document.getElementById('displayList');
const displaySummary = document.getElementById('displaySummary');
const displayOperationStatus = document.getElementById('displayOperationStatus');
const displayRefreshBtn = document.getElementById('displayRefreshBtn');
const identifyDisplaysBtn = document.getElementById('identifyDisplaysBtn');
const openDisplaySettingsBtn = document.getElementById('openDisplaySettingsBtn');
const serviceToggleBtn = document.getElementById('serviceToggleBtn');
const settingsToggleBtn = document.getElementById('settingsToggleBtn');
const settingsCloseBtn = document.getElementById('settingsCloseBtn');
const settingsPanel = document.getElementById('settingsPanel');
const settingsSaveStatus = document.getElementById('settingsSaveStatus');
const settingsSwitches = document.getElementById('settingsSwitches');

function cloneDefaultConfig() {
  return JSON.parse(JSON.stringify(DEFAULT_CONFIG));
}

function mergeConfig(config) {
  const defaults = cloneDefaultConfig();
  const incoming = config ?? {};
  return {
    ...defaults,
    ...incoming,
    network: { ...defaults.network, ...(incoming.network ?? {}) },
    gui: { ...defaults.gui, ...(incoming.gui ?? {}) },
    input: { ...defaults.input, ...(incoming.input ?? {}) },
    gamepad: { ...defaults.gamepad, ...(incoming.gamepad ?? {}) },
    features: { ...defaults.features, ...(incoming.features ?? {}) },
    security: { ...defaults.security, ...(incoming.security ?? {}) },
    known_devices: incoming.known_devices ?? defaults.known_devices,
  };
}

function getPath(target, path) {
  return path.split('.').reduce((value, key) => value?.[key], target);
}

function setPath(target, path, value) {
  const keys = path.split('.');
  const leaf = keys.pop();
  const parent = keys.reduce((value, key) => {
    value[key] ??= {};
    return value[key];
  }, target);
  parent[leaf] = value;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function renderStage() {
  const deviceList = Array.isArray(state.devices) ? state.devices : [];
  const screens = buildScreenLayout(deviceList, state.status);
  const cards = screens
    .map(
      (screen) => `
        <article
          class="screen-card ${escapeHtml(screen.kind)} ${screen.connected ? 'connected' : ''}"
          style="left:${screen.x}px;top:${screen.y}px;width:${screen.width}px;height:${screen.height}px;"
        >
          <div class="screen-bezel">
            <div class="screen-topbar">
              <span></span>
              <span></span>
              <span></span>
            </div>
            <div class="screen-content">
              <div>
                <strong>${escapeHtml(screen.label)}</strong>
                <span>${escapeHtml(screen.subtitle)}</span>
              </div>
            </div>
            <div class="screen-footer">
              <span class="screen-badge">${escapeHtml(screen.status)}</span>
              <span>${escapeHtml(screen.lastSeen)}</span>
            </div>
          </div>
        </article>
      `,
    )
    .join('');

  const emptyHint = deviceList.length
    ? ''
    : `
      <div class="empty-stage">
        No peer devices discovered yet. Start the daemon on another machine in the same LAN.
      </div>
    `;

  screenStage.innerHTML = `${cards}${emptyHint}`;
}

function setDisplayStatus(text, tone = '') {
  state.displayStatus = text;
  if (!displayOperationStatus) {
    return;
  }
  displayOperationStatus.textContent = text;
  displayOperationStatus.dataset.tone = tone;
}

function revokeDisplayPreview(preview) {
  if (preview?.url && window.URL?.revokeObjectURL) {
    window.URL.revokeObjectURL(preview.url);
  }
}

function pruneDisplayPreviews(rows) {
  const displayIds = new Set(rows.map((row) => row.id));
  for (const [displayId, preview] of state.displayPreviews.entries()) {
    if (!displayIds.has(displayId)) {
      revokeDisplayPreview(preview);
      state.displayPreviews.delete(displayId);
    }
  }
}

function selectOptionsHtml(options, selectedValue) {
  return options
    .map(
      (option) => `
        <option value="${escapeHtml(option.value)}" ${
          option.value === selectedValue ? 'selected' : ''
        }>${escapeHtml(option.label)}</option>
      `,
    )
    .join('');
}

function renderDisplayModeControls(row) {
  const controls = row.modeControls;
  const disabled = controls.disabled || row.controlsDisabled ? 'disabled' : '';
  const disabledReason = row.controlsDisabled ? row.controlsDisabledReason : controls.disabledReason;

  return `
    <div class="display-mode-controls">
      <label>
        <span>Resolution</span>
        <select data-display-setting="resolution" ${disabled}>
          ${selectOptionsHtml(controls.resolutionOptions, controls.selected.resolution)}
        </select>
      </label>
      <label>
        <span>Refresh</span>
        <select data-display-setting="refreshRateMillihz" ${disabled}>
          ${selectOptionsHtml(controls.refreshRateOptions, controls.selected.refreshRateMillihz)}
        </select>
      </label>
      <label>
        <span>Orientation</span>
        <select data-display-setting="orientation" ${disabled}>
          ${selectOptionsHtml(controls.orientationOptions, controls.selected.orientation)}
        </select>
      </label>
      <button class="toolbar-btn compact" data-display-action="apply-mode" data-display-id="${escapeHtml(
        row.id,
      )}" ${disabled || !controls.canApply ? 'disabled' : ''}>Apply</button>
    </div>
    ${disabledReason ? `<p class="display-note">${escapeHtml(disabledReason)}</p>` : ''}
  `;
}

function renderLocalDisplays() {
  if (!displayList || !displaySummary) {
    return;
  }

  const view = buildDisplayView(state.localControls?.display, {
    controlsAvailable: state.displayControlsAvailable,
  });
  displaySummary.textContent = !state.displayControlsAvailable
    ? 'Daemon offline'
    : state.localControls
      ? `${view.countLabel} reported by local controls`
      : 'Waiting for local controls';
  pruneDisplayPreviews(view.rows);
  if (identifyDisplaysBtn) {
    identifyDisplaysBtn.disabled = !state.displayControlsAvailable;
  }

  if (view.empty) {
    displayList.innerHTML = `<div class="display-empty">${escapeHtml(view.emptyMessage)}</div>`;
    return;
  }

  displayList.innerHTML = view.rows
    .map((row) => {
      const markerHtml = row.markers
        .map((marker) => `<span class="display-marker">${escapeHtml(marker)}</span>`)
        .join('');
      const unsupportedHtml = row.unsupportedControls.length
        ? `<p class="display-note">Unsupported controls: ${escapeHtml(
            row.unsupportedControls.join(', '),
          )}</p>`
        : '';
      const scaleControl = row.scaleControl;
      const daemonDisabled = row.controlsDisabled ? 'disabled' : '';
      const scaleSettingsButton =
        scaleControl.action === 'open-system-settings'
          ? `<button class="toolbar-btn compact" data-display-action="scale-settings" data-display-id="${escapeHtml(
              row.id,
            )}">Open settings</button>`
          : '';
      const preview = state.displayPreviews.get(row.id);
      const previewSize =
        preview?.width && preview?.height ? `${preview.width} x ${preview.height}` : 'latest';
      const previewHtml = preview
        ? `
          <figure class="display-preview">
            <img src="${escapeHtml(preview.url)}" alt="${escapeHtml(row.title)} preview" />
            <figcaption>${escapeHtml(previewSize)} preview</figcaption>
          </figure>
        `
        : '';

      return `
        <article class="display-card" data-display-id="${escapeHtml(row.id)}">
          <div class="display-card-head">
            <div>
              <h3>${escapeHtml(row.title)}</h3>
              <p>${escapeHtml(row.friendlyName || row.id)}</p>
            </div>
            <div class="display-markers">${markerHtml}</div>
          </div>
          <div class="display-facts">
            <span><strong>${escapeHtml(row.resolution)}</strong> Resolution</span>
            <span><strong>${escapeHtml(row.refreshRate)}</strong> Refresh</span>
            <span><strong>${escapeHtml(row.scale)}</strong> Scale</span>
          </div>
          ${previewHtml}
          <div class="display-controls">
            <button class="toolbar-btn compact" data-display-action="capture" data-display-id="${escapeHtml(
              row.id,
            )}" ${daemonDisabled}>Capture</button>
            <span class="display-readonly">${escapeHtml(scaleControl.label)} ${escapeHtml(
              scaleControl.value,
            )}</span>
            ${scaleSettingsButton}
          </div>
          ${renderDisplayModeControls(row)}
          ${unsupportedHtml}
        </article>
      `;
    })
    .join('');
}

function renderDashboard() {
  const deviceList = Array.isArray(state.devices) ? state.devices : [];
  const banner = buildStatusBanner(state.status, deviceList);
  const connectedCount = deviceList.filter((device) => device.connected).length;

  heroTitle.textContent = banner.title;
  heroSubtitle.textContent = banner.detail;
  serviceToggleBtn.textContent = banner.actionLabel;

  serviceMetric.textContent = state.serviceRunning ? 'Online' : 'Offline';
  serviceDetail.textContent = state.serviceRunning
    ? `PID ${state.status.pid} is responding over local IPC.`
    : 'Daemon unreachable';

  networkMetric.textContent = state.status?.bind_address || 'Daemon offline';
  networkDetail.textContent = state.status
    ? `Discovery UDP port ${state.status.discovery_port}`
    : 'No bind address available';

  devicesMetric.textContent = `${connectedCount} connected / ${deviceList.length} discovered`;
  devicesDetail.textContent = deviceList.length
    ? 'Device cards are built from the discovered daemon snapshot.'
    : 'Only the local screen is visible right now';

  renderStage();
  renderLocalDisplays();
}

function renderSettings() {
  const config = state.config ?? cloneDefaultConfig();
  settingsSwitches.innerHTML = SETTINGS_GROUPS.map(
    (group) => `
      <section class="settings-group">
        <h3>${escapeHtml(group.title)}</h3>
        ${group.items
          .map(([path, label]) => {
            const id = `setting-${path.replaceAll('.', '-')}`;
            const checked = Boolean(getPath(config, path));
            return `
              <label class="setting-row" for="${id}">
                <span>${escapeHtml(label)}</span>
                <span class="switch">
                  <input id="${id}" type="checkbox" data-setting-path="${path}" ${
                    checked ? 'checked' : ''
                  } />
                  <span class="switch-track"></span>
                </span>
              </label>
            `;
          })
          .join('')}
      </section>
    `,
  ).join('');
}

function setSettingsStatus(text, tone = '') {
  settingsSaveStatus.textContent = text;
  settingsSaveStatus.dataset.tone = tone;
}

async function loadConfig() {
  if (!invoke) {
    setSettingsStatus('Settings unavailable outside Tauri.', 'error');
    return;
  }

  try {
    state.config = mergeConfig(await invoke('get_config'));
    renderSettings();
    setSettingsStatus('Config loaded.', 'ok');
  } catch (error) {
    state.config = cloneDefaultConfig();
    renderSettings();
    setSettingsStatus(String(error), 'error');
  }
}

async function saveConfig() {
  if (!invoke || !state.config) {
    return;
  }

  setSettingsStatus('Saving...');
  try {
    await invoke('set_config', { config: state.config });
    setSettingsStatus(
      state.serviceRunning ? 'Saved. Restart service for runtime switches.' : 'Saved.',
      'ok',
    );
  } catch (error) {
    setSettingsStatus(String(error), 'error');
  }
}

async function refreshDashboard() {
  if (!invoke) {
    heroTitle.textContent = 'Tauri bridge unavailable';
    heroSubtitle.textContent = 'window.__TAURI__.core.invoke was not injected.';
    return;
  }

  try {
    const snapshot = await invoke('dashboard_state');
    state.status = snapshot.status;
    state.devices = Array.isArray(snapshot.devices) ? snapshot.devices : [];
    state.serviceRunning = Boolean(snapshot.status);
    if (!state.serviceRunning) {
      state.localControls = null;
      state.displayControlsAvailable = false;
    }
    renderDashboard();
    if (state.serviceRunning) {
      await loadLocalControls({ silent: true });
    }
  } catch (error) {
    state.status = null;
    state.devices = [];
    state.serviceRunning = false;
    state.localControls = null;
    state.displayControlsAvailable = false;
    renderDashboard();
    setDisplayStatus('Daemon offline; display controls disabled.', 'warning');
    heroTitle.textContent = 'Refresh failed';
    heroSubtitle.textContent = String(error);
  }
}

async function loadLocalControls(options = {}) {
  if (!invoke) {
    setDisplayStatus('Display controls unavailable outside Tauri.', 'error');
    return;
  }

  try {
    state.localControls = await invoke('local_controls_state');
    state.displayControlsAvailable = true;
    renderLocalDisplays();
    if (!options.silent) {
      setDisplayStatus('Display state refreshed.', 'ok');
    } else if (state.displayStatus === 'Display state not loaded.') {
      setDisplayStatus('Display state loaded.', 'ok');
    }
  } catch (error) {
    state.localControls = null;
    state.displayControlsAvailable = false;
    renderLocalDisplays();
    setDisplayStatus(`Display controls disabled: ${error}`, 'warning');
  }
}

function firstDisplayRow() {
  const view = buildDisplayView(state.localControls?.display);
  return view.rows.find((row) => row.primary) ?? view.rows[0] ?? null;
}

function displayRowById(displayId) {
  const view = buildDisplayView(state.localControls?.display, {
    controlsAvailable: state.displayControlsAvailable,
  });
  return view.rows.find((row) => row.id === displayId) ?? null;
}

async function identifyDisplays() {
  setDisplayStatus('Identifying displays...');
  try {
    const result = await invoke('identify_displays', { durationMs: 2500 });
    const status = buildDisplayOperationStatus(result, 'Identify displays');
    setDisplayStatus(status.text, status.tone);
  } catch (error) {
    setDisplayStatus(String(error), 'error');
  }
}

async function captureDisplay(displayId = firstDisplayRow()?.id) {
  if (!displayId) {
    setDisplayStatus('No display available to capture.', 'error');
    return;
  }

  setDisplayStatus('Capturing display...');
  try {
    const result = await invoke('capture_display', { displayId, maxWidth: 640 });
    state.displayPreviews = updateDisplayPreviewState(state.displayPreviews, result);
    const size = result.width && result.height ? ` ${result.width} x ${result.height}` : '';
    const status = buildDisplayOperationStatus(
      {
        ...result,
        message: result.message || `Capture: ${result.status}${size}`,
      },
      'Capture',
    );
    setDisplayStatus(status.text, status.tone);
    renderLocalDisplays();
  } catch (error) {
    setDisplayStatus(String(error), 'error');
  }
}

async function applyDisplayMode(displayId, card) {
  const row = displayRowById(displayId);
  if (!row || !card) {
    return;
  }

  const selections = {
    resolution: card.querySelector('[data-display-setting="resolution"]')?.value,
    refreshRateMillihz: card.querySelector('[data-display-setting="refreshRateMillihz"]')?.value,
    orientation: card.querySelector('[data-display-setting="orientation"]')?.value,
  };
  const update = buildDisplaySettingsRequest(row.source, selections);
  if (update.error) {
    setDisplayStatus(update.error, 'warning');
    return;
  }

  const confirmed =
    window.confirm?.(
      `Apply ${update.request.width} x ${update.request.height} at ${Math.round(
        update.request.refresh_rate_millihz / 1000,
      )} Hz to ${row.title}?`,
    ) ?? true;
  if (!confirmed) {
    return;
  }

  setDisplayStatus('Updating display settings...');
  try {
    const result = await invoke('update_display_settings', {
      request: update.request,
    });
    const status = buildDisplayOperationStatus(result, 'Display update');
    setDisplayStatus(status.text, status.tone);
    await loadLocalControls({ silent: true });
  } catch (error) {
    setDisplayStatus(String(error), 'error');
  }
}

async function openSystemDisplaySettings() {
  setDisplayStatus('Opening system display settings...');
  try {
    await invoke('open_display_settings');
    setDisplayStatus('System display settings opened.', 'ok');
  } catch (error) {
    setDisplayStatus(String(error), 'error');
  }
}

async function toggleService() {
  try {
    if (state.serviceRunning) {
      await invoke('stop_service');
    } else {
      await invoke('start_service');
    }
    await refreshDashboard();
  } catch (error) {
    heroSubtitle.textContent = String(error);
  }
}

function toggleSettings(open = !settingsPanel.classList.contains('open')) {
  settingsPanel.classList.toggle('open', open);
  settingsPanel.setAttribute('aria-hidden', String(!open));
}

function wireWindowControls() {
  document.getElementById('minimizeBtn').addEventListener('click', () => invoke('minimize_window'));
  document
    .getElementById('maximizeBtn')
    .addEventListener('click', () => invoke('toggle_maximize_window'));
  document.getElementById('closeBtn').addEventListener('click', () => invoke('close_window'));
}

function wireActions() {
  serviceToggleBtn.addEventListener('click', toggleService);
  document.getElementById('refreshDevicesBtn').addEventListener('click', refreshDashboard);
  displayRefreshBtn?.addEventListener('click', () => loadLocalControls());
  identifyDisplaysBtn?.addEventListener('click', identifyDisplays);
  openDisplaySettingsBtn?.addEventListener('click', openSystemDisplaySettings);
  displayList?.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-display-action]');
    if (!button || button.disabled) {
      return;
    }

    const displayId = button.dataset.displayId;
    if (button.dataset.displayAction === 'capture') {
      captureDisplay(displayId);
    } else if (button.dataset.displayAction === 'apply-mode') {
      applyDisplayMode(displayId, button.closest('.display-card'));
    } else if (button.dataset.displayAction === 'scale-settings') {
      openSystemDisplaySettings();
    }
  });
  settingsToggleBtn.addEventListener('click', () => toggleSettings());
  settingsCloseBtn.addEventListener('click', () => toggleSettings(false));
  settingsSwitches.addEventListener('change', (event) => {
    const input = event.target.closest('input[data-setting-path]');
    if (!input) {
      return;
    }
    state.config = mergeConfig(state.config);
    setPath(state.config, input.dataset.settingPath, input.checked);
    saveConfig();
  });
}

function boot() {
  wireWindowControls();
  wireActions();
  renderDashboard();
  renderSettings();
  loadConfig();
  loadLocalControls();
  refreshDashboard();
  setInterval(refreshDashboard, 1500);
}

boot();
