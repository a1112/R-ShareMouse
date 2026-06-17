const DEFAULT_MOUSE_TIMEOUT_MS = 250;
const DEFAULT_KEYBOARD_TIMEOUT_MS = 750;

export const MOBILE_LONG_PRESS_DRAG_DELAY_MS = 420;

export const MOBILE_TEXT_INPUT_HINTS = Object.freeze({
  enterKeyHint: "send",
  autoCapitalize: "none",
  autoCorrect: "off",
  spellCheck: false,
});

export function shouldCommitMobileTextOnKeyDown(event) {
  const keyCode = Number(event?.keyCode ?? event?.which ?? event?.nativeEvent?.keyCode ?? 0);
  const isComposing =
    event?.isComposing === true || event?.nativeEvent?.isComposing === true || keyCode === 229;
  return event?.key === "Enter" && !isComposing;
}

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

export function createHeldInputController(sendState) {
  let active = false;
  let activePointerId = null;

  function releasePointer(pointerId, force = false) {
    if (!active) {
      return false;
    }
    if (!force && pointerId != null && activePointerId !== pointerId) {
      return false;
    }
    active = false;
    activePointerId = null;
    sendState("Released");
    return true;
  }

  return {
    press(pointerId) {
      releasePointer(null, true);
      active = true;
      activePointerId = pointerId;
      sendState("Pressed");
      return true;
    },
    release(pointerId) {
      return releasePointer(pointerId);
    },
    releaseAll() {
      return releasePointer(null, true);
    },
    releaseIfPointerStillDown(pointerId, buttons) {
      return buttons ? releasePointer(pointerId) : false;
    },
    isPressed() {
      return active;
    },
  };
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

export function buildMouseClickRequests(button, x, y, correlationPrefix) {
  return [
    buildMouseButtonRequest(button, "Pressed", x, y, `${correlationPrefix}-down`),
    buildMouseButtonRequest(button, "Released", x, y, `${correlationPrefix}-up`),
  ];
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

export function isTouchpadTap(start, end, options = {}) {
  if (options.cancelled) {
    return false;
  }
  if (!start || !end) {
    return false;
  }
  const maxDurationMs = Number(options.maxDurationMs ?? 260);
  const maxDistancePx = Number(options.maxDistancePx ?? 12);
  const duration = Number(end.timeMs ?? 0) - Number(start.timeMs ?? 0);
  if (!Number.isFinite(duration) || duration < 0 || duration > maxDurationMs) {
    return false;
  }
  const dx = Number(end.x ?? 0) - Number(start.x ?? 0);
  const dy = Number(end.y ?? 0) - Number(start.y ?? 0);
  return Math.hypot(dx, dy) <= maxDistancePx;
}

export function isTouchpadLongPressDrag(start, current, options = {}) {
  if (options.cancelled) {
    return false;
  }
  if (!start || !current) {
    return false;
  }
  const minDurationMs = Number(options.minDurationMs ?? MOBILE_LONG_PRESS_DRAG_DELAY_MS);
  const maxDistancePx = Number(options.maxDistancePx ?? 12);
  const duration = Number(current.timeMs ?? 0) - Number(start.timeMs ?? 0);
  if (!Number.isFinite(duration) || duration < minDurationMs) {
    return false;
  }
  const dx = Number(current.x ?? 0) - Number(start.x ?? 0);
  const dy = Number(current.y ?? 0) - Number(start.y ?? 0);
  return Math.hypot(dx, dy) <= maxDistancePx;
}

function normalizedTwoFingerTouches(touches) {
  if (!Array.isArray(touches) || touches.length !== 2) {
    return null;
  }
  return touches
    .map((touch) => ({
      id: String(touch?.id ?? ""),
      x: Number(touch?.x ?? 0),
      y: Number(touch?.y ?? 0),
    }))
    .filter((touch) => touch.id && Number.isFinite(touch.x) && Number.isFinite(touch.y))
    .sort((left, right) => left.id.localeCompare(right.id));
}

function centroid(touches) {
  return {
    x: (touches[0].x + touches[1].x) / 2,
    y: (touches[0].y + touches[1].y) / 2,
  };
}

export function twoFingerWheelDelta(previousTouches, currentTouches, options = {}) {
  const previous = normalizedTwoFingerTouches(previousTouches);
  const current = normalizedTwoFingerTouches(currentTouches);
  if (!previous || !current) {
    return null;
  }
  if (previous[0].id !== current[0].id || previous[1].id !== current[1].id) {
    return null;
  }

  const sensitivity = Number(options.sensitivity ?? 0.12);
  const minDeltaPx = Number(options.minDeltaPx ?? 6);
  const previousCenter = centroid(previous);
  const currentCenter = centroid(current);
  const dx = currentCenter.x - previousCenter.x;
  const dy = currentCenter.y - previousCenter.y;
  if (Math.max(Math.abs(dx), Math.abs(dy)) < minDeltaPx) {
    return null;
  }

  const rawDeltaX = Math.round(dx * sensitivity);
  const rawDeltaY = Math.round(dy * sensitivity);
  const deltaX = Object.is(rawDeltaX, -0) ? 0 : rawDeltaX;
  const deltaY = Object.is(rawDeltaY, -0) ? 0 : rawDeltaY;
  if (deltaX === 0 && deltaY === 0) {
    return null;
  }
  return { deltaX, deltaY };
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
