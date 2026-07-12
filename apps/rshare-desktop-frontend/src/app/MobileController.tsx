import { useEffect, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  CornerDownLeft,
  Delete,
  Keyboard,
  MousePointer2,
  Send,
} from "lucide-react";

import {
  MOBILE_EXTRA_KEY_BUTTONS,
  MOBILE_LONG_PRESS_DRAG_DELAY_MS,
  MOBILE_MODIFIER_KEY_BUTTONS,
  MOBILE_POINTER_SENSITIVITY,
  MOBILE_SHORTCUT_BUTTONS,
  MOBILE_TEXT_INPUT_HINTS,
  applyMobileStatusRefreshResult,
  buildHeldIntentReleaseRequests,
  buildKeyChordRequests,
  buildKeyRequest,
  buildMobileReleaseAllRequests,
  buildMouseButtonRequest,
  buildMouseClickRequests,
  buildMouseDoubleClickRequests,
  buildMouseMoveRequest,
  buildMouseWheelRequest,
  buildTextCommitRequest,
  createHeldInputController,
  createHeldInputResetRegistry,
  createMobileHeldIntentTracker,
  createMobileCorrelationId,
  createMobileStatusRefreshController,
  createOrderedMobileRequestQueue,
  createPointerMoveCoalescer,
  createTwoFingerWheelAccumulator,
  formatMobileBackendStatus,
  formatMobileControllerError,
  formatMobileInjectResultStatus,
  isHeldControlActivationKey,
  isMobileTextCommitSupported,
  isTouchpadLongPressDrag,
  isTouchpadTap,
  isTwoFingerTap,
  nextPointerPosition,
  normalizeMobilePointerSensitivity,
  preventMobileGestureDefault,
  resolveMobileDisplayIdAt,
  shouldActivateHeldControlFromClick,
  shouldCommitMobileTextOnKeyDown,
  tauriInvocationForMobileRequest,
} from "./mobile-controller.mjs";
import { preventBrowserNavigationEvent } from "./desktop-shell.mjs";

const DAEMON_IPC_BRIDGE_ENDPOINT = "/__rshare/ipc";

type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

type PointerState = {
  x: number;
  y: number;
  minX: number;
  minY: number;
  width: number;
  height: number;
  displayId: string | null;
  displayEntries: Record<string, unknown>[];
};

type SendState = "idle" | "sending" | "ok" | "error";
type TouchPoint = { id: number; x: number; y: number };
type TwoFingerTapStart = { touches: TouchPoint[]; timeMs: number };
type HeldInputState = "Pressed" | "Released";
type MouseButtonName = "Left" | "Right" | "Middle" | "Back" | "Forward";
type MobileBackendStatus = ReturnType<typeof formatMobileBackendStatus>;
type HeldInputResetRegistry = ReturnType<typeof createHeldInputResetRegistry>;
type HeldIntentOutcome = "accepted" | "rejected" | "unknown";
type QueuedMobileRequest = { request: unknown; quiet: boolean; heldIntent: unknown };

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function responseVariant<T>(response: unknown, variant: string): T {
  if (isRecord(response)) {
    if (Object.prototype.hasOwnProperty.call(response, "Error")) {
      throw new Error(String(response.Error));
    }
    if (Object.prototype.hasOwnProperty.call(response, variant)) {
      return response[variant] as T;
    }
  }
  throw new Error(`Unexpected daemon response: ${JSON.stringify(response)}`);
}

function getTauriInvoke(): TauriInvoke | null {
  const tauriWindow = window as Window & {
    __TAURI__?: {
      core?: {
        invoke?: TauriInvoke;
      };
    };
  };

  return tauriWindow.__TAURI__?.core?.invoke ?? null;
}

async function daemonRequest(request: unknown): Promise<unknown> {
  const invoke = getTauriInvoke();
  const tauriInvocation = tauriInvocationForMobileRequest(request);
  if (invoke && tauriInvocation) {
    const payload = await invoke(tauriInvocation.command, tauriInvocation.args);
    return {
      [tauriInvocation.responseVariant]: payload,
    };
  }

  const response = await fetch(DAEMON_IPC_BRIDGE_ENDPOINT, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });
  const payload = await response.json().catch(() => null);
  if (!response.ok) {
    const message =
      isRecord(payload) && typeof payload.error === "string"
        ? payload.error
        : `HTTP ${response.status}`;
    throw new Error(message);
  }
  return payload;
}

async function sendInjectRequest(request: unknown) {
  return responseVariant(await daemonRequest(request), "EndpointInjectResult");
}

function pointerFromLocalControls(snapshot: unknown): PointerState {
  const record = isRecord(snapshot) ? snapshot : {};
  const mouse = isRecord(record.mouse) ? record.mouse : {};
  const display = isRecord(record.display) ? record.display : {};
  const displays = Array.isArray(display.displays) ? display.displays : [];
  const displayEntries = displays.filter(isRecord);
  const primary = displayEntries.find((item) => item.primary) ?? displayEntries[0];
  const primaryDisplay = isRecord(primary) ? primary : {};
  const minX = Number(display.virtual_x ?? primaryDisplay.x ?? 0);
  const minY = Number(display.virtual_y ?? primaryDisplay.y ?? 0);
  const width = Number(
    display.layout_width ?? display.primary_width ?? primaryDisplay.width ?? primaryDisplay.w ?? 1920,
  );
  const height = Number(
    display.layout_height ??
      display.primary_height ??
      primaryDisplay.height ??
      primaryDisplay.h ??
      1080,
  );

  return {
    x: Number(mouse.x ?? 0),
    y: Number(mouse.y ?? 0),
    minX: Number.isFinite(minX) ? Math.floor(minX) : 0,
    minY: Number.isFinite(minY) ? Math.floor(minY) : 0,
    width: Number.isFinite(width) && width > 0 ? Math.floor(width) : 1920,
    height: Number.isFinite(height) && height > 0 ? Math.floor(height) : 1080,
    displayId:
      typeof mouse.current_display_id === "string"
        ? mouse.current_display_id
        : typeof primaryDisplay.display_id === "string"
          ? primaryDisplay.display_id
        : typeof primaryDisplay.id === "string"
          ? primaryDisplay.id
          : null,
    displayEntries,
  };
}

function useHeldInputController(
  onState: (state: HeldInputState) => void,
  onLifecycleReset: () => void,
  resetRegistry: HeldInputResetRegistry,
) {
  const onStateRef = useRef(onState);
  onStateRef.current = onState;
  const onLifecycleResetRef = useRef(onLifecycleReset);
  onLifecycleResetRef.current = onLifecycleReset;
  const mountedRef = useRef(true);
  const [isPressed, setIsPressed] = useState(false);
  const controllerRef = useRef<ReturnType<typeof createHeldInputController> | null>(null);

  if (!controllerRef.current) {
    controllerRef.current = createHeldInputController((state: HeldInputState) => {
      resetRegistry.markChanged();
      if (mountedRef.current) {
        setIsPressed(state === "Pressed");
      }
      onStateRef.current(state);
    });
  }

  useEffect(() => {
    mountedRef.current = true;
    const resetSilently = () => {
      controllerRef.current?.resetSilently();
      if (mountedRef.current) {
        setIsPressed(false);
      }
      onLifecycleResetRef.current();
    };
    const unregisterSilentReset = resetRegistry.register(resetSilently);
    const releaseForLifecycle = () => {
      controllerRef.current?.releaseAll();
      onLifecycleResetRef.current();
    };
    const releaseWhenHidden = () => {
      if (document.visibilityState === "hidden") {
        releaseForLifecycle();
      }
    };

    window.addEventListener("blur", releaseForLifecycle);
    window.addEventListener("pagehide", releaseForLifecycle);
    document.addEventListener("visibilitychange", releaseWhenHidden);

    return () => {
      mountedRef.current = false;
      releaseForLifecycle();
      window.removeEventListener("blur", releaseForLifecycle);
      window.removeEventListener("pagehide", releaseForLifecycle);
      document.removeEventListener("visibilitychange", releaseWhenHidden);
      unregisterSilentReset();
    };
  }, [resetRegistry]);

  const controller = controllerRef.current;
  return {
    press: controller.press,
    release: controller.release,
    releaseAll: controller.releaseAll,
    releaseIfPointerStillDown: controller.releaseIfPointerStillDown,
    isPressed,
  };
}

async function fetchMobileControlState() {
  const response = await daemonRequest("LocalControls");
  const snapshot = responseVariant(response, "LocalControls");
  return {
    pointer: pointerFromLocalControls(snapshot),
    backendStatus: formatMobileBackendStatus(snapshot),
    textCommitSupported: isMobileTextCommitSupported(snapshot),
  };
}

function useMobileBrowserGuards() {
  useEffect(() => {
    const options: AddEventListenerOptions = { capture: true, passive: false };
    const handleBrowserNavigation = (event: Event) => {
      preventBrowserNavigationEvent(event);
    };
    const browserEventNames = [
      "mousedown",
      "mouseup",
      "auxclick",
      "pointerdown",
      "pointerup",
      "keydown",
    ];
    const gestureEventNames = [
      "contextmenu",
      "dragstart",
      "selectstart",
      "gesturestart",
      "gesturechange",
      "gestureend",
    ];

    for (const eventName of browserEventNames) {
      window.addEventListener(eventName, handleBrowserNavigation, options);
    }
    for (const eventName of gestureEventNames) {
      document.addEventListener(eventName, preventMobileGestureDefault, options);
    }

    return () => {
      for (const eventName of browserEventNames) {
        window.removeEventListener(eventName, handleBrowserNavigation, options);
      }
      for (const eventName of gestureEventNames) {
        document.removeEventListener(eventName, preventMobileGestureDefault, options);
      }
    };
  }, []);
}

export default function MobileController() {
  useMobileBrowserGuards();

  const [pointer, setPointer] = useState<PointerState>({
    x: 0,
    y: 0,
    minX: 0,
    minY: 0,
    width: 1920,
    height: 1080,
    displayId: null,
    displayEntries: [],
  });
  const [pointerSensitivity, setPointerSensitivity] = useState(() => {
    if (typeof window === "undefined") {
      return MOBILE_POINTER_SENSITIVITY.defaultValue;
    }
    try {
      return normalizeMobilePointerSensitivity(
        localStorage.getItem(MOBILE_POINTER_SENSITIVITY.storageKey),
      );
    } catch {
      return MOBILE_POINTER_SENSITIVITY.defaultValue;
    }
  });
  const [text, setText] = useState("");
  const [textCommitSupported, setTextCommitSupported] = useState(false);
  const [sendState, setSendState] = useState<SendState>("idle");
  const [status, setStatus] = useState("连接中");
  const [backendStatus, setBackendStatus] = useState<MobileBackendStatus>(() =>
    formatMobileBackendStatus(null),
  );
  const activePointerRef = useRef<number | null>(null);
  const lastPointRef = useRef<{ x: number; y: number } | null>(null);
  const tapStartRef = useRef<{ x: number; y: number; timeMs: number } | null>(null);
  const touchPointsRef = useRef<Map<number, TouchPoint>>(new Map());
  const lastWheelTouchesRef = useRef<TouchPoint[] | null>(null);
  const wheelAccumulatorRef = useRef(createTwoFingerWheelAccumulator());
  const twoFingerTapStartRef = useRef<TwoFingerTapStart | null>(null);
  const dragTimerRef = useRef<number | null>(null);
  const dragPointerRef = useRef<number | null>(null);
  const sensitivityRef = useRef(pointerSensitivity);
  const pointerRef = useRef(pointer);
  const heldInputResetRegistryRef = useRef<HeldInputResetRegistry | null>(null);
  const heldIntentTrackerRef = useRef<ReturnType<typeof createMobileHeldIntentTracker> | null>(
    null,
  );
  const sendRequestNowRef = useRef<(queued: QueuedMobileRequest) => Promise<boolean>>(async () =>
    false,
  );
  const requestQueueRef = useRef<ReturnType<typeof createOrderedMobileRequestQueue> | null>(null);
  const statusRefreshRef = useRef<ReturnType<
    typeof createMobileStatusRefreshController
  > | null>(null);
  const sendMoveNowRef = useRef<(next: PointerState) => Promise<boolean>>(async () => false);
  const moveCoalescerRef = useRef<ReturnType<typeof createPointerMoveCoalescer> | null>(null);

  if (!heldIntentTrackerRef.current) {
    heldIntentTrackerRef.current = createMobileHeldIntentTracker();
  }
  if (!heldInputResetRegistryRef.current) {
    heldInputResetRegistryRef.current = createHeldInputResetRegistry();
  }

  if (!requestQueueRef.current) {
    requestQueueRef.current = createOrderedMobileRequestQueue((queued: QueuedMobileRequest) =>
      sendRequestNowRef.current(queued),
    );
  }

  useEffect(() => {
    sensitivityRef.current = pointerSensitivity;
  }, [pointerSensitivity]);

  useEffect(() => {
    pointerRef.current = pointer;
  }, [pointer]);

  useEffect(() => {
    let cancelled = false;
    const refreshController = createMobileStatusRefreshController(
      async () => {
        try {
          return { state: await fetchMobileControlState(), error: null };
        } catch (error) {
          return { state: null, error };
        }
      },
      (
        result: {
          state: Awaited<ReturnType<typeof fetchMobileControlState>> | null;
          error: unknown;
        },
        options: { applyPointer: boolean; applyStatus: boolean },
      ) => {
        if (cancelled) {
          return;
        }
        applyMobileStatusRefreshResult(result, options, {
          applyPointer(next: PointerState) {
            pointerRef.current = next;
            setPointer(next);
          },
          applyBackendStatus(next: MobileBackendStatus) {
            setBackendStatus(next);
          },
          applyTextCommitSupported(next: boolean) {
            setTextCommitSupported(next);
          },
          applyStatus(next: string) {
            setStatus(next);
          },
          applyError(error: unknown) {
            setStatus(formatMobileControllerError(error, "移动端状态"));
          },
        });
      },
    );
    statusRefreshRef.current = refreshController;

    const refresh = () => {
      void refreshController.refresh();
    };
    refresh();
    const timer = window.setInterval(refresh, 1500);
    return () => {
      cancelled = true;
      if (statusRefreshRef.current === refreshController) {
        statusRefreshRef.current = null;
      }
      window.clearInterval(timer);
    };
  }, []);

  async function sendRequestNow({ request, quiet, heldIntent }: QueuedMobileRequest) {
    let outcome: HeldIntentOutcome = "unknown";
    if (!quiet) {
      setSendState("sending");
    }
    try {
      if ((heldIntent as { allowed?: boolean } | null)?.allowed === false) {
        outcome = "rejected";
        statusRefreshRef.current?.markStatusChanged();
        setSendState("error");
        setStatus("移动端注入请求失败：同时按住的按键数量超过安全限制");
        return false;
      }
      const result = await sendInjectRequest(request);
      const feedback = formatMobileInjectResultStatus(result);
      outcome = feedback.accepted ? "accepted" : "rejected";
      if (!feedback.accepted) {
        statusRefreshRef.current?.markStatusChanged();
        setSendState("error");
        setStatus(feedback.status);
        return false;
      }
      if (!quiet) {
        setSendState("ok");
      }
      statusRefreshRef.current?.markStatusChanged();
      setStatus(feedback.status);
      return true;
    } catch (error) {
      statusRefreshRef.current?.markStatusChanged();
      setSendState("error");
      setStatus(formatMobileControllerError(error, "移动端注入"));
      return false;
    } finally {
      heldIntentTrackerRef.current!.settle(heldIntent, outcome);
    }
  }

  sendRequestNowRef.current = sendRequestNow;

  function queuedRequest(request: unknown, quiet: boolean): QueuedMobileRequest {
    return {
      request,
      quiet,
      heldIntent: heldIntentTrackerRef.current!.provision(request),
    };
  }

  function sendRequest(request: unknown, quiet = false) {
    return requestQueueRef.current!.enqueue(queuedRequest(request, quiet)) as Promise<boolean>;
  }

  async function sendRequestBatch(requests: readonly unknown[], quiet = false) {
    const responses = (await requestQueueRef.current!.enqueueBatch(
      requests.map((request) => queuedRequest(request, quiet)),
    )) as boolean[];
    return responses.every(Boolean);
  }

  function releaseTrackedInputsForLifecycle() {
    const current = pointerRef.current;
    const requests = buildHeldIntentReleaseRequests(
      current.x,
      current.y,
      createMobileCorrelationId("mobile-lifecycle-release"),
      heldIntentTrackerRef.current!.snapshot(),
    );
    if (requests.length === 0) {
      return;
    }
    const resetRevision = heldInputResetRegistryRef.current!.capture();
    void sendRequestBatch(requests, true).then((released) => {
      if (released) {
        heldInputResetRegistryRef.current!.resetAll(resetRevision);
      }
    });
  }

  function sendMoveNow(next: PointerState) {
    const completePointerWrite = statusRefreshRef.current?.beginPointerWrite();
    return sendRequest(
      buildMouseMoveRequest(
        next.x,
        next.y,
        next.displayId,
        createMobileCorrelationId("mobile-move"),
      ),
      true,
    ).finally(() => completePointerWrite?.());
  }

  sendMoveNowRef.current = sendMoveNow;
  if (!moveCoalescerRef.current) {
    moveCoalescerRef.current = createPointerMoveCoalescer((next: PointerState) =>
      sendMoveNowRef.current(next),
    );
  }

  function sendMove(next: PointerState) {
    moveCoalescerRef.current?.schedule(next);
  }

  function clearDragTimer() {
    if (dragTimerRef.current != null) {
      window.clearTimeout(dragTimerRef.current);
      dragTimerRef.current = null;
    }
  }

  function sendDragButton(state: "Pressed" | "Released") {
    const current = pointerRef.current;
    return sendRequest(
      buildMouseButtonRequest(
        "Left",
        state,
        current.x,
        current.y,
        createMobileCorrelationId(`mobile-touchpad-drag-${state.toLowerCase()}`),
      ),
    );
  }

  function releaseTouchpadDrag(pointerId: number | null = null) {
    if (dragPointerRef.current == null) {
      return null;
    }
    if (pointerId != null && dragPointerRef.current !== pointerId) {
      return null;
    }
    dragPointerRef.current = null;
    moveCoalescerRef.current?.flush();
    return sendDragButton("Released");
  }

  function releaseTouchpadInteraction() {
    clearDragTimer();
    const finalMove = moveCoalescerRef.current?.flush() ?? Promise.resolve();
    const dragRelease = releaseTouchpadDrag();
    activePointerRef.current = null;
    lastPointRef.current = null;
    tapStartRef.current = null;
    lastWheelTouchesRef.current = null;
    wheelAccumulatorRef.current.reset();
    twoFingerTapStartRef.current = null;
    touchPointsRef.current.clear();
    statusRefreshRef.current?.setGestureActive(false);
    return dragRelease ?? finalMove;
  }

  function beginTouchpadDrag(pointerId: number) {
    dragTimerRef.current = null;
    if (
      activePointerRef.current !== pointerId ||
      dragPointerRef.current != null ||
      touchPointsRef.current.size !== 1
    ) {
      return;
    }
    const start = tapStartRef.current;
    const current = touchPointsRef.current.get(pointerId);
    if (
      !isTouchpadLongPressDrag(
        start,
        current
          ? {
              x: current.x,
              y: current.y,
              timeMs: Number(start?.timeMs ?? 0) + MOBILE_LONG_PRESS_DRAG_DELAY_MS,
            }
          : null,
      )
    ) {
      return;
    }
    dragPointerRef.current = pointerId;
    tapStartRef.current = null;
    void sendDragButton("Pressed");
  }

  function handlePointerDown(event: React.PointerEvent<HTMLDivElement>) {
    touchPointsRef.current.set(event.pointerId, {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
    });
    statusRefreshRef.current?.setGestureActive(true);
    activePointerRef.current = event.pointerId;
    lastPointRef.current = { x: event.clientX, y: event.clientY };
    tapStartRef.current = { x: event.clientX, y: event.clientY, timeMs: event.timeStamp };
    event.currentTarget.setPointerCapture(event.pointerId);
    clearDragTimer();
    dragTimerRef.current = window.setTimeout(
      () => beginTouchpadDrag(event.pointerId),
      MOBILE_LONG_PRESS_DRAG_DELAY_MS,
    );
    if (touchPointsRef.current.size >= 2) {
      clearDragTimer();
      releaseTouchpadDrag();
      moveCoalescerRef.current?.flush();
      wheelAccumulatorRef.current.reset();
      const touches = touchPointsRef.current.size === 2 ? touchPointsSnapshot() : null;
      lastWheelTouchesRef.current = touches;
      twoFingerTapStartRef.current = touches ? { touches, timeMs: event.timeStamp } : null;
      activePointerRef.current = null;
      lastPointRef.current = null;
      tapStartRef.current = null;
    }
  }

  function handlePointerMove(event: React.PointerEvent<HTMLDivElement>) {
    if (touchPointsRef.current.has(event.pointerId)) {
      touchPointsRef.current.set(event.pointerId, {
        id: event.pointerId,
        x: event.clientX,
        y: event.clientY,
      });
    }
    if (touchPointsRef.current.size >= 2) {
      clearDragTimer();
      releaseTouchpadDrag();
      if (touchPointsRef.current.size > 2) {
        lastWheelTouchesRef.current = null;
        wheelAccumulatorRef.current.reset();
        twoFingerTapStartRef.current = null;
        return;
      }
      const currentTouches = touchPointsSnapshot();
      const wheelDelta = wheelAccumulatorRef.current.update(
        lastWheelTouchesRef.current,
        currentTouches,
      );
      lastWheelTouchesRef.current = currentTouches;
      if (wheelDelta) {
        twoFingerTapStartRef.current = null;
        wheel(wheelDelta.deltaY, wheelDelta.deltaX);
      }
      return;
    }
    if (activePointerRef.current !== event.pointerId || !lastPointRef.current) {
      return;
    }
    const tapStart = tapStartRef.current;
    if (
      tapStart &&
      Math.hypot(event.clientX - tapStart.x, event.clientY - tapStart.y) > 12
    ) {
      clearDragTimer();
    }
    const last = lastPointRef.current;
    lastPointRef.current = { x: event.clientX, y: event.clientY };
    const current = pointerRef.current;
    const nextPosition = nextPointerPosition(
      current,
      { dx: event.clientX - last.x, dy: event.clientY - last.y },
      {
        x: current.minX,
        y: current.minY,
        width: current.width,
        height: current.height,
        sensitivity: sensitivityRef.current,
      },
    );
    const next = {
      ...current,
      ...nextPosition,
      displayId: resolveMobileDisplayIdAt(
        current.displayEntries,
        nextPosition.x,
        nextPosition.y,
        current.displayId,
      ),
    };
    pointerRef.current = next;
    setPointer(next);
    sendMove(next);
  }

  function handlePointerUp(event: React.PointerEvent<HTMLDivElement>) {
    if (touchPointsRef.current.has(event.pointerId)) {
      touchPointsRef.current.set(event.pointerId, {
        id: event.pointerId,
        x: event.clientX,
        y: event.clientY,
      });
    }
    if (touchPointsRef.current.size === 2 && twoFingerTapStartRef.current) {
      const start = twoFingerTapStartRef.current;
      const currentTouches = touchPointsSnapshot();
      twoFingerTapStartRef.current = null;
      if (
        isTwoFingerTap(start.touches, currentTouches, {
          startTimeMs: start.timeMs,
          endTimeMs: event.timeStamp,
        })
      ) {
        void mouseClick("Right");
      }
    }
    if (activePointerRef.current === event.pointerId) {
      clearDragTimer();
      moveCoalescerRef.current?.flush();
      const tapStart = tapStartRef.current;
      tapStartRef.current = null;
      if (dragPointerRef.current === event.pointerId) {
        releaseTouchpadDrag(event.pointerId);
      } else if (
        isTouchpadTap(tapStart, {
          x: event.clientX,
          y: event.clientY,
          timeMs: event.timeStamp,
        })
      ) {
        void mouseClick("Left");
      }
      activePointerRef.current = null;
      lastPointRef.current = null;
    }
    touchPointsRef.current.delete(event.pointerId);
    if (touchPointsRef.current.size !== 2) {
      lastWheelTouchesRef.current = null;
      wheelAccumulatorRef.current.reset();
      twoFingerTapStartRef.current = null;
    }
    statusRefreshRef.current?.setGestureActive(touchPointsRef.current.size > 0);
  }

  function handlePointerCancel(event: React.PointerEvent<HTMLDivElement>) {
    if (activePointerRef.current === event.pointerId) {
      clearDragTimer();
      releaseTouchpadDrag(event.pointerId);
      moveCoalescerRef.current?.flush();
      tapStartRef.current = null;
      activePointerRef.current = null;
      lastPointRef.current = null;
    }
    touchPointsRef.current.delete(event.pointerId);
    if (touchPointsRef.current.size !== 2) {
      lastWheelTouchesRef.current = null;
      wheelAccumulatorRef.current.reset();
      twoFingerTapStartRef.current = null;
    }
    statusRefreshRef.current?.setGestureActive(touchPointsRef.current.size > 0);
  }

  useEffect(() => {
    function releaseMobileInteractionForLifecycle() {
      releaseTouchpadInteraction();
      releaseTrackedInputsForLifecycle();
    }
    function releaseMobileInteractionWhenHidden() {
      if (document.visibilityState === "hidden") {
        releaseMobileInteractionForLifecycle();
      }
    }

    window.addEventListener("blur", releaseMobileInteractionForLifecycle);
    window.addEventListener("pagehide", releaseMobileInteractionForLifecycle);
    document.addEventListener("visibilitychange", releaseMobileInteractionWhenHidden);

    return () => {
      releaseMobileInteractionForLifecycle();
      window.removeEventListener("blur", releaseMobileInteractionForLifecycle);
      window.removeEventListener("pagehide", releaseMobileInteractionForLifecycle);
      document.removeEventListener("visibilitychange", releaseMobileInteractionWhenHidden);
    };
  }, []);

  async function mouseClick(button: MouseButtonName) {
    moveCoalescerRef.current?.flush();
    const current = pointerRef.current;
    const requests = buildMouseClickRequests(
      button,
      current.x,
      current.y,
      createMobileCorrelationId(`mobile-${button.toLowerCase()}-click`),
    );
    return sendRequestBatch(requests);
  }

  async function mouseDoubleClick(
    button: MouseButtonName,
    correlationPrefix = `mobile-${button.toLowerCase()}-double-click`,
  ) {
    moveCoalescerRef.current?.flush();
    const current = pointerRef.current;
    const requests = buildMouseDoubleClickRequests(
      button,
      current.x,
      current.y,
      createMobileCorrelationId(correlationPrefix),
    );
    return sendRequestBatch(requests);
  }

  function mouseButton(button: MouseButtonName, state: "Pressed" | "Released") {
    moveCoalescerRef.current?.flush();
    return sendRequest(
      buildMouseButtonRequest(
        button,
        state,
        pointer.x,
        pointer.y,
        createMobileCorrelationId(`mobile-${button.toLowerCase()}-${state.toLowerCase()}`),
      ),
    );
  }

  function touchPointsSnapshot() {
    return [...touchPointsRef.current.values()].sort((left, right) => left.id - right.id);
  }

  function wheel(deltaY: number, deltaX = 0) {
    moveCoalescerRef.current?.flush();
    void sendRequest(
      buildMouseWheelRequest(
        deltaX,
        deltaY,
        pointer.x,
        pointer.y,
        createMobileCorrelationId("mobile-wheel"),
      ),
    );
  }

  function keyState(key: string, state: "Pressed" | "Released") {
    void sendRequest(
      buildKeyRequest(
        key,
        state,
        createMobileCorrelationId(`mobile-key-${key}-${state.toLowerCase()}`),
      ),
    );
  }

  async function keyChord(keys: readonly string[], id: string) {
    const requests = buildKeyChordRequests(
      [...keys],
      createMobileCorrelationId(`mobile-shortcut-${id}`),
    );
    return sendRequestBatch(requests);
  }

  async function releaseAllInputs() {
    releaseTouchpadInteraction();
    const current = pointerRef.current;
    const requests = buildMobileReleaseAllRequests(
      current.x,
      current.y,
      createMobileCorrelationId("mobile-release-all"),
      heldIntentTrackerRef.current!.snapshot(),
    );
    const resetRevision = heldInputResetRegistryRef.current!.capture();
    const released = await sendRequestBatch(requests);
    if (released) {
      heldInputResetRegistryRef.current!.resetAll(resetRevision);
    }
    return released;
  }

  async function commitText() {
    if (!textCommitSupported) {
      return;
    }
    const value = text;
    if (!value) {
      return;
    }
    const ok = await sendRequest(
      buildTextCommitRequest(value, createMobileCorrelationId("mobile-text")),
    );
    if (ok) {
      setText("");
    }
  }

  function handleSensitivityChange(event: React.ChangeEvent<HTMLInputElement>) {
    const next = normalizeMobilePointerSensitivity(event.currentTarget.value);
    sensitivityRef.current = next;
    setPointerSensitivity(next);
    try {
      localStorage.setItem(MOBILE_POINTER_SENSITIVITY.storageKey, String(next));
    } catch {
      // Storage can be unavailable in restricted browser modes.
    }
  }

  const statusColor =
    sendState === "error" ? "#f87171" : sendState === "sending" ? "#fbbf24" : "#47c27a";

  return (
    <main
      className="min-h-screen overflow-auto"
      style={{
        background: "#101214",
        color: "#edf2ef",
        overscrollBehavior: "none",
        WebkitTouchCallout: "none",
        fontFamily:
          'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      }}
    >
      <div className="mx-auto flex min-h-screen w-full max-w-[720px] flex-col gap-3 px-3 py-3">
        <header className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <MousePointer2 size={20} color="#47c27a" />
            <div>
              <div className="text-sm font-semibold">R-ShareMouse Mobile</div>
              <div className="text-xs" style={{ color: "#8f9b96" }}>
                {pointer.x}, {pointer.y} / {pointer.width}x{pointer.height}
              </div>
              <div
                className="max-w-[220px] truncate text-xs"
                title={backendStatus.detail}
                style={{ color: backendStatus.state === "ready" ? "#47c27a" : "#d6a64b" }}
              >
                {backendStatus.label} · {backendStatus.detail}
              </div>
            </div>
          </div>
          <div
            className="rounded-md px-2 py-1 text-xs"
            style={{ border: "1px solid #2a302d", color: statusColor }}
          >
            {status}
          </div>
        </header>

        <section
          className="min-h-0 flex-1 rounded-md"
          style={{ background: "#161a1c", border: "1px solid #29302d" }}
        >
          <div
            className="h-full min-h-[260px] touch-none select-none rounded-md sm:min-h-[360px]"
            style={{
              background:
                "linear-gradient(135deg, rgba(71,194,122,0.08), rgba(255,255,255,0.02))",
            }}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerCancel={handlePointerCancel}
            onPointerUp={handlePointerUp}
          >
            <div className="flex h-full items-center justify-center">
              <div
                className="h-3 w-3 rounded-full"
                style={{ background: "#47c27a", boxShadow: "0 0 24px rgba(71,194,122,0.5)" }}
              />
            </div>
          </div>
        </section>

        <section
          className="flex items-center gap-3 rounded-md px-3 py-2"
          style={{ background: "#171b1d", border: "1px solid #29302d" }}
        >
          <label className="shrink-0 text-xs font-medium" htmlFor="mobile-pointer-sensitivity">
            灵敏度
          </label>
          <input
            id="mobile-pointer-sensitivity"
            type="range"
            aria-label="触控板灵敏度"
            min={MOBILE_POINTER_SENSITIVITY.min}
            max={MOBILE_POINTER_SENSITIVITY.max}
            step={MOBILE_POINTER_SENSITIVITY.step}
            value={pointerSensitivity}
            onChange={handleSensitivityChange}
          />
          <span className="w-10 text-right text-xs" style={{ color: "#8f9b96" }}>
            {pointerSensitivity.toFixed(2)}
          </span>
        </section>

        <section className="grid grid-cols-3 gap-2">
          <PressButton
            label="左键"
            onDown={() => mouseButton("Left", "Pressed")}
            onUp={() => mouseButton("Left", "Released")}
            resetRegistry={heldInputResetRegistryRef.current!}
          />
          <PressButton
            label="中键"
            onDown={() => mouseButton("Middle", "Pressed")}
            onUp={() => mouseButton("Middle", "Released")}
            resetRegistry={heldInputResetRegistryRef.current!}
          />
          <PressButton
            label="右键"
            onDown={() => mouseButton("Right", "Pressed")}
            onUp={() => mouseButton("Right", "Released")}
            resetRegistry={heldInputResetRegistryRef.current!}
          />
          <PressButton
            label="后退"
            onDown={() => mouseButton("Back", "Pressed")}
            onUp={() => mouseButton("Back", "Released")}
            resetRegistry={heldInputResetRegistryRef.current!}
          />
          <PressButton
            label="前进"
            onDown={() => mouseButton("Forward", "Pressed")}
            onUp={() => mouseButton("Forward", "Released")}
            resetRegistry={heldInputResetRegistryRef.current!}
          />
          <IconButton label="双击" onClick={() => void mouseDoubleClick("Left", "mobile-left-double-click")}>
            <span className="text-sm font-medium">双击</span>
          </IconButton>
          <IconButton label="释放全部" onClick={() => void releaseAllInputs()}>
            <span className="text-sm font-medium">释放全部</span>
          </IconButton>
        </section>

        <section className="grid grid-cols-4 gap-2">
          <IconButton label="上滚" onClick={() => wheel(3)}>
            <ArrowUp size={20} />
          </IconButton>
          <IconButton label="下滚" onClick={() => wheel(-3)}>
            <ArrowDown size={20} />
          </IconButton>
          <HoldKeyButton
            label="退格"
            keyboardKey="Backspace"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <Delete size={20} />
          </HoldKeyButton>
          <HoldKeyButton
            label="回车"
            keyboardKey="Enter"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <CornerDownLeft size={20} />
          </HoldKeyButton>
        </section>

        <section className="grid grid-cols-4 gap-2">
          <HoldKeyButton
            label="左"
            keyboardKey="Left"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <ArrowLeft size={20} />
          </HoldKeyButton>
          <HoldKeyButton
            label="上"
            keyboardKey="Up"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <ArrowUp size={20} />
          </HoldKeyButton>
          <HoldKeyButton
            label="下"
            keyboardKey="Down"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <ArrowDown size={20} />
          </HoldKeyButton>
          <HoldKeyButton
            label="右"
            keyboardKey="Right"
            onKeyState={keyState}
            resetRegistry={heldInputResetRegistryRef.current!}
          >
            <ArrowRight size={20} />
          </HoldKeyButton>
        </section>

        <section className="grid grid-cols-4 gap-2">
          {MOBILE_MODIFIER_KEY_BUTTONS.map((button) => (
            <HoldKeyButton
              key={button.key}
              label={button.label}
              keyboardKey={button.key}
              onKeyState={keyState}
              resetRegistry={heldInputResetRegistryRef.current!}
            >
              <span className="text-sm font-medium">{button.label}</span>
            </HoldKeyButton>
          ))}
        </section>

        <section className="grid grid-cols-4 gap-2">
          {MOBILE_EXTRA_KEY_BUTTONS.map((button) => (
            <HoldKeyButton
              key={button.key}
              label={button.label}
              keyboardKey={button.key}
              onKeyState={keyState}
              resetRegistry={heldInputResetRegistryRef.current!}
            >
              <span className="text-sm font-medium">{button.label}</span>
            </HoldKeyButton>
          ))}
        </section>

        <section className="grid grid-cols-4 gap-2">
          {MOBILE_SHORTCUT_BUTTONS.map((button) => (
            <button
              key={button.id}
              className="h-12 rounded-md text-sm font-medium"
              style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
              title={button.label}
              onClick={() => void keyChord(button.keys, button.id)}
            >
              {button.label}
            </button>
          ))}
        </section>

        <section>
          <div className="flex gap-2">
            <div
              className="flex min-w-0 flex-1 items-start gap-2 rounded-md px-3 py-2"
              style={{ background: "#171b1d", border: "1px solid #29302d" }}
            >
              <Keyboard className="mt-2 shrink-0" size={18} color="#8f9b96" />
              <textarea
                {...MOBILE_TEXT_INPUT_HINTS}
                className="min-h-16 min-w-0 flex-1 resize-none bg-transparent py-1 text-base leading-snug outline-none disabled:cursor-not-allowed disabled:opacity-60"
                value={text}
                rows={3}
                placeholder="文本"
                style={{ color: "#edf2ef" }}
                disabled={!textCommitSupported}
                onChange={(event) => setText(event.target.value)}
                onKeyDown={(event) => {
                  if (shouldCommitMobileTextOnKeyDown(event)) {
                    event.preventDefault();
                    void commitText();
                  }
                }}
              />
            </div>
            <button
              className="flex w-14 items-center justify-center rounded-md disabled:cursor-not-allowed disabled:opacity-60"
              style={{ background: "#47c27a", color: "#07110b" }}
              title="发送"
              disabled={!textCommitSupported}
              onClick={() => void commitText()}
            >
              <Send size={20} />
            </button>
          </div>
          {!textCommitSupported && (
            <p className="mt-2 text-xs" style={{ color: "#d6a64b" }} role="status">
              当前输入后端不支持文本输入，请使用按键控制。
            </p>
          )}
        </section>
      </div>
    </main>
  );
}

function IconButton({
  children,
  label,
  onClick,
}: {
  children: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      className="flex h-12 items-center justify-center rounded-md"
      style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
      title={label}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function HoldKeyButton({
  children,
  label,
  keyboardKey,
  onKeyState,
  resetRegistry,
}: {
  children: React.ReactNode;
  label: string;
  keyboardKey: string;
  onKeyState: (key: string, state: "Pressed" | "Released") => void;
  resetRegistry: HeldInputResetRegistry;
}) {
  const suppressKeyboardClickRef = useRef(false);
  const resetSyntheticClickSuppression = () => {
    suppressKeyboardClickRef.current = false;
  };
  const held = useHeldInputController(
    (state) => onKeyState(keyboardKey, state),
    resetSyntheticClickSuppression,
    resetRegistry,
  );

  return (
    <button
      className="flex h-12 touch-none items-center justify-center rounded-md"
      style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
      title={label}
      aria-pressed={held.isPressed}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        held.press(event.pointerId);
      }}
      onPointerUp={(event) => held.release(event.pointerId)}
      onPointerCancel={(event) => held.release(event.pointerId)}
      onLostPointerCapture={(event) => held.release(event.pointerId)}
      onPointerLeave={(event) => held.releaseIfPointerStillDown(event.pointerId, event.buttons)}
      onKeyDown={(event) => {
        if (!isHeldControlActivationKey(event)) {
          return;
        }
        event.preventDefault();
        suppressKeyboardClickRef.current = true;
        if (event.repeat) {
          return;
        }
        held.press(-2);
      }}
      onKeyUp={(event) => {
        if (!isHeldControlActivationKey(event)) {
          return;
        }
        event.preventDefault();
        held.release(-2);
        window.setTimeout(resetSyntheticClickSuppression, 0);
      }}
      onBlur={() => {
        held.release(-2);
        resetSyntheticClickSuppression();
      }}
      onClick={(event) => {
        if (!shouldActivateHeldControlFromClick(event)) {
          return;
        }
        if (suppressKeyboardClickRef.current) {
          return;
        }
        held.press(-1);
        held.release(-1);
      }}
    >
      {children}
    </button>
  );
}

function PressButton({
  label,
  onDown,
  onUp,
  resetRegistry,
}: {
  label: string;
  onDown: () => void;
  onUp: () => void;
  resetRegistry: HeldInputResetRegistry;
}) {
  const suppressKeyboardClickRef = useRef(false);
  const resetSyntheticClickSuppression = () => {
    suppressKeyboardClickRef.current = false;
  };
  const held = useHeldInputController(
    (state) => {
      if (state === "Pressed") {
        onDown();
      } else {
        onUp();
      }
    },
    resetSyntheticClickSuppression,
    resetRegistry,
  );

  return (
    <button
      className="h-12 rounded-md text-sm font-medium"
      style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
      aria-pressed={held.isPressed}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        held.press(event.pointerId);
      }}
      onPointerCancel={(event) => held.release(event.pointerId)}
      onLostPointerCapture={(event) => held.release(event.pointerId)}
      onPointerLeave={(event) => held.releaseIfPointerStillDown(event.pointerId, event.buttons)}
      onPointerUp={(event) => held.release(event.pointerId)}
      onKeyDown={(event) => {
        if (!isHeldControlActivationKey(event)) {
          return;
        }
        event.preventDefault();
        suppressKeyboardClickRef.current = true;
        if (event.repeat) {
          return;
        }
        held.press(-2);
      }}
      onKeyUp={(event) => {
        if (!isHeldControlActivationKey(event)) {
          return;
        }
        event.preventDefault();
        held.release(-2);
        window.setTimeout(resetSyntheticClickSuppression, 0);
      }}
      onBlur={() => {
        held.release(-2);
        resetSyntheticClickSuppression();
      }}
      onClick={(event) => {
        if (!shouldActivateHeldControlFromClick(event)) {
          return;
        }
        if (suppressKeyboardClickRef.current) {
          return;
        }
        held.press(-1);
        held.release(-1);
      }}
    >
      {label}
    </button>
  );
}
