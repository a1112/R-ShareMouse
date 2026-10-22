import Foundation
import Network

struct PairingLink: Equatable {
    let controlURL: URL
    let displayURL: URL
    let hostLabel: String

    enum Failure: LocalizedError {
        case invalid, missingToken, publicHost
        var errorDescription: String? {
            switch self {
            case .invalid: return "请粘贴电脑上“移动端控制”提供的完整配对链接。"
            case .missingToken: return "链接缺少配对凭证，请从电脑重新复制。"
            case .publicHost: return "请使用同一局域网中的电脑地址或 .local 主机名。"
            }
        }
    }

    init(_ text: String) throws {
        guard text.utf8.count <= 4096,
              var parts = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              ["http", "https"].contains(parts.scheme?.lowercased() ?? ""),
              let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil,
              ["", "/", "/mobile", "/display"].contains(parts.path),
              parts.port == nil || (1...65535).contains(parts.port!)
        else { throw Failure.invalid }
        guard Self.isLocalHost(host) else { throw Failure.publicHost }
        let tokens = (parts.queryItems ?? []).filter { $0.name == "t" }
        guard tokens.count == 1, let token = tokens.first?.value,
              !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { throw Failure.missingToken }
        parts.scheme = parts.scheme?.lowercased()
        parts.fragment = nil
        parts.queryItems = [URLQueryItem(name: "t", value: token)]
        parts.path = "/mobile"
        guard let control = parts.url else { throw Failure.invalid }
        controlURL = control
        parts.path = "/display"
        guard let display = parts.url else { throw Failure.invalid }
        displayURL = display
        hostLabel = host + (parts.port.map { ":\($0)" } ?? "")
    }

    static func isLocalHost(_ name: String) -> Bool {
        let host = name.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        if host == "localhost" || host.hasSuffix(".local") || host.hasSuffix(".lan") { return true }
        if let address = IPv4Address(host) {
            let bytes = Array(address.rawValue)
            return bytes[0] == 10 || bytes[0] == 127
                || (bytes[0] == 172 && (16...31).contains(bytes[1]))
                || (bytes[0] == 192 && bytes[1] == 168)
                || (bytes[0] == 169 && bytes[1] == 254)
        }
        if let address = IPv6Address(host) {
            let bytes = Array(address.rawValue)
            return address == .loopback || bytes[0] & 0xfe == 0xfc
                || (bytes[0] == 0xfe && bytes[1] & 0xc0 == 0x80)
        }
        return false
    }

    func isPairedDocument(_ url: URL) -> Bool {
        url.scheme == controlURL.scheme && url.host == controlURL.host
            && effectivePort(url) == effectivePort(controlURL)
            && ["/mobile", "/display"].contains(url.path)
            && url.user == nil && url.password == nil
    }

    func allowsNavigation(to url: URL) -> Bool {
        guard isPairedDocument(url),
              effectivePort(url) == effectivePort(controlURL),
              ["/mobile", "/display"].contains(url.path),
              url.user == nil, url.password == nil
        else { return false }
        let expected = URLComponents(url: controlURL, resolvingAgainstBaseURL: false)?.queryItems
        let actual = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        return actual?.filter { $0.name == "t" } == expected
            && !(actual ?? []).contains { $0.name == "role" && $0.value == "host" }
    }

    private func effectivePort(_ url: URL) -> Int { url.port ?? (url.scheme == "https" ? 443 : 80) }
}
