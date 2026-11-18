import XCTest
@testable import TomiKit

// Port of apps/android/.../share/ExpiryPolicyTest.kt. Owner decision: nothing expires unless the user chose it, secrets included.
final class ExpiryPolicyTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_793_786_400) // 2026-11-04T10:00:00Z
    private let parsed = ParsedShare(title: nil, text: RichText(plain: "Hx7!pQ2-zR9"), url: nil, files: [], looksSecret: true)

    func testTheDefaultIsNeverForEveryItemSecretsIncluded() {
        XCTAssertEqual(ExpiryPolicy.defaultChoice, .never)
        for secret in [false, true] {
            let resolved = ExpiryPolicy.resolve(ExpiryPolicy.defaultChoice, secret: secret, hasAttachment: false, now: now)
            XCTAssertNil(resolved.expiresAt, "secret=\(secret)")
            XCTAssertEqual(resolved.readPolicy, .normal)
            // "Never" is the first chip, so it is what the selector shows first.
            XCTAssertEqual(ExpiryPolicy.options(secret: secret, hasAttachment: false).first, .never)
            XCTAssertEqual(ExpiryPolicy.options(secret: secret, hasAttachment: true).first, .never)
        }
    }

    func testOptionsAreNeverFirstThenAfterReadForHiddenContentThenDurationsWithoutFiveMinutes() {
        XCTAssertEqual(
            ExpiryPolicy.options(secret: false, hasAttachment: false),
            [.never, .hour1, .day1, .day7, .day30]
        )
        XCTAssertEqual(
            ExpiryPolicy.options(secret: true, hasAttachment: false),
            [.never, .afterFirstRead, .hour1, .day1, .day7, .day30]
        )
        // No 5-minute choice exists any more.
        XCTAssertTrue(ExpiryChoice.allCases.allSatisfy { $0.duration != 5 * 60 })
    }

    func testASecretDraftBuiltWithTheDefaultsHasNoExpiryAtAll() {
        let draft = OutgoingDraftFactory.create(id: "id", parsed: parsed, attachments: [], secret: true, expiry: ExpiryPolicy.defaultChoice, now: now)
        XCTAssertTrue(draft.secret)
        XCTAssertNil(draft.expiresAt)
        XCTAssertEqual(draft.readPolicy, .normal)
    }

    func testExplicitDurationsAreHonouredForBothKinds() {
        for secret in [false, true] {
            XCTAssertEqual(
                ExpiryPolicy.resolve(.hour1, secret: secret, hasAttachment: false, now: now).expiresAt,
                now.addingTimeInterval(3600)
            )
            XCTAssertEqual(
                ExpiryPolicy.resolve(.day30, secret: secret, hasAttachment: false, now: now).expiresAt,
                now.addingTimeInterval(30 * 86_400)
            )
        }
        XCTAssertEqual(
            ExpiryPolicy.options(secret: true, hasAttachment: false).filter { $0.duration != nil },
            [.hour1, .day1, .day7, .day30]
        )
    }

    func testAfterFirstReadIsAnOptInForSecretsWithoutAHiddenTimeLimit() {
        XCTAssertTrue(ExpiryPolicy.isAllowed(.afterFirstRead, secret: true, hasAttachment: false))
        XCTAssertFalse(ExpiryPolicy.isAllowed(.afterFirstRead, secret: false, hasAttachment: false))
        let resolved = ExpiryPolicy.resolve(.afterFirstRead, secret: true, hasAttachment: false, now: now)
        XCTAssertEqual(resolved.readPolicy, .afterFirstRead)
        XCTAssertNil(resolved.expiresAt, "no expiry is added on top of the user's choice")
    }

    func testAChoiceNotAllowedForTheContentFallsBackToNoExpiry() {
        let resolved = ExpiryPolicy.resolve(.afterFirstRead, secret: false, hasAttachment: false, now: now)
        XCTAssertNil(resolved.expiresAt)
        XCTAssertEqual(resolved.readPolicy, .normal)
    }

    // --- product decision: no "after first read" when the item has an attachment (the file would be lost after "Afficher") ---

    func testAfterFirstReadIsNotOfferedWhenTheItemHasAnAttachment() {
        for secret in [false, true] {
            XCTAssertFalse(ExpiryPolicy.options(secret: secret, hasAttachment: true).contains(.afterFirstRead), "secret=\(secret)")
            XCTAssertFalse(ExpiryPolicy.isAllowed(.afterFirstRead, secret: secret, hasAttachment: true), "secret=\(secret)")
        }
        // The other choices are untouched by an attachment.
        XCTAssertEqual(
            ExpiryPolicy.options(secret: true, hasAttachment: true),
            [.never, .hour1, .day1, .day7, .day30]
        )
        XCTAssertEqual(
            ExpiryPolicy.options(secret: false, hasAttachment: false),
            ExpiryPolicy.options(secret: false, hasAttachment: true)
        )
    }

    func testAfterFirstReadIsKeptForASecretWithoutAttachment() {
        XCTAssertEqual(
            ExpiryPolicy.options(secret: true, hasAttachment: false),
            [.never, .afterFirstRead, .hour1, .day1, .day7, .day30]
        )
    }

    func testAForcedAfterFirstReadOnAnItemWithAnAttachmentResolvesToNoExpiryAtAll() {
        let resolved = ExpiryPolicy.resolve(.afterFirstRead, secret: true, hasAttachment: true, now: now)
        XCTAssertNil(resolved.expiresAt)
        XCTAssertEqual(resolved.readPolicy, .normal)
        // Same through the draft factory, which is what reaches the core.
        let file = DraftAttachment(path: "/tmp/a.pdf", name: "a.pdf", mime: "application/pdf", size: 10)
        let draft = OutgoingDraftFactory.create(id: "id", parsed: parsed, attachments: [file], secret: true, expiry: .afterFirstRead, now: now)
        XCTAssertNil(draft.expiresAt)
        XCTAssertEqual(draft.readPolicy, .normal)
        // Without a file the choice is honoured.
        let noFile = OutgoingDraftFactory.create(id: "id", parsed: parsed, attachments: [], secret: true, expiry: .afterFirstRead, now: now)
        XCTAssertEqual(noFile.readPolicy, .afterFirstRead)
    }

    func testAttachingAFileDropsAnAfterFirstReadChoiceBackToTheDefault() {
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .afterFirstRead, secret: true, hasAttachment: true), .never)
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .day7, secret: true, hasAttachment: true), .day7)
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .afterFirstRead, secret: true, hasAttachment: false), .afterFirstRead)
    }

    func testTogglingTheSecretSwitchKeepsTheUsersChoiceAndNeverAddsAnExpiry() {
        // Turning a message into a secret does not silently add a time limit.
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .never, secret: true, hasAttachment: false), .never)
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .day7, secret: true, hasAttachment: false), .day7)
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .hour1, secret: false, hasAttachment: false), .hour1)
        // "After first read" does not exist for plain content: back to the default.
        XCTAssertEqual(ExpiryPolicy.onContentChanged(current: .afterFirstRead, secret: false, hasAttachment: false), .never)
    }
}
