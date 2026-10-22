import Foundation
import Network

struct PairingLink: Equatable {
    let controlURL: URL
    let displayURL: URL
    let hostLabel: String
    let bearerToken: String
    /// The path supplied by the user, retained so a display pairing opens in
    /// the display tab even though both canonical URLs are stored together.
    let initialPath: String

    /// The URL deliberately contains no credential. The bearer is carried on
    /// its own line so it is never copied into browser history, referrers or
    /// navigation URLs.
    var serialized: String {
        let initialURL = initialPath == "/display" ? displayURL : controlURL
        return "\(initialURL.absoluteString)\nAuthorization: Bearer \(bearerToken)"
    }

    var authorizationHeader: String { "Bearer \(bearerToken)" }

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
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.utf8.count <= 4096 else { throw Failure.invalid }
        let lines = trimmed.split(whereSeparator: \.isNewline).map(String.init)
        guard let urlLine = lines.first,
              var parts = URLComponents(string: urlLine),
              ["http", "https"].contains(parts.scheme?.lowercased() ?? ""),
              let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil,
              ["", "/", "/mobile", "/display"].contains(parts.path),
              parts.port == nil || (1...65535).contains(parts.port!)
        else { throw Failure.invalid }
        guard Self.isLocalHost(host) else { throw Failure.publicHost }
        let suppliedPath = parts.path.isEmpty ? "/mobile" : parts.path
        let queryItems = parts.queryItems ?? []
        let legacyTokens = queryItems.filter { $0.name == "t" }
        let bearerTokens = lines.dropFirst().compactMap(Self.bearerToken(from:))
        guard bearerTokens.count <= 1 else { throw Failure.invalid }
        if !bearerTokens.isEmpty && !legacyTokens.isEmpty { throw Failure.invalid }
        if !bearerTokens.isEmpty && !queryItems.isEmpty { throw Failure.invalid }
        let token: String
        if let bearer = bearerTokens.first {
            token = bearer
        } else if legacyTokens.count == 1, let legacy = legacyTokens.first?.value {
            // Accept links issued by older desktop builds once, then strip
            // the credential from the canonical URLs and Keychain payload.
            token = legacy
        } else {
            throw Failure.missingToken
        }
        guard Self.isValidToken(token) else { throw Failure.missingToken }
        parts.scheme = parts.scheme?.lowercased()
        parts.fragment = nil
        parts.queryItems = nil
        parts.path = "/mobile"
        guard let control = parts.url else { throw Failure.invalid }
        controlURL = control
        parts.path = "/display"
        guard let display = parts.url else { throw Failure.invalid }
        displayURL = display
        hostLabel = host + (parts.port.map { ":\($0)" } ?? "")
        bearerToken = token
        initialPath = suppliedPath
    }

    private static func bearerToken(from line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        let lowercased = trimmed.lowercased()
        if lowercased.hasPrefix("authorization:") {
            let value = String(trimmed.dropFirst("authorization:".count))
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard value.lowercased().hasPrefix("bearer ") else { return nil }
            return String(value.dropFirst("bearer ".count)
                .trimmingCharacters(in: .whitespacesAndNewlines))
        } else if lowercased.hasPrefix("bearer ") {
            return String(trimmed.dropFirst("bearer ".count)
                .trimmingCharacters(in: .whitespacesAndNewlines))
        } else {
            return nil
        }
    }

    private static func isValidToken(_ token: String) -> Bool {
        !token.isEmpty && token.utf8.count <= 4096
            && token.rangeOfCharacter(from: .whitespacesAndNewlines) == nil
            && token.rangeOfCharacter(from: .controlCharacters) == nil
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
            && url.query == nil && url.fragment == nil
    }

    func allowsNavigation(to url: URL) -> Bool {
        guard isPairedDocument(url),
              effectivePort(url) == effectivePort(controlURL),
              ["/mobile", "/display"].contains(url.path),
              url.user == nil, url.password == nil
        else { return false }
        return url.query == nil && url.fragment == nil
    }

    private func effectivePort(_ url: URL) -> Int { url.port ?? (url.scheme == "https" ? 443 : 80) }
}
