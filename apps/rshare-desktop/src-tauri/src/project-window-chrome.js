// Shared desktop window behavior. Keep this file and its regression tests together.
(function (root) {
  'use strict';
  const interactive = 'button,a,input,select,textarea,label,summary,[contenteditable]:not([contenteditable="false"]),[tabindex]:not([tabindex="-1"]),[role="button"],[role="link"],[role="menuitem"],[role="tab"],[role="checkbox"],[role="radio"],[role="switch"],[role="slider"],[role="textbox"],[role="combobox"],[role="option"],[data-no-drag],[data-no-window-drag]';

  function dragRegion(path) {
    let region = false;
    let found = false;
    for (const element of path) {
      if (typeof element?.getAttribute !== 'function') continue;
      // An interactive ancestor must win over a marked passive child too.
      if (element.matches(interactive)) return false;
      const value = element.getAttribute('data-tauri-drag-region');
      if (value === 'false') return false;
      if (!found && ['deep', '', 'true'].includes(value)) {
        region = value === 'deep' || element === path[0];
        found = true;
      }
    }
    return region;
  }

  function canResize(state) {
    return Boolean(state && !state.decorated && state.resizable && !state.maximized && !state.fullscreen);
  }

  function cornerRadius(state, macos) {
    return state && !state.decorated && !state.maximized && !state.fullscreen ? (macos ? 10 : 8) : 0;
  }

  function install(win, doc, macos) {
    if (win !== win.top || !win.__TAURI_INTERNALS__?.invoke || win.__projectWindowChrome) return;
    if (!/^(tauri:|https?:)$/.test(win.location.protocol)) return;
    if (win.location.protocol !== 'tauri:' && !['tauri.localhost', 'localhost', '127.0.0.1', '[::1]'].includes(win.location.hostname)) return;
    const invoke = (command, args = {}) => win.__TAURI_INTERNALS__.invoke('plugin:window|' + command, args);
    let state = null;
    let disposed = false;
    let reading = false;
    let again = false;
    let scheduled = 0;
    let resizeTimer = null;
    let doubleClick = null;
    let edges;
    const removers = [];
    const cornerStyles = ['border-radius', 'clip-path', '--project-window-radius'];
    const savedCorners = cornerStyles.map(name => [name, doc.documentElement.style.getPropertyValue(name), doc.documentElement.style.getPropertyPriority(name)]);
    function restoreCorners() {
      for (const [name, value, priority] of savedCorners) {
        if (value) doc.documentElement.style.setProperty(name, value, priority);
        else doc.documentElement.style.removeProperty(name);
      }
    }
    const report = (error) => {
      if (!disposed) win.dispatchEvent(new win.CustomEvent('project-window-error', { detail: String(error) }));
    };
    const run = (command, args) => invoke(command, args).then(schedule, report);
    function listen(target, name, handler, capture = false) {
      target.addEventListener(name, handler, capture);
      removers.push(() => target.removeEventListener(name, handler, capture));
    }
    function schedule() {
      if (!disposed && !scheduled) scheduled = win.requestAnimationFrame(() => { scheduled = 0; void refresh(); });
    }
    async function refresh() {
      if (disposed) return;
      if (reading) { again = true; return; }
      reading = true;
      try {
        const [decorated, resizable, maximized, fullscreen, focused, maximizable] = await Promise.all(
          ['is_decorated', 'is_resizable', 'is_maximized', 'is_fullscreen', 'is_focused', 'is_maximizable'].map(c => invoke(c))
        );
        if (disposed) return;
        const next = { decorated, resizable, maximized, fullscreen, focused, maximizable };
        if (state && Object.keys(next).every(key => next[key] === state[key])) return;
        state = next;
        const html = doc.documentElement;
        for (const [name, value] of Object.entries(state)) html.setAttribute('data-window-' + name, String(value));
        const radius = cornerRadius(state, macos);
        if (!decorated) {
          html.style.setProperty('--project-window-radius', radius + 'px');
          html.style.setProperty('border-radius', radius + 'px', 'important');
          html.style.setProperty('clip-path', `inset(0 round ${radius}px)`, 'important');
        } else restoreCorners();
        if (edges) edges.style.setProperty('display', canResize(state) ? 'block' : 'none', 'important');
        win.dispatchEvent(new win.CustomEvent('project-window-state', { detail: state }));
      } catch (error) {
        state = null;
        restoreCorners();
        if (edges) edges.style.setProperty('display', 'none', 'important');
        report(error);
      } finally {
        reading = false;
        if (again) { again = false; schedule(); }
      }
    }
    function consume(event) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
    function handles(event) {
      return !event.defaultPrevented && state && !state.decorated && !state.fullscreen && event.button === 0 && dragRegion(event.composedPath());
    }
    // Cancel custom pointer handlers; mousedown below owns the native operation.
    // Do not preventDefault here: that would suppress the compatibility mousedown.
    listen(win, 'pointerdown', event => {
      if (handles(event) && event.pointerType === 'mouse') event.stopImmediatePropagation();
    }, true);
    listen(win, 'mousedown', event => {
      doubleClick = null;
      if (!handles(event)) return;
      consume(event);
      if (event.detail === 2) {
        if (!state.resizable || !state.maximizable) return;
        if (macos) doubleClick = { x: event.clientX, y: event.clientY };
        else void run('internal_toggle_maximize');
      } else if (event.detail === 1) {
        doubleClick = null;
        void run('start_dragging');
      }
    }, true);
    listen(win, 'mouseup', event => {
      const initial = doubleClick;
      doubleClick = null;
      if (!handles(event)) return;
      consume(event);
      if (macos && initial && initial.x === event.clientX && initial.y === event.clientY && state.resizable && state.maximizable) {
        void run('internal_toggle_maximize');
      }
    }, true);
    listen(win, 'dblclick', event => { if (handles(event)) consume(event); }, true);
    listen(win, 'blur', () => { doubleClick = null; schedule(); });
    listen(win, 'focus', schedule);
    // One refresh per interval during live resize, with a final trailing read.
    // Focus and command completion still use the next animation frame.
    function scheduleResize() {
      if (!disposed && resizeTimer === null) resizeTimer = win.setTimeout(() => {
        resizeTimer = null;
        schedule();
      }, 80);
    }
    listen(win, 'resize', scheduleResize);
    listen(win, 'project-native-window-state', scheduleResize);
    listen(win, 'pointercancel', () => { doubleClick = null; }, true);
    listen(win, 'dragstart', () => { doubleClick = null; }, true);
    listen(win, 'keydown', event => { if (event.key === 'Escape') doubleClick = null; }, true);
    listen(doc, 'visibilitychange', () => { if (!doc.hidden) schedule(); });

    function mount() {
      if (disposed || edges) return;
      edges = doc.createElement('div');
      edges.id = 'project-window-resize-edges';
      for (const [key, value] of Object.entries({ position: 'fixed', inset: '0', 'pointer-events': 'none', 'z-index': '2147483646', display: 'none' })) edges.style.setProperty(key, value, 'important');
      edges.setAttribute('aria-hidden', 'true');
      // Shadow DOM prevents application CSS from changing edge geometry/cursors.
      const shadow = edges.attachShadow({ mode: 'closed' });
      for (const [name, direction] of Object.entries({ n: 'North', ne: 'NorthEast', e: 'East', se: 'SouthEast', s: 'South', sw: 'SouthWest', w: 'West', nw: 'NorthWest' })) {
        const edge = doc.createElement('div');
        edge.className = 'edge ' + name;
        // CSSOM assignments also work with strict style-src (no unsafe-inline required).
        Object.assign(edge.style, { position: 'absolute', pointerEvents: 'auto', touchAction: 'none' });
        if (name.length === 2) {
          Object.assign(edge.style, { width: '10px', height: '10px', cursor: name === 'nw' || name === 'se' ? 'nwse-resize' : 'nesw-resize', [name.includes('n') ? 'top' : 'bottom']: '0', [name.includes('w') ? 'left' : 'right']: '0' });
        } else if (name === 'n' || name === 's') {
          Object.assign(edge.style, { left: '10px', right: '10px', height: '4px', cursor: 'ns-resize', [name === 'n' ? 'top' : 'bottom']: '0' });
        } else {
          Object.assign(edge.style, { top: '10px', bottom: '10px', width: '4px', cursor: 'ew-resize', [name === 'w' ? 'left' : 'right']: '0' });
        }
        listen(edge, 'pointerdown', event => {
          if (event.defaultPrevented || event.button !== 0 || !canResize(state)) return;
          consume(event);
          void run('start_resize_dragging', { value: direction });
        });
        shadow.appendChild(edge);
      }
      doc.documentElement.appendChild(edges);
      void refresh();
    }
    function destroy() {
      if (disposed) return;
      disposed = true;
      removers.splice(0).forEach(remove => remove());
      if (scheduled) win.cancelAnimationFrame(scheduled);
      if (resizeTimer !== null) win.clearTimeout(resizeTimer);
      edges?.remove();
      restoreCorners();
      for (const key of ['decorated', 'resizable', 'maximized', 'fullscreen', 'focused', 'maximizable']) doc.documentElement.removeAttribute('data-window-' + key);
      delete win.__projectWindowChrome;
    }
    win.__projectWindowChrome = { refresh: schedule, destroy };
    listen(win, 'pagehide', destroy);
    if (doc.readyState === 'loading') listen(doc, 'DOMContentLoaded', mount);
    else mount();
    return win.__projectWindowChrome;
  }
  if (typeof module === 'object' && module.exports) module.exports = { dragRegion, canResize, cornerRadius, install };
  else {
    const boot = () => install(root, root.document, __PROJECT_CHROME_MACOS__);
    if (root.document.readyState === 'loading') root.document.addEventListener('DOMContentLoaded', boot, { once: true });
    else boot();
    root.addEventListener('pageshow', boot);
  }
})(typeof window === 'undefined' ? globalThis : window);
