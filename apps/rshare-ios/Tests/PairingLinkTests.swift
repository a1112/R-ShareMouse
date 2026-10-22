import XCTest
@testable import RShareMouse

final class PairingLinkTests: XCTestCase {
    @MainActor
    func testDisplayLifecycleReleasesIdleTimerAndImmersiveMode() {
        let session = RemoteSession(persistent: false)
        session.tab = .display
        session.updateDisplayPresentation(playing: true, immersive: true)
        XCTAssertTrue(session.displayImmersive)
        XCTAssertTrue(UIApplication.shared.isIdleTimerDisabled)
        session.suspend()
        XCTAssertFalse(session.displayImmersive)
        XCTAssertFalse(UIApplication.shared.isIdleTimerDisabled)
        session.tab = .remote
        session.updateDisplayPresentation(playing: true, immersive: true)
        XCTAssertFalse(session.displayImmersive)
        XCTAssertFalse(UIApplication.shared.isIdleTimerDisabled)
    }

    @MainActor
    func testDisplayPairingOpensReceiverDirectlyAndReconnectKeepsTab() {
        let session = RemoteSession(persistent: false)
        session.connect("http://127.0.0.1:27437/display?t=test")
        XCTAssertEqual(session.tab, .display)
        session.reconnect()
        XCTAssertEqual(session.tab, .display)
        session.disconnect()
    }
    func testPairingKeepsCredentialAndForcesReceiverRole() throws {
        let link = try PairingLink("http://192.168.1.10:27437/display?role=host&t=a%26b%2Bc#fragment")
        XCTAssertEqual(link.controlURL.path, "/mobile")
        XCTAssertEqual(link.displayURL.path, "/display")
        let query = URLComponents(url: link.displayURL, resolvingAgainstBaseURL: false)?.queryItems
        XCTAssertEqual(query, [URLQueryItem(name: "t", value: "a&b+c")])
        XCTAssertFalse(link.hostLabel.contains("a&b"))
        XCTAssertTrue(link.allowsNavigation(to: link.displayURL))
    }

    func testRejectsUntrustedOrIncompletePairing() {
        for value in ["https://example.com/mobile?t=x", "javascript:alert(1)",
                      "http://192.168.1.10/mobile", "http://192.168.1.10/mobile?t=",
                      "http://192.168.1.10/mobile?t=x&t=y", "http://user:pass@192.168.1.10/mobile?t=x",
                      "http://192.168.1.10/admin?t=x", "http://172.32.0.1/mobile?t=x",
                      "http://192.168.1.10:0/mobile?t=x"] {
            XCTAssertThrowsError(try PairingLink(value), value)
        }
    }

    func testNavigationCannotChangeOriginCredentialOrBecomeHost() throws {
        let link = try PairingLink("http://192.168.1.10:27437/mobile?t=secret")
        for value in ["http://192.168.1.11:27437/mobile?t=secret",
                      "http://192.168.1.10:27438/mobile?t=secret",
                      "https://192.168.1.10:27437/mobile?t=secret",
                      "http://192.168.1.10:27437/mobile?t=other",
                      "http://192.168.1.10:27437/display?t=secret&role=host",
                      "http://192.168.1.10:27437/mobile"] {
            XCTAssertFalse(link.allowsNavigation(to: URL(string: value)!), value)
        }
    }

    func testIPv6AndLocalNames() throws {
        for host in ["[fd12::1]", "[::1]", "macbook.local", "desktop.lan", "10.1.2.3", "172.16.0.1"] {
            XCTAssertNoThrow(try PairingLink("http://\(host):27437/mobile?t=secret"))
        }
        XCTAssertThrowsError(try PairingLink("http://[2001:4860:4860::8888]/mobile?t=x"))
    }
}
