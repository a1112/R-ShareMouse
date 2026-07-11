import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as mobileController from "./mobile-controller.mjs";

import {
  MOBILE_TEXT_INPUT_HINTS,
  MOBILE_EXTRA_KEY_BUTTONS,
  MOBILE_MODIFIER_KEY_BUTTONS,
  MOBILE_POINTER_SENSITIVITY,
  MOBILE_SHORTCUT_BUTTONS,
  buildKeyChordRequests,
  buildKeyRequest,
  buildKeyTapRequests,
  buildMobileReleaseAllRequests,
  buildMouseButtonRequest,
  buildMouseClickRequests,
  buildMouseDoubleClickRequests,
  buildMouseMoveRequest,
  buildMouseWheelRequest,
  createHeldInputController,
  buildTextCommitRequest,
  createMobileStatusRefreshController,
  createOrderedMobileRequestQueue,
  createPointerMoveCoalescer,
  createTwoFingerWheelAccumulator,
  formatMobileBackendStatus,
  formatMobileControllerError,
  formatMobileInjectResultStatus,
  isTouchpadLongPressDrag,
  isTouchpadTap,
  isTwoFingerTap,
  isHeldControlActivationKey,
  isMobileTextCommitSupported,
  nextPointerPosition,
  normalizeMobilePointerSensitivity,
  preventMobileGestureDefault,
  resolveMobileDisplayIdAt,
  shouldCommitMobileTextOnKeyDown,
  shouldActivateHeldControlFromClick,
  shouldPreventMobileGestureDefault,
  tauriInvocationForMobileRequest,
  twoFingerWheelDelta,
} from "./mobile-controller.mjs";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readAppFile(path) {
  return readFileSync(resolve(APP_ROOT, path), "utf8");
}

function deferred() {
  let resolvePromise;
  let rejectPromise;
  const promise = new Promise((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
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
  const source = readAppFile("src/app/MobileController.tsx");

  assert.deepEqual(MOBILE_TEXT_INPUT_HINTS, {
    enterKeyHint: "send",
    autoCapitalize: "none",
    autoCorrect: "off",
    spellCheck: false,
  });
  assert.match(source, /<textarea/);
  assert.match(source, /rows=\{3\}/);
  assert.doesNotMatch(source, /<input\s+[^>]*placeholder="文本"/);
});

test("desktop frontend does not advertise mobile PWA installation over plain HTTP", () => {
  const index = readAppFile("index.html");

  assert.doesNotMatch(index, /rel="manifest"/);
  assert.doesNotMatch(index, /theme-color/);
  assert.doesNotMatch(index, /mobile-web-app-capable/);
  assert.doesNotMatch(index, /apple-mobile-web-app-capable/);
  assert.doesNotMatch(index, /mobile-icon\.svg/);
  assert.equal(existsSync(new URL("../../public/mobile.webmanifest", import.meta.url)), false);
  assert.equal(existsSync(new URL("../../public/mobile-icon.svg", import.meta.url)), false);
});

test("mobile keyboard controls expose common non-text keys and shortcuts", () => {
  assert.deepEqual(
    MOBILE_MODIFIER_KEY_BUTTONS.map((button) => button.key),
    ["ControlLeft", "ShiftLeft", "AltLeft", "SuperLeft"],
  );
  assert.deepEqual(
    MOBILE_EXTRA_KEY_BUTTONS.map((button) => button.key),
    ["Escape", "Tab", "Space", "Delete", "Home", "End", "PageUp", "PageDown"],
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

test("mobile controller exposes holdable modifier keys for keyboard input", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /MOBILE_MODIFIER_KEY_BUTTONS/);
  assert.match(source, /MOBILE_MODIFIER_KEY_BUTTONS\.map/);
  assert.match(source, /keyboardKey=\{button\.key\}/);
});

test("mobile controller exposes an explicit left double click control", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /buildMouseDoubleClickRequests/);
  assert.match(source, /function mouseDoubleClick/);
  assert.match(source, /label="双击"/);
  assert.match(source, /mobile-left-double-click/);
});

test("mobile controller exposes mouse back and forward side buttons", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /label="后退"/);
  assert.match(source, /label="前进"/);
  assert.match(source, /mouseButton\("Back", "Pressed"\)/);
  assert.match(source, /mouseButton\("Forward", "Pressed"\)/);
});

test("mobile controller exposes a release-all input control", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /buildMobileReleaseAllRequests/);
  assert.match(source, /function releaseAllInputs/);
  assert.match(source, /label="释放全部"/);
  assert.match(source, /mobile-release-all/);
});

test("mobile pointer sensitivity is clamped to a phone-friendly range", () => {
  assert.deepEqual(MOBILE_POINTER_SENSITIVITY, {
    storageKey: "rshare.mobile.pointerSensitivity",
    defaultValue: 1.35,
    min: 0.5,
    max: 3,
    step: 0.05,
  });
  assert.equal(normalizeMobilePointerSensitivity(undefined), 1.35);
  assert.equal(normalizeMobilePointerSensitivity("2.4"), 2.4);
  assert.equal(normalizeMobilePointerSensitivity(0.1), 0.5);
  assert.equal(normalizeMobilePointerSensitivity(9), 3);
  assert.equal(normalizeMobilePointerSensitivity(Number.NaN), 1.35);
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

test("formatMobileInjectResultStatus reports rejected endpoint injections", () => {
  assert.deepEqual(
    formatMobileInjectResultStatus({
      accepted: true,
      backend_kind: "Portable",
      error: null,
    }),
    { accepted: true, status: "已连接" },
  );
  assert.deepEqual(
    formatMobileInjectResultStatus({
      accepted: false,
      backend_kind: "Portable",
      error: "PermissionDenied",
    }),
    { accepted: false, status: "注入失败：权限不足 · Portable" },
  );
  assert.deepEqual(
    formatMobileInjectResultStatus({
      EndpointInjectResult: {
        accepted: false,
        backend_kind: "VirtualHid",
        error: "BackendUnavailable",
      },
    }),
    { accepted: false, status: "注入失败：输入后端不可用 · VirtualHid" },
  );
});

test("formatMobileBackendStatus reports mobile injection readiness", () => {
  assert.deepEqual(
    formatMobileBackendStatus({
      inject_backend: {
        active: true,
        kind: "Portable",
        health: "Healthy",
      },
    }),
    {
      state: "ready",
      label: "输入注入就绪",
      detail: "Portable",
    },
  );
  assert.deepEqual(formatMobileBackendStatus({}), {
    state: "pending",
    label: "等待输入后端",
    detail: "尚未收到注入后端状态",
  });
  assert.deepEqual(
    formatMobileBackendStatus({
      inject_backend: {
        active: false,
        kind: "Portable",
        health: { Degraded: { reason: "PermissionDenied" } },
      },
    }),
    {
      state: "blocked",
      label: "输入注入不可用",
      detail: "Portable: PermissionDenied",
    },
  );
});

test("text commit capability accepts only an explicit boolean true", () => {
  assert.equal(
    isMobileTextCommitSupported({ inject_backend: { text_commit_supported: true } }),
    true,
  );
  for (const value of [false, undefined, null, 1, "true", {}, []]) {
    assert.equal(
      isMobileTextCommitSupported({ inject_backend: { text_commit_supported: value } }),
      false,
    );
  }
  assert.equal(isMobileTextCommitSupported({}), false);
  assert.equal(isMobileTextCommitSupported(null), false);
});

test("mobile text controls stay disabled until the daemon explicitly reports support", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(
    source,
    /const \[textCommitSupported, setTextCommitSupported\] = useState\(false\)/,
  );
  assert.match(source, /textCommitSupported: isMobileTextCommitSupported\(snapshot\)/);
  assert.match(source, /applyTextCommitSupported/);
  assert.match(source, /if \(!textCommitSupported\) \{\s*return;\s*\}/);
  assert.match(source, /<textarea[\s\S]*?disabled=\{!textCommitSupported\}/);
  assert.match(source, /title="发送"\s+disabled=\{!textCommitSupported\}/);
  assert.match(source, /当前输入后端不支持文本输入/);
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
  assert.equal(
    shouldCommitMobileTextOnKeyDown({
      key: "Enter",
      shiftKey: true,
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

test("buildMouseDoubleClickRequests emits two ordered click pairs at the current pointer", () => {
  const requests = buildMouseDoubleClickRequests("Left", 320, 240, "mobile-double-click");

  assert.deepEqual(
    requests.map((request) => request.InjectEndpointEvent.request.payload.data.state),
    ["Pressed", "Released", "Pressed", "Released"],
  );
  assert.deepEqual(
    requests.map((request) => request.InjectEndpointEvent.request.payload.data.button),
    ["Left", "Left", "Left", "Left"],
  );
  assert.deepEqual(
    requests.map((request) => request.InjectEndpointEvent.request.correlation_id),
    [
      "mobile-double-click-1-down",
      "mobile-double-click-1-up",
      "mobile-double-click-2-down",
      "mobile-double-click-2-up",
    ],
  );
});

test("buildMobileReleaseAllRequests releases mouse buttons and held modifiers", () => {
  const requests = buildMobileReleaseAllRequests(320, 240, "mobile-release-all");

  assert.deepEqual(
    requests
      .filter((request) => request.InjectEndpointEvent.request.device_kind === "Mouse")
      .map((request) => request.InjectEndpointEvent.request.payload.data),
    [
      { button: "Left", state: "Released", x: 320, y: 240 },
      { button: "Middle", state: "Released", x: 320, y: 240 },
      { button: "Right", state: "Released", x: 320, y: 240 },
      { button: "Back", state: "Released", x: 320, y: 240 },
      { button: "Forward", state: "Released", x: 320, y: 240 },
    ],
  );
  assert.deepEqual(
    requests
      .filter((request) => request.InjectEndpointEvent.request.device_kind === "Keyboard")
      .map((request) => request.InjectEndpointEvent.request.payload.data),
    [
      { key: "ControlLeft", state: "Released" },
      { key: "ShiftLeft", state: "Released" },
      { key: "AltLeft", state: "Released" },
      { key: "SuperLeft", state: "Released" },
    ],
  );
  assert.equal(
    requests.at(0).InjectEndpointEvent.request.correlation_id,
    "mobile-release-all-mouse-left",
  );
  assert.equal(
    requests.at(-1).InjectEndpointEvent.request.correlation_id,
    "mobile-release-all-key-superleft",
  );
});

test("buildMobileReleaseAllRequests unions dynamic held keys and buttons without duplicates", () => {
  const requests = buildMobileReleaseAllRequests(10, 20, "mobile-release-all", {
    keys: ["Backspace", "Enter", "ControlLeft"],
    mouseButtons: ["Back", "Raw:8"],
  });
  const keyboard = requests
    .filter((request) => request.InjectEndpointEvent.request.device_kind === "Keyboard")
    .map((request) => request.InjectEndpointEvent.request.payload.data.key);
  const mouse = requests
    .filter((request) => request.InjectEndpointEvent.request.device_kind === "Mouse")
    .map((request) => request.InjectEndpointEvent.request.payload.data.button);

  assert.deepEqual(keyboard, [
    "ControlLeft",
    "ShiftLeft",
    "AltLeft",
    "SuperLeft",
    "Backspace",
    "Enter",
  ]);
  assert.deepEqual(mouse, ["Left", "Middle", "Right", "Back", "Forward", "Raw:8"]);
});

test("held intent tracker rolls back rejected presses and retains failed releases", () => {
  assert.equal(typeof mobileController.createMobileHeldIntentTracker, "function");
  const tracker = mobileController.createMobileHeldIntentTracker();
  const backspaceDown = buildKeyRequest("Backspace", "Pressed", "backspace-down");
  const backspaceUp = buildKeyRequest("Backspace", "Released", "backspace-up");

  const firstPress = tracker.provision(backspaceDown);
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);
  tracker.settle(firstPress, "accepted");

  const repeatedPress = tracker.provision(backspaceDown);
  tracker.settle(repeatedPress, "rejected");
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);

  const failedRelease = tracker.provision(backspaceUp);
  tracker.settle(failedRelease, "rejected");
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);

  const acceptedRelease = tracker.provision(backspaceUp);
  tracker.settle(acceptedRelease, "accepted");
  assert.deepEqual(tracker.snapshot().keys, []);

  const rejectedNewPress = tracker.provision(buildKeyRequest("Enter", "Pressed", "enter-down"));
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);
  tracker.settle(rejectedNewPress, "rejected");
  assert.deepEqual(tracker.snapshot().keys, []);
});

test("held intent tracker keeps atomic press-release intent until outcomes are known", () => {
  assert.equal(typeof mobileController.createMobileHeldIntentTracker, "function");
  const tracker = mobileController.createMobileHeldIntentTracker();
  const down = buildKeyRequest("Enter", "Pressed", "enter-down");
  const up = buildKeyRequest("Enter", "Released", "enter-up");

  const press = tracker.provision(down);
  const release = tracker.provision(up);
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);

  tracker.settle(press, "accepted");
  tracker.settle(release, "rejected");
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);

  const acceptedRelease = tracker.provision(up);
  const laterPress = tracker.provision(down);
  tracker.settle(acceptedRelease, "accepted");
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);
  tracker.settle(laterPress, "accepted");
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);
});

test("held intent tracker bounds new identities without losing existing state", () => {
  assert.equal(typeof mobileController.createMobileHeldIntentTracker, "function");
  const tracker = mobileController.createMobileHeldIntentTracker({ maxKeys: 1, maxMouseButtons: 1 });
  const first = tracker.provision(buildKeyRequest("Backspace", "Pressed", "first"));
  const overflow = tracker.provision(buildKeyRequest("Enter", "Pressed", "overflow"));

  assert.equal(first.allowed, true);
  assert.equal(overflow.allowed, false);
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);

  tracker.settle(first, "rejected");
  const recovered = tracker.provision(buildKeyRequest("Enter", "Pressed", "recovered"));
  assert.equal(recovered.allowed, true);
  assert.deepEqual(tracker.snapshot().keys, ["Enter"]);
});

test("release-all snapshot includes a press still waiting in the ordered queue", async () => {
  const pressGate = deferred();
  assert.equal(typeof mobileController.createMobileHeldIntentTracker, "function");
  const tracker = mobileController.createMobileHeldIntentTracker();
  const queue = createOrderedMobileRequestQueue(async ({ intent }) => {
    await pressGate.promise;
    tracker.settle(intent, "accepted");
    return true;
  });
  const pressRequest = buildKeyRequest("ArrowLeft", "Pressed", "slow-left-down");
  const intent = tracker.provision(pressRequest);
  const press = queue.enqueue({ request: pressRequest, intent });

  await Promise.resolve();
  const releaseRequests = buildMobileReleaseAllRequests(
    0,
    0,
    "release-slow",
    tracker.snapshot(),
  );
  assert.equal(
    releaseRequests.some(
      (request) => request.InjectEndpointEvent.request.payload.data.key === "ArrowLeft",
    ),
    true,
  );

  pressGate.resolve();
  await press;
});

test("held intent tracker distinguishes unknown outcomes from explicit rejection", () => {
  assert.equal(typeof mobileController.createMobileHeldIntentTracker, "function");
  const tracker = mobileController.createMobileHeldIntentTracker();
  const down = buildKeyRequest("Backspace", "Pressed", "unknown-down");
  const up = buildKeyRequest("Backspace", "Released", "unknown-up");

  const unknownPress = tracker.provision(down);
  tracker.settle(unknownPress, "unknown");
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);

  const unknownRelease = tracker.provision(up);
  tracker.settle(unknownRelease, "unknown");
  assert.deepEqual(tracker.snapshot().keys, ["Backspace"]);

  const acceptedRelease = tracker.provision(up);
  tracker.settle(acceptedRelease, "accepted");
  assert.deepEqual(tracker.snapshot().keys, []);

  const rejectedPress = tracker.provision(buildKeyRequest("Enter", "Pressed", "rejected"));
  tracker.settle(rejectedPress, "rejected");
  assert.deepEqual(tracker.snapshot().keys, []);
});

test("transport-unknown press remains in the release-all retry snapshot", async () => {
  const tracker = mobileController.createMobileHeldIntentTracker();
  const queue = createOrderedMobileRequestQueue(async ({ intent }) => {
    try {
      throw new Error("response lost after invoke");
    } catch {
      tracker.settle(intent, "unknown");
      return false;
    }
  });
  const request = buildKeyRequest("PageDown", "Pressed", "unknown-page-down");
  const intent = tracker.provision(request);

  assert.equal(await queue.enqueue({ request, intent }), false);
  assert.deepEqual(tracker.snapshot().keys, ["PageDown"]);
  const releases = buildMobileReleaseAllRequests(0, 0, "retry", tracker.snapshot());
  assert.equal(
    releases.some(
      (release) => release.InjectEndpointEvent.request.payload.data.key === "PageDown",
    ),
    true,
  );
});

test("held controller reset registry clears local state without sending release", () => {
  assert.equal(typeof mobileController.createHeldInputResetRegistry, "function");
  const states = [];
  const held = createHeldInputController((state) => states.push(state));
  const registry = mobileController.createHeldInputResetRegistry();
  registry.register(() => held.resetSilently());

  held.press(4);
  assert.equal(registry.resetAll(), 1);
  assert.equal(held.isPressed(), false);
  assert.deepEqual(states, ["Pressed"]);
});

test("held controller reset registry never clears input newer than the release batch", () => {
  const registry = mobileController.createHeldInputResetRegistry();
  let resetCount = 0;
  registry.register(() => {
    resetCount += 1;
  });

  const releaseRevision = registry.capture();
  registry.markChanged();
  assert.equal(registry.resetAll(releaseRevision), 0);
  assert.equal(resetCount, 0);

  assert.equal(registry.resetAll(registry.capture()), 1);
  assert.equal(resetCount, 1);
});

test("tracked lifecycle retry releases a failed pointer-up without hardcoded keys", async () => {
  assert.equal(typeof mobileController.buildHeldIntentReleaseRequests, "function");
  const tracker = mobileController.createMobileHeldIntentTracker();
  const down = buildKeyRequest("Backspace", "Pressed", "down");
  const up = buildKeyRequest("Backspace", "Released", "up");
  tracker.settle(tracker.provision(down), "accepted");
  tracker.settle(tracker.provision(up), "unknown");

  const releases = mobileController.buildHeldIntentReleaseRequests(
    5,
    6,
    "lifecycle",
    tracker.snapshot(),
  );
  assert.deepEqual(
    releases.map((request) => request.InjectEndpointEvent.request.payload.data),
    [{ key: "Backspace", state: "Released" }],
  );
  const releaseIntent = tracker.provision(releases[0]);
  tracker.settle(releaseIntent, "accepted");
  assert.deepEqual(tracker.snapshot().keys, []);
});

test("mobile controller retries tracked lifecycle releases and silently resets on success", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /heldInputResetRegistryRef/);
  assert.match(source, /function releaseTrackedInputsForLifecycle\(\)/);
  assert.match(source, /buildHeldIntentReleaseRequests\([\s\S]*heldIntentTrackerRef\.current!\.snapshot\(\)/);
  assert.match(
    source,
    /function releaseMobileInteractionForLifecycle\(\) \{\s*releaseTouchpadInteraction\(\);\s*releaseTrackedInputsForLifecycle\(\);/,
  );
  assert.match(
    source,
    /window\.addEventListener\("pagehide", releaseMobileInteractionForLifecycle\)/,
  );
  assert.match(source, /document\.visibilityState === "hidden"/);
  const releaseAllStart = source.indexOf("async function releaseAllInputs()");
  const releaseAllEnd = source.indexOf("async function commitText()", releaseAllStart);
  const releaseAll = source.slice(releaseAllStart, releaseAllEnd);
  assert.match(releaseAll, /const resetRevision = heldInputResetRegistryRef\.current!\.capture\(\);/);
  assert.match(
    releaseAll,
    /const released = await sendRequestBatch\(requests\);\s*if \(released\) \{\s*heldInputResetRegistryRef\.current!\.resetAll\(resetRevision\);/,
  );
  const mouseClickStart = source.indexOf("async function mouseClick(");
  assert.doesNotMatch(source.slice(mouseClickStart, releaseAllStart), /resetAll\(\)/);
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

test("createHeldInputController ignores repeated presses and releases once after capture loss", () => {
  const states = [];
  const held = createHeldInputController((state) => states.push(state));

  assert.equal(held.press(7), true);
  assert.equal(held.press(7), false);
  assert.equal(held.release(7), true);
  assert.equal(held.releaseAll(), false);
  assert.deepEqual(states, ["Pressed", "Released"]);
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

test("mobile controller releases touchpad drag when the page lifecycle cancels input", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /function releaseTouchpadInteraction\(\)/);
  assert.match(source, /releaseTouchpadDrag\(\);/);
  assert.match(source, /releaseMobileInteractionForLifecycle/);
  assert.match(
    source,
    /window\.addEventListener\("blur", releaseMobileInteractionForLifecycle\)/,
  );
  assert.match(
    source,
    /window\.addEventListener\("pagehide", releaseMobileInteractionForLifecycle\)/,
  );
  assert.match(
    source,
    /document\.addEventListener\("visibilitychange", releaseMobileInteractionWhenHidden\)/,
  );
  assert.match(source, /document\.visibilityState === "hidden"/);
});

test("mobile controller does not claim wake lock over plain HTTP", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.doesNotMatch(source, /wakeLockRef/);
  assert.doesNotMatch(source, /requestMobileWakeLock/);
  assert.doesNotMatch(source, /wakeLockApi\.request/);
});

test("mobile controller installs browser navigation and gesture guards", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /preventBrowserNavigationEvent/);
  assert.match(source, /preventMobileGestureDefault/);
  assert.match(source, /window\.addEventListener\(eventName, handleBrowserNavigation, options\)/);
  assert.match(source, /document\.addEventListener\(eventName, preventMobileGestureDefault, options\)/);
  assert.match(source, /overscrollBehavior: "none"/);
  assert.match(source, /WebkitTouchCallout: "none"/);
});

test("mobile controller exposes a persistent touchpad sensitivity control", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /MOBILE_POINTER_SENSITIVITY/);
  assert.match(source, /normalizeMobilePointerSensitivity/);
  assert.match(source, /sensitivityRef\.current/);
  assert.match(source, /localStorage\.getItem\(MOBILE_POINTER_SENSITIVITY\.storageKey\)/);
  assert.match(source, /localStorage\.setItem\(MOBILE_POINTER_SENSITIVITY\.storageKey/);
  assert.match(source, /type="range"/);
  assert.match(source, /aria-label="触控板灵敏度"/);
  assert.match(source, /sensitivity: sensitivityRef\.current/);
});

test("mobile controller uses the full virtual desktop bounds for pointer movement", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /virtual_x/);
  assert.match(source, /virtual_y/);
  assert.match(source, /layout_width/);
  assert.match(source, /layout_height/);
});

test("mobile controller updates display id from pointer coordinates after cross-screen moves", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /resolveMobileDisplayIdAt/);
  assert.match(source, /displayEntries/);
  assert.match(source, /displayId: resolveMobileDisplayIdAt/);
});

test("preventMobileGestureDefault blocks control-surface browser gestures but preserves text editing", () => {
  const controlTarget = { closest: () => null, tagName: "DIV" };
  const inputTarget = { closest: () => ({}), tagName: "INPUT" };

  assert.equal(
    shouldPreventMobileGestureDefault({ type: "contextmenu", target: controlTarget }),
    true,
  );
  assert.equal(
    shouldPreventMobileGestureDefault({ type: "gesturestart", target: controlTarget }),
    true,
  );
  assert.equal(
    shouldPreventMobileGestureDefault({ type: "selectstart", target: inputTarget }),
    false,
  );
  assert.equal(
    shouldPreventMobileGestureDefault({ type: "pointerdown", target: controlTarget }),
    false,
  );

  const event = {
    type: "contextmenu",
    target: controlTarget,
    returnValue: true,
    preventDefaultCalled: false,
    preventDefault() {
      this.preventDefaultCalled = true;
    },
  };

  assert.equal(preventMobileGestureDefault(event), true);
  assert.equal(event.preventDefaultCalled, true);
  assert.equal(event.returnValue, false);
  assert.equal(preventMobileGestureDefault({ type: "contextmenu", target: inputTarget }), false);
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

test("nextPointerPosition preserves negative coordinates inside a virtual desktop", () => {
  assert.deepEqual(
    nextPointerPosition(
      { x: -2500, y: -230 },
      { dx: -200, dy: -100 },
      { x: -2560, y: -240, width: 4480, height: 1680, sensitivity: 1 },
    ),
    { x: -2560, y: -240 },
  );
  assert.deepEqual(
    nextPointerPosition(
      { x: 1900, y: 1430 },
      { dx: 100, dy: 100 },
      { x: -2560, y: -240, width: 4480, height: 1680, sensitivity: 1 },
    ),
    { x: 1919, y: 1439 },
  );
});

test("resolveMobileDisplayIdAt finds the monitor containing a pointer coordinate", () => {
  const displays = [
    { display_id: "left", x: -2560, y: -240, width: 2560, height: 1440 },
    { display_id: "primary", x: 0, y: 0, width: 1920, height: 1080 },
  ];

  assert.equal(resolveMobileDisplayIdAt(displays, -1200, 34, "primary"), "left");
  assert.equal(resolveMobileDisplayIdAt(displays, 12, 34, "left"), "primary");
  assert.equal(resolveMobileDisplayIdAt(displays, 5000, 5000, "primary"), "primary");
  assert.equal(resolveMobileDisplayIdAt([{ id: "legacy", x: 0, y: 0, w: 800, h: 600 }], 42, 42), "legacy");
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

test("two finger wheel residual accumulates repeated two pixel movement", () => {
  const wheel = createTwoFingerWheelAccumulator({ sensitivity: 0.12, minDeltaPx: 6 });
  let previous = [
    { id: 1, x: 100, y: 100 },
    { id: 2, x: 140, y: 100 },
  ];

  for (const y of [102, 104]) {
    const current = [
      { id: 1, x: 100, y },
      { id: 2, x: 140, y },
    ];
    assert.equal(wheel.update(previous, current), null);
    previous = current;
  }

  const current = [
    { id: 1, x: 100, y: 106 },
    { id: 2, x: 140, y: 106 },
  ];
  assert.deepEqual(wheel.update(previous, current), { deltaX: 0, deltaY: 1 });
});

test("two finger wheel residual preserves direction and reset clears partial motion", () => {
  const wheel = createTwoFingerWheelAccumulator({ sensitivity: 0.12, minDeltaPx: 6 });
  const at = (x) => [
    { id: "a", x, y: 10 },
    { id: "b", x, y: 50 },
  ];

  assert.equal(wheel.update(at(20), at(18)), null);
  assert.equal(wheel.update(at(18), at(16)), null);
  wheel.reset();
  assert.equal(wheel.update(at(16), at(14)), null);
  assert.equal(wheel.update(at(14), at(12)), null);
  assert.deepEqual(wheel.update(at(12), at(10)), { deltaX: -1, deltaY: 0 });
});

test("two finger wheel residual cancels equal movement in the opposite direction", () => {
  const wheel = createTwoFingerWheelAccumulator({ sensitivity: 0.12, minDeltaPx: 6 });
  const at = (y) => [
    { id: 1, x: 10, y },
    { id: 2, x: 50, y },
  ];

  assert.equal(wheel.update(at(10), at(12)), null);
  assert.equal(wheel.update(at(12), at(14)), null);
  assert.equal(wheel.update(at(14), at(12)), null);
  assert.equal(wheel.update(at(12), at(10)), null);
  assert.equal(wheel.update(at(10), at(12)), null);
  assert.equal(wheel.update(at(12), at(14)), null);
  assert.deepEqual(wheel.update(at(14), at(16)), { deltaX: 0, deltaY: 1 });
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

test("ordered request queue never lets release overtake press", async () => {
  const pressGate = deferred();
  const releaseGate = deferred();
  const seen = [];
  const queue = createOrderedMobileRequestQueue(async (value) => {
    seen.push(value);
    await (value === "Pressed" ? pressGate.promise : releaseGate.promise);
  });

  const press = queue.enqueue("Pressed");
  const release = queue.enqueue("Released");
  await Promise.resolve();
  assert.deepEqual(seen, ["Pressed"]);

  pressGate.resolve();
  await press;
  await Promise.resolve();
  assert.deepEqual(seen, ["Pressed", "Released"]);

  releaseGate.resolve();
  await Promise.all([press, release]);
});

test("ordered request queue continues after a failed request", async () => {
  const seen = [];
  const queue = createOrderedMobileRequestQueue(async (value) => {
    seen.push(value);
    if (value === "failed") {
      throw new Error("expected failure");
    }
    return value;
  });

  await assert.rejects(queue.enqueue("failed"), /expected failure/);
  assert.equal(await queue.enqueue("recovered"), "recovered");
  assert.deepEqual(seen, ["failed", "recovered"]);
});

test("ordered request queue keeps a multi-step click atomic against later input", async () => {
  const pressGate = deferred();
  const seen = [];
  const queue = createOrderedMobileRequestQueue(async (value) => {
    seen.push(value);
    if (value === "Pressed") await pressGate.promise;
  });

  const click = queue.enqueueBatch(["Pressed", "Released"]);
  const wheel = queue.enqueue("Wheel");
  await Promise.resolve();
  assert.deepEqual(seen, ["Pressed"]);

  pressGate.resolve();
  await Promise.all([click, wheel]);
  assert.deepEqual(seen, ["Pressed", "Released", "Wheel"]);
});

test("pointer move coalescer keeps only the latest move while one is in flight", async () => {
  const firstGate = deferred();
  const seen = [];
  const frames = [];
  const queue = createOrderedMobileRequestQueue(async (move) => {
    seen.push(move);
    if (seen.length === 1) {
      await firstGate.promise;
    }
  });
  const coalescer = createPointerMoveCoalescer((move) => queue.enqueue(move), {
    requestFrame(callback) {
      frames.push(callback);
      return frames.length;
    },
    cancelFrame() {},
  });

  coalescer.schedule({ x: 1, y: 1 });
  frames.shift()();
  coalescer.schedule({ x: 2, y: 2 });
  coalescer.schedule({ x: 3, y: 3 });
  const flushed = coalescer.flush();

  await Promise.resolve();
  assert.deepEqual(seen, [{ x: 1, y: 1 }]);
  firstGate.resolve();
  await flushed;
  assert.deepEqual(seen, [
    { x: 1, y: 1 },
    { x: 3, y: 3 },
  ]);
});

test("pointer move flush completes before a queued release", async () => {
  const firstMoveGate = deferred();
  const secondMoveGate = deferred();
  const secondMoveStarted = deferred();
  const seen = [];
  const frames = [];
  const queue = createOrderedMobileRequestQueue(async (value) => {
    seen.push(value);
    if (value === "move-1") await firstMoveGate.promise;
    if (value === "move-2") {
      secondMoveStarted.resolve();
      await secondMoveGate.promise;
    }
  });
  const coalescer = createPointerMoveCoalescer((move) => queue.enqueue(move), {
    requestFrame(callback) {
      frames.push(callback);
      return frames.length;
    },
    cancelFrame() {},
  });

  coalescer.schedule("move-1");
  frames.shift()();
  await Promise.resolve();
  coalescer.schedule("move-2");
  const release = (async () => {
    await coalescer.flush();
    await queue.enqueue("Released");
  })();

  assert.deepEqual(seen, ["move-1"]);
  firstMoveGate.resolve();
  await secondMoveStarted.promise;
  assert.deepEqual(seen, ["move-1", "move-2"]);
  secondMoveGate.resolve();
  await release;
  assert.deepEqual(seen, ["move-1", "move-2", "Released"]);
});

test("status refresh is single flight and ignores a poll invalidated by a gesture", async () => {
  const pollGate = deferred();
  const applied = [];
  let fetchCount = 0;
  const refresh = createMobileStatusRefreshController(
    async () => {
      fetchCount += 1;
      return pollGate.promise;
    },
    (snapshot, options) => applied.push({ snapshot, options }),
  );

  const first = refresh.refresh();
  const duplicate = refresh.refresh();
  assert.equal(first, duplicate);
  assert.equal(fetchCount, 1);

  refresh.setGestureActive(true);
  refresh.setGestureActive(false);
  pollGate.resolve({ pointer: { x: 10, y: 20 }, status: "old" });
  await first;
  assert.deepEqual(applied, [
    {
      snapshot: { pointer: { x: 10, y: 20 }, status: "old" },
      options: { applyPointer: false, applyStatus: true },
    },
  ]);
});

test("status refresh keeps active gesture coordinates and rejects stale status", async () => {
  const activeGate = deferred();
  const staleGate = deferred();
  const gates = [activeGate, staleGate];
  const applied = [];
  const refresh = createMobileStatusRefreshController(
    () => gates.shift().promise,
    (snapshot, options) => applied.push({ snapshot, options }),
  );

  refresh.setGestureActive(true);
  const activePoll = refresh.refresh();
  activeGate.resolve({ pointer: { x: 1, y: 2 }, status: "connected" });
  await activePoll;
  assert.deepEqual(applied, [
    {
      snapshot: { pointer: { x: 1, y: 2 }, status: "connected" },
      options: { applyPointer: false, applyStatus: true },
    },
  ]);

  refresh.setGestureActive(false);
  const stalePoll = refresh.refresh();
  refresh.markStatusChanged();
  staleGate.resolve({ pointer: { x: 3, y: 4 }, status: "stale" });
  await stalePoll;
  assert.deepEqual(applied[1], {
    snapshot: { pointer: { x: 3, y: 4 }, status: "stale" },
    options: { applyPointer: true, applyStatus: false },
  });
});

test("status refresh waits for every pending pointer write before accepting coordinates", async () => {
  const firstPoll = deferred();
  const secondPoll = deferred();
  const thirdPoll = deferred();
  const polls = [firstPoll, secondPoll, thirdPoll];
  const applied = [];
  const refresh = createMobileStatusRefreshController(
    () => polls.shift().promise,
    (snapshot, options) => applied.push({ snapshot, options }),
  );

  refresh.setGestureActive(true);
  const finishOldMove = refresh.beginPointerWrite();
  refresh.setGestureActive(false);
  const duringOldMove = refresh.refresh();
  firstPoll.resolve({ pointer: { x: 1, y: 1 } });
  await duringOldMove;
  assert.equal(applied[0].options.applyPointer, false);

  const finishNewMove = refresh.beginPointerWrite();
  finishOldMove();
  const duringNewMove = refresh.refresh();
  secondPoll.resolve({ pointer: { x: 2, y: 2 } });
  await duringNewMove;
  assert.equal(applied[1].options.applyPointer, false);

  finishNewMove();
  const afterAllAcks = refresh.refresh();
  thirdPoll.resolve({ pointer: { x: 3, y: 3 } });
  await afterAllAcks;
  assert.equal(applied[2].options.applyPointer, true);
});

test("mobile status result keeps backend status behind the status revision gate", () => {
  assert.equal(typeof mobileController.applyMobileStatusRefreshResult, "function");
  const applied = [];
  const handlers = {
    applyPointer(value) {
      applied.push(["pointer", value]);
    },
    applyBackendStatus(value) {
      applied.push(["backend", value]);
    },
    applyTextCommitSupported(value) {
      applied.push(["text-commit", value]);
    },
    applyStatus(value) {
      applied.push(["status", value]);
    },
    applyError(value) {
      applied.push(["error", value]);
    },
  };
  const result = {
    state: {
      pointer: { x: 9, y: 8 },
      backendStatus: { state: "blocked" },
      textCommitSupported: true,
    },
    error: null,
  };

  mobileController.applyMobileStatusRefreshResult(
    result,
    { applyPointer: true, applyStatus: false },
    handlers,
  );
  assert.deepEqual(applied, [["pointer", { x: 9, y: 8 }]]);

  mobileController.applyMobileStatusRefreshResult(
    result,
    { applyPointer: false, applyStatus: true },
    handlers,
  );
  assert.deepEqual(applied.slice(1), [
    ["backend", { state: "blocked" }],
    ["text-commit", true],
    ["status", "已连接"],
  ]);
});

test("current status refresh errors fail text capability closed but stale errors do not", async () => {
  const currentPoll = deferred();
  const stalePoll = deferred();
  let pollCount = 0;
  const applied = [];
  const controller = createMobileStatusRefreshController(
    () => (pollCount++ === 0 ? currentPoll.promise : stalePoll.promise),
    (result, options) => {
      mobileController.applyMobileStatusRefreshResult(result, options, {
        applyTextCommitSupported(value) {
          applied.push(["text-commit", value]);
        },
        applyError(error) {
          applied.push(["error", error.message]);
        },
      });
    },
  );

  const currentRefresh = controller.refresh();
  currentPoll.resolve({ state: null, error: new Error("offline") });
  await currentRefresh;
  assert.deepEqual(applied, [
    ["text-commit", false],
    ["error", "offline"],
  ]);

  applied.length = 0;
  const staleRefresh = controller.refresh();
  controller.markStatusChanged();
  stalePoll.resolve({ state: null, error: new Error("stale offline") });
  await staleRefresh;
  assert.deepEqual(applied, []);
});

test("aborted status refresh errors never overwrite the current text capability", () => {
  const applied = [];
  const abortError = new Error("aborted");
  abortError.name = "AbortError";

  mobileController.applyMobileStatusRefreshResult(
    { state: null, error: abortError },
    { applyPointer: false, applyStatus: true },
    {
      applyTextCommitSupported(value) {
        applied.push(["text-commit", value]);
      },
      applyError(error) {
        applied.push(["error", error.message]);
      },
    },
  );

  assert.deepEqual(applied, []);
});

test("held controls use click only for keyboard or assistive activation", () => {
  assert.equal(shouldActivateHeldControlFromClick({ detail: 0 }), true);
  assert.equal(shouldActivateHeldControlFromClick({ detail: 1 }), false);
  assert.equal(shouldActivateHeldControlFromClick({ detail: 2 }), false);
});

test("held controls recognize Enter and Space even on repeat so browsers stay suppressed", () => {
  assert.equal(isHeldControlActivationKey({ key: "Enter", repeat: false }), true);
  assert.equal(isHeldControlActivationKey({ key: " ", repeat: false }), true);
  assert.equal(isHeldControlActivationKey({ key: "Spacebar", repeat: false }), true);
  assert.equal(isHeldControlActivationKey({ key: "Enter", repeat: true }), true);
  assert.equal(isHeldControlActivationKey({ key: "ArrowDown", repeat: false }), false);
});

test("held control repeat keydowns prevent native clicks without sending another press", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(
    source,
    /onKeyDown=\{\(event\) => \{[\s\S]*?event\.preventDefault\(\);[\s\S]*?if \(event\.repeat\) \{[\s\S]*?return;[\s\S]*?held\.press\(-2\);/,
  );
  assert.doesNotMatch(
    source,
    /if \(suppressKeyboardClickRef\.current\) \{\s*suppressKeyboardClickRef\.current = false;/,
  );
});

test("held control blur releases the key and restores assistive click activation", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(
    source,
    /onBlur=\{\(\) => \{\s*held\.release\(-2\);\s*resetSyntheticClickSuppression\(\);\s*\}\}/,
  );
});

test("held control lifecycle release also clears synthetic click suppression", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /const onLifecycleResetRef = useRef\(onLifecycleReset\);/);
  assert.match(
    source,
    /const releaseForLifecycle = \(\) => \{\s*controllerRef\.current\?\.releaseAll\(\);\s*onLifecycleResetRef\.current\(\);\s*\};/,
  );
  assert.equal(
    (source.match(
      /useHeldInputController\([\s\S]*?resetSyntheticClickSuppression,\s*resetRegistry,\s*\);/g,
    ) ?? []).length,
    2,
  );
});

test("mobile held controls expose pressed state and capture-loss cleanup", () => {
  const source = readAppFile("src/app/MobileController.tsx");

  assert.match(source, /aria-pressed=\{held\.isPressed\}/);
  assert.match(source, /onLostPointerCapture=/);
  assert.match(source, /onKeyDown=/);
  assert.match(source, /onKeyUp=/);
  assert.match(source, /shouldActivateHeldControlFromClick\(event\)/);
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
