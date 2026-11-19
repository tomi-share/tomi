import Foundation
import XCTest
@testable import Tomi

/// The content rule list of the HTML web view: valid JSON that names only blocking rules for network schemes (WebKit
/// refuses to compile anything else; the compilation itself needs WebKit, not covered here).
@MainActor
final class HTMLContentRulesTests: XCTestCase {
    func testTheRulesBlockEveryNetworkSchemeAndNothingElse() throws {
        let data = try XCTUnwrap(HTMLContentRules.rules.data(using: .utf8))
        let list = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: [String: String]]])
        XCTAssertEqual(list.compactMap { $0["action"]?["type"] }, ["block", "block", "block"])
        XCTAssertEqual(list.compactMap { $0["trigger"]?["url-filter"] }, ["^https?://", "^wss?://", "^ftp://"])
    }
}
