import XCTest
@testable import TomiKit

// Port of the `RelayAddress` / `RelayChoice` tests of the Android app, and the creation input (spec/pairing-v1.md 4.4
// rule 6, 10.3). The core is the judge of a valid relay; these tests use a stand-in that follows its rules.
final class RelayAddressTests: XCTestCase {
    private let reader = FakeLinkReader()
    private func canonical(_ raw: String) -> String? { RelayAddress.canonical(raw, reader: reader) }

    func testARelayIsTypedWithOrWithoutItsScheme() {
        XCTAssertEqual(canonical("relay.example.org"), "https://relay.example.org")
        XCTAssertEqual(canonical("https://relay.example.org"), "https://relay.example.org")
        XCTAssertEqual(canonical("  HTTPS://Relay.Example.org/  \n"), "https://relay.example.org")
        XCTAssertEqual(canonical("relay.example.org:8443"), "https://relay.example.org:8443")
        XCTAssertEqual(canonical("https://relay.example.org:443"), "https://relay.example.org")
    }

    func testWhatIsNotARelayAddressIsRefused() {
        XCTAssertNil(canonical(""))
        XCTAssertNil(canonical("   "))
        XCTAssertNil(canonical("http://relay.example.org"), "https only")
        XCTAssertNil(canonical("ftp://relay.example.org"))
        XCTAssertNil(canonical("https://user@relay.example.org"), "no user information")
        XCTAssertNil(canonical("https://relay.example.org/path"), "no path")
        XCTAssertNil(canonical("https://relay.example.org?a=1"), "no query")
        XCTAssertNil(canonical("https://relay.example.org#frag"), "no fragment")
        XCTAssertNil(canonical("relay example.org"), "no blank inside")
        XCTAssertNil(canonical("https://relay.example.org:0"))
        XCTAssertNil(canonical("https://relay.example.org:99999"))
        XCTAssertNil(canonical("https://rélay.example.org"), "ASCII host")
        XCTAssertNil(canonical(String(repeating: "a", count: RelayAddress.maxInput + 1)))
        XCTAssertNil(RelayAddress.canonical("relay.example.org", reader: UnavailableLinkReader()), "no core, no guess")
    }

    func testTheAuthorityIsShownInFull() {
        XCTAssertEqual(RelayAddress.authority(of: "https://relay.example.org"), "relay.example.org")
        XCTAssertEqual(RelayAddress.authority(of: "https://Relay.Example.org:8443"), "relay.example.org:8443")
        XCTAssertEqual(RelayAddress.authority(of: "https://relay.example.org:443"), "relay.example.org")
        XCTAssertNil(RelayAddress.authority(of: ""))
    }

    func testAnEmptyFieldWithoutADeviceRelayIsMissing() {
        let choice = RelayAddress.choose(field: "  ", deviceRelay: nil, reader: reader)
        XCTAssertEqual(choice, .missing)
        XCTAssertEqual(choice.errorKey, "pair_error_relay_required")
        XCTAssertNil(choice.urlToPass)
        XCTAssertNil(choice.authority)
    }

    func testAnEmptyFieldUsesTheRelayOfTheDevice() {
        let choice = RelayAddress.choose(field: "", deviceRelay: "https://relay.example.org", reader: reader)
        XCTAssertEqual(choice, .device(authority: "relay.example.org"))
        XCTAssertNil(choice.urlToPass, "the core uses its own relay when none is passed")
        XCTAssertEqual(choice.authority, "relay.example.org")
        XCTAssertNil(choice.errorKey)
    }

    func testATypedRelayIsPassedAndShown() {
        let choice = RelayAddress.choose(field: "other.example.net:8443", deviceRelay: "https://relay.example.org", reader: reader)
        XCTAssertEqual(choice, .typed(url: "https://other.example.net:8443", authority: "other.example.net:8443"))
        XCTAssertEqual(choice.urlToPass, "https://other.example.net:8443")
        XCTAssertEqual(choice.authority, "other.example.net:8443")
    }

    func testATypedRelayThatIsNotAnAddressIsInvalidWhateverTheDeviceHas() {
        for device in [nil, "https://relay.example.org"] {
            let choice = RelayAddress.choose(field: "http://relay.example.org", deviceRelay: device, reader: reader)
            XCTAssertEqual(choice, .invalid)
            XCTAssertEqual(choice.errorKey, "pair_error_relay_invalid")
        }
    }
}

final class CreateInputTests: XCTestCase {
    private let reader = FakeLinkReader()
    private let token = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"

    func testACreationLinkGivesItsRelayAndItsToken() {
        XCTAssertEqual(
            CreateInput.read("https://relay.example.org/create#\(token)", reader: reader),
            .link(relayUrl: "https://relay.example.org", token: token)
        )
        XCTAssertEqual(
            CreateInput.read("  https://relay.example.org:8443/create/\(token)\n", reader: reader),
            .link(relayUrl: "https://relay.example.org:8443", token: token)
        )
    }

    func testABareTokenIsKeptAsIs() {
        XCTAssertEqual(CreateInput.read(" \(token) ", reader: reader), .token(token))
        XCTAssertEqual(CreateInput.read(token.uppercased(), reader: reader), .token(token.uppercased()))
    }

    func testAnythingElseIsRefused() {
        XCTAssertNil(CreateInput.read("", reader: reader))
        XCTAssertNil(CreateInput.read("https://example.org/", reader: reader))
        XCTAssertNil(CreateInput.read("https://relay.example.org/l/pair#4821-crayon-renard", reader: reader), "a pairing link")
        XCTAssertNil(CreateInput.read("4821-crayon-renard", reader: reader))
        XCTAssertNil(CreateInput.read("http://relay.example.org/create#\(token)", reader: reader))
        XCTAssertNil(CreateInput.read(String(repeating: "x", count: CreateInput.maxRaw + 1), reader: reader))
        XCTAssertNil(CreateInput.read(token, reader: UnavailableLinkReader()), "no core, no guess")
    }

    func testTheReadingPrintsNoToken() {
        for reading in [
            CreateInput.Reading.link(relayUrl: "https://relay.example.org", token: token),
            CreateInput.Reading.token(token),
        ] {
            XCTAssertFalse(String(describing: reading).contains(token))
        }
    }
}
