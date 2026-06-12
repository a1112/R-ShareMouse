const DEFAULT_MOUSE_TIMEOUT_MS = 250;
const DEFAULT_KEYBOARD_TIMEOUT_MS = 750;

function daemonInjectRequest(deviceKind, payload, correlationId, options = {}) {
  return {
    InjectEndpointEvent: {
      target: "Local",
      request: {
        correlation_id: correlationId,
        device_kind: deviceKind,
        payload,
        mode: options.mode ?? "RequireHealthyBackend",
        timeout_ms: options.timeoutMs ?? DEFAULT_KEYBOARD_TIMEOUT_MS,
      },
    },
  };
}

export function buildTextCommitRequest(text, correlationId) {
  return daemonInjectRequest(
    "Keyboard",
    {
      kind: "TextCommit",
      data: {
        text,
      },
    },
    correlationId,
  );
}

export function buildKeyRequest(key, state, correlationId) {
  return daemonInjectRequest(
    "Keyboard",
    {
      kind: "Keyboard",
      data: {
        key,
        state,
      },
    },
    correlationId,
  );
}

export function buildKeyTapRequests(key, correlationPrefix) {
  return [
    buildKeyRequest(key, "Pressed", `${correlationPrefix}-down`),
    buildKeyRequest(key, "Released", `${correlationPrefix}-up`),
  ];
}

export function buildMouseMoveRequest(x, y, displayId, correlationId) {
  return daemonInjectRequest(
    "Mouse",
    {
      kind: "MouseMove",
      data: {
        x,
        y,
        display_id: displayId ?? null,
      },
    },
    correlationId,
    {
      mode: "BestEffort",
      timeoutMs: DEFAULT_MOUSE_TIMEOUT_MS,
    },
  );
}

export function buildMouseButtonRequest(button, state, x, y, correlationId) {
  return daemonInjectRequest(
    "Mouse",
    {
      kind: "MouseButton",
      data: {
        button,
        state,
        x,
        y,
      },
    },
    correlationId,
    {
      mode: "BestEffort",
      timeoutMs: DEFAULT_MOUSE_TIMEOUT_MS,
    },
  );
}

export function buildMouseWheelRequest(deltaX, deltaY, x, y, correlationId) {
  return daemonInjectRequest(
    "Mouse",
    {
      kind: "MouseWheel",
      data: {
        delta_x: deltaX,
        delta_y: deltaY,
        x,
        y,
      },
    },
    correlationId,
    {
      mode: "BestEffort",
      timeoutMs: DEFAULT_MOUSE_TIMEOUT_MS,
    },
  );
}

export function nextPointerPosition(current, delta, bounds) {
  const width = Math.max(1, Math.floor(Number(bounds?.width ?? 1)));
  const height = Math.max(1, Math.floor(Number(bounds?.height ?? 1)));
  const sensitivity = Number.isFinite(Number(bounds?.sensitivity))
    ? Number(bounds.sensitivity)
    : 1;
  const x = Math.round(Number(current?.x ?? 0) + Number(delta?.dx ?? 0) * sensitivity);
  const y = Math.round(Number(current?.y ?? 0) + Number(delta?.dy ?? 0) * sensitivity);

  return {
    x: Math.max(0, Math.min(width - 1, x)),
    y: Math.max(0, Math.min(height - 1, y)),
  };
}

export function createPointerMoveCoalescer(sendMove, scheduler = {}) {
  const requestFrame =
    scheduler.requestFrame ??
    ((callback) => {
      if (typeof requestAnimationFrame === "function") {
        return requestAnimationFrame(callback);
      }
      return setTimeout(callback, 16);
    });
  const cancelFrame =
    scheduler.cancelFrame ??
    ((frameId) => {
      if (typeof cancelAnimationFrame === "function") {
        cancelAnimationFrame(frameId);
      } else {
        clearTimeout(frameId);
      }
    });
  let pendingMove = null;
  let frameId = null;

  function drain() {
    frameId = null;
    const next = pendingMove;
    pendingMove = null;
    if (next) {
      sendMove(next);
    }
  }

  return {
    schedule(next) {
      pendingMove = next;
      if (frameId != null) {
        return;
      }
      frameId = requestFrame(drain);
    },
    flush() {
      const next = pendingMove;
      pendingMove = null;
      if (frameId != null) {
        cancelFrame(frameId);
        frameId = null;
      }
      if (next) {
        sendMove(next);
      }
    },
  };
}

export function tauriInvocationForMobileRequest(request) {
  if (request === "LocalControls") {
    return {
      command: "local_controls_state",
      args: {},
      responseVariant: "LocalControls",
    };
  }

  const inject = request?.InjectEndpointEvent;
  if (inject) {
    return {
      command: "inject_endpoint_event",
      args: {
        target: inject.target,
        request: inject.request,
      },
      responseVariant: "EndpointInjectResult",
    };
  }

  return null;
}

export function createMobileCorrelationId(prefix) {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `${prefix}-${crypto.randomUUID()}`;
  }
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
