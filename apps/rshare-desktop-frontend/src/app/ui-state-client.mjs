const UI_STATE_EVENT = "rshare://ui-state";
const UI_STATE_PROTOCOL_VERSION = 1;
const RECONNECT_DELAYS_MS = Object.freeze([100, 250, 500, 1000]);
const FALLBACK_DELAY_MS = 1000;
const FALLBACK_POLL_INTERVAL_MS = 2000;
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const RESYNC_REASONS = new Set([
  "boot_changed",
  "revision_gap",
  "history_expired",
  "projection_rebuilt",
]);
const OBJECT_CHANGES = new Set([
  "status",
  "capabilities",
  "device_upsert",
  "topology",
  "display_inventory",
  "session",
  "diagnostics",
  "media_session_upsert",
]);

function isRecord(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function isRevision(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function isUuid(value) {
  return typeof value === "string" && UUID_PATTERN.test(value);
}

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function validPointer(value) {
  return (
    isRecord(value) &&
    Number.isSafeInteger(value.x) &&
    Number.isSafeInteger(value.y) &&
    isRevision(value.observed_at_ms) &&
    (!hasOwn(value, "display_id") ||
      value.display_id === null ||
      typeof value.display_id === "string")
  );
}

function validMouseButton(value) {
  if (
    typeof value === "string" &&
    ["Left", "Middle", "Right", "Back", "Forward"].includes(value)
  ) {
    return true;
  }
  if (!isRecord(value) || Object.keys(value).length !== 1 || !hasOwn(value, "Other")) {
    return false;
  }
  return (
    Number.isInteger(value.Other) &&
    value.Other >= 0 &&
    value.Other <= 255
  );
}

function validDiscreteInput(value) {
  if (!isRecord(value) || !isRevision(value.observed_at_ms)) {
    return false;
  }
  switch (value.type) {
    case "key":
      return Number.isSafeInteger(value.key_code) && typeof value.state === "string";
    case "mouse_button":
      return validMouseButton(value.button) && typeof value.state === "string";
    case "wheel":
      return Number.isSafeInteger(value.delta_x) && Number.isSafeInteger(value.delta_y);
    case "gamepad_button":
      return (
        Number.isSafeInteger(value.gamepad_id) &&
        (typeof value.button === "string" || isRecord(value.button)) &&
        typeof value.state === "string"
      );
    default:
      return false;
  }
}

function validChange(change) {
  if (!isRecord(change) || typeof change.type !== "string" || !hasOwn(change, "payload")) {
    return false;
  }
  if (OBJECT_CHANGES.has(change.type)) {
    return isRecord(change.payload);
  }
  switch (change.type) {
    case "device_remove":
    case "media_session_remove":
      return isUuid(change.payload);
    case "pointer":
      return validPointer(change.payload);
    case "gamepads":
      return (
        Array.isArray(change.payload) &&
        change.payload.every((gamepad) => isRecord(gamepad))
      );
    case "key_button":
      return validDiscreteInput(change.payload);
    default:
      return false;
  }
}

function validSnapshot(payload) {
  return (
    isRecord(payload) &&
    payload.protocol_version === UI_STATE_PROTOCOL_VERSION &&
    isUuid(payload.boot_id) &&
    isRevision(payload.revision) &&
    isRecord(payload.status) &&
    Array.isArray(payload.devices) &&
    payload.devices.every((device) => isRecord(device)) &&
    isRecord(payload.layout) &&
    isRecord(payload.capabilities) &&
    isRecord(payload.display_inventory) &&
    isRecord(payload.dynamic_state) &&
    isRecord(payload.active_sessions)
  );
}

function validEnvelope(envelope) {
  if (!isRecord(envelope) || typeof envelope.type !== "string" || !isRecord(envelope.payload)) {
    return false;
  }
  const payload = envelope.payload;
  switch (envelope.type) {
    case "snapshot":
      return validSnapshot(payload);
    case "delta":
      return (
        isUuid(payload.boot_id) &&
        isRevision(payload.revision) &&
        validChange(payload.change)
      );
    case "heartbeat":
      return (
        isUuid(payload.boot_id) &&
        isRevision(payload.revision) &&
        isRevision(payload.sent_at_ms)
      );
    case "resync_required":
      return (
        isUuid(payload.boot_id) &&
        isRevision(payload.current_revision) &&
        RESYNC_REASONS.has(payload.reason)
      );
    default:
      return false;
  }
}

function safeCall(callback, value) {
  try {
    return callback(value);
  } catch {
    return undefined;
  }
}

function abortError() {
  const error = new Error("UI state connection attempt was aborted");
  error.name = "AbortError";
  return error;
}

function ownerStreamId(value) {
  if (typeof value === "number" || typeof value === "string") {
    return value;
  }
  if (isRecord(value)) {
    return value.stream_id ?? value.streamId;
  }
  return undefined;
}

export function createTauriUiStateConnector({ invoke, listen }) {
  if (typeof invoke !== "function" || typeof listen !== "function") {
    throw new TypeError("Tauri UI state connector requires invoke and listen");
  }
  return async ({ cursor, onEnvelope, onDisconnect, signal }) => {
    let closed = false;
    let started = false;
    let streamId;
    let resyncGeneration = 0;
    let unlisten = () => {};
    let closePromise = null;

    const stopOwner = async (owner) => {
      if (owner !== undefined && owner !== null) {
        await invoke("stop_ui_state_stream", { streamId: owner });
      }
    };
    const close = () => {
      if (closePromise) return closePromise;
      closed = true;
      resyncGeneration += 1;
      signal?.removeEventListener("abort", abort);
      unlisten();
      closePromise = started ? stopOwner(streamId) : Promise.resolve();
      return closePromise;
    };
    const abort = () => {
      void close().catch(onDisconnect);
    };
    signal?.addEventListener("abort", abort, { once: true });

    try {
      unlisten = await listen(UI_STATE_EVENT, (event) => {
        onEnvelope(event?.payload);
      });
      if (closed || signal?.aborted) {
        throw abortError();
      }

      const reserved = ownerStreamId(
        await invoke("reserve_ui_state_stream"),
      );
      if (reserved === undefined) {
        throw new Error("reserve_ui_state_stream did not return an owner stream id");
      }
      if (closed || signal?.aborted) {
        throw abortError();
      }

      await invoke("start_ui_state_stream", {
        cursor,
        streamId: reserved,
      });
      started = true;
      streamId = reserved;
      if (closed || signal?.aborted) {
        await stopOwner(reserved);
        throw abortError();
      }
    } catch (error) {
      signal?.removeEventListener("abort", abort);
      unlisten();
      throw error;
    }

    return {
      close,
      async requestFullResync() {
        if (closed) return;
        const generation = ++resyncGeneration;
        const nextOwner = ownerStreamId(
          await invoke("reserve_ui_state_stream"),
        );
        if (nextOwner === undefined) {
          throw new Error("full UI resync reservation did not return an owner stream id");
        }
        if (closed || generation !== resyncGeneration || signal?.aborted) {
          return;
        }

        await invoke("start_ui_state_stream", {
          cursor: null,
          streamId: nextOwner,
        });
        if (closed || generation !== resyncGeneration || signal?.aborted) {
          await stopOwner(nextOwner);
          return;
        }
        streamId = nextOwner;
      },
    };
  };
}

export function createWebSocketUiStateConnector({ WebSocket, url }) {
  if (typeof WebSocket !== "function" || typeof url !== "string") {
    throw new TypeError("WebSocket UI state connector requires WebSocket and url");
  }
  return ({ cursor, onEnvelope, onDisconnect, signal }) =>
    new Promise((resolve, reject) => {
      const socket = new WebSocket(url);
      let opened = false;
      let closed = false;
      let disconnectReported = false;

      const cleanupAbort = () => signal?.removeEventListener("abort", abort);
      const reportDisconnect = (error) => {
        if (closed || disconnectReported) return;
        disconnectReported = true;
        if (opened) {
          onDisconnect(error);
        } else {
          cleanupAbort();
          reject(error);
        }
      };
      const close = () => {
        if (closed) return;
        closed = true;
        cleanupAbort();
        socket.close(1000);
      };
      const abort = () => {
        close();
        if (!opened) reject(abortError());
      };
      signal?.addEventListener("abort", abort, { once: true });

      socket.addEventListener("open", () => {
        if (closed || signal?.aborted) {
          close();
          reject(abortError());
          return;
        }
        opened = true;
        socket.send(JSON.stringify({ type: "subscribe", cursor }));
        resolve({
          close,
          requestFullResync() {
            if (!closed && socket.readyState === WebSocket.OPEN) {
              socket.send(JSON.stringify({ type: "resync" }));
            }
          },
        });
      });
      socket.addEventListener("message", (event) => {
        try {
          if (typeof event.data !== "string") {
            throw new Error("UI state WebSocket returned a non-text envelope");
          }
          onEnvelope(JSON.parse(event.data));
        } catch (error) {
          reportDisconnect(error);
        }
      });
      socket.addEventListener("error", () => {
        reportDisconnect(new Error("UI state WebSocket is unavailable"));
      });
      socket.addEventListener("close", (event) => {
        if (!closed) {
          reportDisconnect(
            new Error(
              event.reason ||
                `UI state WebSocket closed (${event.code || "unknown"})`,
            ),
          );
        }
      });
    });
}

/**
 * Owns ordering, liveness, resync and fallback independently of transport.
 */
export class UiStateClient {
  constructor({
    connect,
    onEnvelope,
    onStatus,
    heartbeatTimeoutMs = 12000,
  }) {
    if (typeof connect !== "function") {
      throw new TypeError("UiStateClient connect must be a function");
    }
    if (typeof onEnvelope !== "function") {
      throw new TypeError("UiStateClient onEnvelope must be a function");
    }
    if (typeof onStatus !== "function") {
      throw new TypeError("UiStateClient onStatus must be a function");
    }
    if (!Number.isFinite(heartbeatTimeoutMs) || heartbeatTimeoutMs <= 0) {
      throw new RangeError("heartbeatTimeoutMs must be positive");
    }

    this.connect = connect;
    this.onEnvelope = onEnvelope;
    this.onStatus = onStatus;
    this.heartbeatTimeoutMs = heartbeatTimeoutMs;
    this.running = false;
    this.connection = null;
    this.connectionController = null;
    this.pendingAttempt = null;
    this.connectionEpoch = 0;
    this.lifecycleGeneration = 0;
    this.retryIndex = 0;
    this.bootId = null;
    this.revision = 0;
    this.snapshot = null;
    this.status = null;
    this.resyncRequested = false;
    this.resyncPending = false;
    this.reconnectTimer = null;
    this.heartbeatTimer = null;
    this.fallbackTimer = null;
    this.fallbackGeneration = 0;
  }

  async start() {
    if (this.running) return;
    this.lifecycleGeneration += 1;
    this.running = true;
    this.attemptConnect();
    await Promise.resolve();
  }

  async stop() {
    if (!this.running && !this.connection && !this.pendingAttempt) return;
    const lifecycleGeneration = ++this.lifecycleGeneration;
    this.running = false;
    this.clearReconnectTimer();
    this.clearHeartbeatTimer();
    this.stopFallback();
    const connection = this.detachTransport();
    await Promise.resolve(connection?.close?.()).catch(() => {});
    if (
      lifecycleGeneration === this.lifecycleGeneration &&
      !this.running
    ) {
      this.setStatus("stopped");
    }
  }

  currentRevision() {
    return this.revision;
  }

  handleEnvelope(envelope, epoch = this.connectionEpoch) {
    if (!this.running) return false;
    if (!validEnvelope(envelope)) {
      this.requestFullResync("invalid_envelope");
      return false;
    }

    this.markAlive(epoch);
    const payload = envelope.payload;
    if (envelope.type === "snapshot") {
      this.bootId = payload.boot_id;
      this.revision = payload.revision;
      this.snapshot = payload;
      this.retryIndex = 0;
      this.resyncRequested = false;
      this.resyncPending = false;
      this.stopFallback();
      this.setStatus("healthy");
      safeCall(this.onEnvelope, envelope);
      return true;
    }

    if (envelope.type === "delta") {
      const isNextRevision =
        payload.boot_id === this.bootId && payload.revision === this.revision + 1;
      if (!isNextRevision) {
        this.requestFullResync(
          payload.boot_id !== this.bootId ? "boot_changed" : "revision_gap",
        );
        return false;
      }
      this.revision = payload.revision;
      this.retryIndex = 0;
      this.setStatus("healthy");
      safeCall(this.onEnvelope, envelope);
      return true;
    }

    if (envelope.type === "heartbeat") {
      if (this.bootId === null) {
        this.requestFullResync("missing_snapshot");
      } else {
        const bootChanged = payload.boot_id !== this.bootId;
        const serverIsAhead =
          payload.boot_id === this.bootId && payload.revision > this.revision;
        if (bootChanged || serverIsAhead) {
          this.requestFullResync(bootChanged ? "boot_changed" : "revision_gap");
        } else if (!this.resyncRequested) {
          this.retryIndex = 0;
          this.setStatus("healthy");
        }
      }
      safeCall(this.onEnvelope, envelope);
      return true;
    }

    this.requestFullResync(payload.reason);
    safeCall(this.onEnvelope, envelope);
    return true;
  }

  attemptConnect() {
    if (!this.running) return;
    const epoch = ++this.connectionEpoch;
    const controller = new AbortController();
    this.pendingAttempt = { epoch, controller };
    this.setStatus(this.status === null ? "connecting" : "reconnecting");
    this.armWatchdog(epoch);

    let attempt;
    try {
      const cursor =
        this.resyncRequested || this.bootId === null
          ? null
          : { boot_id: this.bootId, revision: this.revision };
      attempt = Promise.resolve(
        this.connect({
          cursor,
          signal: controller.signal,
          onEnvelope: (envelope) => {
            if (this.running && epoch === this.connectionEpoch) {
              this.handleEnvelope(envelope, epoch);
            }
          },
          onDisconnect: (error) => {
            if (this.running && epoch === this.connectionEpoch) {
              this.handleDisconnect(error);
            }
          },
        }),
      );
    } catch (error) {
      attempt = Promise.reject(error);
    }

    attempt.then(
      (connection) => this.finishConnect(epoch, controller, connection),
      (error) => {
        if (this.running && epoch === this.connectionEpoch) {
          this.handleDisconnect(error);
        }
      },
    );
  }

  async finishConnect(epoch, controller, connection) {
    if (!this.running || epoch !== this.connectionEpoch) {
      await Promise.resolve(connection?.close?.()).catch(() => {});
      return;
    }
    this.pendingAttempt = null;
    this.connection = connection ?? {};
    this.connectionController = controller;
    if (this.status !== "healthy") this.setStatus("connected");
    if (this.resyncPending) {
      this.resyncPending = false;
      this.sendFullResyncRequest();
    }
  }

  detachTransport() {
    const connection = this.connection;
    const controller =
      this.connectionController ?? this.pendingAttempt?.controller ?? null;
    this.connection = null;
    this.connectionController = null;
    this.pendingAttempt = null;
    this.connectionEpoch += 1;
    controller?.abort();
    return connection;
  }

  handleDisconnect(error) {
    if (!this.running) return;
    const connection = this.detachTransport();
    Promise.resolve(connection?.close?.()).catch(() => {});
    this.clearHeartbeatTimer();
    this.setStatus("disconnected", { error });
    this.startFallbackAfterDelay("disconnected");
    this.scheduleReconnect();
  }

  scheduleReconnect() {
    if (!this.running || this.reconnectTimer !== null) return;
    const retryDelayMs =
      RECONNECT_DELAYS_MS[
        Math.min(this.retryIndex, RECONNECT_DELAYS_MS.length - 1)
      ];
    this.retryIndex += 1;
    this.setStatus("reconnecting", { retryDelayMs });
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.attemptConnect();
    }, retryDelayMs);
  }

  armWatchdog(epoch) {
    this.clearHeartbeatTimer();
    this.heartbeatTimer = setTimeout(() => {
      this.heartbeatTimer = null;
      if (!this.running || epoch !== this.connectionEpoch) return;
      this.setStatus("stale");
      this.startFallbackAfterDelay("stale");
      void this.reconnectStaleTransport();
    }, this.heartbeatTimeoutMs);
  }

  markAlive(epoch) {
    this.armWatchdog(epoch);
  }

  async reconnectStaleTransport() {
    const connection = this.detachTransport();
    await Promise.resolve(connection?.close?.()).catch(() => {});
    if (this.running) {
      this.clearReconnectTimer();
      this.scheduleReconnect();
    }
  }

  requestFullResync(reason) {
    if (this.resyncRequested) return;
    this.resyncRequested = true;
    this.setStatus("resyncing", { reason });
    if (this.connection) {
      this.sendFullResyncRequest();
    } else {
      this.resyncPending = true;
    }
  }

  sendFullResyncRequest() {
    const connection = this.connection;
    const epoch = this.connectionEpoch;
    const request = connection?.requestFullResync;
    if (typeof request === "function") {
      Promise.resolve(request.call(connection)).catch((error) => {
        if (
          this.running &&
          epoch === this.connectionEpoch &&
          connection === this.connection
        ) {
          this.handleDisconnect(error);
        }
      });
      return;
    }

    this.resyncPending = false;
    const oldConnection = this.detachTransport();
    Promise.resolve(oldConnection?.close?.()).catch(() => {});
    this.clearReconnectTimer();
    this.scheduleReconnect();
  }

  startFallbackAfterDelay(reason) {
    if (!this.running || this.fallbackTimer !== null || this.status === "healthy") {
      return;
    }
    const generation = this.fallbackGeneration;
    this.fallbackTimer = setTimeout(() => {
      this.fallbackTimer = null;
      void this.runFallbackPoll(reason, generation);
    }, FALLBACK_DELAY_MS);
  }

  async runFallbackPoll(reason, generation) {
    if (
      !this.running ||
      generation !== this.fallbackGeneration ||
      this.status === "healthy"
    ) {
      return;
    }
    const result = safeCall(
      this.onStatus,
      this.statusValue("fallback_poll", { reason }),
    );
    try {
      const envelope = await Promise.resolve(result);
      if (envelope && typeof envelope === "object") {
        this.handleEnvelope(envelope);
      }
    } catch {
      // Retain the last snapshot and retry at low frequency.
    }
    if (
      this.running &&
      generation === this.fallbackGeneration &&
      this.status !== "healthy"
    ) {
      this.fallbackTimer = setTimeout(() => {
        this.fallbackTimer = null;
        void this.runFallbackPoll(reason, generation);
      }, FALLBACK_POLL_INTERVAL_MS);
    }
  }

  stopFallback() {
    this.fallbackGeneration += 1;
    if (this.fallbackTimer !== null) {
      clearTimeout(this.fallbackTimer);
      this.fallbackTimer = null;
    }
  }

  clearReconnectTimer() {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  clearHeartbeatTimer() {
    if (this.heartbeatTimer !== null) {
      clearTimeout(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  statusValue(state, details = {}) {
    return {
      state,
      cursor:
        this.bootId === null
          ? null
          : { boot_id: this.bootId, revision: this.revision },
      snapshot: this.snapshot,
      ...details,
    };
  }

  setStatus(state, details = {}) {
    if (this.status === state && Object.keys(details).length === 0) return;
    this.status = state;
    safeCall(this.onStatus, this.statusValue(state, details));
  }
}
