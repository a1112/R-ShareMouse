import XCTest

final class ConnectionUITests: XCTestCase {
    func testConnectionScreenAndOfflineNavigation() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing"]
        app.launch()
        XCTAssertTrue(app.textFields["pairingLink"].waitForExistence(timeout: 8)
            || app.textViews["pairingLink"].waitForExistence(timeout: 3))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "iOS connection page"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.tabBars.buttons["遥控"].tap()
        XCTAssertTrue(app.buttons["前往连接"].waitForExistence(timeout: 3))
        app.tabBars.buttons["扩展屏"].tap()
        XCTAssertTrue(app.staticTexts["准备好另一块屏幕"].waitForExistence(timeout: 3))
        app.tabBars.buttons["设置"].tap()
        XCTAssertTrue(app.buttons["忘记这台电脑"].waitForExistence(timeout: 3))
        XCTAssertFalse(app.buttons["忘记这台电脑"].isEnabled)
    }

    func testInvalidPairingStaysOnConnectionPage() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing"]
        app.launch()
        let input = app.textViews["pairingLink"].exists ? app.textViews["pairingLink"] : app.textFields["pairingLink"]
        XCTAssertTrue(input.waitForExistence(timeout: 8))
        input.tap()
        input.typeText("https://example.com/mobile?t=invalid")
        app.buttons["connectHost"].tap()
        XCTAssertTrue(app.staticTexts["connectionError"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.tabBars.buttons["连接"].isSelected)
    }
}
