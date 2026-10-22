import SwiftUI

@main
struct RShareMouseApp: App {
    @StateObject private var session = RemoteSession()
    @StateObject private var usb = USBDisplayReceiver()
    @AppStorage("appearance") private var appearance = "system"
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            MainView(session: session, usb: usb)
                .tint(Color(red: 0.23, green: 0.70, blue: 0.43))
                .preferredColorScheme(appearance == "dark" ? .dark : appearance == "light" ? .light : nil)
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { if !usb.listening { session.resume() } }
                    else { usb.stop(); session.suspend() }
                }
        }
    }
}

struct MainView: View {
    @ObservedObject var session: RemoteSession
    @ObservedObject var usb: USBDisplayReceiver
    @State private var usbMode = false
    @State private var linkText = ""
    @AppStorage("appearance") private var appearance = "system"

    var body: some View {
        TabView(selection: $session.tab) {
            NavigationStack { connectionPage }
                .tabItem { Label("连接", systemImage: "point.3.connected.trianglepath.dotted") }.tag(AppTab.connection)
            NavigationStack { remotePage(display: false) }
                .tabItem { Label("遥控", systemImage: "cursorarrow.motionlines") }.tag(AppTab.remote)
            NavigationStack {
                Group {
                    if usbMode { USBDisplayView(receiver: usb) }
                    else { remotePage(display: true) }
                }.toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button(usbMode ? "浏览器接收" : "连接 Mac 附屏") {
                            usbMode.toggle(); usb.stop(); session.suspend()
                            if !usbMode { session.loadCurrentPage(force: true) }
                        }
                    }
                }
            }
                .tabItem { Label("扩展屏", systemImage: "rectangle.on.rectangle") }.tag(AppTab.display)
            NavigationStack { settingsPage }
                .tabItem { Label("设置", systemImage: "slider.horizontal.3") }.tag(AppTab.settings)
        }
        .onChange(of: session.tab) { _, tab in
            if tab != .display { usb.stop() }
            if tab == .display && usbMode { session.suspend() }
            else { session.loadCurrentPage() }
        }
        .onAppear {
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--usb-display-test"),
               let code = ProcessInfo.processInfo.environment["RSHARE_USB_CODE"] {
                usbMode = true; session.tab = .display
                usb.lan = ProcessInfo.processInfo.environment["RSHARE_DISPLAY_TRANSPORT"] == "lan"
                DispatchQueue.main.async { usb.start(testCode: code) }
            }
            #endif
        }
    }

    private var connectionPage: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                HStack {
                    Label("R-SHAREMOUSE", systemImage: "cursorarrow.and.square.on.square.dashed")
                        .font(.system(size: 12, weight: .bold, design: .rounded)).tracking(2)
                    Spacer()
                    Text("iOS").font(.caption.weight(.semibold)).padding(.horizontal, 10).padding(.vertical, 5)
                        .background(.thinMaterial, in: Capsule())
                }
                VStack(alignment: .leading, spacing: 12) {
                    Text("电脑控制，\n随手接续。")
                        .font(.system(size: 38, weight: .bold, design: .rounded)).lineSpacing(4)
                    Text("把 iPhone 变成触控板与键盘，\n让 iPad 成为你的另一块屏幕。")
                        .font(.subheadline).foregroundStyle(.secondary).lineSpacing(5)
                }.padding(.top, 12)
                HStack(spacing: 18) {
                    Image(systemName: "desktopcomputer").font(.system(size: 48, weight: .light))
                    Image(systemName: "arrow.left.arrow.right").foregroundStyle(.secondary)
                    Image(systemName: "iphone.gen3").font(.system(size: 42, weight: .light))
                    Spacer()
                    Image(systemName: "wifi").font(.title2).foregroundStyle(.green)
                }
                .padding(24).frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.green.opacity(0.07), in: RoundedRectangle(cornerRadius: 26))
                if let pairing = session.pairing {
                    VStack(alignment: .leading, spacing: 14) {
                        Label("上次连接", systemImage: "clock.arrow.circlepath").font(.caption).foregroundStyle(.secondary)
                        Text(pairing.hostLabel).font(.headline).textSelection(.enabled)
                        Button(action: session.reconnect) { Label("重新连接", systemImage: "arrow.up.right").frame(maxWidth: .infinity) }
                            .buttonStyle(.borderedProminent).controlSize(.large)
                    }.padding(20).background(.thinMaterial, in: RoundedRectangle(cornerRadius: 24))
                }
                VStack(alignment: .leading, spacing: 14) {
                    Text("连接一台电脑").font(.headline)
                    Text("在电脑端打开「设置 → 移动端控制」，\n复制配对链接，然后粘贴到这里。")
                        .font(.subheadline).foregroundStyle(.secondary).lineSpacing(4)
                    TextField("http://电脑地址:端口/mobile?t=…", text: $linkText, axis: .vertical)
                        .font(.system(.subheadline, design: .monospaced))
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .keyboardType(.URL).lineLimit(2...3).padding(14)
                        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 14))
                        .accessibilityIdentifier("pairingLink")
                    HStack {
                        PasteButton(payloadType: String.self) { values in linkText = values.first ?? "" }
                            .buttonStyle(.bordered).controlSize(.large)
                        Button {
                            session.connect(linkText)
                            if session.error == nil { linkText = "" }
                        } label: { Label("连接电脑", systemImage: "arrow.right").frame(maxWidth: .infinity) }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                        .disabled(linkText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("connectHost")
                    }
                    if let error = session.error { Label(error, systemImage: "exclamationmark.circle").font(.footnote).foregroundStyle(.orange).accessibilityIdentifier("connectionError") }
                }.padding(20).background(.thinMaterial, in: RoundedRectangle(cornerRadius: 24))
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: "network")
                    Text("手机和电脑需要在同一局域网。配对凭证只保存在本机钥匙串。")
                        .font(.footnote).lineSpacing(4)
                }.foregroundStyle(.secondary).padding(.horizontal, 4)
            }.padding(24).frame(maxWidth: 640)
        }
        .background(background).toolbar(.hidden, for: .navigationBar)
    }

    @ViewBuilder private func remotePage(display: Bool) -> some View {
        if session.pairing == nil || session.status == "未连接" {
            ContentUnavailableView {
                Label(display ? "准备好另一块屏幕" : "先连接你的电脑", systemImage: display ? "display" : "computermouse")
            } description: {
                Text(display ? "连接后，在 Windows 主机选择扩展屏并开始分享。" : "连接后即可使用触控板、鼠标按键、快捷键和文字输入。")
            } actions: {
                Button("前往连接") { session.tab = .connection }.buttonStyle(.borderedProminent)
            }.navigationTitle(display ? "扩展屏" : "遥控")
        } else {
            VStack(spacing: 0) {
                if !display || !session.displayImmersive {
                    HStack {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(session.pairing?.hostLabel ?? "").font(.caption).foregroundStyle(.secondary)
                            Text(session.status).font(.subheadline.weight(.medium)).accessibilityIdentifier("remoteStatus")
                            if display && session.displayPlaying {
                                Label("画面播放中", systemImage: "play.circle.fill")
                                    .font(.caption).foregroundStyle(.green).accessibilityIdentifier("displayPlayback")
                            }
                        }
                        Spacer()
                        if session.loading { ProgressView() }
                    }.padding(.horizontal, 20).padding(.vertical, 12)
                }
                if let error = session.error {
                    ContentUnavailableView {
                        Label("暂时无法连接", systemImage: "wifi.exclamationmark")
                    } description: { Text(error) } actions: {
                        Button("重试") { session.loadCurrentPage(force: true) }.buttonStyle(.borderedProminent)
                        Button("更换配对链接") { session.disconnect() }
                    }
                } else if session.tab == (display ? .display : .remote) {
                    RemoteWebView(session: session)
                } else { Color.clear }
            }
            .background(background)
            .navigationTitle(display ? "扩展屏" : "触控板与键盘")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(display && session.displayImmersive ? .hidden : .visible, for: .navigationBar, .tabBar)
            .statusBarHidden(display && session.displayImmersive)
            .ignoresSafeArea(edges: display && session.displayImmersive ? .all : [])
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { session.loadCurrentPage(force: true) } label: { Image(systemName: "arrow.clockwise") }
                        .accessibilityLabel("重新加载")
                }
            }
        }
    }

    private var settingsPage: some View {
        Form {
            Section("外观") {
                Picker("主题", selection: $appearance) {
                    Text("跟随系统").tag("system")
                    Text("浅色").tag("light")
                    Text("深色").tag("dark")
                }
            }
            Section("当前连接") {
                LabeledContent("电脑", value: session.pairing?.hostLabel ?? "尚未配对")
                LabeledContent("状态", value: session.status)
                if !session.backendStatus.isEmpty { Text(session.backendStatus).font(.footnote).foregroundStyle(.secondary) }
                Button("断开连接") { session.disconnect() }.disabled(session.status == "未连接")
                Button("忘记这台电脑", role: .destructive) { session.disconnect(forget: true) }.disabled(session.pairing == nil)
            }
            Section("使用提示") {
                Label("单指移动指针，双指滚动；长按可拖动。", systemImage: "hand.draw")
                Label("切到后台、离开控制页或断开时，会请求释放按键。", systemImage: "hand.raised")
                Label("扩展屏需要 Windows 主机的虚拟显示驱动，并在主机浏览器选择共享屏幕。", systemImage: "display")
                Label("Mac 可使用普通 USB 数据线：在扩展屏中选择 USB 连接 Mac 并开启接收。", systemImage: "cable.connector")
            }.font(.subheadline)
            Section { LabeledContent("R-ShareMouse iOS", value: "0.1.0") }
        }.navigationTitle("设置")
    }

    private var background: some View {
        Color(uiColor: .systemGroupedBackground).overlay(alignment: .topTrailing) {
            Circle().fill(Color.green.opacity(0.07)).frame(width: 260).blur(radius: 80).offset(x: 100, y: -80)
        }.ignoresSafeArea()
    }
}
