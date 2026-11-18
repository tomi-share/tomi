import XCTest
@testable import TomiKit

// Port of apps/android/.../deeplink/DeepLinkParserTest.kt, plus the DeepLink.Send / DeepLink.Pair cases of
// core/RedactionTest.kt (they must print no content).
final class DeepLinkTests: XCTestCase {
    /// The core reads pairing and creation links; the tests use a stand-in that follows its rules.
    private func parse(_ raw: String?) -> DeepLink? { DeepLinkParser.parse(raw, reader: FakeLinkReader()) }
    private let token = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"

    func testSendWithUrlAndTitle() {
        let link = parse("tomi-share://send?url=https%3A%2F%2Fexample.org%2Fa%3Fb%3D1&title=Mon%20titre")
        XCTAssertEqual(link, .send(title: "Mon titre", url: "https://example.org/a?b=1", text: nil, droppedParameters: false))
    }

    func testSendWithText() {
        let link = parse("tomi-share://send?text=Bonjour%20%C3%A0%20tous")
        XCTAssertEqual(link, .send(title: nil, url: nil, text: "Bonjour à tous", droppedParameters: false))
    }

    func testNonHttpUrlIsDropped() {
        let link = parse("tomi-share://send?url=javascript%3Aalert(1)")
        XCTAssertEqual(link, .send(title: nil, url: nil, text: nil, droppedParameters: true))
    }

    func testOversizedParametersAreDropped() {
        let long = String(repeating: "a", count: DeepLinkParser.maxTextParam + 1)
        let link = parse("tomi-share://send?text=\(long)")
        XCTAssertEqual(link, .send(title: nil, url: nil, text: nil, droppedParameters: true))
    }

    func testOversizedUriIsRejectedEntirely() {
        XCTAssertNil(parse("tomi-share://send?text=" + String(repeating: "a", count: DeepLinkParser.maxURILength)))
    }

    func testThereIsNoSecretParameter() {
        // Android checks by reflection that Send has no field beyond title/url/text/droppedParameters. The Swift case has
        // exactly those four associated values, and a "secret" parameter changes nothing in the parsed value.
        let withSecret = parse("tomi-share://send?text=abc&secret=true")
        XCTAssertEqual(withSecret, .send(title: nil, url: nil, text: "abc", droppedParameters: false))
        XCTAssertEqual(withSecret, parse("tomi-share://send?text=abc"))
    }

    func testItemLinkValidatesTheId() {
        XCTAssertEqual(parse("tomi-share://item/abc-123_X"), .item(id: "abc-123_X"))
        XCTAssertNil(parse("tomi-share://item/"))
        XCTAssertNil(parse("tomi-share://item/../../etc"))
        XCTAssertNil(parse("tomi-share://item/" + String(repeating: "a", count: 65)))
    }

    func testOpenLink() {
        XCTAssertEqual(parse("tomi-share://open"), .open)
    }

    func testUnknownCommandsAreIgnored() {
        XCTAssertNil(parse("tomi-share://delete?id=1"))
        XCTAssertNil(parse("tomi-share://revoke/dev-1"))
        XCTAssertNil(parse("tomi-share://settings?relay=https://evil.example"))
        XCTAssertNil(parse("ftp://example.org"))
        XCTAssertNil(parse(nil))
        XCTAssertNil(parse("not a uri"))
    }

    func testTheFallbackPairingLinkNamesNoRelay() {
        XCTAssertEqual(parse("tomi-share://pair#4821-crayon-renard"), .pair(relayUrl: nil, code: "4821-crayon-renard"))
        // The code is in the fragment: a query string (it reaches logs) or no fragment at all is not a pairing link.
        XCTAssertNil(parse("tomi-share://pair"))
        XCTAssertNil(parse("tomi-share://pair?code=4821-crayon-renard"))
        XCTAssertNil(parse("tomi-share://pair/4821-crayon-renard"))
        XCTAssertNil(parse("tomi-share://pair#4821 crayon renard"))
        XCTAssertNil(parse("tomi-share://pair#" + String(repeating: "a", count: 600)))
        // Another host cannot pose as the pairing link.
        XCTAssertNil(parse("tomi-share://evil.example#4821-crayon-renard"))
    }

    func testAPairingLinkNeverPrintsItsCode() {
        XCTAssertEqual(String(describing: parse("tomi-share://pair#4821-crayon-renard")!), "Pair(<redacted>)")
        XCTAssertFalse(String(describing: ParsedPairLink(relayUrl: nil, code: "4821-crayon-renard")).contains("crayon"))
    }

    func testAnAppLinkPairingNamesItsRelayWhateverItIs() {
        XCTAssertEqual(
            parse("https://relay.example.org/l/pair#abcDEF123_-"),
            .pair(relayUrl: "https://relay.example.org", code: "abcDEF123_-")
        )
        // The relay of a link is read, not trusted: another one is a candidate like any other, shown in full by the screens.
        XCTAssertEqual(
            parse("https://other.example.net:8443/l/pair#abc"),
            .pair(relayUrl: "https://other.example.net:8443", code: "abc")
        )
        XCTAssertNil(parse("https://relay.example.org/l/pair"), "no code")
        XCTAssertNil(parse("https://user@relay.example.org/l/pair#abc"), "no user information in a relay")
        XCTAssertNil(parse("https://relay.example.org/l/pair#" + String(repeating: "a", count: 600)))
        XCTAssertNil(parse("http://relay.example.org/l/pair#abc"), "https only")
    }

    func testAnUnavailableReaderRefusesEveryPairingAndCreationLink() {
        let none = UnavailableLinkReader()
        XCTAssertNil(DeepLinkParser.parse("tomi-share://pair#4821-crayon-renard", reader: none))
        XCTAssertNil(DeepLinkParser.parse("https://relay.example.org/l/pair#abc", reader: none))
        XCTAssertNil(DeepLinkParser.parse("https://relay.example.org/create#\(token)", reader: none))
        XCTAssertEqual(DeepLinkParser.parse("tomi-share://open", reader: none), .open, "the others need no reader")
    }

    func testACreationLinkCarriesItsRelayAndToken() {
        XCTAssertEqual(
            parse("https://relay.example.org/create#\(token)"),
            .setup(relayUrl: "https://relay.example.org", token: token)
        )
        XCTAssertEqual(
            parse("https://relay.example.org/create/\(token)"),
            .setup(relayUrl: "https://relay.example.org", token: token)
        )
        XCTAssertNil(parse("https://relay.example.org/create"), "no token")
        XCTAssertNil(parse("https://relay.example.org/create#not-a-token"))
        XCTAssertNil(parse("http://relay.example.org/create#\(token)"))
        XCTAssertNil(parse("tomi-share://create#\(token)"), "no custom-scheme form")
    }

    func testACreationLinkNeverPrintsItsToken() {
        let link = parse("https://relay.example.org/create#\(token)")!
        for printed in [String(describing: link), String(reflecting: link)] {
            XCTAssertFalse(printed.contains(token), printed)
            XCTAssertTrue(printed.contains("redacted"), printed)
        }
        XCTAssertFalse(String(describing: ParsedSetupLink(relayUrl: "https://relay.example.org", token: token)).contains(token))
    }

    func testOtherAppLinkPathsJustOpenTheApp() {
        XCTAssertEqual(parse("https://relay.example.org/l/anything"), .open)
        XCTAssertNil(parse("https://relay.example.org/other"))
    }

    // --- iOS additions: URL input and injected reader ----------------------------------------------------------------

    func testParsesFromAURL() throws {
        let url = try XCTUnwrap(URL(string: "https://relay.example.org/l/pair#abc"))
        XCTAssertEqual(DeepLinkParser.parse(url, reader: FakeLinkReader()), .pair(relayUrl: "https://relay.example.org", code: "abc"))
        XCTAssertEqual(
            DeepLinkParser.parse(try XCTUnwrap(URL(string: "tomi-share://pair#abc")), reader: FakeLinkReader()),
            .pair(relayUrl: nil, code: "abc")
        )
        XCTAssertNil(DeepLinkParser.parse(nil as URL?, reader: FakeLinkReader()))
    }

    func testQueryDecodingFirstOccurrenceWinsAndBadEscapesAreSkipped() {
        XCTAssertEqual(DeepLinkParser.parseQuery("a=1&a=2&b=x+y&c=%zz&d"), ["a": "1", "b": "x y", "d": ""])
        XCTAssertEqual(DeepLinkParser.parseQuery("a=%C3%A9"), ["a": "é"])
        XCTAssertEqual(DeepLinkParser.parseQuery(""), [:])
        let many = (0..<30).map { "k\($0)=v" }.joined(separator: "&")
        XCTAssertEqual(DeepLinkParser.parseQuery(many).count, 16)
    }

    // --- RedactionTest.kt: draftsAndSharedContentPrintNoContent (DeepLink part) ----------------------------------------------

    func testDeepLinksCarryingContentPrintNoContent() {
        let clear = "Hx7!pQ2-zR9-s3cr3t"
        let send = DeepLink.send(title: clear, url: clear, text: clear, droppedParameters: false)
        let pair = DeepLink.pair(relayUrl: "https://relay.example.org", code: clear)
        let setup = DeepLink.setup(relayUrl: "https://relay.example.org", token: clear)
        for value in [send, pair, setup] {
            for printed in [String(describing: value), String(reflecting: value)] {
                XCTAssertFalse(printed.contains(clear), printed)
                XCTAssertTrue(printed.contains("redacted"), printed)
            }
            var dumped = ""
            dump(value, to: &dumped)
            XCTAssertFalse(dumped.contains(clear))
        }
        XCTAssertEqual(String(describing: send), "Send(droppedParameters=false, <redacted>)")
    }

    func testTheAuthorityOfARelayIsShownByTheRuleOfTheCore() {
        XCTAssertEqual(DeepLinkParser.relayAuthority(of: URL(string: "https://relay.example.org")!), "relay.example.org")
        XCTAssertEqual(DeepLinkParser.relayAuthority(of: URL(string: "https://Relay.Example.org:443")!), "relay.example.org")
        XCTAssertEqual(DeepLinkParser.relayAuthority(of: URL(string: "http://localhost:8080")!), "localhost:8080")
        XCTAssertEqual(DeepLinkParser.relayAuthority(of: URL(string: "http://[::1]:8080")!), "[::1]:8080")
        XCTAssertEqual(DeepLinkParser.relayAuthority(of: URL(string: "https://relay.example.org:8443")!), "relay.example.org:8443")
    }
}
