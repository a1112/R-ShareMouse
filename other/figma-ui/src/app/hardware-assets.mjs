export const BUILTIN_HARDWARE_ASSET_MANIFESTS = Object.freeze([
  "/assets/hardware/live2d/keyboard/manifest.json",
  "/assets/hardware/live2d/keyboard/gaming/manifest.json",
  "/assets/hardware/live2d/mouse/manifest.json",
  "/assets/hardware/live2d/mouse/gaming/manifest.json",
]);

export function normalizeHardwareAssetManifest(raw, baseUrl = "") {
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
      src: layer.src ? resolveAssetUrl(baseUrl, layer.src) : null,
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

function resolveAssetUrl(baseUrl, src) {
  if (/^(https?:|data:|blob:|\/)/i.test(src)) {
    return src;
  }
  return `${baseUrl.replace(/\/?$/, "/")}${src}`;
}

function normalizeRegion(region) {
  return {
    id: String(region.id),
    label: String(region.label ?? region.id),
    action: region.action ?? inferLegacyAction(region),
    shape: region.shape ?? legacyRectShape(region),
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
  if (activity.lastKey && candidates.has(normalizeKeyToken(activity.lastKey))) {
    return true;
  }
  return (activity.keyboardEvents ?? []).some((event) => {
    const key = keyboardEventKey(event);
    return key ? candidates.has(normalizeKeyToken(key)) : false;
  });
}

function mouseActionMatches(action, activity) {
  const candidates = new Set((action.buttons ?? []).map(normalizeButtonToken));
  const buttons = [
    ...(activity.pressedButtons ?? []),
    ...(activity.recentButtons ?? []),
  ];
  return buttons.some((button) => candidates.has(normalizeButtonToken(button)));
}

function gamepadActionMatches(action, activity) {
  const candidates = new Set((action.buttons ?? []).map(normalizeButtonToken));
  return (activity.pressedButtons ?? []).some((button) =>
    candidates.has(normalizeButtonToken(button)),
  );
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
