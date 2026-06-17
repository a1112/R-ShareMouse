const DEFAULT_MOUSE_TIMEOUT_MS = 250;
const DEFAULT_KEYBOARD_TIMEOUT_MS = 750;

export const MOBILE_LONG_PRESS_DRAG_DELAY_MS = 420;

export const MOBILE_POINTER_SENSITIVITY = Object.freeze({
  storageKey: "rshare.mobile.pointerSensitivity",
  defaultValue: 1.35,
  min: 0.5,
  max: 3,
  step: 0.05,
});

export const MOBILE_TEXT_INPUT_HINTS = Object.freeze({
  enterKeyHint: "send",
  autoCapitalize: "none",
  autoCorrect: "off",
  spellCheck: false,
});

export const MOBILE_MODIFIER_KEY_BUTTONS = Object.freeze([
  Object.freeze({ label: "Ctrl", key: "ControlLeft" }),
  Object.freeze({ label: "Shift", key: "ShiftLeft" }),
  Object.freeze({ label: "Alt", key: "AltLeft" }),
  Object.freeze({ label: "Win", key: "SuperLeft" }),
]);

export const MOBILE_EXTRA_KEY_BUTTONS = Object.freeze([
  Object.freeze({ label: "Esc", key: "Escape" }),
  Object.freeze({ label: "Tab", key: "Tab" }),
  Object.freeze({ label: "Space", key: "Space" }),
  Object.freeze({ label: "Del", key: "Delete" }),
  Object.freeze({ label: "Home", key: "Home" }),
  Object.freeze({ label: "End", key: "End" }),
  Object.freeze({ label: "PgUp", key: "PageUp" }),
  Object.freeze({ label: "PgDn", key: "PageDown" }),
]);

export const MOBILE_SHORTCUT_BUTTONS = Object.freeze([
  Object.freeze({ id: "copy", label: "复制", keys: Object.freeze(["ControlLeft", "C"]) }),
  Object.freeze({ id: "paste", label: "粘贴", keys: Object.freeze(["ControlLeft", "V"]) }),
  Object.freeze({ id: "cut", label: "剪切", keys: Object.freeze(["ControlLeft", "X"]) }),
  Object.freeze({ id: "select-all", label: "全选", keys: Object.freeze(["ControlLeft", "A"]) }),
]);

export function formatMobileControllerError(error, scope = "移动端") {
  const message = error instanceof Error ? error.message : String(error ?? "");
  if (/failed to fetch|networkerror|fetch failed|load failed/i.test(message)) {
    return `${scope}网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络`;
  }
  return `${scope}请求失败：${message || "未知错误"}`;
}

export function normalizeMobilePointerSensitivity(value, config = MOBILE_POINTER_SENSITIVITY) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    return config.defaultValue;
  }
  const clamped = Math.max(config.min, Math.min(config.max, parsed));
  const stepped = Math.round(clamped / config.step) * config.step;
  return Number(stepped.toFixed(2));
}

function isEditableMobileTarget(target) {
  if (!target || typeof target !== "object") {
    return false;
  }
  const tagName = String(target.tagName ?? "").toUpperCase();
  if (tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT") {
    return true;
  }
  if (target.isContentEditable === true) {
    return true;
  }
  if (typeof target.closest === "function") {
    return Boolean(target.closest("input, textarea, select, [contenteditable='true']"));
  }
  return false;
}

export function shouldPreventMobileGestureDefault(event) {
  const type = String(event?.type ?? "").toLowerCase();
  if (
    ![
      "contextmenu",
      "dragstart",
      "selectstart",
      "gesturestart",
      "gesturechange",
      "gestureend",
    ].includes(type)
  ) {
    return false;
  }
  return !isEditableMobileTarget(event?.target);
}

export function preventMobileGestureDefault(event) {
  if (!shouldPreventMobileGestureDefault(event)) {
    return false;
  }
  event?.preventDefault?.();
  if ("returnValue" in event) {
    event.returnValue = false;
  }
  return true;
}

export function shouldCommitMobileTextOnKeyDown(event) {
  const keyCode = Number(event?.keyCode ?? event?.which ?? event?.nativeEvent?.keyCode ?? 0);
  const isComposing =
    event?.isComposing === true || event?.nativeEvent?.isComposing === true || keyCode === 229;
  return event?.key === "Enter" && event?.shiftKey !== true && !isComposing;
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

function keyCorrelationSlug(key) {
  return String(key).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

export function buildKeyChordRequests(keys, correlationPrefix) {
  const normalizedKeys = Array.isArray(keys) ? keys.map(String).filter(Boolean) : [];
  return [
    ...normalizedKeys.map((key, index) =>
      buildKeyRequest(key, "Pressed", `${correlationPrefix}-down-${index}-${keyCorrelationSlug(key)}`),
    ),
    ...[...normalizedKeys].reverse().map((key, index) =>
      buildKeyRequest(key, "Released", `${correlationPrefix}-up-${index}-${keyCorrelationSlug(key)}`),
    ),
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
  const minX = Math.floor(Number(bounds?.x ?? bounds?.minX ?? 0));
  const minY = Math.floor(Number(bounds?.y ?? bounds?.minY ?? 0));
  const width = Math.max(1, Math.floor(Number(bounds?.width ?? 1)));
  const height = Math.max(1, Math.floor(Number(bounds?.height ?? 1)));
  const sensitivity = Number.isFinite(Number(bounds?.sensitivity))
    ? Number(bounds.sensitivity)
    : 1;
  const x = Math.round(Number(current?.x ?? 0) + Number(delta?.dx ?? 0) * sensitivity);
  const y = Math.round(Number(current?.y ?? 0) + Number(delta?.dy ?? 0) * sensitivity);
  const maxX = minX + width - 1;
  const maxY = minY + height - 1;

  return {
    x: Math.max(minX, Math.min(maxX, x)),
    y: Math.max(minY, Math.min(maxY, y)),
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

export function isTwoFingerTap(startTouches, endTouches, options = {}) {
  if (options.cancelled) {
    return false;
  }
  const start = normalizedTwoFingerTouches(startTouches);
  const end = normalizedTwoFingerTouches(endTouches);
  if (!start || !end) {
    return false;
  }
  if (start[0].id !== end[0].id || start[1].id !== end[1].id) {
    return false;
  }

  const startTimeMs = Number(options.startTimeMs ?? 0);
  const endTimeMs = Number(options.endTimeMs ?? startTimeMs);
  const maxDurationMs = Number(options.maxDurationMs ?? 260);
  const duration = endTimeMs - startTimeMs;
  if (!Number.isFinite(duration) || duration < 0 || duration > maxDurationMs) {
    return false;
  }

  const maxCenterDistancePx = Number(options.maxCenterDistancePx ?? 12);
  const maxFingerDistanceDeltaPx = Number(options.maxFingerDistanceDeltaPx ?? 12);
  const startCenter = centroid(start);
  const endCenter = centroid(end);
  const centerDistance = Math.hypot(endCenter.x - startCenter.x, endCenter.y - startCenter.y);
  if (centerDistance > maxCenterDistancePx) {
    return false;
  }

  const startDistance = Math.hypot(start[1].x - start[0].x, start[1].y - start[0].y);
  const endDistance = Math.hypot(end[1].x - end[0].x, end[1].y - end[0].y);
  return Math.abs(endDistance - startDistance) <= maxFingerDistanceDeltaPx;
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
