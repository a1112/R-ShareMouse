export const FIGMA_DESKTOP_THEME = {
  frame: "#2b2b2b",
  toolbar: "#2b2b2b",
  sidebar: "#252525",
  canvas: "#1c1c1c",
  border: "#3a3a3a",
  text: "#e0e0e0",
  textSub: "#a0a0a0",
  textMuted: "#6a6a6a",
  accent: "#5b8bd6",
  accentSoft: "rgba(91, 139, 214, 0.18)",
  success: "#49b35c",
  danger: "#c53030",
  gridDot: "rgba(255,255,255,0.08)",
  panelShadow: "0 18px 50px rgba(0,0,0,0.22)",
};

export const FIGMA_DESKTOP_LIGHT_THEME = {
  frame: "#e2e2e2",
  toolbar: "#e8e8e8",
  sidebar: "#eaeaea",
  canvas: "#dcdcdc",
  border: "#d0d0d0",
  text: "#2b2b2b",
  textSub: "#555555",
  textMuted: "#888888",
  accent: "#4d7ed6",
  accentSoft: "rgba(77, 126, 214, 0.14)",
  success: "#2f9a48",
  danger: "#c53030",
  gridDot: "rgba(0,0,0,0.08)",
  panelShadow: "0 14px 34px rgba(0,0,0,0.08)",
};

export function buildPageChrome(page, theme = FIGMA_DESKTOP_THEME) {
  if (page === "layout") {
    return {
      fullBleed: true,
      contentPadding: 0,
      surface: theme.frame,
      panel: theme.frame,
      border: theme.border,
    };
  }

  return {
    fullBleed: false,
    contentPadding: 20,
    surface: theme.canvas,
    panel: theme.sidebar,
    border: theme.border,
  };
}

export function getDesktopTheme(isDark) {
  return isDark ? FIGMA_DESKTOP_THEME : FIGMA_DESKTOP_LIGHT_THEME;
}
