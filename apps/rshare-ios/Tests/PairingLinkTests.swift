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
        let link = try PairingLink("http://192.168.1.10:27437/display\nAuthorization: Bearer a&b+c")
        XCTAssertEqual(link.controlURL.path, "/mobile")
        XCTAssertEqual(link.displayURL.path, "/display")
        XCTAssertNil(link.controlURL.query)
        XCTAssertNil(link.displayURL.query)
        XCTAssertEqual(link.bearerToken, "a&b+c")
        XCTAssertEqual(link.authorizationHeader, "Bearer a&b+c")
        XCTAssertEqual(link.initialPath, "/display")
        XCTAssertEqual(link.serialized, "http://192.168.1.10:27437/display\nAuthorization: Bearer a&b+c")
        let roundTrip = try PairingLink(link.serialized)
        XCTAssertEqual(roundTrip.initialPath, "/display")
        XCTAssertEqual(roundTrip.serialized, link.serialized)
        XCTAssertTrue(link.allowsNavigation(to: link.displayURL))

        let legacy = try PairingLink("http://192.168.1.10:27437/display?role=host&t=a%26b%2Bc#fragment")
        XCTAssertEqual(legacy.bearerToken, "a&b+c")
        XCTAssertNil(legacy.displayURL.query)
        XCTAssertEqual(legacy.initialPath, "/display")
    }

    func testRejectsUntrustedOrIncompletePairing() {
        for value in ["https://example.com/mobile?t=x", "javascript:alert(1)",
                      "http://192.168.1.10/mobile", "http://192.168.1.10/mobile?t=",
                      "http://192.168.1.10/mobile?t=x&t=y", "http://user:pass@192.168.1.10/mobile?t=x",
                      "http://192.168.1.10/mobile\nAuthorization: Basic x",
                      "http://192.168.1.10/mobile\nAuthorization: Bearer first\nBearer second",
                      "http://192.168.1.10/admin?t=x", "http://172.32.0.1/mobile?t=x",
                      "http://192.168.1.10:0/mobile?t=x"] {
            XCTAssertThrowsError(try PairingLink(value), value)
        }
        XCTAssertThrowsError(try PairingLink("http://192.168.1.10/mobile\nBearer bad\u{0000}token"))
    }

    func testNavigationCannotChangeOriginCredentialOrBecomeHost() throws {
        let link = try PairingLink("http://192.168.1.10:27437/mobile\nBearer secret")
        for value in ["http://192.168.1.11:27437/mobile",
                      "http://192.168.1.10:27438/mobile",
                      "https://192.168.1.10:27437/mobile",
                      "http://192.168.1.10:27437/mobile?role=host",
                      "http://192.168.1.10:27437/mobile?t=secret",
                      "http://192.168.1.10:27437/mobile#fragment"] {
            XCTAssertFalse(link.allowsNavigation(to: URL(string: value)!), value)
        }
    }

    @MainActor
    func testInitialWebViewRequestCarriesBearerOutsideTheURL() throws {
        let session = RemoteSession(persistent: false)
        session.pairing = try PairingLink("http://192.168.1.10:27437/mobile\nBearer secret")
        let request = session.authorizedRequest(for: session.pairing!.controlURL)
        XCTAssertEqual(request.url?.query, nil)
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer secret")
    }

    @MainActor
    func testDisplayPairingWithSeparateBearerSelectsDisplayTab() {
        let session = RemoteSession(persistent: false)
        session.connect("http://127.0.0.1:27437/display\nBearer test")
        XCTAssertEqual(session.tab, .display)
        session.disconnect()
    }

    @MainActor
    func testReconnectFromConnectionLoadsSavedMobilePairing() throws {
        let session = RemoteSession(persistent: false)
        session.pairing = try PairingLink("http://127.0.0.1:27437/mobile\nBearer test")
        XCTAssertEqual(session.tab, .connection)

        session.reconnect()

        XCTAssertEqual(session.tab, .remote)
        XCTAssertTrue(session.loading)
        XCTAssertEqual(session.status, "正在连接")
        session.disconnect()
    }

    @MainActor
    func testReconnectFromConnectionLoadsSavedDisplayPairing() throws {
        let session = RemoteSession(persistent: false)
        session.pairing = try PairingLink("http://127.0.0.1:27437/display\nBearer test")
        XCTAssertEqual(session.tab, .connection)

        session.reconnect()

        XCTAssertEqual(session.tab, .display)
        XCTAssertTrue(session.loading)
        XCTAssertEqual(session.status, "等待扩展屏")
        session.disconnect()
    }

    func testIPv6AndLocalNames() throws {
        for host in ["[fd12::1]", "[::1]", "macbook.local", "desktop.lan", "10.1.2.3", "172.16.0.1"] {
            XCTAssertNoThrow(try PairingLink("http://\(host):27437/mobile\nBearer secret"))
        }
        XCTAssertThrowsError(try PairingLink("http://[2001:4860:4860::8888]/mobile\nBearer x"))
    }
}
