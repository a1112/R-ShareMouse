import test from "node:test";
import assert from "node:assert/strict";

import {
  buildKeyTapRequests,
  buildMouseButtonRequest,
  buildMouseClickRequests,
  buildMouseMoveRequest,
  buildMouseWheelRequest,
  buildTextCommitRequest,
  createPointerMoveCoalescer,
  isTouchpadTap,
  nextPointerPosition,
  tauriInvocationForMobileRequest,
} from "./mobile-controller.mjs";

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
