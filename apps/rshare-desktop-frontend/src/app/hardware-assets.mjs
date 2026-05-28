export const BUILTIN_HARDWARE_ASSET_MANIFESTS = Object.freeze([
  "/assets/hardware/live2d/keyboard/manifest.json",
  "/assets/hardware/live2d/keyboard/gaming/manifest.json",
  "/assets/hardware/live2d/mouse/manifest.json",
  "/assets/hardware/live2d/mouse/gaming/manifest.json",
  "/assets/hardware/live2d/gamepad/manifest.json",
]);

export function normalizeHardwareAssetManifest(raw, baseUrlOrOptions = "") {
  const options =
    typeof baseUrlOrOptions === "string"
      ? { baseUrl: baseUrlOrOptions }
      : (baseUrlOrOptions ?? {});
  const baseUrl = options.baseUrl ?? "";
  const resolveUrl = options.resolveUrl;
  const baseSize = raw.base_size ?? raw.baseSize ?? { width: 1, height: 1 };
  return {
    id: String(raw.id),
    name: String(raw.name ?? raw.id),
    kind: String(raw.kind),
    schemaVersion: Number(raw.schema_version ?? raw.schemaVersion ?? 1),
    baseSize: {
      width: Number(baseSize.width ?? 1),
      height: Number(baseSize.height ?? 1),
    },
    layers: (raw.layers ?? []).map((layer) => ({
      id: String(layer.id),
      role: String(layer.role),
      render: layer.render ?? (layer.src ? "image" : "runtime"),
      src: layer.src ? resolveAssetUrl(baseUrl, layer.src, resolveUrl) : null,
      opacity: layer.opacity == null ? 1 : Number(layer.opacity),
    })),
    regions: (raw.regions ?? raw.hotspots ?? []).map(normalizeRegion),
    mask: raw.mask ?? null,
    readonly: Boolean(raw.readonly ?? raw.builtin),
  };
}

export function buildHardwareAssetChoices(assets = []) {
  return {
    keyboard: assets.filter((asset) => asset.kind === "keyboard").map(assetChoice),
    mouse: assets.filter((asset) => asset.kind === "mouse").map(assetChoice),
    gamepad: assets.filter((asset) => asset.kind === "gamepad").map(assetChoice),
  };
}

export function resolveActiveHardwareRegions(asset, activity = {}) {
  return (asset?.regions ?? []).filter((region) =>
    regionMatchesActivity(region, activity),
  );
}

export function resolveSelectedHardwareAsset(assets = [], kind, selectedId) {
  return (
    assets.find((asset) => asset.kind === kind && asset.id === selectedId) ??
    assets.find((asset) => asset.kind === kind) ??
    null
  );
}

export function buildGamepadAnalogFeedback(activity = {}) {
  const leftStick = normalizeStickFeedback(
    activity.leftStickX,
    activity.leftStickY,
  );
  const rightStick = normalizeStickFeedback(
    activity.rightStickX,
    activity.rightStickY,
  );
  const leftTrigger = normalizeTriggerFeedback(activity.leftTrigger);
  const rightTrigger = normalizeTriggerFeedback(activity.rightTrigger);
  const pressedButtons = uniqueTokens([
    ...(activity.pressedButtons ?? []),
    ...(leftTrigger.active ? ["LeftTrigger", "LT"] : []),
    ...(rightTrigger.active ? ["RightTrigger", "RT"] : []),
    ...(leftStick.active ? ["LeftThumbstick", "LeftStick", "LS"] : []),
    ...(rightStick.active ? ["RightThumbstick", "RightStick", "RS"] : []),
  ]);

  return {
    leftStick,
    rightStick,
    leftTrigger,
    rightTrigger,
    pressedButtons,
  };
}

function resolveAssetUrl(baseUrl, src, resolveUrl) {
  if (/^([a-z][a-z0-9+.-]*:|\/)/i.test(src)) {
    return src;
  }
  if (typeof resolveUrl === "function") {
    return resolveUrl(src);
  }
  return `${baseUrl.replace(/\/?$/, "/")}${src}`;
}

function normalizeRegion(region) {
  return {
    id: String(region.id),
    label: String(region.label ?? region.id),
    action: region.action ?? inferLegacyAction(region),
    shape: normalizeShape(region.shape ?? legacyRectShape(region)),
  };
}

function normalizeShape(shape) {
  if (shape?.kind === "polygon") {
    return {
      kind: "polygon",
      points: (shape.points ?? []).map((point) => ({
        x: Number(point.x ?? 0),
        y: Number(point.y ?? 0),
      })),
    };
  }
  return {
    kind: "rect",
    x: Number(shape?.x ?? 0),
    y: Number(shape?.y ?? 0),
    w: Number(shape?.w ?? 0),
    h: Number(shape?.h ?? 0),
    radius: Number(shape?.radius ?? 7),
  };
}

function assetChoice(asset) {
  return {
    id: asset.id,
    name: asset.name,
    kind: asset.kind,
    readonly: Boolean(asset.readonly),
  };
}

function regionMatchesActivity(region, activity) {
  switch (region.action?.kind) {
    case "keyboard_key":
      return keyboardActionMatches(region.action, activity);
    case "mouse_button":
      return mouseActionMatches(region.action, activity);
    case "gamepad_button":
      return gamepadActionMatches(region.action, activity);
    default:
      return false;
  }
}

function keyboardActionMatches(action, activity) {
  const candidates = new Set((action.codes ?? []).map(normalizeKeyToken));
  const pressedKeys = activity.pressedKeys ?? [];
  if (pressedKeys.some((key) => candidates.has(normalizeKeyToken(key)))) {
    return true;
  }
  const latestEvent = [...(activity.keyboardEvents ?? [])].reverse().find((event) => {
    const key = keyboardEventKey(event);
    return key ? candidates.has(normalizeKeyToken(key)) : false;
  });
  return latestEvent ? keyboardEventIsPressed(latestEvent) : false;
}

function mouseActionMatches(action, activity) {
  const candidates = new Set((action.buttons ?? []).map(normalizeButtonToken));
  for (const [button, active] of Object.entries(mouseBooleanState(activity))) {
    if (active && candidates.has(normalizeButtonToken(button))) {
      return true;
    }
  }
  const buttons = [
    ...(activity.pressedButtons ?? []),
    ...(activity.recentButtons ?? []),
  ];
  return buttons.some((button) => candidates.has(normalizeButtonToken(button)));
}

function mouseBooleanState(activity) {
  return {
    Left: Boolean(activity.leftDown),
    Right: Boolean(activity.rightDown),
    Middle: Boolean(activity.middleDown || activity.wheelActive),
    Wheel: Boolean(activity.middleDown || activity.wheelActive),
    Back: Boolean(activity.backDown),
    Forward: Boolean(activity.forwardDown),
  };
}

function gamepadActionMatches(action, activity) {
  const candidates = new Set((action.buttons ?? []).map(normalizeButtonToken));
  const feedback = buildGamepadAnalogFeedback(activity);
  return feedback.pressedButtons.some((button) =>
    candidates.has(normalizeButtonToken(button)),
  );
}

function normalizeStickFeedback(rawX, rawY) {
  const x = normalizeGamepadAxis(rawX);
  const y = normalizeGamepadAxis(rawY);
  const magnitude = Math.min(1, Math.sqrt(x * x + y * y));
  return {
    x,
    y,
    magnitude,
    active: magnitude > 0.06,
  };
}

function normalizeTriggerFeedback(rawValue) {
  const value = normalizeGamepadTrigger(rawValue);
  return {
    value,
    active: value > 0.02,
  };
}

function normalizeGamepadAxis(value) {
  const numeric = Number(value ?? 0);
  if (!Number.isFinite(numeric)) {
    return 0;
  }
  if (Math.abs(numeric) <= 1) {
    return clamp(numeric, -1, 1);
  }
  return clamp(numeric / 32767, -1, 1);
}

function normalizeGamepadTrigger(value) {
  const numeric = Number(value ?? 0);
  if (!Number.isFinite(numeric)) {
    return 0;
  }
  if (numeric >= 0 && numeric <= 1) {
    return clamp(numeric, 0, 1);
  }
  return clamp(numeric / 65535, 0, 1);
}

function uniqueTokens(values) {
  const seen = new Set();
  const result = [];
  for (const value of values) {
    const token = String(value ?? "");
    if (!token || seen.has(token)) {
      continue;
    }
    seen.add(token);
    result.push(token);
  }
  return result;
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function normalizeKeyToken(value) {
  return String(value ?? "").toLowerCase().replace(/\s/g, "");
}

function normalizeButtonToken(value) {
  return String(value ?? "").toLowerCase().replace(/[\s_-]/g, "");
}

function keyboardEventKey(event) {
  if (!event || event.device_kind !== "Keyboard") {
    return null;
  }
  if (event.payload?.key) {
    return normalizeIncomingKeyName(event.payload.key);
  }
  const match = String(event.summary ?? "").match(
    /Key\s+(.+?)\s+(Pressed|Released|Down|Up)$/i,
  );
  return normalizeIncomingKeyName(match?.[1] ?? null);
}

function keyboardEventIsPressed(event) {
  const state = event?.payload?.state ?? event?.summary ?? "";
  return /\b(pressed|down)\b/i.test(state);
}

function normalizeIncomingKeyName(value) {
  if (!value) {
    return null;
  }
  const letter = String(value).match(/^Key([A-Z])$/i);
  if (letter) {
    return `Char(${letter[1].toUpperCase().charCodeAt(0)})`;
  }
  const digit = String(value).match(/^Num([0-9])$/i);
  if (digit) {
    return `Char(${digit[1].charCodeAt(0)})`;
  }
  return String(value);
}

function inferLegacyAction(region) {
  if (Array.isArray(region.codes)) {
    return { kind: "keyboard_key", codes: region.codes };
  }
  return { kind: "mouse_button", buttons: [region.id, region.label].filter(Boolean) };
}

function legacyRectShape(region) {
  return {
    kind: "rect",
    x: Number(region.x ?? 0),
    y: Number(region.y ?? 0),
    w: Number(region.w ?? 0),
    h: Number(region.h ?? 0),
    radius: Number(region.radius ?? 7),
  };
}
