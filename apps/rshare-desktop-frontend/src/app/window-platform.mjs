const MACOS_PATTERN = /macintosh|mac os x|macintel|macppc|mac68k/i;
const WINDOWS_PATTERN = /windows|win32|win64|wow64/i;
const LINUX_PATTERN = /linux|x11/i;

function getPlatformHint(navigatorLike) {
  if (!navigatorLike || typeof navigatorLike !== "object") {
    return "";
  }

  return String(
    navigatorLike.userAgentData?.platform ??
      navigatorLike.platform ??
      navigatorLike.userAgent ??
      "",
  );
}

/**
 * Returns the desktop platform without depending on Tauri or browser globals.
 * Passing a navigator-like value keeps SSR and node:test callers deterministic.
 */
export function getDesktopPlatform(navigatorLike = globalThis.navigator) {
  const platformHint = getPlatformHint(navigatorLike);
  if (MACOS_PATTERN.test(platformHint)) {
    return "macos";
  }
  if (WINDOWS_PATTERN.test(platformHint)) {
    return "windows";
  }
  if (LINUX_PATTERN.test(platformHint)) {
    return "linux";
  }
  return "unknown";
}

export function getDesktopShellState(navigatorLike = globalThis.navigator) {
  const platform = getDesktopPlatform(navigatorLike);
  const isMacOS = platform === "macos";

  return {
    platform,
    isMacOS,
    rootClassName: isMacOS
      ? "rshare-desktop-shell--macos"
      : "rshare-desktop-shell--standard",
    dataDesktopPlatform: platform,
    dataMacosFrameless: String(isMacOS),
  };
}

/**
 * Models Tauri 2.11's data-tauri-drag-region path contract. The real drag and
 * macOS double-click handling remains Tauri-owned; this exists to keep our
 * markup choices covered without attaching a second event handler.
 */
export function isTauriDragRegionPath(path) {
  for (let index = 0; index < path.length; index += 1) {
    const element = path[index] ?? {};
    const attr = element.dragRegion;

    if (element.clickable && attr == null) {
      return false;
    }
    if (attr == null) {
      continue;
    }
    if (attr === "false") {
      return false;
    }
    if (attr === "deep") {
      return true;
    }
    if (attr === "" || attr === "true") {
      return index === 0;
    }
  }

  return false;
}

export function getTitlebarDragRegionAttributes() {
  return {
    root: "true",
    blank: "deep",
    interactive: "false",
  };
}
