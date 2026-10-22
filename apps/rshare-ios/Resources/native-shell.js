(() => {
  // SwiftUI owns fullscreen containment in the native app. A second WebKit
  // fullscreen presentation would reparent the video during device rotation.
  window.rshareNativeDisplay = true;
  // WindowBase-inspired native safe-area containment and adaptive web content.
  const style = document.createElement('style');
  style.textContent = `
    html, body { background: transparent !important; }
    main { padding: 16px 18px 28px; max-width: 760px; -webkit-user-select: none; }
    input, textarea { -webkit-user-select: text; }
    main > header { display: none; }
    #extendedDisplayLink { display: none; }
    #pad { min-height: 280px; border-radius: 26px; }
    button, .rangeRow, .inputWrap { border-radius: 14px; }
    button { min-height: 48px; }
    .rangeRow { padding: 12px; }
    @media (prefers-color-scheme: light) {
      html, body { color: #172b27 !important; color-scheme: light; }
      button, .rangeRow, .inputWrap { background: #ffffff; color: #172b27; border-color: #d9e5df; }
      input, textarea { color: #172b27; }
      #pad { background: linear-gradient(145deg, #e8f5ed, #f6faf7); border-color: #d4e9db; }
      .sub, .rangeValue { color: #597168; }
      .muted { color: #526b60; }
      #status { color: #247344; }
      section { background: #eef5f0; }
      button:active { background: #daefdf; }
      .send { background: #258a55; color: white; }
    }
    @media (orientation: landscape) { #pad { min-height: 180px; } }
  `;
  document.head.append(style);
  const status = document.getElementById('status');
  const backend = document.getElementById('backendStatus');
  const video = document.getElementById('video');
  let display = { playing: false, immersive: false };
  const report = () => window.webkit.messageHandlers.rshareStatus.postMessage({
    status: status?.textContent?.trim() || '页面已载入',
    backend: backend?.textContent?.trim() || '',
    ...display
  });
  window.addEventListener('rshare-display-state', event => { display = event.detail; report(); });
  // The page module may have started before this atDocumentEnd user script.
  if (video) display = {
    playing: !!video.srcObject && !video.paused && !video.ended && !document.hidden
      && video.readyState >= 2 && video.videoWidth > 0 && video.videoHeight > 0,
    immersive: document.body.classList.contains('immersive')
  };
  for (const node of [status, backend]) {
    if (node) new MutationObserver(report).observe(node, { childList: true, subtree: true, characterData: true });
  }
  report();
})();
