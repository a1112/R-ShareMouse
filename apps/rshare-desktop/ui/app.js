import { buildScreenLayout, buildStatusBanner } from './layout.mjs';

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;

const state = {
  serviceRunning: false,
  status: null,
  devices: [],
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
const serviceToggleBtn = document.getElementById('serviceToggleBtn');

function renderStage() {
  const screens = buildScreenLayout(state.devices, state.status);
  const cards = screens
    .map(
      (screen) => `
        <article
          class="screen-card ${screen.kind} ${screen.connected ? 'connected' : ''}"
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
                <strong>${screen.label}</strong>
                <span>${screen.subtitle}</span>
              </div>
            </div>
            <div class="screen-footer">
              <span class="screen-badge">${screen.status}</span>
              <span>${screen.lastSeen}</span>
            </div>
          </div>
        </article>
      `,
    )
    .join('');

  const emptyHint = state.devices.length
    ? ''
    : `
      <div class="empty-stage">
        还没有发现其他设备。当前只显示本机屏幕，启动 daemon 后会把发现到的设备模拟到这个画布里。
      </div>
    `;

  screenStage.innerHTML = `${cards}${emptyHint}`;
}

function renderDashboard() {
  const banner = buildStatusBanner(state.status, state.devices);
  const connectedCount = state.devices.filter((device) => device.connected).length;

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

  devicesMetric.textContent = `${connectedCount} connected / ${state.devices.length} discovered`;
  devicesDetail.textContent = state.devices.length
    ? 'Device cards are simulated from the discovered daemon snapshot.'
    : 'Only the local screen is visible right now';

  renderStage();
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
    state.devices = snapshot.devices ?? [];
    state.serviceRunning = Boolean(snapshot.status);
    renderDashboard();
  } catch (error) {
    heroTitle.textContent = 'Refresh failed';
    heroSubtitle.textContent = String(error);
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
}

function boot() {
  wireWindowControls();
  wireActions();
  renderDashboard();
  refreshDashboard();
  setInterval(refreshDashboard, 1500);
}

boot();
