use anyhow::{anyhow, bail, Context, Result};
use rshare_core::{
    DaemonRequest, DaemonResponse, EndpointInjectRequest, EndpointInjectTarget,
    LocalInputDiagnosticEvent, MobileAccessSnapshot,
};
use rshare_input::InjectBackend;
use rshare_net::NetworkManager;
use serde_json::json;
use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{endpoint_runtime::inject_endpoint_event, DaemonState};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct MobileGatewayAccess {
    enabled: bool,
    bind_addr: SocketAddr,
    token: String,
    advertise_host: String,
}

impl MobileGatewayAccess {
    pub(crate) fn new(bind_addr: SocketAddr, token: String, advertise_host: String) -> Self {
        Self {
            enabled: true,
            bind_addr,
            token,
            advertise_host,
        }
    }

    pub(crate) fn disabled(_reason: String) -> Self {
        Self {
            enabled: false,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            token: String::new(),
            advertise_host: String::new(),
        }
    }

    pub(crate) fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn snapshot(&self) -> MobileAccessSnapshot {
        if !self.enabled {
            return MobileAccessSnapshot {
                enabled: false,
                bind_address: "不可用".to_string(),
                page_url: "不可用".to_string(),
                token: String::new(),
            };
        }

        MobileAccessSnapshot {
            enabled: true,
            bind_address: self.bind_addr.to_string(),
            page_url: format!(
                "http://{}:{}/mobile?t={}",
                self.advertise_host,
                self.bind_addr.port(),
                self.token
            ),
            token: self.token.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MobileHttpRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl MobileHttpRequest {
    #[cfg(test)]
    pub(crate) fn new(method: &str, target: &str, headers: Vec<(&str, &str)>) -> Self {
        Self {
            method: method.to_ascii_uppercase(),
            target: target.to_string(),
            headers: headers
                .into_iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobileGatewayRoute {
    Page,
    Manifest,
    Icon,
    LocalControls,
    Inject,
    NotFound,
}

pub(crate) fn route_mobile_http_request(method: &str, target: &str) -> MobileGatewayRoute {
    let path = target
        .split('?')
        .next()
        .unwrap_or(target)
        .trim_end_matches('/');
    match (method.to_ascii_uppercase().as_str(), path) {
        ("GET", "" | "/mobile") => MobileGatewayRoute::Page,
        ("GET", "/mobile.webmanifest") => MobileGatewayRoute::Manifest,
        ("GET", "/mobile-icon.svg") => MobileGatewayRoute::Icon,
        ("GET", "/api/local-controls") => MobileGatewayRoute::LocalControls,
        ("POST", "/api/inject") => MobileGatewayRoute::Inject,
        _ => MobileGatewayRoute::NotFound,
    }
}

pub(crate) fn is_authorized_mobile_request(request: &MobileHttpRequest, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }

    mobile_token_from_target(&request.target)
        .or_else(|| mobile_token_from_authorization(request.header("authorization")))
        .or_else(|| request.header("x-rshare-mobile-token").map(str::to_string))
        .as_deref()
        == Some(expected)
}

pub(crate) fn preferred_mobile_advertise_host(hostname: &str) -> String {
    detect_lan_ipv4()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| {
            let trimmed = hostname.trim();
            if trimmed.is_empty() {
                "127.0.0.1".to_string()
            } else {
                trimmed.to_string()
            }
        })
}

pub(crate) async fn run_mobile_gateway_server(
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let listener = match TcpListener::bind(access.bind_addr()).await {
        Ok(listener) => listener,
        Err(error) => {
            let reason = format!(
                "Mobile gateway unavailable on {}: {}",
                access.bind_addr(),
                error
            );
            tracing::warn!("{}", reason);
            {
                let mut state = state.write().await;
                state.mobile_access = MobileGatewayAccess::disabled(reason);
            }
            let _ = shutdown_rx.recv().await;
            return Ok(());
        }
    };
    tracing::info!("Mobile gateway listening on {}", access.bind_addr());

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let access = access.clone();
                let state = state.clone();
                let network_manager = network_manager.clone();
                let inject_backend = inject_backend.clone();
                let local_events_tx = local_events_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_mobile_gateway_client(
                        stream,
                        access,
                        state,
                        network_manager,
                        inject_backend,
                        local_events_tx,
                    )
                    .await
                    {
                        tracing::debug!("Mobile gateway client error: {}", error);
                    }
                });
            }
            _ = shutdown_rx.recv() => break,
        }
    }

    Ok(())
}

async fn handle_mobile_gateway_client(
    mut stream: TcpStream,
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
) -> Result<()> {
    let request = read_mobile_http_request(&mut stream).await?;
    let route = route_mobile_http_request(&request.method, &request.target);

    if route == MobileGatewayRoute::NotFound {
        return write_mobile_response(
            &mut stream,
            404,
            "application/json; charset=utf-8",
            json!({ "error": "not found" }).to_string().into_bytes(),
        )
        .await;
    }

    if !is_authorized_mobile_request(&request, access.token()) {
        return write_mobile_response(
            &mut stream,
            401,
            "application/json; charset=utf-8",
            json!({ "error": "unauthorized" }).to_string().into_bytes(),
        )
        .await;
    }

    match route {
        MobileGatewayRoute::Page => {
            write_mobile_response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                render_mobile_page_with_token(access.token()).into_bytes(),
            )
            .await
        }
        MobileGatewayRoute::Manifest => {
            let manifest = render_mobile_manifest(&request.target)?;
            write_mobile_response(
                &mut stream,
                200,
                "application/manifest+json; charset=utf-8",
                serde_json::to_vec(&manifest)?,
            )
            .await
        }
        MobileGatewayRoute::Icon => {
            write_mobile_response(
                &mut stream,
                200,
                "image/svg+xml; charset=utf-8",
                render_mobile_icon_svg().into_bytes(),
            )
            .await
        }
        MobileGatewayRoute::LocalControls => {
            let snapshot = {
                let mut state = state.write().await;
                state.refresh_local_controls_platform();
                state.local_control_snapshot()
            };
            write_mobile_json(&mut stream, &DaemonResponse::LocalControls(snapshot)).await
        }
        MobileGatewayRoute::Inject => {
            let request = mobile_inject_request_from_body(&request.body)?;
            let result = inject_endpoint_event(
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                EndpointInjectTarget::Local,
                request,
            )
            .await;
            write_mobile_json(&mut stream, &DaemonResponse::EndpointInjectResult(result)).await
        }
        MobileGatewayRoute::NotFound => unreachable!("handled above"),
    }
}

fn mobile_inject_request_from_body(body: &[u8]) -> Result<EndpointInjectRequest> {
    if body.is_empty() {
        bail!("empty mobile inject body");
    }

    if let Ok(DaemonRequest::InjectEndpointEvent { target, request }) =
        serde_json::from_slice::<DaemonRequest>(body)
    {
        match target {
            EndpointInjectTarget::Local => return Ok(request),
            EndpointInjectTarget::Remote(_) => {
                bail!("mobile gateway only injects into the local endpoint")
            }
        }
    }

    serde_json::from_slice::<EndpointInjectRequest>(body)
        .context("failed to decode mobile inject request")
}

async fn write_mobile_json<T: serde::Serialize>(stream: &mut TcpStream, value: &T) -> Result<()> {
    write_mobile_response(
        stream,
        200,
        "application/json; charset=utf-8",
        serde_json::to_vec(value)?,
    )
    .await
}

async fn read_mobile_http_request(stream: &mut TcpStream) -> Result<MobileHttpRequest> {
    let mut header_bytes = Vec::new();
    let mut byte = [0u8; 1];
    while !header_bytes.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            bail!("mobile HTTP client closed before headers");
        }
        header_bytes.push(byte[0]);
        if header_bytes.len() > MAX_HEADER_BYTES {
            bail!("mobile HTTP headers exceeded limit");
        }
    }

    let header_text =
        std::str::from_utf8(&header_bytes).context("mobile HTTP headers not UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP method"))?
        .to_ascii_uppercase();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP target"))?
        .to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("mobile HTTP body exceeded limit");
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await?;
    }

    Ok(MobileHttpRequest {
        method,
        target,
        headers,
        body,
    })
}

async fn write_mobile_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

fn mobile_token_from_authorization(value: Option<&str>) -> Option<String> {
    let value = value?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn mobile_token_from_target(target: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if key == "t" || key == "token" {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn detect_lan_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(addr) if !addr.is_loopback() => Some(addr),
        _ => None,
    }
}

fn render_mobile_manifest(target: &str) -> Result<serde_json::Value> {
    let token = mobile_token_from_target(target).unwrap_or_default();
    let token_query = mobile_token_query(&token);

    Ok(json!({
        "name": "R-ShareMouse Mobile",
        "short_name": "R-ShareMouse",
        "description": "Phone touchpad, keyboard shortcuts, and IME text input for R-ShareMouse.",
        "start_url": format!("/mobile{token_query}"),
        "scope": "/",
        "display": "standalone",
        "orientation": "portrait",
        "theme_color": "#101214",
        "background_color": "#101214",
        "icons": [
            {
                "src": format!("/mobile-icon.svg{token_query}"),
                "sizes": "any",
                "type": "image/svg+xml",
                "purpose": "any maskable"
            }
        ]
    }))
}

fn render_mobile_icon_svg() -> String {
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" role="img" aria-labelledby="title">
  <title id="title">R-ShareMouse Mobile</title>
  <rect width="512" height="512" rx="112" fill="#101214"/>
  <path d="M152 104l224 152-118 28-42 124-64-304z" fill="#47c27a"/>
  <path d="M196 177l33 157 22-65 62-15-117-77z" fill="#101214"/>
</svg>"##
        .to_string()
}

fn render_mobile_page() -> String {
    render_mobile_page_with_token("")
}

fn render_mobile_page_with_token(token: &str) -> String {
    let token_query = mobile_token_query(token);
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string());
    let manifest_href = format!("/mobile.webmanifest{token_query}");
    let icon_href = format!("/mobile-icon.svg{token_query}");

    r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <meta name='theme-color' content='#101214'>
  <meta name='mobile-web-app-capable' content='yes'>
  <meta name='apple-mobile-web-app-capable' content='yes'>
  <meta name='apple-mobile-web-app-title' content='R-ShareMouse'>
  <link rel='manifest' href='__MOBILE_MANIFEST_HREF__'>
  <link rel='icon' href='__MOBILE_ICON_HREF__' type='image/svg+xml'>
  <link rel='apple-touch-icon' href='__MOBILE_ICON_HREF__'>
  <title>R-ShareMouse Mobile</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    * { box-sizing: border-box; }
    html, body { width: 100%; min-height: 100%; margin: 0; overflow: auto; background: #101214; color: #edf2ef; }
    body { overscroll-behavior: none; touch-action: manipulation; -webkit-touch-callout: none; }
    main { min-height: 100dvh; display: flex; flex-direction: column; gap: 12px; padding: 12px; max-width: 720px; margin: 0 auto; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
    h1 { margin: 0; font-size: 14px; line-height: 1.2; }
    .sub { margin-top: 3px; font-size: 12px; color: #8f9b96; }
    .status { border: 1px solid #2a302d; border-radius: 6px; padding: 5px 8px; color: #47c27a; font-size: 12px; max-width: 45%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    #pad { flex: 1; min-height: min(260px, 42dvh); border: 1px solid #29302d; border-radius: 6px; background: linear-gradient(135deg, rgba(71,194,122,.08), rgba(255,255,255,.02)); touch-action: none; display: grid; place-items: center; user-select: none; -webkit-user-select: none; -webkit-touch-callout: none; }
    .dot { width: 12px; height: 12px; border-radius: 50%; background: #47c27a; box-shadow: 0 0 24px rgba(71,194,122,.5); }
    .grid3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
    .grid4 { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
    button, input, textarea { border-radius: 6px; font: inherit; font-size: 14px; }
    button, input { height: 48px; }
    button { border: 1px solid #29302d; background: #171b1d; color: #d8dedb; touch-action: manipulation; user-select: none; -webkit-user-select: none; -webkit-touch-callout: none; }
    button:active { background: rgba(71,194,122,.18); border-color: #47c27a; }
    .textRow { display: flex; gap: 8px; }
    .rangeRow { display: flex; align-items: center; gap: 10px; min-height: 40px; border: 1px solid #29302d; border-radius: 6px; background: #171b1d; padding: 7px 10px; }
    .rangeRow label { flex: 0 0 auto; font-size: 12px; font-weight: 600; }
    .rangeRow input { flex: 1; min-width: 0; height: 28px; accent-color: #47c27a; }
    .rangeValue { width: 42px; text-align: right; font-size: 12px; color: #8f9b96; }
    .inputWrap { min-width: 0; flex: 1; min-height: 76px; display: flex; align-items: flex-start; border: 1px solid #29302d; border-radius: 6px; background: #171b1d; padding: 9px 12px; }
    input, textarea { min-width: 0; flex: 1; border: 0; outline: 0; background: transparent; color: #edf2ef; font-size: 16px; }
    textarea { min-height: 56px; resize: none; line-height: 1.35; }
    .send { width: 58px; background: #47c27a; color: #07110b; border-color: #47c27a; }
  </style>
</head>
<body>
<main>
  <header>
    <div>
      <h1>R-ShareMouse Mobile</h1>
      <div class="sub" id="pos">0, 0 / 1920x1080</div>
    </div>
    <div class="status" id="status">连接中</div>
  </header>
  <section id="pad"><div class="dot"></div></section>
  <section class="rangeRow">
    <label for="sensitivity">灵敏度</label>
    <input id="sensitivity" type="range" aria-label="触控板灵敏度" min="0.5" max="3" step="0.05" value="1.35">
    <span class="rangeValue" id="sensitivityValue">1.35</span>
  </section>
  <section class="grid3">
    <button data-button="Left">左键</button>
    <button data-button="Middle">中键</button>
    <button data-button="Right">右键</button>
  </section>
  <section class="grid4">
    <button data-wheel="3">上滚</button>
    <button data-wheel="-3">下滚</button>
    <button data-key="Backspace">退格</button>
    <button data-key="Enter">回车</button>
  </section>
  <section class="grid4">
    <button data-key="Left">左</button>
    <button data-key="Up">上</button>
    <button data-key="Down">下</button>
    <button data-key="Right">右</button>
  </section>
  <section class="grid4">
    <button data-key="ControlLeft">Ctrl</button>
    <button data-key="ShiftLeft">Shift</button>
    <button data-key="AltLeft">Alt</button>
    <button data-key="SuperLeft">Win</button>
  </section>
  <section class="grid4">
    <button data-key="Escape">Esc</button>
    <button data-key="Tab">Tab</button>
    <button data-key="Space">Space</button>
    <button data-key="Delete">Del</button>
  </section>
  <section class="grid4">
    <button data-key="Home">Home</button>
    <button data-key="End">End</button>
    <button data-key="PageUp">PgUp</button>
    <button data-key="PageDown">PgDn</button>
  </section>
  <section class="grid4">
    <button data-shortcut="ControlLeft,C">复制</button>
    <button data-shortcut="ControlLeft,V">粘贴</button>
    <button data-shortcut="ControlLeft,X">剪切</button>
    <button data-shortcut="ControlLeft,A">全选</button>
  </section>
  <section class="textRow">
    <div class="inputWrap"><textarea id="text" placeholder="文本" autocomplete="off" autocapitalize="none" autocorrect="off" spellcheck="false" enterkeyhint="send" rows="3"></textarea></div>
    <button class="send" id="send">发送</button>
  </section>
</main>
<script>
const token = __MOBILE_TOKEN_JSON__ || new URLSearchParams(location.search).get("t") || "";
const statusEl = document.getElementById("status");
const posEl = document.getElementById("pos");
const pad = document.getElementById("pad");
const textInput = document.getElementById("text");
const sensitivityInput = document.getElementById("sensitivity");
const sensitivityValue = document.getElementById("sensitivityValue");
const mobileEventOptions = { capture: true, passive: false };
let pointer = { x: 0, y: 0, minX: 0, minY: 0, width: 1920, height: 1080, displayId: null };
let displayEntries = [];
let activePointer = null;
let lastPoint = null;
let tapStart = null;
let touchPoints = new Map();
let lastWheelTouches = null;
let twoFingerTapStart = null;
let pendingMove = null;
let pendingMoveFrame = 0;
let dragTimer = 0;
let dragPointer = null;
const LONG_PRESS_DRAG_DELAY_MS = 420;
const POINTER_SENSITIVITY_STORAGE_KEY = "rshare.mobile.pointerSensitivity";
const POINTER_SENSITIVITY_DEFAULT = 1.35;
const POINTER_SENSITIVITY_MIN = 0.5;
const POINTER_SENSITIVITY_MAX = 3;
const POINTER_SENSITIVITY_STEP = 0.05;
function clampPointerSensitivity(value) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return POINTER_SENSITIVITY_DEFAULT;
  const clamped = Math.max(POINTER_SENSITIVITY_MIN, Math.min(POINTER_SENSITIVITY_MAX, parsed));
  return Number((Math.round(clamped / POINTER_SENSITIVITY_STEP) * POINTER_SENSITIVITY_STEP).toFixed(2));
}
function loadPointerSensitivity() {
  try {
    return clampPointerSensitivity(localStorage.getItem(POINTER_SENSITIVITY_STORAGE_KEY));
  } catch {
    return POINTER_SENSITIVITY_DEFAULT;
  }
}
let pointerSensitivity = clampPointerSensitivity(loadPointerSensitivity());
function setPointerSensitivity(value) {
  pointerSensitivity = clampPointerSensitivity(value);
  sensitivityInput.value = String(pointerSensitivity);
  sensitivityValue.textContent = pointerSensitivity.toFixed(2);
  try {
    localStorage.setItem(POINTER_SENSITIVITY_STORAGE_KEY, String(pointerSensitivity));
  } catch {}
}
sensitivityInput.value = String(pointerSensitivity);
sensitivityValue.textContent = pointerSensitivity.toFixed(2);
sensitivityInput.addEventListener("input", (event) => setPointerSensitivity(event.target.value));
function cid(prefix) {
  return `${prefix}-${crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`}`;
}
function formatMobileError(error, scope = "移动端") {
  const message = error instanceof Error ? error.message : String(error || "");
  if (/failed to fetch|networkerror|fetch failed|load failed/i.test(message)) {
    return `${scope}网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络`;
  }
  return `${scope}请求失败：${message || "未知错误"}`;
}
function editableMobileTarget(target) {
  if (!target || typeof target !== "object") return false;
  const tagName = String(target.tagName || "").toUpperCase();
  if (tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT") return true;
  if (target.isContentEditable === true) return true;
  return Boolean(target.closest && target.closest("input, textarea, select, [contenteditable='true']"));
}
function displayIdForPointer(displays, x, y, fallbackId = null) {
  if (!Array.isArray(displays)) return fallbackId || null;
  for (const display of displays) {
    if (!display || typeof display !== "object") continue;
    const displayId = display.display_id || display.id;
    const left = Number(display.x || 0);
    const top = Number(display.y || 0);
    const width = Number(display.width || display.w || 0);
    const height = Number(display.height || display.h || 0);
    if (displayId && Number.isFinite(left) && Number.isFinite(top) && Number.isFinite(width) && Number.isFinite(height) && width > 0 && height > 0 && x >= left && x < left + width && y >= top && y < top + height) {
      return String(displayId);
    }
  }
  return fallbackId || null;
}
function shouldPreventBrowserNavigationEvent(event) {
  const type = String(event?.type || "");
  const button = Number(event?.button);
  if (/^(mouse|pointer|auxclick)/i.test(type) && Number.isFinite(button) && (button === 3 || button === 4)) return true;
  if (type === "keydown") {
    const key = String(event?.key || "");
    if (key === "BrowserBack" || key === "BrowserForward") return true;
    if (event?.altKey && (key === "ArrowLeft" || key === "ArrowRight")) return true;
  }
  return false;
}
function preventBrowserNavigationEvent(event) {
  if (!shouldPreventBrowserNavigationEvent(event)) return false;
  event.preventDefault();
  event.returnValue = false;
  return true;
}
function preventMobileGestureDefault(event) {
  if (editableMobileTarget(event.target)) return false;
  if (!["contextmenu", "dragstart", "selectstart", "gesturestart", "gesturechange", "gestureend"].includes(String(event.type || "").toLowerCase())) return false;
  event.preventDefault();
  event.returnValue = false;
  return true;
}
["mousedown", "mouseup", "auxclick", "pointerdown", "pointerup", "keydown"].forEach((eventName) => {
  window.addEventListener(eventName, preventBrowserNavigationEvent, mobileEventOptions);
});
["contextmenu", "dragstart", "selectstart", "gesturestart", "gesturechange", "gestureend"].forEach((eventName) => {
  document.addEventListener(eventName, preventMobileGestureDefault, mobileEventOptions);
});
async function api(path, options = {}) {
  const headers = { Authorization: `Bearer ${token}`, ...(options.headers || {}) };
  const response = await fetch(path, { ...options, headers });
  const payload = await response.json().catch(() => null);
  if (!response.ok) throw new Error(payload?.error || `HTTP ${response.status}`);
  return payload;
}
function daemonRequest(deviceKind, payload, correlationId, mode = "BestEffort", timeoutMs = 250) {
  return {
    InjectEndpointEvent: {
      target: "Local",
      request: { correlation_id: correlationId, device_kind: deviceKind, payload, mode, timeout_ms: timeoutMs }
    }
  };
}
async function inject(request) {
  try {
    const result = await api("/api/inject", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request)
    });
    const accepted = result.EndpointInjectResult?.accepted !== false;
    statusEl.textContent = accepted ? "已连接" : "注入失败";
    return accepted;
  } catch (error) {
    statusEl.textContent = formatMobileError(error, "移动端注入");
    return false;
  }
}
async function refresh() {
  try {
    const payload = await api("/api/local-controls");
    const snapshot = payload.LocalControls || {};
    const mouse = snapshot.mouse || {};
    const display = snapshot.display || {};
    displayEntries = Array.isArray(display.displays) ? display.displays : [];
    const primary = displayEntries.find((item) => item.primary) || displayEntries[0] || {};
    const minX = Number(display.virtual_x ?? primary.x ?? 0);
    const minY = Number(display.virtual_y ?? primary.y ?? 0);
    const layoutWidth = Number(display.layout_width ?? display.primary_width ?? primary.width ?? primary.w ?? 1920);
    const layoutHeight = Number(display.layout_height ?? display.primary_height ?? primary.height ?? primary.h ?? 1080);
    pointer = {
      x: Number(mouse.x || 0),
      y: Number(mouse.y || 0),
      minX: Number.isFinite(minX) ? Math.floor(minX) : 0,
      minY: Number.isFinite(minY) ? Math.floor(minY) : 0,
      width: Math.max(1, Math.floor(Number.isFinite(layoutWidth) ? layoutWidth : 1920)),
      height: Math.max(1, Math.floor(Number.isFinite(layoutHeight) ? layoutHeight : 1080)),
      displayId: displayIdForPointer(displayEntries, Number(mouse.x || 0), Number(mouse.y || 0), mouse.current_display_id || primary.display_id || primary.id || null)
    };
    posEl.textContent = `${pointer.x}, ${pointer.y} / ${pointer.width}x${pointer.height}`;
    statusEl.textContent = "已连接";
  } catch (error) {
    statusEl.textContent = formatMobileError(error, "移动端状态");
  }
}
function sendMoveNow(next) {
  inject(daemonRequest("Mouse", { kind: "MouseMove", data: { x: next.x, y: next.y, display_id: next.displayId } }, cid("mobile-move")));
}
function scheduleMove(next) {
  pendingMove = { ...next };
  if (pendingMoveFrame) return;
  pendingMoveFrame = requestAnimationFrame(() => {
    pendingMoveFrame = 0;
    const next = pendingMove;
    pendingMove = null;
    if (next) sendMoveNow(next);
  });
}
function flushMove() {
  const next = pendingMove;
  pendingMove = null;
  if (pendingMoveFrame) {
    cancelAnimationFrame(pendingMoveFrame);
    pendingMoveFrame = 0;
  }
  if (next) sendMoveNow(next);
}
function isTouchpadTap(start, end) {
  if (!start || !end) return false;
  const duration = end.timeMs - start.timeMs;
  if (duration < 0 || duration > 260) return false;
  return Math.hypot(end.x - start.x, end.y - start.y) <= 12;
}
function isTouchpadLongPressDrag(start, current) {
  if (!start || !current) return false;
  const duration = current.timeMs - start.timeMs;
  if (duration < LONG_PRESS_DRAG_DELAY_MS) return false;
  return Math.hypot(current.x - start.x, current.y - start.y) <= 12;
}
async function sendTapClick() {
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-tap-down")));
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-tap-up")));
}
function clearDragTimer() {
  if (dragTimer) {
    clearTimeout(dragTimer);
    dragTimer = 0;
  }
}
function sendDragButton(state) {
  inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state, x: pointer.x, y: pointer.y } }, cid(`mobile-touchpad-drag-${state}`)));
}
function beginTouchpadDrag(pointerId) {
  dragTimer = 0;
  if (activePointer !== pointerId || dragPointer !== null || touchPoints.size !== 1) return;
  const current = touchPoints.get(pointerId);
  if (!tapStart || !current) return;
  if (!isTouchpadLongPressDrag(tapStart, { ...current, timeMs: tapStart.timeMs + LONG_PRESS_DRAG_DELAY_MS })) return;
  dragPointer = pointerId;
  tapStart = null;
  sendDragButton("Pressed");
}
function releaseTouchpadDrag(pointerId = null) {
  if (dragPointer === null) return;
  if (pointerId !== null && dragPointer !== pointerId) return;
  dragPointer = null;
  sendDragButton("Released");
}
function releaseTouchpadInteraction() {
  clearDragTimer();
  flushMove();
  releaseTouchpadDrag();
  activePointer = null;
  lastPoint = null;
  tapStart = null;
  lastWheelTouches = null;
  twoFingerTapStart = null;
  touchPoints.clear();
}
function releaseTouchpadInteractionWhenHidden() {
  if (document.visibilityState === "hidden") {
    releaseTouchpadInteraction();
  }
}
function touchPointsSnapshot() {
  return Array.from(touchPoints.values()).sort((left, right) => left.id - right.id);
}
function centerOfTouches(touches) {
  return { x: (touches[0].x + touches[1].x) / 2, y: (touches[0].y + touches[1].y) / 2 };
}
function twoFingerWheelDelta(previousTouches, currentTouches) {
  if (!previousTouches || !currentTouches || previousTouches.length !== 2 || currentTouches.length !== 2) return null;
  if (previousTouches[0].id !== currentTouches[0].id || previousTouches[1].id !== currentTouches[1].id) return null;
  const previousCenter = centerOfTouches(previousTouches);
  const currentCenter = centerOfTouches(currentTouches);
  const dx = currentCenter.x - previousCenter.x;
  const dy = currentCenter.y - previousCenter.y;
  if (Math.max(Math.abs(dx), Math.abs(dy)) < 6) return null;
  const wheel = { deltaX: Math.round(dx * 0.12), deltaY: Math.round(dy * 0.12) };
  return wheel.deltaX || wheel.deltaY ? wheel : null;
}
function isTwoFingerTap(startTouches, endTouches, startTimeMs, endTimeMs) {
  if (!startTouches || !endTouches || startTouches.length !== 2 || endTouches.length !== 2) return false;
  if (startTouches[0].id !== endTouches[0].id || startTouches[1].id !== endTouches[1].id) return false;
  const duration = endTimeMs - startTimeMs;
  if (duration < 0 || duration > 260) return false;
  const startCenter = centerOfTouches(startTouches);
  const endCenter = centerOfTouches(endTouches);
  if (Math.hypot(endCenter.x - startCenter.x, endCenter.y - startCenter.y) > 12) return false;
  const startDistance = Math.hypot(startTouches[1].x - startTouches[0].x, startTouches[1].y - startTouches[0].y);
  const endDistance = Math.hypot(endTouches[1].x - endTouches[0].x, endTouches[1].y - endTouches[0].y);
  return Math.abs(endDistance - startDistance) <= 12;
}
function sendWheelDelta(wheel) {
  inject(daemonRequest("Mouse", { kind: "MouseWheel", data: { delta_x: wheel.deltaX, delta_y: wheel.deltaY, x: pointer.x, y: pointer.y } }, cid("mobile-wheel")));
}
async function sendTwoFingerTapClick() {
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Right", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-two-finger-tap-down")));
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Right", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-two-finger-tap-up")));
}
pad.addEventListener("pointerdown", (event) => {
  touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  activePointer = event.pointerId;
  lastPoint = { x: event.clientX, y: event.clientY };
  tapStart = { x: event.clientX, y: event.clientY, timeMs: event.timeStamp };
  pad.setPointerCapture(event.pointerId);
  clearDragTimer();
  dragTimer = setTimeout(() => beginTouchpadDrag(event.pointerId), LONG_PRESS_DRAG_DELAY_MS);
  if (touchPoints.size >= 2) {
    clearDragTimer();
    releaseTouchpadDrag();
    flushMove();
    const touches = touchPoints.size === 2 ? touchPointsSnapshot() : null;
    lastWheelTouches = touches;
    twoFingerTapStart = touches ? { touches, timeMs: event.timeStamp } : null;
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
});
pad.addEventListener("pointermove", (event) => {
  if (touchPoints.has(event.pointerId)) {
    touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  }
  if (touchPoints.size >= 2) {
    clearDragTimer();
    releaseTouchpadDrag();
    if (touchPoints.size > 2) {
      lastWheelTouches = null;
      twoFingerTapStart = null;
      return;
    }
    const currentTouches = touchPointsSnapshot();
    const wheel = twoFingerWheelDelta(lastWheelTouches, currentTouches);
    lastWheelTouches = currentTouches;
    if (wheel) {
      twoFingerTapStart = null;
      sendWheelDelta(wheel);
    }
    return;
  }
  if (activePointer !== event.pointerId || !lastPoint) return;
  if (tapStart && Math.hypot(event.clientX - tapStart.x, event.clientY - tapStart.y) > 12) {
    clearDragTimer();
  }
  const dx = Math.round((event.clientX - lastPoint.x) * pointerSensitivity);
  const dy = Math.round((event.clientY - lastPoint.y) * pointerSensitivity);
  lastPoint = { x: event.clientX, y: event.clientY };
  const maxX = pointer.minX + pointer.width - 1;
  const maxY = pointer.minY + pointer.height - 1;
  pointer = { ...pointer, x: Math.max(pointer.minX, Math.min(maxX, pointer.x + dx)), y: Math.max(pointer.minY, Math.min(maxY, pointer.y + dy)) };
  pointer.displayId = displayIdForPointer(displayEntries, pointer.x, pointer.y, pointer.displayId);
  posEl.textContent = `${pointer.x}, ${pointer.y} / ${pointer.width}x${pointer.height}`;
  scheduleMove(pointer);
});
function clearPointer(event) {
  if (touchPoints.has(event.pointerId)) {
    touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  }
  if (touchPoints.size === 2 && twoFingerTapStart) {
    const start = twoFingerTapStart;
    const currentTouches = touchPointsSnapshot();
    twoFingerTapStart = null;
    if (isTwoFingerTap(start.touches, currentTouches, start.timeMs, event.timeStamp)) {
      sendTwoFingerTapClick();
    }
  }
  if (activePointer === event.pointerId) {
    clearDragTimer();
    flushMove();
    if (dragPointer === event.pointerId) {
      releaseTouchpadDrag(event.pointerId);
    } else if (isTouchpadTap(tapStart, { x: event.clientX, y: event.clientY, timeMs: event.timeStamp })) {
      sendTapClick();
    }
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
  touchPoints.delete(event.pointerId);
  if (touchPoints.size !== 2) {
    lastWheelTouches = null;
    twoFingerTapStart = null;
  }
}
function cancelPointer(event) {
  if (activePointer === event.pointerId) {
    clearDragTimer();
    releaseTouchpadDrag(event.pointerId);
    flushMove();
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
  touchPoints.delete(event.pointerId);
  if (touchPoints.size !== 2) {
    lastWheelTouches = null;
    twoFingerTapStart = null;
  }
}
pad.addEventListener("pointerup", clearPointer);
pad.addEventListener("pointercancel", cancelPointer);
window.addEventListener("blur", releaseTouchpadInteraction);
window.addEventListener("pagehide", releaseTouchpadInteraction);
document.addEventListener("visibilitychange", releaseTouchpadInteractionWhenHidden);
function attachHeldButton(button, sendState) {
  let activePointer = null;
  function release(force, pointerId = null) {
    if (activePointer === null) return;
    if (!force && pointerId !== null && activePointer !== pointerId) return;
    activePointer = null;
    sendState("Released");
  }
  button.addEventListener("pointerdown", (event) => {
    release(true);
    activePointer = event.pointerId;
    button.setPointerCapture(event.pointerId);
    sendState("Pressed");
  });
  button.addEventListener("pointerup", (event) => release(false, event.pointerId));
  button.addEventListener("pointercancel", (event) => release(false, event.pointerId));
  button.addEventListener("pointerleave", (event) => {
    if (event.buttons) release(false, event.pointerId);
  });
  window.addEventListener("blur", () => release(true));
  window.addEventListener("pagehide", () => release(true));
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") release(true);
  });
}
document.querySelectorAll("[data-button]").forEach((button) => {
  const name = button.dataset.button;
  const sendButton = (state) => inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: name, state, x: pointer.x, y: pointer.y } }, cid(`mobile-${name}-${state}`)));
  attachHeldButton(button, (state) => {
    if (state === "Pressed") return sendButton("Pressed");
    return sendButton("Released");
  });
});
document.querySelectorAll("[data-wheel]").forEach((button) => button.addEventListener("click", () => {
  inject(daemonRequest("Mouse", { kind: "MouseWheel", data: { delta_x: 0, delta_y: Number(button.dataset.wheel), x: pointer.x, y: pointer.y } }, cid("mobile-wheel")));
}));
function sendKeyState(button, state) {
  const key = button.dataset.key;
  return inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state } }, cid(`mobile-${key}-${state}`), "RequireHealthyBackend", 750));
}
async function sendKeyChord(keys) {
  for (const key of keys) {
    await inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Pressed" } }, cid(`mobile-shortcut-${key}-down`), "RequireHealthyBackend", 750));
  }
  for (const key of [...keys].reverse()) {
    await inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Released" } }, cid(`mobile-shortcut-${key}-up`), "RequireHealthyBackend", 750));
  }
}
document.querySelectorAll("[data-key]").forEach((button) => {
  attachHeldButton(button, (state) => {
    if (state === "Pressed") return sendKeyState(button, "Pressed");
    return sendKeyState(button, "Released");
  });
});
document.querySelectorAll("[data-shortcut]").forEach((button) => button.addEventListener("click", () => {
  const keys = String(button.dataset.shortcut || "").split(",").filter(Boolean);
  sendKeyChord(keys);
}));
async function sendText() {
  const text = textInput.value;
  if (!text) return;
  if (await inject(daemonRequest("Keyboard", { kind: "TextCommit", data: { text } }, cid("mobile-text"), "RequireHealthyBackend", 750))) {
    textInput.value = "";
  }
}
function shouldSendTextOnKeydown(event) {
  return event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229;
}
document.getElementById("send").addEventListener("click", sendText);
textInput.addEventListener("keydown", (event) => { if (shouldSendTextOnKeydown(event)) { event.preventDefault(); sendText(); } });
refresh();
setInterval(refresh, 1500);
</script>
</body>
</html>
"#
    .replace("__MOBILE_MANIFEST_HREF__", &manifest_href)
    .replace("__MOBILE_ICON_HREF__", &icon_href)
    .replace("__MOBILE_TOKEN_JSON__", &token_json)
}

fn mobile_token_query(token: &str) -> String {
    if token.is_empty() {
        String::new()
    } else {
        format!("?t={token}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{
        BackendHealth, BackendKind, DeviceId, EndpointEventKind, EndpointEventPayload,
        EndpointInjectMode, EndpointInjectRequest, ServiceStatusSnapshot,
    };
    use rshare_input::InputEvent;
    use std::fmt;
    use tokio::time::{sleep, timeout, Duration};

    struct NoopInjectBackend;

    impl fmt::Debug for NoopInjectBackend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("NoopInjectBackend").finish()
        }
    }

    impl InjectBackend for NoopInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, _event: InputEvent) -> Result<()> {
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[test]
    fn decodes_mobile_daemon_inject_request_as_local_only() {
        let request = DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Local,
            request: EndpointInjectRequest {
                correlation_id: "mobile-text-1".to_string(),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::TextCommit {
                    text: "你好".to_string(),
                },
                mode: EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        };
        let body = serde_json::to_vec(&request).unwrap();

        let decoded = mobile_inject_request_from_body(&body).unwrap();

        assert!(matches!(
            decoded.payload,
            EndpointEventPayload::TextCommit { text } if text == "你好"
        ));
    }

    #[test]
    fn rejects_mobile_remote_inject_request() {
        let request = DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Remote(DeviceId::nil()),
            request: EndpointInjectRequest {
                correlation_id: "mobile-remote-1".to_string(),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::Keyboard {
                    key: "Enter".to_string(),
                    state: "Pressed".to_string(),
                },
                mode: EndpointInjectMode::BestEffort,
                timeout_ms: 250,
            },
        };
        let body = serde_json::to_vec(&request).unwrap();

        let error = mobile_inject_request_from_body(&body).unwrap_err();

        assert!(error.to_string().contains("local endpoint"));
    }

    #[test]
    fn rendered_mobile_page_coalesces_pointer_moves_and_flushes_on_release() {
        let page = render_mobile_page();

        assert!(page.contains("function scheduleMove(next)"));
        assert!(page.contains("requestAnimationFrame"));
        assert!(page.contains("function flushMove()"));
        assert!(page.contains("cancelAnimationFrame"));
        assert!(page.contains("scheduleMove(pointer);"));
        assert!(page.contains("flushMove();"));
    }

    #[test]
    fn rendered_mobile_page_turns_short_touchpad_taps_into_left_clicks() {
        let page = render_mobile_page();

        assert!(page.contains("function isTouchpadTap"));
        assert!(page.contains("function sendTapClick()"));
        assert!(page.contains("sendTapClick();"));
        assert!(page.contains("pad.addEventListener(\"pointercancel\", cancelPointer);"));
        assert!(page.contains("button: \"Left\", state: \"Pressed\""));
        assert!(page.contains("button: \"Left\", state: \"Released\""));
    }

    #[test]
    fn rendered_mobile_page_supports_two_finger_wheel_gestures() {
        let page = render_mobile_page();

        assert!(page.contains("function twoFingerWheelDelta"));
        assert!(page.contains("function touchPointsSnapshot"));
        assert!(page.contains("sendWheelDelta(wheel);"));
        assert!(page.contains("kind: \"MouseWheel\""));
        assert!(page.contains("lastWheelTouches ="));
        assert!(page.contains("touchPointsSnapshot()"));
    }

    #[test]
    fn rendered_mobile_page_turns_two_finger_taps_into_right_clicks() {
        let page = render_mobile_page();

        assert!(page.contains("function isTwoFingerTap"));
        assert!(page.contains("function sendTwoFingerTapClick()"));
        assert!(page.contains("sendTwoFingerTapClick();"));
        assert!(page.contains("button: \"Right\", state: \"Pressed\""));
        assert!(page.contains("button: \"Right\", state: \"Released\""));
        assert!(page.contains("twoFingerTapStart"));
    }

    #[test]
    fn rendered_mobile_page_respects_ime_composition_before_text_commit() {
        let page = render_mobile_page();

        assert!(page.contains("<link rel='manifest' href='/mobile.webmanifest'>"));
        assert!(page.contains("<link rel='icon' href='/mobile-icon.svg'"));
        assert!(page.contains("<link rel='apple-touch-icon' href='/mobile-icon.svg'>"));
        assert!(page.contains("mobile-web-app-capable"));
        assert!(page.contains("apple-mobile-web-app-capable"));
        assert!(page.contains("theme-color"));
        assert!(page.contains("enterkeyhint=\"send\""));
        assert!(page.contains("function shouldSendTextOnKeydown"));
        assert!(page.contains("event.isComposing"));
        assert!(page.contains("event.keyCode !== 229"));
        assert!(page.contains("if (shouldSendTextOnKeydown(event))"));
    }

    #[test]
    fn rendered_mobile_page_uses_multiline_textarea_for_ime_input() {
        let page = render_mobile_page();

        assert!(page.contains("<textarea id=\"text\""));
        assert!(page.contains("rows=\"3\""));
        assert!(page.contains("!event.shiftKey"));
        assert!(!page.contains("<input id=\"text\""));
    }

    #[test]
    fn rendered_mobile_page_scopes_install_assets_to_the_mobile_token() {
        let page = render_mobile_page_with_token("mobile-secret");

        assert!(page.contains("<link rel='manifest' href='/mobile.webmanifest?t=mobile-secret'>"));
        assert!(page.contains("<link rel='icon' href='/mobile-icon.svg?t=mobile-secret'"));
        assert!(
            page.contains("<link rel='apple-touch-icon' href='/mobile-icon.svg?t=mobile-secret'>")
        );
        assert!(page.contains("const token = \"mobile-secret\" ||"));
        assert!(!page.contains("href='/mobile-icon.svg'"));
    }

    #[test]
    fn rendered_mobile_page_hides_raw_fetch_failures_and_keeps_text_on_failure() {
        let page = render_mobile_page();

        assert!(page.contains("function formatMobileError"));
        assert!(page.contains("failed to fetch|networkerror|fetch failed|load failed"));
        assert!(page.contains("网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络"));
        assert!(page.contains("return false;"));
        assert!(page.contains("if (await inject"));
        assert!(page.contains("textInput.value = \"\";"));
    }

    #[test]
    fn rendered_mobile_page_holds_keyboard_buttons_until_pointer_release() {
        let page = render_mobile_page();

        assert!(page.contains("function sendKeyState"));
        assert!(page.contains("data-key"));
        assert!(page.contains("button.addEventListener(\"pointerdown\""));
        assert!(page.contains("button.addEventListener(\"pointerup\""));
        assert!(page.contains("button.addEventListener(\"pointercancel\""));
        assert!(page.contains("sendKeyState(button, \"Pressed\")"));
        assert!(page.contains("sendKeyState(button, \"Released\")"));
    }

    #[test]
    fn rendered_mobile_page_exposes_common_keyboard_keys_and_shortcuts() {
        let page = render_mobile_page();

        for key in ["ControlLeft", "ShiftLeft", "AltLeft", "SuperLeft"] {
            assert!(page.contains(&format!("data-key=\"{key}\"")));
        }
        for key in [
            "Escape", "Tab", "Space", "Delete", "Home", "End", "PageUp", "PageDown",
        ] {
            assert!(page.contains(&format!("data-key=\"{key}\"")));
        }
        for shortcut in [
            "ControlLeft,C",
            "ControlLeft,V",
            "ControlLeft,X",
            "ControlLeft,A",
        ] {
            assert!(page.contains(&format!("data-shortcut=\"{shortcut}\"")));
        }
        assert!(page.contains("async function sendKeyChord(keys)"));
        assert!(page.contains("for (const key of keys)"));
        assert!(page.contains("for (const key of [...keys].reverse())"));
        assert!(page.contains("overflow: auto"));
        assert!(page.contains("min-height: 100dvh"));
    }

    #[test]
    fn renders_mobile_manifest_with_token_scoped_start_url_and_icon() {
        let manifest = render_mobile_manifest("/mobile.webmanifest?t=mobile-secret").unwrap();

        assert_eq!(manifest["name"], "R-ShareMouse Mobile");
        assert_eq!(manifest["short_name"], "R-ShareMouse");
        assert_eq!(manifest["start_url"], "/mobile?t=mobile-secret");
        assert_eq!(manifest["scope"], "/");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["orientation"], "portrait");
        assert_eq!(manifest["theme_color"], "#101214");
        assert_eq!(manifest["background_color"], "#101214");
        assert_eq!(
            manifest["icons"][0]["src"],
            "/mobile-icon.svg?t=mobile-secret"
        );
        assert_eq!(manifest["icons"][0]["type"], "image/svg+xml");
    }

    #[test]
    fn renders_mobile_icon_svg_for_pwa_installation() {
        let icon = render_mobile_icon_svg();

        assert!(icon.contains("<svg"));
        assert!(icon.contains("R-ShareMouse"));
        assert!(icon.contains("#47c27a"));
    }

    #[test]
    fn rendered_mobile_page_releases_held_buttons_when_pointer_or_page_cancels() {
        let page = render_mobile_page();

        assert!(page.contains("function attachHeldButton"));
        assert!(page.contains("button.addEventListener(\"pointerleave\""));
        assert!(page.contains("window.addEventListener(\"blur\""));
        assert!(page.contains("window.addEventListener(\"pagehide\""));
        assert!(page.contains("document.addEventListener(\"visibilitychange\""));
        assert!(page.contains("document.visibilityState === \"hidden\""));
        assert!(page.contains("release(true);"));
    }

    #[test]
    fn rendered_mobile_page_supports_touchpad_long_press_drag() {
        let page = render_mobile_page();

        assert!(page.contains("const LONG_PRESS_DRAG_DELAY_MS"));
        assert!(page.contains("function beginTouchpadDrag"));
        assert!(page.contains("function clearDragTimer"));
        assert!(page.contains("dragTimer = setTimeout"));
        assert!(page.contains("sendDragButton(\"Pressed\")"));
        assert!(page.contains("sendDragButton(\"Released\")"));
    }

    #[test]
    fn rendered_mobile_page_exposes_persistent_touchpad_sensitivity() {
        let page = render_mobile_page();

        assert!(page.contains("const POINTER_SENSITIVITY_STORAGE_KEY"));
        assert!(page.contains("function clampPointerSensitivity"));
        assert!(page.contains("let pointerSensitivity = clampPointerSensitivity"));
        assert!(page.contains("<input id=\"sensitivity\" type=\"range\""));
        assert!(page.contains("aria-label=\"触控板灵敏度\""));
        assert!(page.contains("localStorage.getItem(POINTER_SENSITIVITY_STORAGE_KEY)"));
        assert!(page.contains("localStorage.setItem(POINTER_SENSITIVITY_STORAGE_KEY"));
        assert!(page.contains("event.clientX - lastPoint.x) * pointerSensitivity"));
        assert!(page.contains("event.clientY - lastPoint.y) * pointerSensitivity"));
    }

    #[test]
    fn rendered_mobile_page_uses_virtual_desktop_bounds_for_pointer_movement() {
        let page = render_mobile_page();

        assert!(page.contains("display.virtual_x"));
        assert!(page.contains("display.virtual_y"));
        assert!(page.contains("display.layout_width"));
        assert!(page.contains("display.layout_height"));
        assert!(page.contains("minX"));
        assert!(page.contains("minY"));
    }

    #[test]
    fn rendered_mobile_page_updates_display_id_from_pointer_coordinates() {
        let page = render_mobile_page();

        assert!(page.contains("function displayIdForPointer"));
        assert!(page.contains("display.display_id || display.id"));
        assert!(page.contains("pointer.displayId = displayIdForPointer"));
    }

    #[test]
    fn rendered_mobile_page_releases_touchpad_drag_when_page_lifecycle_cancels_input() {
        let page = render_mobile_page();

        assert!(page.contains("function releaseTouchpadInteraction()"));
        assert!(page.contains("releaseTouchpadDrag();"));
        assert!(page.contains("window.addEventListener(\"blur\", releaseTouchpadInteraction);"));
        assert!(page.contains("window.addEventListener(\"pagehide\", releaseTouchpadInteraction);"));
        assert!(page.contains(
            "document.addEventListener(\"visibilitychange\", releaseTouchpadInteractionWhenHidden);"
        ));
        assert!(page.contains("document.visibilityState === \"hidden\""));
    }

    #[test]
    fn rendered_mobile_page_prevents_browser_navigation_and_default_gestures() {
        let page = render_mobile_page();

        assert!(page.contains("function shouldPreventBrowserNavigationEvent(event)"));
        assert!(page.contains("function preventBrowserNavigationEvent(event)"));
        assert!(page.contains("function preventMobileGestureDefault(event)"));
        assert!(
            page.contains("target.closest(\"input, textarea, select, [contenteditable='true']\")")
        );
        assert!(page.contains("[\"mousedown\", \"mouseup\", \"auxclick\", \"pointerdown\", \"pointerup\", \"keydown\"]"));
        assert!(page.contains("[\"contextmenu\", \"dragstart\", \"selectstart\", \"gesturestart\", \"gesturechange\", \"gestureend\"]"));
        assert!(page.contains("window.addEventListener(eventName, preventBrowserNavigationEvent, mobileEventOptions);"));
        assert!(page.contains("document.addEventListener(eventName, preventMobileGestureDefault, mobileEventOptions);"));
        assert!(page.contains("touch-action: manipulation"));
        assert!(page.contains("-webkit-touch-callout: none"));
    }

    #[test]
    fn disabled_mobile_access_snapshot_does_not_expose_a_token_link() {
        let access = MobileGatewayAccess::disabled("mobile port is already in use".to_string());

        let snapshot = access.snapshot();

        assert!(!snapshot.enabled);
        assert_eq!(snapshot.bind_address, "不可用");
        assert_eq!(snapshot.page_url, "不可用");
        assert_eq!(snapshot.token, "");
    }

    #[tokio::test]
    async fn bind_failure_marks_mobile_access_disabled_without_failing_daemon() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = occupied.local_addr().unwrap();
        let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))));
        let access =
            MobileGatewayAccess::new(addr, "mobile-secret".to_string(), "127.0.0.1".to_string());
        {
            let mut state = state.write().await;
            state.mobile_access = access.clone();
        }
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
        )));
        let inject_backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(NoopInjectBackend)));
        let (local_events_tx, _) = broadcast::channel(1);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let task = tokio::spawn(run_mobile_gateway_server(
            access,
            state.clone(),
            network_manager,
            inject_backend,
            local_events_tx,
            shutdown_rx,
        ));

        timeout(Duration::from_secs(1), async {
            loop {
                if !state.read().await.mobile_access.snapshot().enabled {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let snapshot = state.read().await.mobile_access.snapshot();
        assert!(!snapshot.enabled);
        assert_eq!(snapshot.page_url, "不可用");

        let _ = shutdown_tx.send(());
        let result = timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok());
    }
}
