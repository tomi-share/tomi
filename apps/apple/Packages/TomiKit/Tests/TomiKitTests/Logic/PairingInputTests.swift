import TomiKit
import XCTest

final class PairingInputTests: XCTestCase {
    /// Android `PairingUiLogicTest`: the core reads the code, only what is clearly not one is refused here.
    func testOnlyWhatIsClearlyNotACodeIsRefused() {
        XCTAssertTrue(PairingInput.isValidCode("7-crayon-lune"))
        XCTAssertTrue(PairingInput.isValidCode("  4821-crayon-renard \n"))
        XCTAssertTrue(PairingInput.isValidCode("7 crayon renard"))
        XCTAssertTrue(PairingInput.isValidCode("7.Crayon.Rénard"))
        XCTAssertFalse(PairingInput.isValidCode(""))
        XCTAssertFalse(PairingInput.isValidCode("   "))
        XCTAssertFalse(PairingInput.isValidCode("7-crayon\u{0007}renard"))
        XCTAssertFalse(PairingInput.isValidCode(String(repeating: "a", count: 65)))
        XCTAssertTrue(PairingInput.isValidCode(String(repeating: "a", count: 64)))
    }

    func testAScannedCodeMustBeAPairingLinkAndItsRelayIsKept() {
        let reader = FakeLinkReader()
        XCTAssertEqual(
            PairingInput.linkFromQR("https://relay.example.org/l/pair#4821-crayon-renard", reader: reader),
            ParsedPairLink(relayUrl: "https://relay.example.org", code: "4821-crayon-renard")
        )
        // Any relay is a candidate: the screens show it in full for the user to confirm.
        XCTAssertEqual(
            PairingInput.linkFromQR(" https://other.example.net:8443/l/pair#4821-crayon-renard \n", reader: reader)?.relayUrl,
            "https://other.example.net:8443"
        )
        let fallback = PairingInput.linkFromQR("tomi-share://pair#4821-crayon-renard", reader: reader)
        XCTAssertNotNil(fallback, "the fallback form is a pairing link too")
        XCTAssertNil(fallback?.relayUrl, "…which names no relay")
        XCTAssertNil(PairingInput.linkFromQR("https://relay.example.org/l/item/abc", reader: reader))
        XCTAssertNil(PairingInput.linkFromQR("WIFI:S:maison;T:WPA;P:secret;;", reader: reader))
        XCTAssertNil(PairingInput.linkFromQR("", reader: reader))
        XCTAssertNil(PairingInput.linkFromQR("https://relay.example.org/l/pair#" + String(repeating: "a", count: 600), reader: reader))
        XCTAssertNil(PairingInput.linkFromQR("https://relay.example.org/l/pair#4821-crayon-renard", reader: UnavailableLinkReader()))
    }

    func testTheVerificationCodeReadsInTwoGroups() {
        XCTAssertEqual(PairingInput.groupedVerificationCode("482193"), "482 193")
        XCTAssertEqual(PairingInput.groupedVerificationCode("12"), "12")
    }
}
