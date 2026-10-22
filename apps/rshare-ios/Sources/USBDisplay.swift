import SwiftUI
import Network
import AVFoundation
import VideoToolbox
import Security

enum DisplayLAN {
    static func key(_ text: String) -> Data? {
        guard text.count == 64, text.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { return nil }
        let chars = Array(text), bytes = stride(from: 0, to: 64, by: 2).compactMap { UInt8(String(chars[$0...($0+1)]), radix: 16) }
        return Data(bytes)
    }
    static func parameters(key: Data) -> NWParameters {
        let tls = NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
        sec_protocol_options_set_max_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
        sec_protocol_options_append_tls_ciphersuite(tls.securityProtocolOptions, tls_ciphersuite_t(rawValue: UInt16(TLS_PSK_WITH_AES_128_GCM_SHA256))!)
        key.withUnsafeBytes { bytes in
            let identity = Data("rshare-display-v1".utf8)
            identity.withUnsafeBytes { label in
                sec_protocol_options_add_pre_shared_key(tls.securityProtocolOptions, DispatchData(bytes: bytes) as __DispatchData, DispatchData(bytes: label) as __DispatchData)
            }
        }
        let tcp = NWProtocolTCP.Options(); tcp.noDelay = true
        let parameters = NWParameters(tls: tls, tcp: tcp)
        parameters.prohibitedInterfaceTypes = [.cellular]
        return parameters
    }
    static func address() -> String? {
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0 else { return nil }
        defer { freeifaddrs(head) }
        var candidates: [(String, String)] = [], current = head
        while let node = current {
            defer { current = node.pointee.ifa_next }
            guard let address = node.pointee.ifa_addr, address.pointee.sa_family == UInt8(AF_INET) else { continue }
            let name = String(cString: node.pointee.ifa_name)
            guard name.hasPrefix("en") else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            if getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
                let text = String(cString: host), octets = text.split(separator: ".").compactMap { Int($0) }
                if octets.count == 4 && (octets[0] == 10 || (octets[0] == 192 && octets[1] == 168) || (octets[0] == 172 && (16...31).contains(octets[1]))) { candidates.append((name, text)) }
            }
        }
        return candidates.sorted { $0.0 < $1.0 }.first?.1
    }
}

enum USBDisplayFailure: Error { case invalidPacket, invalidVideo, codec(OSStatus) }

struct USBPacketParser {
    static let maximum = 8 * 1024 * 1024
    private var buffer = Data()
    static func packet(kind: UInt8, body: Data) -> Data {
        var size = UInt32(body.count).bigEndian
        return withUnsafeBytes(of: &size) { Data($0) } + Data([kind]) + body
    }
    mutating func append(_ bytes: Data) throws -> [(kind: UInt8, body: Data)] {
        buffer.append(bytes)
        var result: [(UInt8, Data)] = []
        while buffer.count >= 5 {
            let size = buffer.prefix(4).reduce(0) { ($0 << 8) | Int($1) }
            let kind = buffer[buffer.startIndex + 4]
            guard size <= Self.maximum, (1...4).contains(kind) else { throw USBDisplayFailure.invalidPacket }
            guard buffer.count >= size + 5 else { break }
            result.append((kind, Data(buffer.dropFirst(5).prefix(size))))
            buffer = Data(buffer.dropFirst(size + 5))
        }
        return result
    }
}

struct USBVideoPacket {
    let sequence: UInt64
    let sps: Data, pps: Data, avcc: Data
    init(_ body: Data) throws {
        let bytes = [UInt8](body)
        guard bytes.count > 16 else { throw USBDisplayFailure.invalidVideo }
        sequence = bytes.prefix(8).reduce(0) { ($0 << 8) | UInt64($1) }
        let a = Int(bytes[8]) << 8 | Int(bytes[9]), b = Int(bytes[10]) << 8 | Int(bytes[11])
        guard (a == 0) == (b == 0), 12 + a + b < bytes.count else { throw USBDisplayFailure.invalidVideo }
        sps = Data(bytes[12..<(12+a)]); pps = Data(bytes[(12+a)..<(12+a+b)])
        avcc = Data(bytes.dropFirst(12+a+b))
        var offset = 0
        while offset < avcc.count {
            guard offset + 4 <= avcc.count else { throw USBDisplayFailure.invalidVideo }
            let length = avcc[offset..<(offset+4)].reduce(0) { ($0 << 8) | Int($1) }
            guard length > 0, length <= avcc.count - offset - 4 else { throw USBDisplayFailure.invalidVideo }
            offset += length + 4
        }
    }
}

private final class USBH264Decoder {
    private var format: CMVideoFormatDescription?
    private var decoder: VTDecompressionSession?
    private var parameterSets = Data()
    deinit { if let decoder { VTDecompressionSessionInvalidate(decoder) } }
    func decode(_ frame: USBVideoPacket, completion: @escaping (CVPixelBuffer?) -> Void) throws {
        if !frame.sps.isEmpty && frame.sps + frame.pps != parameterSets {
            if let decoder { VTDecompressionSessionInvalidate(decoder) }; decoder = nil
            let result = frame.sps.withUnsafeBytes { sps in frame.pps.withUnsafeBytes { pps in
                let pointers = [sps.bindMemory(to: UInt8.self).baseAddress!, pps.bindMemory(to: UInt8.self).baseAddress!]
                let sizes = [frame.sps.count, frame.pps.count]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(allocator: kCFAllocatorDefault,
                    parameterSetCount: 2, parameterSetPointers: pointers, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            } }
            guard result == noErr, let format else { throw USBDisplayFailure.codec(result) }
            let dimensions = CMVideoFormatDescriptionGetDimensions(format)
            guard dimensions.width > 0, dimensions.height > 0, dimensions.width <= 1920, dimensions.height <= 1080 else { throw USBDisplayFailure.invalidVideo }
            let status = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: format,
                decoderSpecification: nil, imageBufferAttributes: [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA] as CFDictionary,
                outputCallback: nil, decompressionSessionOut: &decoder)
            guard status == noErr else { throw USBDisplayFailure.codec(status) }
            parameterSets = frame.sps + frame.pps
        }
        guard let decoder, let format else { throw USBDisplayFailure.invalidVideo }
        var block: CMBlockBuffer?
        var result = CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil,
            blockLength: frame.avcc.count, blockAllocator: kCFAllocatorDefault, customBlockSource: nil,
            offsetToData: 0, dataLength: frame.avcc.count, flags: 0, blockBufferOut: &block)
        guard result == noErr, let block else { throw USBDisplayFailure.codec(result) }
        result = frame.avcc.withUnsafeBytes { CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: frame.avcc.count) }
        guard result == noErr else { throw USBDisplayFailure.codec(result) }
        var sample: CMSampleBuffer?, size = frame.avcc.count
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: CMTime(value: Int64(frame.sequence), timescale: 60), decodeTimeStamp: .invalid)
        result = CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
            sampleCount: 1, sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample)
        guard result == noErr, let sample else { throw USBDisplayFailure.codec(result) }
        // Synchronous decode: at most one frame awaits presentation/ACK.
        result = VTDecompressionSessionDecodeFrame(decoder, sampleBuffer: sample, flags: [], infoFlagsOut: nil) { status, _, image, _, _ in
            completion(status == noErr ? image : nil)
        }
        guard result == noErr else { throw USBDisplayFailure.codec(result) }
    }
}

@MainActor final class USBDisplayReceiver: ObservableObject {
    @Published var lan = false
    @Published var pairingLink = ""
    var transportName: String { lan ? "局域网" : "USB" }
    @Published var listening = false
    @Published var playing = false
    @Published var code = ""
    @Published var status = "用 USB 数据线连接 Mac，然后开启接收。"
    @Published var frames: UInt64 = 0
    @Published var dimensions = ""
    @Published var immersive = false
    let layer = AVSampleBufferDisplayLayer()
    private var transport: USBDisplayTransport?
    private var generation = UUID()
    init() { layer.videoGravity = .resizeAspect }
    func start(testCode: String? = nil) {
        stop()
        var secret = [UInt8](repeating: 0, count: 32)
        guard !lan || SecRandomCopyBytes(kSecRandomDefault, secret.count, &secret) == errSecSuccess else { status = "无法生成配对密钥"; return }
        let generated = lan ? secret.map { String(format: "%02x", $0) }.joined() : String(format: "%06d", Int.random(in: 0...999999))
        #if DEBUG
        code = testCode ?? generated
        #else
        code = generated
        #endif
        if lan {
            guard let address = DisplayLAN.address(), DisplayLAN.key(code) != nil else { status = "请连接 Wi-Fi 或局域网后重试"; return }
            pairingLink = "rshare-display://\(address):27439#\(code)"
        }
        let revision = generation
        listening = true; status = lan ? "等待 Mac 加密连接；将配对链接粘贴到电脑。" : "等待 Mac 连接 USB；在电脑输入下方配对码。"
        UIApplication.shared.isIdleTimerDisabled = true
        transport = USBDisplayTransport(code: code, lan: lan, state: { [weak self] message in
            DispatchQueue.main.async { guard let self, self.generation == revision else { return }; self.status = message }
        }, frame: { [weak self] buffer, sequence, done in
            DispatchQueue.main.async {
                guard let self, self.generation == revision, self.listening else { done(false); return }
                do {
                    var format: CMVideoFormatDescription?
                    var sample: CMSampleBuffer?
                    guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: buffer, formatDescriptionOut: &format) == noErr, let format else { throw USBDisplayFailure.invalidVideo }
                    var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: .zero, decodeTimeStamp: .invalid)
                    guard CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: buffer, formatDescription: format, sampleTiming: &timing, sampleBufferOut: &sample) == noErr, let sample else { throw USBDisplayFailure.invalidVideo }
                    if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) as? [NSMutableDictionary] {
                        attachments.first?[kCMSampleAttachmentKey_DisplayImmediately] = true
                    }
                    if self.layer.status == .failed { self.layer.flush() }
                    guard self.layer.isReadyForMoreMediaData else { done(false); return }
                    self.layer.enqueue(sample)
                    self.frames += 1; self.playing = true
                    self.dimensions = "\(CVPixelBufferGetWidth(buffer)) × \(CVPixelBufferGetHeight(buffer))"
                    self.status = "\(self.transportName) 画面播放中 · H.264"
                    UIApplication.shared.isIdleTimerDisabled = true
                    done(true)
                } catch { done(false) }
            }
        }, closed: { [weak self] message in
            DispatchQueue.main.async {
                guard let self, self.generation == revision else { return }
                self.stop(); self.status = message
            }
        })
        transport?.start()
    }
    func stop() {
        generation = UUID(); transport?.stop(); transport = nil
        listening = false; playing = false; immersive = false; frames = 0; dimensions = ""
        UIApplication.shared.isIdleTimerDisabled = false; layer.flushAndRemoveImage()
        pairingLink = ""; code = ""
        status = "\(transportName) 接收已停止"
    }
}

private final class USBDisplayTransport {
    private let queue = DispatchQueue(label: "com.rshare.usb.receiver")
    private var listener: NWListener?, connection: NWConnection?
    private var parser = USBPacketParser(), decoder = USBH264Decoder()
    private var authenticated = false, presenting = false, failures = 0, decoded: UInt64 = 0, lastSequence: UInt64 = 0
    private let code: String
    private let lan: Bool
    private var name: String { lan ? "局域网" : "USB" }
    private let state: (String) -> Void, closed: (String) -> Void
    private let frame: (CVPixelBuffer, UInt64, @escaping (Bool) -> Void) -> Void
    init(code: String, lan: Bool, state: @escaping (String) -> Void,
         frame: @escaping (CVPixelBuffer, UInt64, @escaping (Bool) -> Void) -> Void, closed: @escaping (String) -> Void) {
        self.code = code; self.lan = lan; self.state = state; self.frame = frame; self.closed = closed
    }
    func start() { queue.async { [self] in
        do {
            let parameters: NWParameters
            if lan, let key = DisplayLAN.key(code) { parameters = DisplayLAN.parameters(key: key) }
            else { parameters = .tcp; parameters.requiredInterfaceType = .loopback }
            let listener = try NWListener(using: parameters, on: lan ? 27439 : 27438)
            self.listener = listener
            listener.stateUpdateHandler = { [weak self] value in
                if case .failed(let error) = value { self?.closed("接收无法启动：\(error.localizedDescription)") }
            }
            listener.newConnectionHandler = { [weak self] in self?.accept($0) }
            listener.start(queue: queue)
        } catch { closed("接收无法启动：\(error.localizedDescription)") }
    } }
    func stop() { queue.async { [self] in
        connection?.stateUpdateHandler = nil; connection?.cancel(); connection = nil
        listener?.cancel(); listener = nil; decoder = USBH264Decoder()
    } }
    private func accept(_ next: NWConnection) {
        guard connection == nil else { next.cancel(); return }
        connection = next; authenticated = false; presenting = false; decoded = 0; lastSequence = 0
        parser = USBPacketParser(); decoder = USBH264Decoder()
        next.stateUpdateHandler = { [weak self, weak next] value in
            guard let self, let next, self.connection === next else { return }
            if case .ready = value { self.receive(next) }
            if case .failed = value {
                if self.authenticated { self.closed("\(self.name) 连接已中断") }
                else { self.reject(next) }
            }
        }
        next.start(queue: queue)
        queue.asyncAfter(deadline: .now() + 5) { [weak self, weak next] in
            guard let self, let next, self.connection === next, !self.authenticated else { return }
            self.reject(next)
        }
    }
    private func reject(_ socket: NWConnection) {
        failures += 1; socket.stateUpdateHandler = nil; socket.cancel()
        if connection === socket { connection = nil }
        if failures >= 5 { closed("配对失败次数过多，请重新开启 USB 接收。") }
        else { state("配对码不正确，请在 Mac 重新输入。") }
    }
    private func reply(_ socket: NWConnection, _ value: [String: Any]) {
        guard let body = try? JSONSerialization.data(withJSONObject: value) else { return }
        socket.send(content: USBPacketParser.packet(kind: 3, body: body), completion: .contentProcessed { [weak self] error in
            if error != nil { self?.closed("连接已断开") }
        })
    }
    private func receive(_ socket: NWConnection) {
        socket.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] bytes, _, ended, error in
            guard let self, self.connection === socket else { return }
            do {
                if let bytes { for packet in try self.parser.append(bytes) {
                    guard self.connection === socket else { return }
                    try self.handle(packet, socket)
                } }
                if ended || error != nil { self.closed("\(self.name) 已断开，电脑将移除附屏。"); return }
                if self.connection === socket { self.receive(socket) }
            } catch { self.closed("视频数据无效，已停止接收。") }
        }
    }
    private func handle(_ packet: (kind: UInt8, body: Data), _ socket: NWConnection) throws {
        if !authenticated {
            guard packet.kind == 1, packet.body.count <= 2048,
                  let hello = try JSONSerialization.jsonObject(with: packet.body) as? [String: Any],
                  hello["version"] as? Int == 1, hello["code"] as? String == code else { reject(socket); return }
            authenticated = true; state("\(name) 已连接，等待 Mac 附屏画面。")
            reply(socket, ["accepted": true, "version": 1]); return
        }
        guard !presenting else { throw USBDisplayFailure.invalidPacket }
        if packet.kind == 4 {
            guard packet.body.isEmpty else { throw USBDisplayFailure.invalidPacket }
            reply(socket, ["frames_decoded": decoded, "sequence": lastSequence]); return
        }
        guard packet.kind == 2 else { throw USBDisplayFailure.invalidPacket }
        let video = try USBVideoPacket(packet.body)
        guard video.sequence > lastSequence else { throw USBDisplayFailure.invalidVideo }
        lastSequence = video.sequence; presenting = true
        try decoder.decode(video) { [weak self] image in
            guard let self else { return }
            guard let image else { self.closed("H.264 解码失败"); return }
            self.frame(image, video.sequence) { [weak self] rendered in
                guard let self else { return }
                self.queue.async {
                    guard self.connection === socket else { return }
                    self.presenting = false
                    if rendered { self.decoded += 1 }
                    self.reply(socket, ["frames_decoded": self.decoded, "sequence": video.sequence])
                }
            }
        }
    }
}

private struct USBVideoSurface: UIViewRepresentable {
    let layer: AVSampleBufferDisplayLayer
    func makeUIView(context: Context) -> Surface { let view = Surface(); view.video = layer; view.layer.addSublayer(layer); return view }
    func updateUIView(_ uiView: Surface, context: Context) {}
    final class Surface: UIView {
        var video: AVSampleBufferDisplayLayer?
        override func layoutSubviews() { super.layoutSubviews(); video?.frame = bounds }
    }
}

struct USBDisplayView: View {
    @ObservedObject var receiver: USBDisplayReceiver
    var body: some View {
        VStack(spacing: 16) {
            if !receiver.immersive {
                Picker("连接方式", selection: $receiver.lan) {
                    Text("USB 数据线").tag(false); Text("局域网 / Wi-Fi").tag(true)
                }.pickerStyle(.segmented).disabled(receiver.listening)
                Text(receiver.status).accessibilityIdentifier("usbDisplayStatus")
                if receiver.listening && !receiver.playing {
                    if receiver.lan {
                        Text("已开启加密局域网接收").font(.headline)
                        ShareLink("复制 / 分享配对链接", item: receiver.pairingLink)
                        Text(receiver.pairingLink).font(.caption2.monospaced()).textSelection(.enabled).lineLimit(4)
                        Text("Mac 与手机接入同一局域网，在电脑粘贴此链接。链接每次开启接收时更新。").font(.footnote).foregroundStyle(.secondary)
                    } else {
                        Text(receiver.code).font(.system(size: 36, weight: .bold, design: .monospaced)).accessibilityIdentifier("usbPairingCode")
                        Text("Mac 端选择这部设备并输入配对码。无需个人热点。保持本页在前台。").font(.footnote).foregroundStyle(.secondary)
                    }
                }
            }
            USBVideoSurface(layer: receiver.layer).background(.black).accessibilityIdentifier("usbDisplayVideo")
            if !receiver.immersive { Text("\(receiver.dimensions) · 已显示 \(receiver.frames) 帧").font(.caption).monospacedDigit().accessibilityIdentifier("usbDisplayFrames") }
            HStack {
                Button(receiver.listening ? "停止 \(receiver.transportName) 接收" : "开启 \(receiver.transportName) 接收") { receiver.listening ? receiver.stop() : receiver.start() }.buttonStyle(.borderedProminent)
                if receiver.playing { Button(receiver.immersive ? "退出全屏" : "全屏") { receiver.immersive.toggle() } }
            }
        }.padding(receiver.immersive ? 0 : 16)
            .toolbar(receiver.immersive ? .hidden : .visible, for: .navigationBar, .tabBar)
            .statusBarHidden(receiver.immersive).ignoresSafeArea(edges: receiver.immersive ? .all : [])
            .navigationTitle("Mac 扩展屏")
    }
}
