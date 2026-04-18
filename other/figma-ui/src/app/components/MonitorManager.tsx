import React, { useState, useRef, useCallback, useEffect, useMemo } from "react";
import {
  Settings,
  Info,
  Monitor as MonitorIcon,
  Laptop,
  Plus,
  Link,
  RotateCcw,
  ZoomIn,
  ZoomOut,
  Maximize2,
  ChevronDown,
  ChevronRight,
  Wifi,
  WifiOff,
  Move,
  Layers,
  Eye,
  EyeOff,
  Sun,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  ArrowUp,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
} from "lucide-react";

/* ---------- Types ---------- */
export interface MonitorData {
  id: string;
  label: string;
  name: string;
  deviceId: string;
  resWidth: number;
  resHeight: number;
  color: string;
  x: number;
  y: number;
  w: number;
  h: number;
  primary: boolean;
  enabled: boolean;
}

export interface DeviceData {
  id: string;
  name: string;
  color: string;
  online: boolean;
  type: "desktop" | "laptop";
  expanded: boolean;
  connected?: boolean;
}

/* ---------- Theme ---------- */
interface Theme {
  bg: string;
  sidebarBg: string;
  border: string;
  canvasBg: string;
  text: string;
  textSub: string;
  textMuted: string;
  toolbarBg: string;
  btnBg: string;
  btnHover: string;
  selectedBg: string;
  hoverBg: string;
  dot: string;
  snapColor: string;
  selectBorder: string;
  statusBg: string;
  linkIcon: string;
}

const darkTheme: Theme = {
  bg: "#2b2b2b",
  sidebarBg: "#252525",
  border: "#3a3a3a",
  canvasBg: "#1c1c1c",
  text: "#e0e0e0",
  textSub: "#a0a0a0",
  textMuted: "#6a6a6a",
  toolbarBg: "#2b2b2b",
  btnBg: "#3a3a3a",
  btnHover: "#4a4a4a",
  selectedBg: "#3a3a3a",
  hoverBg: "rgba(255,255,255,0.06)",
  dot: "#333333",
  snapColor: "#ff6b6b",
  selectBorder: "#b0b0b0",
  statusBg: "#252525",
  linkIcon: "#1c1c1c",
};

const lightTheme: Theme = {
  bg: "#e2e2e2",
  sidebarBg: "#eaeaea",
  border: "#d0d0d0",
  canvasBg: "#dcdcdc",
  text: "#2b2b2b",
  textSub: "#555555",
  textMuted: "#888888",
  toolbarBg: "#e8e8e8",
  btnBg: "#d6d6d6",
  btnHover: "#cbcbcb",
  selectedBg: "#cfd0d0",
  hoverBg: "rgba(0,0,0,0.06)",
  dot: "#c8c8c8",
  snapColor: "#e04068",
  selectBorder: "#777777",
  statusBg: "#e5e5e5",
  linkIcon: "#e8e8e8",
};

/* ---------- Constants ---------- */
const SNAP_DISTANCE = 12;
const SCALE_FACTOR = 0.12;

function resW(r: number) { return Math.round(r * SCALE_FACTOR); }
function resH(r: number) { return Math.round(r * SCALE_FACTOR); }

/* ---------- Initial Data ---------- */
const initialDevices: DeviceData[] = [
  { id: "dev1", name: "MS-WORKSTATION", color: "#5b8bd6", online: true, type: "desktop", expanded: true },
  { id: "dev2", name: "MacBook-Pro", color: "#49b35c", online: true, type: "laptop", expanded: true },
  { id: "dev3", name: "DESKTOP-HOME", color: "#d6a64b", online: false, type: "desktop", expanded: true },
];

const initialMonitors: MonitorData[] = [
  { id: "m1", label: "A", name: "DELL U2723QE", deviceId: "dev1", resWidth: 3840, resHeight: 2160, color: "#5b8bd6", x: 80, y: 180, w: resW(3840), h: resH(2160), primary: true, enabled: true },
  { id: "m2", label: "B", name: "DELL P2419H", deviceId: "dev1", resWidth: 1920, resHeight: 1080, color: "#5b8bd6", x: 80 + resW(3840), y: 180 + resH(2160) - resH(1080), w: resW(1920), h: resH(1080), primary: false, enabled: true },
  { id: "m3", label: "C", name: "Built-in Retina", deviceId: "dev2", resWidth: 2560, resHeight: 1600, color: "#49b35c", x: 80 + resW(3840) + resW(1920) + 4, y: 180 + resH(2160) - resH(1600), w: resW(2560), h: resH(1600), primary: true, enabled: true },
  { id: "m4", label: "D", name: "LG 27UL850", deviceId: "dev3", resWidth: 3840, resHeight: 2160, color: "#d6a64b", x: 80 + resW(3840) + resW(1920) + resW(2560) + 8, y: 180, w: resW(3840), h: resH(2160), primary: true, enabled: true },
];

/* ---------- SnapLine type ---------- */
interface SnapLine {
  orientation: "h" | "v";
  pos: number;
  start: number;
  end: number;
}

interface MonitorManagerProps {
  devices?: DeviceData[];
  monitors?: MonitorData[];
  statusText?: string;
  footerText?: string;
  isDark?: boolean;
  showThemeToggle?: boolean;
  showFooter?: boolean;
}

function normalizeDevice(device: DeviceData): DeviceData {
  return {
    ...device,
    type: device.type ?? "desktop",
    expanded: device.expanded ?? true,
  };
}

/* ---------- Component ---------- */
export default function MonitorManager({
  devices: externalDevices,
  monitors: externalMonitors,
  statusText,
  footerText,
  isDark: externalIsDark,
  showThemeToggle = true,
  showFooter = true,
}: MonitorManagerProps) {
  const [monitors, setMonitors] = useState<MonitorData[]>(
    externalMonitors && externalMonitors.length ? externalMonitors : initialMonitors
  );
  const [devices, setDevices] = useState<DeviceData[]>(
    externalDevices && externalDevices.length
      ? externalDevices.map(normalizeDevice)
      : initialDevices
  );
  const [dragging, setDragging] = useState<string | null>(null);
  const [dragOffset, setDragOffset] = useState({ x: 0, y: 0 });
  const [selected, setSelected] = useState<string | null>(
    (externalMonitors && externalMonitors[0]?.id) ?? "m1"
  );
  const [snapLines, setSnapLines] = useState<SnapLine[]>([]);
  const [zoom, setZoom] = useState(1);
  const [panOffset, setPanOffset] = useState({ x: 0, y: 0 });
  const [internalIsDark, setInternalIsDark] = useState(true);
  const [isPanning, setIsPanning] = useState(false);
  const [panStart, setPanStart] = useState({ x: 0, y: 0 });
  const [panOffsetStart, setPanOffsetStart] = useState({ x: 0, y: 0 });
  const [spaceHeld, setSpaceHeld] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const canvasRef = useRef<HTMLDivElement>(null);

  const isDark = externalIsDark ?? internalIsDark;
  const t = isDark ? darkTheme : lightTheme;

  useEffect(() => {
    if (!externalMonitors || externalMonitors.length === 0) {
      return;
    }

    setMonitors(externalMonitors);
  }, [externalMonitors]);

  useEffect(() => {
    if (!externalDevices || externalDevices.length === 0) {
      return;
    }

    setDevices((prev) => {
      const expandedLookup = new Map(prev.map((device) => [device.id, device.expanded]));
      return externalDevices.map((device) => ({
        ...normalizeDevice(device),
        expanded: expandedLookup.get(device.id) ?? device.expanded ?? true,
      }));
    });
  }, [externalDevices]);

  useEffect(() => {
    if (!monitors.length) {
      setSelected(null);
      return;
    }

    if (!selected || !monitors.some((monitor) => monitor.id === selected)) {
      setSelected(monitors[0].id);
    }
  }, [monitors, selected]);

  /* ---- Snap logic ---- */
  const computeSnap = useCallback(
    (dragId: string, rawX: number, rawY: number, w: number, h: number) => {
      let bestX = rawX;
      let bestY = rawY;
      const lines: SnapLine[] = [];
      const others = monitors.filter((m) => m.id !== dragId && m.enabled);

      const dragEdges = {
        left: rawX, right: rawX + w, top: rawY, bottom: rawY + h,
        cx: rawX + w / 2, cy: rawY + h / 2,
      };

      let minDx = SNAP_DISTANCE + 1;
      let minDy = SNAP_DISTANCE + 1;

      for (const o of others) {
        const oEdges = {
          left: o.x, right: o.x + o.w, top: o.y, bottom: o.y + o.h,
          cx: o.x + o.w / 2, cy: o.y + o.h / 2,
        };

        const vPairs: [number, number][] = [
          [dragEdges.left, oEdges.left], [dragEdges.left, oEdges.right],
          [dragEdges.right, oEdges.left], [dragEdges.right, oEdges.right],
          [dragEdges.cx, oEdges.cx],
        ];
        for (const [dv, ov] of vPairs) {
          const dist = Math.abs(dv - ov);
          if (dist < SNAP_DISTANCE && dist <= minDx) {
            minDx = dist;
            bestX = rawX + (ov - dv);
            lines.push({ orientation: "v", pos: ov, start: Math.min(rawY, o.y), end: Math.max(rawY + h, o.y + o.h) });
          }
        }

        const hPairs: [number, number][] = [
          [dragEdges.top, oEdges.top], [dragEdges.top, oEdges.bottom],
          [dragEdges.bottom, oEdges.top], [dragEdges.bottom, oEdges.bottom],
          [dragEdges.cy, oEdges.cy],
        ];
        for (const [dv, ov] of hPairs) {
          const dist = Math.abs(dv - ov);
          if (dist < SNAP_DISTANCE && dist <= minDy) {
            minDy = dist;
            bestY = rawY + (ov - dv);
            lines.push({ orientation: "h", pos: ov, start: Math.min(rawX, o.x), end: Math.max(rawX + w, o.x + o.w) });
          }
        }
      }
      return { x: bestX, y: bestY, lines };
    },
    [monitors]
  );

  /* ---- Collision resolution ---- */
  const resolveCollision = useCallback(
    (dragId: string, x: number, y: number, w: number, h: number, rawX: number, rawY: number) => {
      const others = monitors.filter((m) => m.id !== dragId && m.enabled);

      const overlaps = (px: number, py: number) =>
        others.some(
          (o) => px < o.x + o.w && px + w > o.x && py < o.y + o.h && py + h > o.y
        );

      if (!overlaps(x, y)) return { x, y };

      // Find the nearest non-overlapping position by pushing to the closest edge
      let bestPos = { x: rawX, y: rawY };
      let bestDist = Infinity;

      for (const o of others) {
        // Skip if no overlap with this particular monitor
        const candidates = [
          { cx: o.x + o.w, cy: y },      // push right of o
          { cx: o.x - w, cy: y },         // push left of o
          { cx: x, cy: o.y + o.h },       // push below o
          { cx: x, cy: o.y - h },         // push above o
        ];

        for (const c of candidates) {
          if (!overlaps(c.cx, c.cy)) {
            const dist = Math.hypot(c.cx - rawX, c.cy - rawY);
            if (dist < bestDist) {
              bestDist = dist;
              bestPos = { x: c.cx, y: c.cy };
            }
          }
        }
      }

      // If still overlapping with multi-monitor scenarios, try combined edge pushes
      if (overlaps(bestPos.x, bestPos.y)) {
        for (const o1 of others) {
          const xCandidates = [o1.x + o1.w, o1.x - w];
          const yCandidates = [o1.y + o1.h, o1.y - h];
          for (const cx of xCandidates) {
            for (const cy of yCandidates) {
              if (!overlaps(cx, cy)) {
                const dist = Math.hypot(cx - rawX, cy - rawY);
                if (dist < bestDist) {
                  bestDist = dist;
                  bestPos = { x: cx, y: cy };
                }
              }
            }
          }
        }
      }

      return bestPos;
    },
    [monitors]
  );

  /* ---- Drag handlers ---- */
  const handleMouseDown = useCallback(
    (e: React.MouseEvent, id: string) => {
      e.preventDefault();
      e.stopPropagation();
      setSelected(id);
      const mon = monitors.find((m) => m.id === id);
      if (!mon || !canvasRef.current) return;
      const rect = canvasRef.current.getBoundingClientRect();
      setDragOffset({
        x: (e.clientX - rect.left) / zoom - panOffset.x - mon.x,
        y: (e.clientY - rect.top) / zoom - panOffset.y - mon.y,
      });
      setDragging(id);
    },
    [monitors, zoom, panOffset]
  );

  const handleMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!dragging || !canvasRef.current) return;
      const rect = canvasRef.current.getBoundingClientRect();
      const mon = monitors.find((m) => m.id === dragging);
      if (!mon) return;
      const rawX = (e.clientX - rect.left) / zoom - panOffset.x - dragOffset.x;
      const rawY = (e.clientY - rect.top) / zoom - panOffset.y - dragOffset.y;
      const { x, y, lines } = computeSnap(dragging, rawX, rawY, mon.w, mon.h);
      const resolved = resolveCollision(dragging, x, y, mon.w, mon.h, rawX, rawY);
      setSnapLines(resolved.x === x && resolved.y === y ? lines : []);
      setMonitors((prev) => prev.map((m) => (m.id === dragging ? { ...m, x: resolved.x, y: resolved.y } : m)));
    },
    [dragging, dragOffset, zoom, panOffset, computeSnap, resolveCollision, monitors]
  );

  const handleMouseUp = useCallback(() => {
    setDragging(null);
    setSnapLines([]);
  }, []);

  useEffect(() => {
    if (dragging) {
      window.addEventListener("mousemove", handleMouseMove);
      window.addEventListener("mouseup", handleMouseUp);
      return () => {
        window.removeEventListener("mousemove", handleMouseMove);
        window.removeEventListener("mouseup", handleMouseUp);
      };
    }
  }, [dragging, handleMouseMove, handleMouseUp]);

  /* ---- Panning: middle mouse / space+left click ---- */
  useEffect(() => {
    if (!isPanning) return;
    const onMove = (e: MouseEvent) => {
      const dx = (e.clientX - panStart.x) / zoom;
      const dy = (e.clientY - panStart.y) / zoom;
      setPanOffset({ x: panOffsetStart.x + dx, y: panOffsetStart.y + dy });
    };
    const onUp = () => setIsPanning(false);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [isPanning, panStart, panOffsetStart, zoom]);

  /* ---- Space key for panning mode ---- */
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.code === "Space" && !e.repeat) {
        e.preventDefault();
        setSpaceHeld(true);
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space") {
        e.preventDefault();
        setSpaceHeld(false);
        setIsPanning(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  /* ---- Zoom with mouse center ---- */
  const handleWheel = useCallback(
    (e: React.WheelEvent) => {
      e.preventDefault();
      if (!canvasRef.current) return;
      const rect = canvasRef.current.getBoundingClientRect();
      // Mouse position relative to canvas element
      const mouseX = e.clientX - rect.left;
      const mouseY = e.clientY - rect.top;

      const oldZoom = zoom;
      const delta = -e.deltaY * 0.001;
      const newZoom = Math.min(3, Math.max(0.3, oldZoom + delta));

      // World position under cursor before zoom
      const worldX = mouseX / oldZoom - panOffset.x;
      const worldY = mouseY / oldZoom - panOffset.y;

      // Adjust pan so world position stays under cursor after zoom
      const newPanX = mouseX / newZoom - worldX;
      const newPanY = mouseY / newZoom - worldY;

      setZoom(newZoom);
      setPanOffset({ x: newPanX, y: newPanY });
    },
    [zoom, panOffset]
  );

  /* ---- Auto-arrange ---- */
  const autoArrange = useCallback(() => {
    setMonitors((prev) => {
      const sorted = [...prev].sort((a, b) => {
        if (a.deviceId !== b.deviceId) return a.deviceId.localeCompare(b.deviceId);
        return a.primary ? -1 : 1;
      });
      let curX = 60;
      const baseY = 180;
      return sorted.map((m) => {
        const newM = { ...m, x: curX, y: baseY + (260 - m.h) };
        curX += m.w + 4;
        return newM;
      });
    });
  }, []);

  /* ---- Toggle monitor ---- */
  const toggleMonitor = useCallback((id: string) => {
    setMonitors((prev) => prev.map((m) => (m.id === id ? { ...m, enabled: !m.enabled } : m)));
  }, []);

  /* ---- Check adjacency ---- */
  const getAdjacentPairs = useMemo(() => {
    const pairs: [string, string][] = [];
    for (let i = 0; i < monitors.length; i++) {
      for (let j = i + 1; j < monitors.length; j++) {
        const a = monitors[i], b = monitors[j];
        if (!a.enabled || !b.enabled) continue;
        const touchRight = Math.abs(a.x + a.w - b.x) < 6;
        const touchLeft = Math.abs(b.x + b.w - a.x) < 6;
        const touchBottom = Math.abs(a.y + a.h - b.y) < 6;
        const touchTop = Math.abs(b.y + b.h - a.y) < 6;
        const overlapH = a.y < b.y + b.h && a.y + a.h > b.y;
        const overlapV = a.x < b.x + b.w && a.x + a.w > b.x;
        if ((touchRight || touchLeft) && overlapH) pairs.push([a.id, b.id]);
        if ((touchBottom || touchTop) && overlapV) pairs.push([a.id, b.id]);
      }
    }
    return pairs;
  }, [monitors]);

  const connectionLines = useMemo(() => {
    return getAdjacentPairs.map(([aId, bId]) => {
      const a = monitors.find((m) => m.id === aId)!;
      const b = monitors.find((m) => m.id === bId)!;
      return {
        aId, bId,
        x1: a.x + a.w / 2, y1: a.y + a.h / 2,
        x2: b.x + b.w / 2, y2: b.y + b.h / 2,
        sameDevice: a.deviceId === b.deviceId,
        colorA: a.color, colorB: b.color,
      };
    });
  }, [getAdjacentPairs, monitors]);

  /* ---- Swap two monitors' positions ---- */
  const swapMonitors = useCallback((aId: string, bId: string) => {
    setMonitors((prev) => {
      const a = prev.find((m) => m.id === aId);
      const b = prev.find((m) => m.id === bId);
      if (!a || !b) return prev;

      // Determine spatial relationship: are they side-by-side (horizontal) or stacked (vertical)?
      const aCx = a.x + a.w / 2, aCy = a.y + a.h / 2;
      const bCx = b.x + b.w / 2, bCy = b.y + b.h / 2;
      const dx = Math.abs(aCx - bCx);
      const dy = Math.abs(aCy - bCy);

      let newAx: number, newAy: number, newBx: number, newBy: number;

      if (dx >= dy) {
        // Horizontal arrangement: preserve the left/right edges
        const aIsLeft = aCx < bCx;
        const leftMon = aIsLeft ? a : b;
        const rightMon = aIsLeft ? b : a;
        // After swap: the right one goes to left position, left one goes to right
        // New left position: starts where old left started
        const newLeftX = leftMon.x;
        // New right position: starts right after the new left monitor
        const newRightX = newLeftX + rightMon.w;
        // Align bottoms (common in monitor setups)
        const baseBottom = Math.max(leftMon.y + leftMon.h, rightMon.y + rightMon.h);

        if (aIsLeft) {
          // a was left, b was right -> now b goes left, a goes right
          newBx = newLeftX;
          newBy = baseBottom - b.h;
          newAx = newRightX;
          newAy = baseBottom - a.h;
        } else {
          // b was left, a was right -> now a goes left, b goes right
          newAx = newLeftX;
          newAy = baseBottom - a.h;
          newBx = newRightX;
          newBy = baseBottom - b.h;
        }
      } else {
        // Vertical arrangement: preserve the top/bottom edges
        const aIsTop = aCy < bCy;
        const topMon = aIsTop ? a : b;
        const bottomMon = aIsTop ? b : a;
        const newTopY = topMon.y;
        const newBottomY = newTopY + bottomMon.h;
        // Align left edges
        const baseLeft = Math.min(topMon.x, bottomMon.x);

        if (aIsTop) {
          // a was top, b was bottom -> now b goes top, a goes bottom
          newBx = baseLeft;
          newBy = newTopY;
          newAx = baseLeft;
          newAy = newBottomY;
        } else {
          newAx = baseLeft;
          newAy = newTopY;
          newBx = baseLeft;
          newBy = newBottomY;
        }
      }

      // Verify no overlap with other monitors
      const others = prev.filter((m) => m.id !== aId && m.id !== bId && m.enabled);
      const overlaps = (x: number, y: number, w: number, h: number) =>
        others.some((o) => x < o.x + o.w && x + w > o.x && y < o.y + o.h && y + h > o.y);

      // Also check the two swapped monitors don't overlap each other
      const selfOverlap = newAx < newBx + b.w && newAx + a.w > newBx && newAy < newBy + b.h && newAy + a.h > newBy;

      if (selfOverlap || overlaps(newAx, newAy, a.w, a.h) || overlaps(newBx, newBy, b.w, b.h)) {
        // Fallback: place them side by side from the leftmost position
        const minX = Math.min(a.x, b.x);
        const baseBottom = Math.max(a.y + a.h, b.y + b.h);
        const aWasLeft = aCx < bCx;
        if (aWasLeft) {
          newBx = minX; newBy = baseBottom - b.h;
          newAx = minX + b.w; newAy = baseBottom - a.h;
        } else {
          newAx = minX; newAy = baseBottom - a.h;
          newBx = minX + a.w; newBy = baseBottom - b.h;
        }
      }

      return prev.map((m) => {
        if (m.id === aId) return { ...m, x: newAx, y: newAy };
        if (m.id === bId) return { ...m, x: newBx, y: newBy };
        return m;
      });
    });
  }, []);

  const toggleDeviceExpand = (id: string) => {
    setDevices((prev) => prev.map((d) => (d.id === id ? { ...d, expanded: !d.expanded } : d)));
  };

  /* ---- Monitor fill colors adjusted for theme ---- */
  const monFill = (color: string) => isDark ? color + "1a" : color + "18";
  const monBorderIdle = (color: string) => isDark ? color + "55" : color + "66";
  const monBorderAdj = (color: string) => isDark ? color + "aa" : color + "bb";

  /* ---- Move monitor in direction, snapping to nearest neighbor edge ---- */
  const moveMonitor = useCallback((id: string, dir: "left" | "right" | "up" | "down") => {
    setMonitors((prev) => {
      const mon = prev.find((m) => m.id === id);
      if (!mon) return prev;
      const others = prev.filter((m) => m.id !== id && m.enabled);
      const STEP = 20;
      let bestX = mon.x;
      let bestY = mon.y;

      if (dir === "left" || dir === "right") {
        // Find nearest neighbor edge in horizontal direction
        let bestTarget = dir === "left" ? mon.x - STEP : mon.x + STEP;
        let bestDist = Infinity;
        for (const o of others) {
          if (dir === "right") {
            // Snap to right edge of o (place our left edge at o's right edge)
            const target = o.x + o.w;
            const dist = target - mon.x;
            if (dist > 0 && dist < bestDist) {
              // Check no overlap at this position
              const wouldOverlap = others.some(
                (oo) => target < oo.x + oo.w && target + mon.w > oo.x && mon.y < oo.y + oo.h && mon.y + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist; bestTarget = target; }
            }
            // Snap our right edge to o's left edge
            const target2 = o.x - mon.w;
            const dist2 = target2 - mon.x;
            if (dist2 > 0 && dist2 < bestDist) {
              const wouldOverlap = others.some(
                (oo) => target2 < oo.x + oo.w && target2 + mon.w > oo.x && mon.y < oo.y + oo.h && mon.y + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist2; bestTarget = target2; }
            }
          } else {
            // left: snap to left edge of o (place our right edge at o's left edge)
            const target = o.x - mon.w;
            const dist = mon.x - target;
            if (dist > 0 && dist < bestDist) {
              const wouldOverlap = others.some(
                (oo) => target < oo.x + oo.w && target + mon.w > oo.x && mon.y < oo.y + oo.h && mon.y + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist; bestTarget = target; }
            }
            // Snap our left edge to o's right edge
            const target2 = o.x + o.w;
            const dist2 = mon.x - target2;
            if (dist2 > 0 && dist2 < bestDist) {
              const wouldOverlap = others.some(
                (oo) => target2 < oo.x + oo.w && target2 + mon.w > oo.x && mon.y < oo.y + oo.h && mon.y + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist2; bestTarget = target2; }
            }
          }
        }
        bestX = bestTarget;
      } else {
        // up/down
        let bestTarget = dir === "up" ? mon.y - STEP : mon.y + STEP;
        let bestDist = Infinity;
        for (const o of others) {
          if (dir === "down") {
            const target = o.y + o.h;
            const dist = target - mon.y;
            if (dist > 0 && dist < bestDist) {
              const wouldOverlap = others.some(
                (oo) => mon.x < oo.x + oo.w && mon.x + mon.w > oo.x && target < oo.y + oo.h && target + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist; bestTarget = target; }
            }
            const target2 = o.y - mon.h;
            const dist2 = target2 - mon.y;
            if (dist2 > 0 && dist2 < bestDist) {
              const wouldOverlap = others.some(
                (oo) => mon.x < oo.x + oo.w && mon.x + mon.w > oo.x && target2 < oo.y + oo.h && target2 + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist2; bestTarget = target2; }
            }
          } else {
            const target = o.y - mon.h;
            const dist = mon.y - target;
            if (dist > 0 && dist < bestDist) {
              const wouldOverlap = others.some(
                (oo) => mon.x < oo.x + oo.w && mon.x + mon.w > oo.x && target < oo.y + oo.h && target + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist; bestTarget = target; }
            }
            const target2 = o.y + o.h;
            const dist2 = mon.y - target2;
            if (dist2 > 0 && dist2 < bestDist) {
              const wouldOverlap = others.some(
                (oo) => mon.x < oo.x + oo.w && mon.x + mon.w > oo.x && target2 < oo.y + oo.h && target2 + mon.h > oo.y
              );
              if (!wouldOverlap) { bestDist = dist2; bestTarget = target2; }
            }
          }
        }
        bestY = bestTarget;
      }

      return prev.map((m) => (m.id === id ? { ...m, x: bestX, y: bestY } : m));
    });
  }, []);

  return (
    <div className="w-full h-full flex flex-col select-none overflow-hidden" style={{ background: t.bg, color: t.text }}>
      {/* ===== Top Toolbar (full width) ===== */}
      <div className="h-[36px] flex items-center px-3 gap-2.5 shrink-0" style={{ borderBottom: `1px solid ${t.border}`, background: t.toolbarBg }}>
        <button
          onClick={() => setSidebarOpen(!sidebarOpen)}
          className="p-1 rounded transition-colors"
          style={{ backgroundColor: "transparent" }}
          onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = t.btnBg)}
          onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "transparent")}
          title={sidebarOpen ? "隐藏侧栏" : "显示侧栏"}
        >
          {sidebarOpen ? <PanelLeftClose size={14} style={{ color: t.textSub }} /> : <PanelLeftOpen size={14} style={{ color: t.textSub }} />}
        </button>

        <div className="w-px h-4 shrink-0" style={{ backgroundColor: t.border }} />

        <button
          onClick={autoArrange}
          className="flex items-center gap-1 px-2.5 py-[3px] rounded text-[11px] transition-colors"
          style={{ backgroundColor: t.btnBg }}
          onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = t.btnHover)}
          onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = t.btnBg)}
        >
          <RotateCcw size={11} /> 自动排列
        </button>
        <button
          className="flex items-center gap-1 px-2.5 py-[3px] rounded text-[11px] transition-colors"
          style={{ backgroundColor: t.btnBg }}
          onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = t.btnHover)}
          onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = t.btnBg)}
        >
          <Link size={11} /> 自动吸附
        </button>

        <div className="flex-1" />

        <span className="text-[10px]" style={{ color: t.textMuted }}>
          {statusText ?? `已连接 ${getAdjacentPairs.length} 对`}
        </span>

        <div className="flex items-center gap-0.5 ml-2">
          <ToolbarBtn theme={t} onClick={() => {
            if (!canvasRef.current) return;
            const rect = canvasRef.current.getBoundingClientRect();
            const cx = rect.width / 2, cy = rect.height / 2;
            const newZoom = Math.max(0.3, zoom - 0.15);
            const wx = cx / zoom - panOffset.x, wy = cy / zoom - panOffset.y;
            setPanOffset({ x: cx / newZoom - wx, y: cy / newZoom - wy });
            setZoom(newZoom);
          }}>
            <ZoomOut size={13} />
          </ToolbarBtn>
          <span className="text-[10px] w-[38px] text-center" style={{ color: t.textSub }}>{Math.round(zoom * 100)}%</span>
          <ToolbarBtn theme={t} onClick={() => {
            if (!canvasRef.current) return;
            const rect = canvasRef.current.getBoundingClientRect();
            const cx = rect.width / 2, cy = rect.height / 2;
            const newZoom = Math.min(3, zoom + 0.15);
            const wx = cx / zoom - panOffset.x, wy = cy / zoom - panOffset.y;
            setPanOffset({ x: cx / newZoom - wx, y: cy / newZoom - wy });
            setZoom(newZoom);
          }}>
            <ZoomIn size={13} />
          </ToolbarBtn>
          <ToolbarBtn theme={t} onClick={() => { setZoom(1); setPanOffset({ x: 0, y: 0 }); }}>
            <Maximize2 size={12} />
          </ToolbarBtn>
        </div>

        {showThemeToggle ? (
          <>
            <div className="w-px h-4 shrink-0" style={{ backgroundColor: t.border }} />

            <button
              className="p-1 rounded transition-colors"
              style={{ backgroundColor: "transparent" }}
              onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = t.btnBg)}
              onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "transparent")}
              onClick={() => setInternalIsDark(!isDark)}
              title="切换主题"
            >
              {isDark ? <Sun size={13} style={{ color: "#e8c468" }} /> : <Moon size={13} style={{ color: "#666" }} />}
            </button>
          </>
        ) : null}

      </div>

      {/* ===== Middle: Sidebar + Canvas ===== */}
      <div className="flex-1 flex min-h-0">
        {/* Left Panel */}
        <div
          className="shrink-0 flex flex-col overflow-hidden"
          style={{
            width: sidebarOpen ? 240 : 0,
            opacity: sidebarOpen ? 1 : 0,
            background: t.sidebarBg,
            borderRight: sidebarOpen ? `1px solid ${t.border}` : "none",
            transition: "width 200ms cubic-bezier(0.4,0,0.2,1), opacity 150ms ease",
          }}
        >
          <div className="w-[240px] flex flex-col h-full">
            <div className="h-[32px] flex items-center px-4 gap-2 shrink-0" style={{ borderBottom: `1px solid ${t.border}` }}>
              <Layers size={13} style={{ color: t.textMuted }} />
              <span className="text-[11px]" style={{ color: t.textSub }}>设备与显示器</span>
            </div>

            <div className="flex-1 overflow-y-auto py-2">
              {devices.map((dev) => {
                const devMonitors = monitors.filter((m) => m.deviceId === dev.id);
                return (
                  <div key={dev.id} className="mb-1">
                    <div
                      className="flex items-center gap-2 px-3 py-[6px] cursor-pointer transition-colors"
                      style={{ borderRadius: 4 }}
                      onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = t.hoverBg)}
                      onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "transparent")}
                      onClick={() => toggleDeviceExpand(dev.id)}
                    >
                      {dev.expanded ? <ChevronDown size={14} style={{ color: t.textMuted }} /> : <ChevronRight size={14} style={{ color: t.textMuted }} />}
                      {dev.type === "laptop" ? <Laptop size={14} style={{ color: dev.color }} /> : <MonitorIcon size={14} style={{ color: dev.color }} />}
                      <span className="text-[12px] flex-1 truncate">{dev.name}</span>
                      {dev.online ? <Wifi size={12} style={{ color: "#49b35c" }} /> : <WifiOff size={12} style={{ color: "#e04068" }} />}
                    </div>

                    {dev.expanded && (
                      <div className="ml-5">
                        {devMonitors.map((mon) => (
                          <div
                            key={mon.id}
                            className="flex items-center gap-2 px-3 py-[5px] cursor-pointer rounded-md mx-1 transition-colors"
                            style={{ background: selected === mon.id ? t.selectedBg : "transparent" }}
                            onMouseEnter={(e) => { if (selected !== mon.id) e.currentTarget.style.backgroundColor = t.hoverBg; }}
                            onMouseLeave={(e) => { if (selected !== mon.id) e.currentTarget.style.backgroundColor = "transparent"; }}
                            onClick={() => setSelected(mon.id)}
                          >
                            <div
                              className="w-[18px] h-[13px] rounded-[2px] border flex items-center justify-center"
                              style={{
                                borderColor: mon.color,
                                backgroundColor: mon.enabled ? mon.color + "33" : "transparent",
                                opacity: mon.enabled ? 1 : 0.4,
                              }}
                            >
                              <span className="text-[8px]" style={{ color: mon.color }}>{mon.label}</span>
                            </div>
                            <span className={`text-[11px] flex-1 truncate ${mon.enabled ? "" : "line-through opacity-40"}`}>{mon.name}</span>
                            <button
                              className="opacity-60 hover:opacity-100 transition-opacity"
                              onClick={(e) => { e.stopPropagation(); toggleMonitor(mon.id); }}
                            >
                              {mon.enabled ? <Eye size={12} /> : <EyeOff size={12} style={{ color: "#e04068" }} />}
                            </button>
                            {mon.primary && (
                              <span className="text-[9px] px-1 rounded" style={{ color: "#d6a64b", background: isDark ? "rgba(214,166,75,0.1)" : "rgba(214,166,75,0.15)" }}>
                                主
                              </span>
                            )}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>

            <div className="px-3 py-2 flex gap-2 shrink-0" style={{ borderTop: `1px solid ${t.border}` }}>
              <button className="flex items-center gap-1 text-[11px] transition-colors" style={{ color: "#5b8bd6" }}>
                <Plus size={13} /> 添加设备
              </button>
            </div>
          </div>
        </div>

        {/* Canvas */}
        <div
          ref={canvasRef}
          className="flex-1 relative overflow-hidden"
          style={{ background: t.canvasBg, cursor: isPanning ? "grabbing" : spaceHeld ? "grab" : dragging ? "grabbing" : "default" }}
          onWheel={handleWheel}
          onClick={() => { if (!isPanning) setSelected(null); }}
          onContextMenu={(e) => e.preventDefault()}
          onMouseDown={(e) => {
            // Middle mouse button pan
            if (e.button === 1) {
              e.preventDefault();
              setIsPanning(true);
              setPanStart({ x: e.clientX, y: e.clientY });
              setPanOffsetStart(panOffset);
              return;
            }
            // Right mouse button pan
            if (e.button === 2) {
              e.preventDefault();
              setIsPanning(true);
              setPanStart({ x: e.clientX, y: e.clientY });
              setPanOffsetStart(panOffset);
              return;
            }
            // Space + left click pan
            if (e.button === 0 && spaceHeld) {
              e.preventDefault();
              setIsPanning(true);
              setPanStart({ x: e.clientX, y: e.clientY });
              setPanOffsetStart(panOffset);
            }
          }}
          tabIndex={0}
        >
          {/* Grid dots */}
          <div
            className="absolute inset-0 pointer-events-none"
            style={{
              backgroundImage: `radial-gradient(circle, ${t.dot} 0.8px, transparent 0.8px)`,
              backgroundSize: `${30 * zoom}px ${30 * zoom}px`,
              backgroundPosition: `${panOffset.x * zoom}px ${panOffset.y * zoom}px`,
            }}
          />

          {/* Transform group */}
          <div
            className="absolute inset-0"
            style={{
              transform: `scale(${zoom}) translate(${panOffset.x}px, ${panOffset.y}px)`,
              transformOrigin: "0 0",
              pointerEvents: "none",
            }}
          >
            {/* Snap lines */}
            <svg className="absolute pointer-events-none" style={{ zIndex: 20, pointerEvents: "none", overflow: "visible", top: 0, left: 0, width: 0, height: 0 }}>
              {snapLines.map((sl, i) =>
                sl.orientation === "v" ? (
                  <line key={i} x1={sl.pos} y1={sl.start - 30} x2={sl.pos} y2={sl.end + 30} stroke={t.snapColor} strokeWidth={1} strokeDasharray="4 3" style={{ pointerEvents: "none" }} />
                ) : (
                  <line key={i} x1={sl.start - 30} y1={sl.pos} x2={sl.end + 30} y2={sl.pos} stroke={t.snapColor} strokeWidth={1} strokeDasharray="4 3" style={{ pointerEvents: "none" }} />
                )
              )}
            </svg>

            {/* Monitors */}
            {monitors.filter((m) => m.enabled).map((mon) => {
              const isSelected = selected === mon.id;
              const isDraggingThis = dragging === mon.id;
              const isAdj = getAdjacentPairs.some(([a, b]) => a === mon.id || b === mon.id);
              return (
                <div
                  key={mon.id}
                  onMouseDown={(e) => handleMouseDown(e, mon.id)}
                  onClick={(e) => { e.stopPropagation(); setSelected(mon.id); }}
                  className="absolute rounded-[5px] border-2 transition-shadow duration-150 flex flex-col items-center justify-center"
                  style={{
                    left: mon.x, top: mon.y, width: mon.w, height: mon.h,
                    backgroundColor: monFill(mon.color),
                    borderColor: isSelected ? t.selectBorder : isDraggingThis ? t.snapColor : isAdj ? monBorderAdj(mon.color) : monBorderIdle(mon.color),
                    cursor: isDraggingThis ? "grabbing" : "grab",
                    zIndex: isDraggingThis ? 15 : isSelected ? 12 : 5,
                    boxShadow: isSelected
                      ? `0 0 0 1px ${t.selectBorder}, 0 0 24px ${mon.color}22`
                      : isDraggingThis
                      ? `0 4px 20px ${isDark ? "rgba(0,0,0,0.5)" : "rgba(0,0,0,0.15)"}`
                      : `0 1px 4px ${isDark ? "rgba(0,0,0,0.3)" : "rgba(0,0,0,0.08)"}`,
                    backdropFilter: isDark ? "none" : "blur(2px)",
                    pointerEvents: "auto",
                  }}
                >
                  <div
                    className="w-[32px] h-[32px] rounded-full flex items-center justify-center mb-1"
                    style={{ backgroundColor: mon.color + "33", border: `2px solid ${mon.color}` }}
                  >
                    <span className="text-[14px]" style={{ color: mon.color, fontWeight: 700 }}>{mon.label}</span>
                  </div>
                  <span className="text-[10px] truncate max-w-[90%] text-center" style={{ color: t.textSub }}>{mon.name}</span>
                  <span className="text-[9px] mt-[2px]" style={{ color: t.textMuted }}>
                    {mon.resWidth}×{mon.resHeight}
                  </span>
                  {mon.primary && (
                    <div className="absolute top-1.5 right-1.5 w-[6px] h-[6px] rounded-full bg-[#d6a64b]" title="主显示器" />
                  )}
                  {/* Directional move buttons - visible when selected */}
                  {isSelected && !isDraggingThis && (
                    <>
                      {/* Up */}
                      <button
                        className="absolute flex items-center justify-center rounded-full transition-all opacity-0 hover:opacity-100"
                        style={{
                          top: -10, left: "50%", transform: "translateX(-50%)",
                          width: 20, height: 20,
                          backgroundColor: isDark ? "#3a3a3a" : "#e0e0e0",
                          border: `1px solid ${t.border}`,
                          cursor: "pointer", opacity: 0.7, zIndex: 20,
                        }}
                        onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                        onClick={(e) => { e.stopPropagation(); moveMonitor(mon.id, "up"); }}
                        title="上移"
                      >
                        <ArrowUp size={10} style={{ color: t.text }} />
                      </button>
                      {/* Down */}
                      <button
                        className="absolute flex items-center justify-center rounded-full transition-all"
                        style={{
                          bottom: -10, left: "50%", transform: "translateX(-50%)",
                          width: 20, height: 20,
                          backgroundColor: isDark ? "#3a3a3a" : "#e0e0e0",
                          border: `1px solid ${t.border}`,
                          cursor: "pointer", opacity: 0.7, zIndex: 20,
                        }}
                        onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                        onClick={(e) => { e.stopPropagation(); moveMonitor(mon.id, "down"); }}
                        title="下移"
                      >
                        <ArrowDown size={10} style={{ color: t.text }} />
                      </button>
                      {/* Left */}
                      <button
                        className="absolute flex items-center justify-center rounded-full transition-all"
                        style={{
                          left: -10, top: "50%", transform: "translateY(-50%)",
                          width: 20, height: 20,
                          backgroundColor: isDark ? "#3a3a3a" : "#e0e0e0",
                          border: `1px solid ${t.border}`,
                          cursor: "pointer", opacity: 0.7, zIndex: 20,
                        }}
                        onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                        onClick={(e) => { e.stopPropagation(); moveMonitor(mon.id, "left"); }}
                        title="左移"
                      >
                        <ArrowLeft size={10} style={{ color: t.text }} />
                      </button>
                      {/* Right */}
                      <button
                        className="absolute flex items-center justify-center rounded-full transition-all"
                        style={{
                          right: -10, top: "50%", transform: "translateY(-50%)",
                          width: 20, height: 20,
                          backgroundColor: isDark ? "#3a3a3a" : "#e0e0e0",
                          border: `1px solid ${t.border}`,
                          cursor: "pointer", opacity: 0.7, zIndex: 20,
                        }}
                        onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                        onClick={(e) => { e.stopPropagation(); moveMonitor(mon.id, "right"); }}
                        title="右移"
                      >
                        <ArrowRight size={10} style={{ color: t.text }} />
                      </button>
                    </>
                  )}
                  <div className="absolute top-1 left-1 opacity-30">
                    <Move size={10} />
                  </div>
                </div>
              );
            })}

            {/* Connection lines — rendered AFTER monitors so they appear on top */}
            <svg className="absolute inset-0 w-full h-full pointer-events-none" style={{ zIndex: 16, shapeRendering: "geometricPrecision" }}>
              {connectionLines.map((line, i) => (
                <g key={i}>
                  <line
                    x1={line.x1} y1={line.y1} x2={line.x2} y2={line.y2}
                    stroke={line.sameDevice ? (isDark ? "#5b8bd688" : "#5b8bd6") : (isDark ? "#49b35c88" : "#49b35c")}
                    strokeWidth={2}
                    strokeDasharray={line.sameDevice ? "none" : "6 4"}
                  />
                </g>
              ))}
            </svg>

            {/* Swap buttons — rendered AFTER lines so they appear on top */}
            {connectionLines.map((line, i) => {
              const cx = (line.x1 + line.x2) / 2;
              const cy = (line.y1 + line.y2) / 2;
              const baseColor = line.sameDevice ? "#5b8bd6" : "#49b35c";
              return (
                <div
                  key={`swap-${i}`}
                  className="absolute flex items-center justify-center rounded-full cursor-pointer transition-all duration-150"
                  style={{
                    left: cx - 10,
                    top: cy - 10,
                    width: 20,
                    height: 20,
                    backgroundColor: baseColor,
                    opacity: 0.85,
                    pointerEvents: "auto",
                    zIndex: 18,
                    boxShadow: `0 1px 4px rgba(0,0,0,0.3)`,
                  }}
                  onMouseEnter={(e) => {
                    e.currentTarget.style.transform = "scale(1.3)";
                    e.currentTarget.style.opacity = "1";
                    e.currentTarget.style.boxShadow = `0 0 10px ${baseColor}88`;
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.transform = "scale(1)";
                    e.currentTarget.style.opacity = "0.85";
                    e.currentTarget.style.boxShadow = `0 1px 4px rgba(0,0,0,0.3)`;
                  }}
                  onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                  onClick={(e) => { e.stopPropagation(); swapMonitors(line.aId, line.bId); }}
                  title="交换位置"
                >
                  <span style={{ color: "#fff", fontSize: 10, fontWeight: 700, lineHeight: 1 }}>⇆</span>
                </div>
              );
            })}
          </div>
        </div>
      </div>

      {showFooter ? (
        <div className="h-[28px] flex items-center px-4 shrink-0" style={{ background: t.statusBg, borderTop: `1px solid ${t.border}` }}>
          <Info size={12} style={{ color: "#5b8bd6" }} className="mr-2" />
          <span className="text-[11px]" style={{ color: t.textMuted }}>
            {footerText ?? "拖拽显示器至相邻位置即可自动吸附连接 · 滚轮缩放 · 中键/右键/空格+左键 拖拽画布"}
          </span>
          <div className="ml-auto">
            <span className="text-[10px]" style={{ color: t.textMuted }}>
              {monitors.filter((m) => m.enabled).length} 块显示器 · {devices.filter((d) => d.online).length} 台在线
            </span>
          </div>
        </div>
      ) : null}
    </div>
  );
}

/* ---- Toolbar button ---- */
function ToolbarBtn({ children, theme, onClick }: { children: React.ReactNode; theme: Theme; onClick: () => void }) {
  return (
    <button
      className="p-1 rounded transition-colors"
      style={{ backgroundColor: "transparent" }}
      onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = theme.btnBg)}
      onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "transparent")}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
