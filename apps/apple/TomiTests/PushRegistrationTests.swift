#if os(iOS)
import XCTest
@testable import Tomi

/// The push doorbell of the iPhone: which endpoint of Apple a token is for.
final class PushRegistrationTests: XCTestCase {
    private func profile(_ entitlements: String) -> Data {
        // A provisioning profile: binary signature around a property list in clear.
        var data = Data([0x30, 0x82, 0x00, 0xff, 0xfe])
        data.append(Data("""
        <plist><dict><key>Entitlements</key><dict>\(entitlements)</dict></dict></plist>
        """.utf8))
        data.append(Data([0x00, 0xff]))
        return data
    }

    func testABuildInstalledFromXcodeIsForTheDevelopmentEndpoint() {
        let data = profile("<key>aps-environment</key>\n\t<string>development</string>")
        XCTAssertEqual(PushRegistration.environment(inProfile: data), "development")
    }

    func testADistributionProfileIsForProduction() {
        let data = profile("<key>aps-environment</key><string>production</string>")
        XCTAssertEqual(PushRegistration.environment(inProfile: data), "production")
    }

    func testAProfileWithoutPushSaysNothing() {
        XCTAssertNil(PushRegistration.environment(inProfile: profile("<key>get-task-allow</key><true/>")))
        // The value of another key is never taken for it.
        let other = profile("<key>aps-environment</key><true/><key>team</key><string>development</string>")
        XCTAssertNil(PushRegistration.environment(inProfile: other))
        XCTAssertNil(PushRegistration.environment(inProfile: Data()))
    }

    func testTheGenericNotificationHasItsFixedWords() {
        // The relay names these two texts (`apns.rs`): they must exist in the app, or iOS shows the raw keys.
        XCTAssertEqual(L10n.text("ios_push_title"), "Tomi")
        // Whatever the language of the run: a text of its own, never the key.
        XCTAssertNotEqual(L10n.text("ios_push_body"), "ios_push_body")
        XCTAssertFalse(L10n.text("ios_push_body").isEmpty)
    }
}
#endif
