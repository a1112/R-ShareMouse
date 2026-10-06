// Canonical wire namespace from rshare-input/events.rs (not native OS scan codes).
const WIRE_KEYS = {
  8: "Backspace", 9: "Tab", 13: "Enter", 16: "Shift", 17: "Control", 18: "Alt",
  19: "Pause", 20: "CapsLock", 27: "Escape", 32: "Space", 33: "PageUp", 34: "PageDown",
  35: "End", 36: "Home", 37: "Left", 38: "Up", 39: "Right", 40: "Down",
  44: "PrintScreen", 45: "Insert", 46: "Delete", 91: "SuperLeft", 92: "SuperRight", 93: "Menu",
  106: "KeypadMultiply", 107: "KeypadAdd", 109: "KeypadSubtract", 110: "KeypadDecimal", 111: "KeypadDivide",
  144: "NumLock", 145: "ScrollLock", 160: "ShiftLeft", 161: "ShiftRight",
  162: "ControlLeft", 163: "ControlRight", 164: "AltLeft", 165: "AltRight",
  186: ";", 187: "=", 188: ",", 189: "-", 190: ".", 191: "/", 192: "`",
  219: "[", 220: "\\", 221: "]", 222: "'", 57372: "KeypadEnter",
};

export function wireKeyName(code) {
  if ((code >= 48 && code <= 57) || (code >= 65 && code <= 90)) return `Char(${code})`;
  if (code >= 96 && code <= 105) return `Keypad${code - 96}`;
  if (code >= 112 && code <= 135) return `F${code - 111}`;
  return WIRE_KEYS[code] ?? `Raw(${code})`;
}

export function normalizeKeyToken(value) {
  if (typeof value === "number") value = wireKeyName(value);
  let key = String(value ?? "");
  const raw = key.match(/^Raw\((\d+)\)$/i);
  if (raw) key = wireKeyName(Number(raw[1]));
  const char = key.match(/^Char\((\d+)\)$/i);
  if (char) key = String.fromCharCode(Number(char[1]));
  key = key.toLowerCase();
  if (key === " ") return "space";
  key = key.replace(/\s/g, "");
  if (/^key[a-z]$/.test(key)) return key.slice(3);
  if (/^(digit|num)[0-9]$/.test(key)) return key.slice(-1);
  key = key.replace(/^numpad/, "keypad").replace(/^arrow/, "");
  const aliases = {
    esc: "escape", return: "enter", spacebar: "space", caps: "capslock", num: "numlock",
    del: "delete", ins: "insert", pgup: "pageup", pgdn: "pagedown", prtsc: "printscreen",
    snapshot: "printscreen", scroll: "scrolllock", contextmenu: "menu", ctrl: "control",
    ctrlleft: "controlleft", ctrlright: "controlright", winleft: "superleft", winright: "superright",
    metaleft: "superleft", metaright: "superright", osleft: "superleft", osright: "superright",
    backquote: "`", minus: "-", equal: "=", bracketleft: "[", bracketright: "]", backslash: "\\",
    semicolon: ";", quote: "'", comma: ",", period: ".", slash: "/", "←": "left", "↑": "up", "↓": "down", "→": "right",
  };
  return aliases[key] ?? key;
}

export function inputButtonName(button) {
  if (typeof button === "string") return button;
  const [name, value] = Object.entries(button ?? {})[0] ?? ["Unknown", null];
  return value == null ? name : `${name}(${value})`;
}

export function normalizeMouseButtonToken(button) {
  const token = inputButtonName(button).toLowerCase().replace(/[\s_-]/g, "");
  return ({"button1": "left", "button2": "middle", "button3": "right", "button4": "back", "button5": "forward", "other(4)": "back", "other(5)": "forward", "xbutton1": "back", "xbutton2": "forward"})[token] ?? token;
}
