import XCTest

final class CoreJourneyTests: XCTestCase {
    /// Core journey smoke test — expanded in Phase 9 (T072).
    func testAppLaunches() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))
    }

    /// T072/quickstart: open → search → browse → bookmark → theme switch.
    func testCoreJourney() {
        let app = XCUIApplication()
        app.launch()
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))

        // Browse: enter a domain in the address bar (URL-vs-search resolution).
        let address = window.textFields["addressField"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.click()
        address.typeText("example.com\n")

        // Bookmark: the star enables once the navigation is under way.
        let star = window.buttons["bookmarkButton"]
        XCTAssertTrue(star.waitForExistence(timeout: 5))
        let enabled = NSPredicate(format: "isEnabled == true")
        expectation(for: enabled, evaluatedWith: star)
        waitForExpectations(timeout: 20)
        star.click()

        // Theme switch: Settings (⌘,) → Dark → back to System, instant apply.
        app.typeKey(",", modifierFlags: .command)
        let dark = segment(app, "Dark")
        XCTAssertTrue(dark.waitForExistence(timeout: 5), "settings theme picker missing")
        dark.click()
        segment(app, "System").click()

        // App is still responsive after the journey.
        XCTAssertTrue(window.exists)
    }

    /// Segmented pickers expose segments as radio buttons on some macOS
    /// versions and plain buttons on others.
    private func segment(_ app: XCUIApplication, _ label: String) -> XCUIElement {
        let radio = app.radioButtons[label]
        return radio.exists ? radio : app.buttons[label]
    }
}
