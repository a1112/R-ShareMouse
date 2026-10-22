import XCTest

final class LiveClientUITests: XCTestCase {
    func testNativeUSBVideoDecoderFullscreenAndBackgroundCleanup() throws {
        guard let code = ProcessInfo.processInfo.environment["RSHARE_USB_TEST_CODE"] else {
            throw XCTSkip("Run scripts/test-ios-usb-receiver.py with a dedicated simulator")
        }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--usb-display-test"]
        app.launchEnvironment["RSHARE_USB_CODE"] = code
        let lan = ProcessInfo.processInfo.environment["RSHARE_DISPLAY_TEST_TRANSPORT"] == "lan"
        app.launchEnvironment["RSHARE_DISPLAY_TRANSPORT"] = lan ? "lan" : "usb"
        app.launch()
        defer { XCUIDevice.shared.orientation = .portrait; app.terminate() }
        let status = app.staticTexts["usbDisplayStatus"]
        let playing = NSPredicate(format: "label CONTAINS %@", "\(lan ? "局域网" : "USB") 画面播放中")
        expectation(for: playing, evaluatedWith: status)
        waitForExpectations(timeout: 30)
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "Native USB H264 decoded video"; screenshot.lifetime = .keepAlways; add(screenshot)
        app.buttons["全屏"].tap()
        XCUIDevice.shared.orientation = .landscapeLeft
        let surface = app.otherElements["usbDisplayVideo"]
        XCTAssertTrue(surface.waitForExistence(timeout: 5))
        XCTAssertGreaterThan(surface.frame.width, surface.frame.height)
        let landscape = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        landscape.name = "Native USB H264 landscape"; landscape.lifetime = .keepAlways; add(landscape)
        app.buttons["退出全屏"].tap()
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(app.buttons["开启 \(lan ? "局域网" : "USB") 接收"].waitForExistence(timeout: 8), "Background must close the native receiver")
    }
    func testMobileDisplayPlaybackFullscreenRotationAndRecovery() throws {
        guard let url = ProcessInfo.processInfo.environment["RSHARE_DISPLAY_TEST_URL"] else {
            throw XCTSkip("Run tests/mobile-display-ios.mjs to start the WebRTC fixture")
        }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing"]
        app.launchEnvironment["RSHARE_PAIRING_URL"] = url
        app.launch()
        defer { XCUIDevice.shared.orientation = .portrait; app.terminate() }
        let playback = app.staticTexts["displayPlayback"]
        XCTAssertTrue(playback.waitForExistence(timeout: 25), "Must decode real WebRTC video before fullscreen")
        capture(app, "Mobile display decoded video")
        app.webViews.buttons["播放 / 全屏"].tap()
        let exit = app.webViews.buttons["退出全屏"]
        XCTAssertTrue(exit.waitForExistence(timeout: 5))
        let settingsTab = app.buttons["设置"].firstMatch
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: settingsTab)], timeout: 5) == .completed)
        XCUIDevice.shared.orientation = .landscapeLeft
        let fitsLandscape = NSPredicate { _, _ in
            let window = app.frame, web = app.webViews.firstMatch.frame
            return window.width > window.height && web.width >= window.width - 4
                && exit.frame.maxX <= window.maxX && exit.frame.maxY <= window.maxY
        }
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: fitsLandscape, object: app)], timeout: 5) == .completed,
                      "Fullscreen must resize the web surface and keep its exit control on screen")
        capture(app, "Mobile display landscape fullscreen")
        exit.tap()
        XCTAssertTrue(settingsTab.waitForExistence(timeout: 5))
        app.webViews.buttons["断开"].tap()
        app.webViews.buttons["重新连接"].tap()
        XCTAssertTrue(playback.waitForExistence(timeout: 20))
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(playback.waitForExistence(timeout: 20))
        capture(app, "Mobile display foreground recovery")
    }

    func testPairedPhoneConnectsAndOpensDisplayReceiver() throws {
        let app = XCUIApplication()
        let preset = ProcessInfo.processInfo.environment["RSHARE_LIVE_PAIRING_URL"]
        #if targetEnvironment(simulator)
        guard preset != nil else { throw XCTSkip("Requires an explicitly supplied live LAN pairing URL") }
        #endif
        if let preset {
            app.launchArguments = ["--ui-testing"]
            app.launchEnvironment["RSHARE_PAIRING_URL"] = preset
        }
        app.launch()
        if preset == nil {
            XCTAssertTrue(app.buttons["重新连接"].waitForExistence(timeout: 8))
            capture(app, "iPhone connection page")
            app.buttons["重新连接"].tap()
        }
        let status = app.staticTexts["remoteStatus"]
        XCTAssertTrue(status.waitForExistence(timeout: 10))
        XCTAssertTrue(NSPredicate(format: "label == %@", "已连接").evaluate(with: status)
            || XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "label == %@", "已连接"), object: status)], timeout: 10) == .completed)
        capture(app, "iPhone connected touchpad")
        // Scroll from the page margin, outside the touchpad's input surface.
        let web = app.webViews.firstMatch
        web.coordinate(withNormalizedOffset: CGVector(dx: 0.99, dy: 0.90))
            .press(forDuration: 0.05, thenDragTo: web.coordinate(withNormalizedOffset: CGVector(dx: 0.99, dy: 0.15)))
        XCTAssertTrue(web.buttons["发送"].waitForExistence(timeout: 5))
        XCTAssertTrue(web.buttons["发送"].isHittable)
        capture(app, "iPhone keyboard controls")
        app.tabBars.buttons["扩展屏"].tap()
        XCTAssertTrue(app.webViews.firstMatch.waitForExistence(timeout: 8))
        XCTAssertTrue(app.webViews.staticTexts["扩展显示器 · 接收端"].waitForExistence(timeout: 8))
        capture(app, "iPhone extended display receiver")
        app.tabBars.buttons["遥控"].tap()
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "label == %@", "已连接"), object: status)], timeout: 10) == .completed)
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        // Capture the composited display: application-scoped screenshots on
        // iOS can retain portrait backing coordinates after window rotation.
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
