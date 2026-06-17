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
  buildKeyChordRequests,
  buildKeyRequest,
  buildMouseButtonRequest,
  buildMouseClickRequests,
  buildMouseMoveRequest,
  buildMouseWheelRequest,
  buildTextCommitRequest,
  createHeldInputController,
  createMobileCorrelationId,
  createPointerMoveCoalescer,
  formatMobileControllerError,
  isTouchpadLongPressDrag,
  isTouchpadTap,
  isTwoFingerTap,
  nextPointerPosition,
  normalizeMobilePointerSensitivity,
  preventMobileGestureDefault,
  shouldCommitMobileTextOnKeyDown,
  tauriInvocationForMobileRequest,
  twoFingerWheelDelta,
} from "./mobile-controller.mjs";
import { preventBrowserNavigationEvent } from "./desktop-shell.mjs";

const DAEMON_IPC_BRIDGE_ENDPOINT = "/__rshare/ipc";

type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

type PointerState = {
  x: number;
  y: number;
  width: number;
  height: number;
  displayId: string | null;
};

type SendState = "idle" | "sending" | "ok" | "error";
type TouchPoint = { id: number; x: number; y: number };
type TwoFingerTapStart = { touches: TouchPoint[]; timeMs: number };
type HeldInputState = "Pressed" | "Released";

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
  const primary = displays.find((item) => isRecord(item) && item.primary) ?? displays[0];
  const primaryDisplay = isRecord(primary) ? primary : {};
  const width = Number(display.primary_width ?? primaryDisplay.w ?? 1920);
  const height = Number(display.primary_height ?? primaryDisplay.h ?? 1080);

  return {
    x: Number(mouse.x ?? 0),
    y: Number(mouse.y ?? 0),
    width: Number.isFinite(width) && width > 0 ? Math.floor(width) : 1920,
    height: Number.isFinite(height) && height > 0 ? Math.floor(height) : 1080,
    displayId:
      typeof mouse.current_display_id === "string"
        ? mouse.current_display_id
        : typeof primaryDisplay.id === "string"
          ? primaryDisplay.id
          : null,
  };
}

function useHeldInputController(onState: (state: HeldInputState) => void) {
  const onStateRef = useRef(onState);
  onStateRef.current = onState;
  const controllerRef = useRef<ReturnType<typeof createHeldInputController> | null>(null);

  if (!controllerRef.current) {
    controllerRef.current = createHeldInputController((state: HeldInputState) =>
      onStateRef.current(state),
    );
  }

  useEffect(() => {
    const release = () => {
      controllerRef.current?.releaseAll();
    };
    const releaseWhenHidden = () => {
      if (document.visibilityState === "hidden") {
        release();
      }
    };

    window.addEventListener("blur", release);
    window.addEventListener("pagehide", release);
    document.addEventListener("visibilitychange", releaseWhenHidden);

    return () => {
      release();
      window.removeEventListener("blur", release);
      window.removeEventListener("pagehide", release);
      document.removeEventListener("visibilitychange", releaseWhenHidden);
    };
  }, []);

  return controllerRef.current;
}

async function fetchPointerState() {
  const response = await daemonRequest("LocalControls");
  return pointerFromLocalControls(responseVariant(response, "LocalControls"));
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
    width: 1920,
    height: 1080,
    displayId: null,
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
  const [sendState, setSendState] = useState<SendState>("idle");
  const [status, setStatus] = useState("连接中");
  const activePointerRef = useRef<number | null>(null);
  const lastPointRef = useRef<{ x: number; y: number } | null>(null);
  const tapStartRef = useRef<{ x: number; y: number; timeMs: number } | null>(null);
  const touchPointsRef = useRef<Map<number, TouchPoint>>(new Map());
  const lastWheelTouchesRef = useRef<TouchPoint[] | null>(null);
  const twoFingerTapStartRef = useRef<TwoFingerTapStart | null>(null);
  const dragTimerRef = useRef<number | null>(null);
  const dragPointerRef = useRef<number | null>(null);
  const sensitivityRef = useRef(pointerSensitivity);
  const pointerRef = useRef(pointer);
  const sendMoveNowRef = useRef<(next: PointerState) => void>(() => {});
  const moveCoalescerRef = useRef<ReturnType<typeof createPointerMoveCoalescer> | null>(null);

  useEffect(() => {
    sensitivityRef.current = pointerSensitivity;
  }, [pointerSensitivity]);

  useEffect(() => {
    pointerRef.current = pointer;
  }, [pointer]);

  useEffect(() => {
    let cancelled = false;
    async function refresh() {
      try {
        const next = await fetchPointerState();
        if (!cancelled) {
          setPointer(next);
          setStatus("已连接");
        }
      } catch (error) {
        if (!cancelled) {
          setStatus(formatMobileControllerError(error, "移动端状态"));
        }
      }
    }

    void refresh();
    const timer = window.setInterval(refresh, 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  async function sendRequest(request: unknown, quiet = false) {
    if (!quiet) {
      setSendState("sending");
    }
    try {
      await sendInjectRequest(request);
      if (!quiet) {
        setSendState("ok");
      }
      setStatus("已连接");
      return true;
    } catch (error) {
      setSendState("error");
      setStatus(formatMobileControllerError(error, "移动端注入"));
      return false;
    }
  }

  function sendMoveNow(next: PointerState) {
    void sendRequest(
      buildMouseMoveRequest(
        next.x,
        next.y,
        next.displayId,
        createMobileCorrelationId("mobile-move"),
      ),
      true,
    );
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
    void sendRequest(
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
      return;
    }
    if (pointerId != null && dragPointerRef.current !== pointerId) {
      return;
    }
    dragPointerRef.current = null;
    sendDragButton("Released");
  }

  function releaseTouchpadInteraction() {
    clearDragTimer();
    moveCoalescerRef.current?.flush();
    releaseTouchpadDrag();
    activePointerRef.current = null;
    lastPointRef.current = null;
    tapStartRef.current = null;
    lastWheelTouchesRef.current = null;
    twoFingerTapStartRef.current = null;
    touchPointsRef.current.clear();
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
    sendDragButton("Pressed");
  }

  function handlePointerDown(event: React.PointerEvent<HTMLDivElement>) {
    touchPointsRef.current.set(event.pointerId, {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
    });
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
        twoFingerTapStartRef.current = null;
        return;
      }
      const currentTouches = touchPointsSnapshot();
      const wheelDelta = twoFingerWheelDelta(lastWheelTouchesRef.current, currentTouches);
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
    const next = {
      ...current,
      ...nextPointerPosition(
        current,
        { dx: event.clientX - last.x, dy: event.clientY - last.y },
        { width: current.width, height: current.height, sensitivity: sensitivityRef.current },
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
      twoFingerTapStartRef.current = null;
    }
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
      twoFingerTapStartRef.current = null;
    }
  }

  useEffect(() => {
    function releaseTouchpadInteractionWhenHidden() {
      if (document.visibilityState === "hidden") {
        releaseTouchpadInteraction();
      }
    }

    window.addEventListener("blur", releaseTouchpadInteraction);
    window.addEventListener("pagehide", releaseTouchpadInteraction);
    document.addEventListener("visibilitychange", releaseTouchpadInteractionWhenHidden);

    return () => {
      releaseTouchpadInteraction();
      window.removeEventListener("blur", releaseTouchpadInteraction);
      window.removeEventListener("pagehide", releaseTouchpadInteraction);
      document.removeEventListener("visibilitychange", releaseTouchpadInteractionWhenHidden);
    };
  }, []);

  async function mouseClick(button: "Left" | "Right" | "Middle") {
    const current = pointerRef.current;
    const requests = buildMouseClickRequests(
      button,
      current.x,
      current.y,
      createMobileCorrelationId(`mobile-${button.toLowerCase()}-click`),
    );
    for (const request of requests) {
      await sendRequest(request);
    }
  }

  function mouseButton(button: "Left" | "Right" | "Middle", state: "Pressed" | "Released") {
    void sendRequest(
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
    for (const request of requests) {
      await sendRequest(request);
    }
  }

  async function commitText() {
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
          />
          <PressButton
            label="中键"
            onDown={() => mouseButton("Middle", "Pressed")}
            onUp={() => mouseButton("Middle", "Released")}
          />
          <PressButton
            label="右键"
            onDown={() => mouseButton("Right", "Pressed")}
            onUp={() => mouseButton("Right", "Released")}
          />
        </section>

        <section className="grid grid-cols-4 gap-2">
          <IconButton label="上滚" onClick={() => wheel(3)}>
            <ArrowUp size={20} />
          </IconButton>
          <IconButton label="下滚" onClick={() => wheel(-3)}>
            <ArrowDown size={20} />
          </IconButton>
          <HoldKeyButton label="退格" keyboardKey="Backspace" onKeyState={keyState}>
            <Delete size={20} />
          </HoldKeyButton>
          <HoldKeyButton label="回车" keyboardKey="Enter" onKeyState={keyState}>
            <CornerDownLeft size={20} />
          </HoldKeyButton>
        </section>

        <section className="grid grid-cols-4 gap-2">
          <HoldKeyButton label="左" keyboardKey="Left" onKeyState={keyState}>
            <ArrowLeft size={20} />
          </HoldKeyButton>
          <HoldKeyButton label="上" keyboardKey="Up" onKeyState={keyState}>
            <ArrowUp size={20} />
          </HoldKeyButton>
          <HoldKeyButton label="下" keyboardKey="Down" onKeyState={keyState}>
            <ArrowDown size={20} />
          </HoldKeyButton>
          <HoldKeyButton label="右" keyboardKey="Right" onKeyState={keyState}>
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

        <section className="flex gap-2">
          <div
            className="flex min-w-0 flex-1 items-start gap-2 rounded-md px-3 py-2"
            style={{ background: "#171b1d", border: "1px solid #29302d" }}
          >
            <Keyboard className="mt-2 shrink-0" size={18} color="#8f9b96" />
            <textarea
              {...MOBILE_TEXT_INPUT_HINTS}
              className="min-h-16 min-w-0 flex-1 resize-none bg-transparent py-1 text-base leading-snug outline-none"
              value={text}
              rows={3}
              placeholder="文本"
              style={{ color: "#edf2ef" }}
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
            className="flex w-14 items-center justify-center rounded-md"
            style={{ background: "#47c27a", color: "#07110b" }}
            title="发送"
            onClick={() => void commitText()}
          >
            <Send size={20} />
          </button>
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
}: {
  children: React.ReactNode;
  label: string;
  keyboardKey: string;
  onKeyState: (key: string, state: "Pressed" | "Released") => void;
}) {
  const held = useHeldInputController((state) => onKeyState(keyboardKey, state));

  return (
    <button
      className="flex h-12 touch-none items-center justify-center rounded-md"
      style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
      title={label}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        held.press(event.pointerId);
      }}
      onPointerUp={(event) => held.release(event.pointerId)}
      onPointerCancel={(event) => held.release(event.pointerId)}
      onPointerLeave={(event) => held.releaseIfPointerStillDown(event.pointerId, event.buttons)}
    >
      {children}
    </button>
  );
}

function PressButton({
  label,
  onDown,
  onUp,
}: {
  label: string;
  onDown: () => void;
  onUp: () => void;
}) {
  const held = useHeldInputController((state) => {
    if (state === "Pressed") {
      onDown();
    } else {
      onUp();
    }
  });

  return (
    <button
      className="h-12 rounded-md text-sm font-medium"
      style={{ background: "#171b1d", border: "1px solid #29302d", color: "#d8dedb" }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        held.press(event.pointerId);
      }}
      onPointerCancel={(event) => held.release(event.pointerId)}
      onPointerLeave={(event) => held.releaseIfPointerStillDown(event.pointerId, event.buttons)}
      onPointerUp={(event) => held.release(event.pointerId)}
    >
      {label}
    </button>
  );
}
