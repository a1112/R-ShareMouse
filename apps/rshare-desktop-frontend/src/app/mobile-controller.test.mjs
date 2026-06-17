import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  MOBILE_TEXT_INPUT_HINTS,
  MOBILE_EXTRA_KEY_BUTTONS,
  MOBILE_SHORTCUT_BUTTONS,
  buildKeyChordRequests,
  buildKeyRequest,
  buildKeyTapRequests,
  buildMouseButtonRequest,
  buildMouseClickRequests,
  buildMouseMoveRequest,
  buildMouseWheelRequest,
  createHeldInputController,
  buildTextCommitRequest,
  createPointerMoveCoalescer,
  formatMobileControllerError,
  isTouchpadLongPressDrag,
  isTouchpadTap,
  isTwoFingerTap,
  nextPointerPosition,
  shouldCommitMobileTextOnKeyDown,
  tauriInvocationForMobileRequest,
  twoFingerWheelDelta,
} from "./mobile-controller.mjs";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readAppFile(path) {
  return readFileSync(resolve(APP_ROOT, path), "utf8");
}

test("buildTextCommitRequest creates daemon IPC inject request for unicode input", () => {
  assert.deepEqual(buildTextCommitRequest("你好🙂", "mobile-text-1"), {
    InjectEndpointEvent: {
      target: "Local",
      request: {
        correlation_id: "mobile-text-1",
        device_kind: "Keyboard",
        payload: {
          kind: "TextCommit",
          data: {
            text: "你好🙂",
          },
        },
        mode: "RequireHealthyBackend",
        timeout_ms: 750,
      },
    },
  });
});

test("mobile text input uses phone keyboard send hints", () => {
  assert.deepEqual(MOBILE_TEXT_INPUT_HINTS, {
    enterKeyHint: "send",
    autoCapitalize: "none",
    autoCorrect: "off",
    spellCheck: false,
  });
});

test("desktop frontend exposes mobile PWA metadata", () => {
  const index = readAppFile("index.html");
  const manifest = JSON.parse(readAppFile("public/mobile.webmanifest"));

  assert.match(index, /<link rel="manifest" href="\/mobile\.webmanifest"/);
  assert.match(index, /<meta name="theme-color" content="#101214"/);
  assert.match(index, /<meta name="mobile-web-app-capable" content="yes"/);
  assert.match(index, /<meta name="apple-mobile-web-app-capable" content="yes"/);
  assert.equal(manifest.name, "R-ShareMouse Mobile");
  assert.equal(manifest.start_url, "/mobile");
  assert.equal(manifest.display, "standalone");
  assert.equal(manifest.orientation, "portrait");
  assert.equal(manifest.theme_color, "#101214");
  assert.ok(manifest.icons.some((icon) => icon.src === "/mobile-icon.svg"));
});

test("mobile keyboard controls expose common non-text keys and shortcuts", () => {
  assert.deepEqual(
    MOBILE_EXTRA_KEY_BUTTONS.map((button) => button.key),
    ["Escape", "Tab", "Space", "Delete"],
  );
  assert.deepEqual(
    MOBILE_SHORTCUT_BUTTONS.map((button) => button.keys),
    [
      ["ControlLeft", "C"],
      ["ControlLeft", "V"],
      ["ControlLeft", "X"],
      ["ControlLeft", "A"],
    ],
  );
});

test("buildKeyChordRequests presses keys in order and releases in reverse", () => {
  const requests = buildKeyChordRequests(["ControlLeft", "C"], "mobile-copy");

  assert.deepEqual(
    requests.map((request) => request.InjectEndpointEvent.request.payload.data),
    [
      { key: "ControlLeft", state: "Pressed" },
      { key: "C", state: "Pressed" },
      { key: "C", state: "Released" },
      { key: "ControlLeft", state: "Released" },
    ],
  );
  assert.deepEqual(
    requests.map((request) => request.InjectEndpointEvent.request.correlation_id),
    [
      "mobile-copy-down-0-controlleft",
      "mobile-copy-down-1-c",
      "mobile-copy-up-0-c",
      "mobile-copy-up-1-controlleft",
    ],
  );
});

test("formatMobileControllerError hides raw browser fetch failures", () => {
  assert.equal(
    formatMobileControllerError(new TypeError("Failed to fetch"), "移动端注入"),
    "移动端注入网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络",
  );
  assert.equal(
    formatMobileControllerError(new Error("HTTP 401"), "移动端状态"),
    "移动端状态请求失败：HTTP 401",
  );
});

test("shouldCommitMobileTextOnKeyDown ignores IME composition enter", () => {
  assert.equal(shouldCommitMobileTextOnKeyDown({ key: "Enter" }), true);
  assert.equal(
    shouldCommitMobileTextOnKeyDown({
      key: "Enter",
      isComposing: true,
    }),
    false,
  );
  assert.equal(
    shouldCommitMobileTextOnKeyDown({
      key: "Enter",
      keyCode: 229,
    }),
    false,
  );
  assert.equal(
    shouldCommitMobileTextOnKeyDown({
      key: "Enter",
      nativeEvent: { isComposing: true },
    }),
    false,
  );
  assert.equal(shouldCommitMobileTextOnKeyDown({ key: "a" }), false);
});

test("mobile mouse requests use local endpoint injection payloads", () => {
  assert.deepEqual(buildMouseMoveRequest(320, 240, "display-1", "mobile-move-1"), {
    InjectEndpointEvent: {
      target: "Local",
      request: {
        correlation_id: "mobile-move-1",
        device_kind: "Mouse",
        payload: {
          kind: "MouseMove",
          data: {
            x: 320,
            y: 240,
            display_id: "display-1",
          },
        },
        mode: "BestEffort",
        timeout_ms: 250,
      },
    },
  });

  assert.equal(
    buildMouseButtonRequest("Left", "Pressed", 320, 240, "mobile-left-down").InjectEndpointEvent
      .request.payload.data.button,
    "Left",
  );
  assert.equal(
    buildMouseWheelRequest(0, -3, 320, 240, "mobile-wheel").InjectEndpointEvent.request.payload
      .data.delta_y,
    -3,
  );
});

test("buildMouseClickRequests emits press and release at the current pointer", () => {
  const [down, up] = buildMouseClickRequests("Left", 320, 240, "mobile-tap");

  assert.equal(down.InjectEndpointEvent.request.payload.data.button, "Left");
  assert.equal(down.InjectEndpointEvent.request.payload.data.state, "Pressed");
  assert.equal(up.InjectEndpointEvent.request.payload.data.state, "Released");
  assert.equal(down.InjectEndpointEvent.request.payload.data.x, 320);
  assert.equal(up.InjectEndpointEvent.request.payload.data.y, 240);
  assert.equal(down.InjectEndpointEvent.request.correlation_id, "mobile-tap-down");
  assert.equal(up.InjectEndpointEvent.request.correlation_id, "mobile-tap-up");
});

test("buildKeyTapRequests emits press and release requests with stable ordering", () => {
  const [down, up] = buildKeyTapRequests("Backspace", "mobile-backspace");

  assert.equal(down.InjectEndpointEvent.request.payload.data.state, "Pressed");
  assert.equal(up.InjectEndpointEvent.request.payload.data.state, "Released");
  assert.equal(down.InjectEndpointEvent.request.correlation_id, "mobile-backspace-down");
  assert.equal(up.InjectEndpointEvent.request.correlation_id, "mobile-backspace-up");
});

test("buildKeyRequest emits a single keyboard hold transition", () => {
  const down = buildKeyRequest("Left", "Pressed", "mobile-left-down");
  const up = buildKeyRequest("Left", "Released", "mobile-left-up");

  assert.equal(down.InjectEndpointEvent.request.device_kind, "Keyboard");
  assert.equal(down.InjectEndpointEvent.request.payload.kind, "Keyboard");
  assert.deepEqual(down.InjectEndpointEvent.request.payload.data, {
    key: "Left",
    state: "Pressed",
  });
  assert.deepEqual(up.InjectEndpointEvent.request.payload.data, {
    key: "Left",
    state: "Released",
  });
});

test("createHeldInputController releases a held input once from pointer or global cancellation", () => {
  const states = [];
  const held = createHeldInputController((state) => states.push(state));

  assert.equal(held.press(7), true);
  assert.deepEqual(states, ["Pressed"]);
  assert.equal(held.releaseIfPointerStillDown(7, 1), true);
  assert.deepEqual(states, ["Pressed", "Released"]);
  assert.equal(held.release(7), false);
  assert.deepEqual(states, ["Pressed", "Released"]);

  assert.equal(held.press(8), true);
  assert.equal(held.release(9), false);
  assert.equal(held.isPressed(), true);
  assert.equal(held.releaseAll(), true);
  assert.equal(held.isPressed(), false);
  assert.deepEqual(states, ["Pressed", "Released", "Pressed", "Released"]);
});

test("isTouchpadLongPressDrag accepts still long presses and rejects early or moved gestures", () => {
  assert.equal(
    isTouchpadLongPressDrag(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 106, y: 104, timeMs: 1450 },
      { minDurationMs: 420 },
    ),
    true,
  );
  assert.equal(
    isTouchpadLongPressDrag(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 106, y: 104, timeMs: 1300 },
      { minDurationMs: 420 },
    ),
    false,
  );
  assert.equal(
    isTouchpadLongPressDrag(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 126, y: 104, timeMs: 1450 },
      { minDurationMs: 420, maxDistancePx: 12 },
    ),
    false,
  );
});

test("isTouchpadTap accepts short still taps and rejects drags or long presses", () => {
  assert.equal(
    isTouchpadTap(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 106, y: 104, timeMs: 1140 },
    ),
    true,
  );
  assert.equal(
    isTouchpadTap(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 135, y: 105, timeMs: 1100 },
    ),
    false,
  );
  assert.equal(
    isTouchpadTap(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 104, y: 103, timeMs: 1500 },
    ),
    false,
  );
  assert.equal(
    isTouchpadTap(
      { x: 100, y: 100, timeMs: 1000 },
      { x: 104, y: 103, timeMs: 1100 },
      { cancelled: true },
    ),
    false,
  );
});

test("nextPointerPosition applies sensitivity and clamps to desktop bounds", () => {
  assert.deepEqual(
    nextPointerPosition(
      { x: 100, y: 100 },
      { dx: 50, dy: -300 },
      { width: 1920, height: 1080, sensitivity: 1.5 },
    ),
    { x: 175, y: 0 },
  );
  assert.deepEqual(
    nextPointerPosition(
      { x: 1910, y: 1070 },
      { dx: 40, dy: 40 },
      { width: 1920, height: 1080, sensitivity: 1 },
    ),
    { x: 1919, y: 1079 },
  );
});

test("twoFingerWheelDelta maps two finger movement into wheel delta", () => {
  assert.deepEqual(
    twoFingerWheelDelta(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 140, y: 100 },
      ],
      [
        { id: 1, x: 98, y: 60 },
        { id: 2, x: 138, y: 60 },
      ],
      { sensitivity: 0.12, minDeltaPx: 6 },
    ),
    { deltaX: 0, deltaY: -5 },
  );

  assert.deepEqual(
    twoFingerWheelDelta(
      [
        { id: "a", x: 100, y: 100 },
        { id: "b", x: 140, y: 100 },
      ],
      [
        { id: "a", x: 140, y: 103 },
        { id: "b", x: 180, y: 103 },
      ],
      { sensitivity: 0.1, minDeltaPx: 6 },
    ),
    { deltaX: 4, deltaY: 0 },
  );
});

test("twoFingerWheelDelta ignores single finger tiny or mismatched gestures", () => {
  assert.equal(
    twoFingerWheelDelta([{ id: 1, x: 100, y: 100 }], [{ id: 1, x: 100, y: 40 }]),
    null,
  );
  assert.equal(
    twoFingerWheelDelta(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 140, y: 100 },
      ],
      [
        { id: 1, x: 102, y: 103 },
        { id: 2, x: 142, y: 103 },
      ],
      { minDeltaPx: 8 },
    ),
    null,
  );
  assert.equal(
    twoFingerWheelDelta(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 140, y: 100 },
      ],
      [
        { id: 1, x: 110, y: 70 },
        { id: 3, x: 150, y: 70 },
      ],
    ),
    null,
  );
});

test("isTwoFingerTap accepts short still two finger taps", () => {
  assert.equal(
    isTwoFingerTap(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 150, y: 100 },
      ],
      [
        { id: 1, x: 104, y: 102 },
        { id: 2, x: 153, y: 101 },
      ],
      { startTimeMs: 1000, endTimeMs: 1120 },
    ),
    true,
  );
});

test("isTwoFingerTap rejects scrolls long presses and changed fingers", () => {
  assert.equal(
    isTwoFingerTap(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 150, y: 100 },
      ],
      [
        { id: 1, x: 100, y: 140 },
        { id: 2, x: 150, y: 140 },
      ],
      { startTimeMs: 1000, endTimeMs: 1120 },
    ),
    false,
  );
  assert.equal(
    isTwoFingerTap(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 150, y: 100 },
      ],
      [
        { id: 1, x: 104, y: 102 },
        { id: 2, x: 153, y: 101 },
      ],
      { startTimeMs: 1000, endTimeMs: 1500 },
    ),
    false,
  );
  assert.equal(
    isTwoFingerTap(
      [
        { id: 1, x: 100, y: 100 },
        { id: 2, x: 150, y: 100 },
      ],
      [
        { id: 1, x: 104, y: 102 },
        { id: 3, x: 153, y: 101 },
      ],
      { startTimeMs: 1000, endTimeMs: 1120 },
    ),
    false,
  );
});

test("createPointerMoveCoalescer sends only the latest move per animation frame", () => {
  const sent = [];
  const frameCallbacks = [];
  const coalescer = createPointerMoveCoalescer((move) => sent.push(move), {
    requestFrame(callback) {
      frameCallbacks.push(callback);
      return frameCallbacks.length;
    },
    cancelFrame() {},
  });

  coalescer.schedule({ x: 10, y: 10 });
  coalescer.schedule({ x: 20, y: 30 });

  assert.deepEqual(sent, []);
  assert.equal(frameCallbacks.length, 1);

  frameCallbacks.shift()();

  assert.deepEqual(sent, [{ x: 20, y: 30 }]);
});

test("createPointerMoveCoalescer flushes the final pending move without duplicate send", () => {
  const sent = [];
  const frameCallbacks = [];
  const cancelled = [];
  const coalescer = createPointerMoveCoalescer((move) => sent.push(move), {
    requestFrame(callback) {
      frameCallbacks.push(callback);
      return frameCallbacks.length;
    },
    cancelFrame(frameId) {
      cancelled.push(frameId);
    },
  });

  coalescer.schedule({ x: 40, y: 50 });
  coalescer.schedule({ x: 70, y: 80 });
  coalescer.flush();

  assert.deepEqual(sent, [{ x: 70, y: 80 }]);
  assert.deepEqual(cancelled, [1]);

  frameCallbacks.shift()();

  assert.deepEqual(sent, [{ x: 70, y: 80 }]);
});

test("tauriInvocationForMobileRequest maps mobile requests to desktop commands", () => {
  assert.deepEqual(tauriInvocationForMobileRequest("LocalControls"), {
    command: "local_controls_state",
    args: {},
    responseVariant: "LocalControls",
  });

  const injectRequest = buildTextCommitRequest("你好", "mobile-text-2");
  assert.deepEqual(tauriInvocationForMobileRequest(injectRequest), {
    command: "inject_endpoint_event",
    args: injectRequest.InjectEndpointEvent,
    responseVariant: "EndpointInjectResult",
  });

  assert.equal(tauriInvocationForMobileRequest("Unknown"), null);
});
