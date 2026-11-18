import XCTest
@testable import TomiKit

// Port of apps/android/.../share/SecretDetectorTest.kt.
final class SecretDetectorTests: XCTestCase {
    func testMixedSingleWordIsASecret() {
        XCTAssertTrue(SecretDetector.looksLikeSecret("Tr0ub4dor&3"))
        XCTAssertTrue(SecretDetector.looksLikeSecret("  correcthorse9Battery!  "))
    }

    func testLongHexTokenIsASecret() {
        XCTAssertTrue(SecretDetector.looksLikeSecret("3f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c"))
    }

    func testKnownTokenPrefixesAndPrivateKeysAreSecrets() {
        XCTAssertTrue(SecretDetector.looksLikeSecret("ghp_abcdefghijklmnopqrstuvwxyz"))
        XCTAssertTrue(SecretDetector.looksLikeSecret("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"))
    }

    func testOrdinaryTextIsNotASecret() {
        XCTAssertFalse(SecretDetector.looksLikeSecret("Rendez-vous demain à 14 h"))
        XCTAssertFalse(SecretDetector.looksLikeSecret("bonjour"))
        XCTAssertFalse(SecretDetector.looksLikeSecret("crayon-lune"))
        XCTAssertFalse(SecretDetector.looksLikeSecret(""))
        XCTAssertFalse(SecretDetector.looksLikeSecret(nil))
    }

    func testURLsAndEmailsAreNotSecrets() {
        XCTAssertFalse(SecretDetector.looksLikeSecret("https://example.org/A1b2C3d4E5"))
        XCTAssertFalse(SecretDetector.looksLikeSecret("jean.dupont42@example.org"))
    }

    func testTooShortOrTooLongIsNotASecret() {
        XCTAssertFalse(SecretDetector.looksLikeSecret("Ab1!"))
        XCTAssertFalse(SecretDetector.looksLikeSecret(String(repeating: "A1!", count: 300)))
    }

    func testKnownPasswordManagersAreRecognised() {
        XCTAssertTrue(SecretDetector.isPasswordManager("com.x8bit.bitwarden"))
        XCTAssertFalse(SecretDetector.isPasswordManager("com.android.chrome"))
        XCTAssertFalse(SecretDetector.isPasswordManager(nil))
    }
}
