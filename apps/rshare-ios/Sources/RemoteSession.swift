import SwiftUI
import WebKit

enum AppTab: String { case connection, remote, display, settings }

@MainActor
final class RemoteSession: NSObject, ObservableObject, WKNavigationDelegate {
    @Published var pairing: PairingLink?
    @Published var tab: AppTab = .connection
    @Published var status = "未连接"
    @Published var backendStatus = ""
    @Published var error: String?
    @Published var loading = false
    @Published var displayImmersive = false
    @Published var displayPlaying = false
    private var foreground = true
    fileprivate var requestedURL: URL?
    private var navigationRevision = 0
    private var pendingAuthorizedNavigation: URL?
    private let persistent: Bool
    private let bridge = StatusBridge()

    lazy var webView: WKWebView = {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        bridge.owner = self
        configuration.userContentController.add(bridge, name: "rshareStatus")
        if let path = Bundle.main.url(forResource: "native-shell", withExtension: "js"),
           let script = try? String(contentsOf: path, encoding: .utf8) {
            configuration.userContentController.addUserScript(WKUserScript(
                source: script, injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        }
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = self
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        view.scrollView.keyboardDismissMode = .interactive
        view.allowsBackForwardNavigationGestures = false
        view.accessibilityIdentifier = "remoteWebView"
        return view
    }()

    init(persistent: Bool = true) {
        #if DEBUG
        self.persistent = persistent && !ProcessInfo.processInfo.arguments.contains("--ui-testing")
        #else
        self.persistent = persistent
        #endif
        super.init()
        if self.persistent, let saved = PairingStore.read(), let savedPairing = try? PairingLink(saved) {
            pairing = savedPairing
            // Migrate older query-token links to the URL-free Keychain format.
            try? PairingStore.save(savedPairing)
        }
        #if DEBUG
        if let preset = ProcessInfo.processInfo.environment["RSHARE_PAIRING_URL"] {
            connect(preset)
        }
        #endif
    }

    func connect(_ text: String) {
        do {
            let link = try PairingLink(text)
            if persistent { try PairingStore.save(link) }
            pairing = link
            error = nil
            tab = link.initialPath == "/display" ? .display : .remote
            loadCurrentPage(force: true)
        } catch {
            self.error = error.localizedDescription
        }
    }

    func reconnect() {
        guard pairing != nil else { return }
        loadCurrentPage(force: true)
    }

    func updateDisplayPresentation(playing: Bool, immersive: Bool) {
        let active = playing && foreground && tab == .display
        displayPlaying = active
        displayImmersive = active && immersive
        UIApplication.shared.isIdleTimerDisabled = active
    }

    func loadCurrentPage(force: Bool = false) {
        guard let pairing, tab == .remote || tab == .display else { suspend(); return }
        foreground = true
        let url = tab == .display ? pairing.displayURL : pairing.controlURL
        if !force && requestedURL == url { resume(); return }
        navigationRevision += 1
        let revision = navigationRevision
        requestedURL = url
        pendingAuthorizedNavigation = url
        loading = true
        error = nil
        status = tab == .display ? "等待扩展屏" : "正在连接"
        backendStatus = ""
        updateDisplayPresentation(playing: false, immersive: false)
        // Wait for the old page's release handlers before replacing its document.
        webView.evaluateJavaScript("window.dispatchEvent(new Event('pagehide'))") { [weak self] _, _ in
            guard let self, self.navigationRevision == revision else { return }
            self.webView.load(self.authorizedRequest(for: url))
        }
    }

    func authorizedRequest(for url: URL) -> URLRequest {
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 12)
        if let token = pairing?.bearerToken {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    func suspend() {
        foreground = false
        updateDisplayPresentation(playing: false, immersive: false)
        if requestedURL?.path == "/display" { requestedURL = nil }
        webView.evaluateJavaScript("window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('pagehide'))", completionHandler: nil)
        webView.pauseAllMediaPlayback(completionHandler: nil)
    }

    func resume() {
        foreground = true
        guard tab == .remote || tab == .display else { return }
        if requestedURL == nil, pairing != nil, error == nil {
            loadCurrentPage(force: true)
            return
        }
        webView.evaluateJavaScript("window.dispatchEvent(new Event('pageshow')); window.dispatchEvent(new Event('focus'))", completionHandler: nil)
    }

    func disconnect(forget: Bool = false) {
        navigationRevision += 1
        let revision = navigationRevision
        suspend()
        requestedURL = nil
        pendingAuthorizedNavigation = nil
        status = "未连接"
        backendStatus = ""
        error = nil
        loading = false
        tab = .connection
        webView.evaluateJavaScript("window.dispatchEvent(new Event('pagehide'))") { [weak self] _, _ in
            guard let self, self.navigationRevision == revision else { return }
            self.webView.loadHTMLString("", baseURL: nil)
        }
        if forget {
            if persistent { PairingStore.remove() }
            pairing = nil
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        if url.absoluteString == "about:blank" { decisionHandler(.allow); return }
        guard action.targetFrame?.isMainFrame == true,
              let pairing,
              pairing.allowsNavigation(to: url) else {
            decisionHandler(.cancel)
            return
        }
        if action.request.value(forHTTPHeaderField: "Authorization") == nil,
           pendingAuthorizedNavigation != url {
            pendingAuthorizedNavigation = url
            var request = action.request
            request.setValue(pairing.authorizationHeader, forHTTPHeaderField: "Authorization")
            decisionHandler(.cancel)
            webView.load(request)
            return
        }
        pendingAuthorizedNavigation = nil
        requestedURL = url
        let next: AppTab = url.path == "/display" ? .display : .remote
        if tab != next { DispatchQueue.main.async { self.tab = next } }
        decisionHandler(.allow)
    }

    func webView(_ webView: WKWebView, decidePolicyFor response: WKNavigationResponse,
                 decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void) {
        if let http = response.response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            loading = false
            status = "连接失败"
            error = http.statusCode == 401 || http.statusCode == 403
                ? "配对链接已失效，请从电脑重新复制。" : "电脑暂时无法提供此页面，请检查主机服务。"
            decisionHandler(.cancel)
        } else { decisionHandler(.allow) }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loading = false }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError failure: Error) { failed(failure) }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError failure: Error) { failed(failure) }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        updateDisplayPresentation(playing: false, immersive: false)
        loading = false
        status = "连接已中断"
        error = "控制页面已停止，请点右上角重新连接。"
        requestedURL = nil
    }
    private func failed(_ failure: Error) {
        guard (failure as NSError).code != NSURLErrorCancelled else { return }
        updateDisplayPresentation(playing: false, immersive: false)
        loading = false
        status = "无法连接"
        error = "请确认电脑已开启移动端控制，且手机与电脑在同一局域网。"
        requestedURL = nil
    }
}

@MainActor
private final class StatusBridge: NSObject, WKScriptMessageHandler {
    weak var owner: RemoteSession?
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              owner?.requestedURL != nil,
              let url = message.frameInfo.request.url,
              owner?.pairing?.isPairedDocument(url) == true,
              let body = message.body as? [String: Any] else { return }
        owner?.status = String((body["status"] as? String ?? "等待主机").prefix(100))
        owner?.backendStatus = String((body["backend"] as? String ?? "").prefix(200))
        owner?.updateDisplayPresentation(
            playing: url.path == "/display" && body["playing"] as? Bool == true,
            immersive: body["immersive"] as? Bool == true)
    }
}

struct RemoteWebView: UIViewRepresentable {
    let session: RemoteSession
    func makeUIView(context: Context) -> WKWebView { session.webView }
    func updateUIView(_ view: WKWebView, context: Context) {}
}
