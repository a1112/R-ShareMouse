const defaultRequestAnimationFrame = (callback) => {
  if (typeof globalThis.requestAnimationFrame === "function") {
    return globalThis.requestAnimationFrame(callback);
  }
  return globalThis.setTimeout(() => callback(Date.now()), 16);
};

const defaultCancelAnimationFrame = (handle) => {
  if (typeof globalThis.cancelAnimationFrame === "function") {
    globalThis.cancelAnimationFrame(handle);
  } else {
    globalThis.clearTimeout(handle);
  }
};

const EMPTY_LAYOUT = Object.freeze({ version: 0, nodes: [], links: [] });
const EMPTY_INPUT = Object.freeze({
  pointer: null,
  gamepads: [],
  pressedKeys: [],
  pressedMouseButtons: [],
  pressedGamepadButtons: [],
  lastDiscreteTransition: null,
});

function initialState() {
  return {
    bootId: null,
    revision: 0,
    topologyRevision: 0,
    topology: {
      layout: EMPTY_LAYOUT,
      displayInventory: { displays: [] },
    },
    connections: {
      status: null,
      devices: [],
      capabilities: null,
    },
    inputVisuals: EMPTY_INPUT,
    diagnostics: null,
    mediaSession: {
      control: null,
      mediaSessions: [],
    },
  };
}

function removeByIdentity(values, identity) {
  return values.filter((value) => value?.id !== identity);
}

function replaceByIdentity(values, incoming) {
  const next = removeByIdentity(values, incoming.id);
  next.push(incoming);
  return next;
}

function mouseButtonKey(button) {
  return typeof button === "string" ? button : JSON.stringify(button);
}

function gamepadButtonKey(button) {
  return `${button.gamepad_id}:${JSON.stringify(button.button)}`;
}

function setMembership(values, value, present, keyOf = (entry) => entry) {
  const key = keyOf(value);
  const without = values.filter((entry) => keyOf(entry) !== key);
  return present ? [...without, value] : without;
}

function applyDiscrete(input, transition) {
  const next = {
    ...input,
    lastDiscreteTransition: transition,
  };
  const pressed = transition.state === "Pressed";
  switch (transition.type) {
    case "key":
      next.pressedKeys = setMembership(
        input.pressedKeys,
        transition.key_code,
        pressed,
      ).sort((left, right) => left - right);
      break;
    case "mouse_button":
      next.pressedMouseButtons = setMembership(
        input.pressedMouseButtons,
        transition.button,
        pressed,
        mouseButtonKey,
      );
      break;
    case "gamepad_button":
      next.pressedGamepadButtons = setMembership(
        input.pressedGamepadButtons,
        {
          gamepad_id: transition.gamepad_id,
          button: transition.button,
        },
        pressed,
        gamepadButtonKey,
      );
      break;
    default:
      break;
  }
  return next;
}

function stateFromSnapshot(snapshot, topologyRevision) {
  const dynamic = snapshot.dynamic_state ?? {};
  const sessions = snapshot.active_sessions ?? {};
  return {
    bootId: snapshot.boot_id,
    revision: snapshot.revision,
    topologyRevision,
    topology: {
      layout: snapshot.layout ?? EMPTY_LAYOUT,
      displayInventory: snapshot.display_inventory ?? { displays: [] },
    },
    connections: {
      status: snapshot.status ?? null,
      devices: snapshot.devices ?? [],
      capabilities: snapshot.capabilities ?? null,
    },
    inputVisuals: {
      pointer: dynamic.pointer ?? null,
      gamepads: dynamic.gamepads ?? [],
      pressedKeys: dynamic.pressed_keys ?? [],
      pressedMouseButtons: dynamic.pressed_mouse_buttons ?? [],
      pressedGamepadButtons: dynamic.pressed_gamepad_buttons ?? [],
      lastDiscreteTransition: null,
    },
    diagnostics:
      dynamic.diagnostics ?? snapshot.status?.latency_feedback ?? null,
    mediaSession: {
      control: sessions.control ?? null,
      mediaSessions: sessions.media_sessions ?? [],
    },
  };
}

export function createUiStore({
  requestAnimationFrame = defaultRequestAnimationFrame,
  cancelAnimationFrame = defaultCancelAnimationFrame,
} = {}) {
  let state = initialState();
  let frame = null;
  let pendingPointer;
  let pendingGamepads;
  let pendingRevision = null;
  let acceptedRevision = 0;
  let acceptedVersion = 0;
  const subscriptions = new Set();

  const publish = (next) => {
    if (Object.is(next, state)) return;
    state = next;
    for (const subscription of [...subscriptions]) {
      const selected = subscription.selector(state);
      if (!subscription.equality(subscription.value, selected)) {
        const previous = subscription.value;
        subscription.value = selected;
        subscription.callback(selected, previous);
      }
    }
  };

  const cancelPendingFrame = () => {
    if (frame !== null) {
      cancelAnimationFrame(frame);
      frame = null;
    }
    pendingPointer = undefined;
    pendingGamepads = undefined;
    pendingRevision = null;
  };

  const commitContinuous = () => {
    frame = null;
    const inputVisuals = {
      ...state.inputVisuals,
      ...(pendingPointer !== undefined ? { pointer: pendingPointer } : {}),
      ...(pendingGamepads !== undefined ? { gamepads: pendingGamepads } : {}),
    };
    const revision = Math.max(pendingRevision ?? state.revision, state.revision);
    pendingPointer = undefined;
    pendingGamepads = undefined;
    pendingRevision = null;
    publish({ ...state, revision, inputVisuals });
  };

  const scheduleContinuous = (revision, type, payload) => {
    acceptedRevision = Math.max(acceptedRevision, revision);
    acceptedVersion += 1;
    if (type === "pointer") pendingPointer = payload;
    if (type === "gamepads") pendingGamepads = payload;
    pendingRevision = Math.max(pendingRevision ?? revision, revision);
    if (frame === null) {
      frame = requestAnimationFrame(commitContinuous);
    }
  };

  const applyReliableDelta = (payload) => {
    acceptedRevision = Math.max(acceptedRevision, payload.revision);
    acceptedVersion += 1;
    const change = payload.change;
    let next = { ...state, revision: payload.revision };
    switch (change.type) {
      case "status":
        next.connections = { ...state.connections, status: change.payload };
        next.diagnostics =
          change.payload?.latency_feedback ?? state.diagnostics;
        break;
      case "capabilities":
        next.connections = {
          ...state.connections,
          capabilities: change.payload,
        };
        break;
      case "device_upsert":
        next.connections = {
          ...state.connections,
          devices: replaceByIdentity(state.connections.devices, change.payload),
        };
        break;
      case "device_remove":
        next.connections = {
          ...state.connections,
          devices: removeByIdentity(state.connections.devices, change.payload),
        };
        break;
      case "topology":
        next.topologyRevision = state.topologyRevision + 1;
        next.topology = { ...state.topology, layout: change.payload };
        break;
      case "display_inventory":
        next.topologyRevision = state.topologyRevision + 1;
        next.topology = {
          ...state.topology,
          displayInventory: change.payload,
        };
        break;
      case "key_button":
        next.inputVisuals = applyDiscrete(state.inputVisuals, change.payload);
        break;
      case "session":
        next.mediaSession = {
          control: change.payload?.control ?? null,
          mediaSessions: change.payload?.media_sessions ?? [],
        };
        break;
      case "diagnostics":
        next.diagnostics = change.payload;
        break;
      case "media_session_upsert": {
        const mediaSessions = state.mediaSession.mediaSessions.filter(
          (session) => session.session_id !== change.payload.session_id,
        );
        next.mediaSession = {
          ...state.mediaSession,
          mediaSessions: [...mediaSessions, change.payload],
        };
        break;
      }
      case "media_session_remove":
        next.mediaSession = {
          ...state.mediaSession,
          mediaSessions: state.mediaSession.mediaSessions.filter(
            (session) => session.session_id !== change.payload,
          ),
        };
        break;
      default:
        return;
    }
    publish(next);
  };

  return {
    getState() {
      return state;
    },
    currentRevision() {
      return acceptedRevision;
    },
    currentVersion() {
      return acceptedVersion;
    },
    subscribe(selector, callback, equality = Object.is) {
      const subscription = {
        selector,
        callback,
        equality,
        value: selector(state),
      };
      subscriptions.add(subscription);
      return () => subscriptions.delete(subscription);
    },
    applySnapshot(snapshot) {
      cancelPendingFrame();
      acceptedRevision = snapshot.revision;
      acceptedVersion += 1;
      publish(stateFromSnapshot(snapshot, state.topologyRevision + 1));
    },
    applyDashboardSnapshot(snapshot, expectedVersion = acceptedVersion) {
      if (expectedVersion !== acceptedVersion) return false;
      acceptedVersion += 1;
      publish({
        ...state,
        topologyRevision: state.topologyRevision + 1,
        topology: {
          ...state.topology,
          layout:
            snapshot.visible_layout ?? snapshot.layout ?? state.topology.layout,
        },
        connections: {
          status: snapshot.status ?? null,
          devices: snapshot.devices ?? [],
          capabilities: snapshot.capabilities ?? null,
        },
      });
      return true;
    },
    applyEnvelope(envelope) {
      if (envelope?.type === "snapshot") {
        this.applySnapshot(envelope.payload);
        return;
      }
      if (envelope?.type !== "delta" || !envelope.payload?.change) return;
      const { revision, change } = envelope.payload;
      if (change.type === "pointer" || change.type === "gamepads") {
        scheduleContinuous(revision, change.type, change.payload);
      } else {
        applyReliableDelta(envelope.payload);
      }
    },
    destroy() {
      cancelPendingFrame();
      subscriptions.clear();
    },
  };
}

export function selectTopologyProjection(state) {
  return state.topology;
}

export const selectConnections = (state) => state.connections;
export const selectInputVisuals = (state) => state.inputVisuals;
export const selectDiagnostics = (state) => state.diagnostics;
export const selectMediaSession = (state) => state.mediaSession;
export const selectHasAuthoritativeSnapshot = (state) => state.bootId !== null;

const dashboardMemo = new WeakMap();

export function selectDashboardPayload(state) {
  let byTopology = dashboardMemo.get(state.connections);
  if (!byTopology) {
    byTopology = new WeakMap();
    dashboardMemo.set(state.connections, byTopology);
  }
  let payload = byTopology.get(state.topology);
  if (!payload) {
    payload = {
      status: state.connections.status,
      devices: state.connections.devices,
      layout: state.topology.layout,
      visible_layout: state.topology.layout,
      layout_error: null,
      capabilities: state.connections.capabilities,
      display_inventory:
        state.bootId !== null ? state.topology.displayInventory : undefined,
    };
    byTopology.set(state.topology, payload);
  }
  return payload;
}

export function createOwnerlessStreamCoordinator({ start, stop }) {
  let nextLeaseId = 1;
  let tail = Promise.resolve();

  return {
    acquire() {
      let releaseLease;
      const released = new Promise((resolve) => {
        releaseLease = resolve;
      });
      const lease = {
        id: nextLeaseId,
        active: false,
        cancelled: false,
        released: false,
        ready: null,
        finished: null,
        releaseLease,
      };
      nextLeaseId += 1;

      lease.ready = tail.catch(() => {}).then(async () => {
        if (lease.cancelled) return false;
        try {
          await start();
        } catch (error) {
          if (lease.cancelled) return false;
          throw error;
        }
        if (lease.cancelled) {
          await Promise.resolve(stop()).catch(() => {});
          return false;
        }
        lease.active = true;
        return true;
      });
      lease.finished = lease.ready
        .catch(() => false)
        .then(async () => {
          await released;
          if (lease.active) {
            lease.active = false;
            await stop();
          }
        });
      tail = lease.finished.catch(() => {});
      return lease;
    },
    release(lease) {
      if (!lease.released) {
        lease.released = true;
        lease.cancelled = true;
        lease.releaseLease();
      }
      return lease.finished;
    },
  };
}

export function createUiStateAppBindings({
  store,
  loadFallbackSnapshot,
  onFallbackApplied,
}) {
  let fallbackInFlight = null;
  return {
    onEnvelope(envelope) {
      if (envelope?.type === "snapshot" || envelope?.type === "delta") {
        store.applyEnvelope(envelope);
      }
    },
    onStatus(status) {
      if (status?.state !== "fallback_poll") return undefined;
      if (!fallbackInFlight) {
        const expectedVersion = store.currentVersion();
        fallbackInFlight = Promise.resolve(loadFallbackSnapshot())
          .then((snapshot) => {
            let accepted = false;
            if (snapshot?.protocol_version === 1) {
              if (expectedVersion === store.currentVersion()) {
                store.applySnapshot(snapshot);
                accepted = true;
              }
            } else {
              accepted = store.applyDashboardSnapshot(
                snapshot,
                expectedVersion,
              );
            }
            if (accepted && typeof onFallbackApplied === "function") {
              onFallbackApplied(snapshot);
            }
            return undefined;
          })
          .finally(() => {
            fallbackInFlight = null;
          });
      }
      return fallbackInFlight;
    },
  };
}
