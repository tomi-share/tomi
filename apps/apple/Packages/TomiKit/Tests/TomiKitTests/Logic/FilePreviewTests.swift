import XCTest
@testable import TomiKit

final class FilePreviewTests: XCTestCase {
    private func file(_ name: String, _ mime: String = "application/octet-stream") -> Attachment {
        Attachment(name: name, mime: mime, size: 1)
    }

    func testEachFileGoesToItsScreen() {
        XCTAssertEqual(FilePreview.kind(file("photo.jpg", "image/jpeg")), .image)
        XCTAssertEqual(FilePreview.kind(file("main.rs", "text/plain")), .code)
        // Plain text and tables: Quick Look (a CSV as a table, any size, any encoding).
        XCTAssertEqual(FilePreview.kind(file("notes.txt", "text/plain")), .document)
        XCTAssertEqual(FilePreview.kind(file("export.csv", "text/csv")), .document)
        XCTAssertEqual(FilePreview.kind(file("film.MP4", "video/mp4")), .media)
        XCTAssertEqual(FilePreview.kind(file("voix.m4a", "audio/mp4")), .media)
        XCTAssertEqual(FilePreview.kind(file("contrat.pdf", "application/pdf")), .document)
        XCTAssertEqual(FilePreview.kind(file("rapport.docx")), .document)
    }

    func testFormatsIOSDoesNotPlayAreLeftToAnotherApp() {
        XCTAssertEqual(FilePreview.kind(file("film.mkv", "video/x-matroska")), .document)
        XCTAssertEqual(FilePreview.kind(file("clip.webm", "video/webm")), .document)
    }

    func testWithoutExtensionTheTypeDecides() {
        XCTAssertEqual(FilePreview.kind(file("enregistrement", "audio/mpeg")), .media)
        XCTAssertEqual(FilePreview.kind(file("document", "application/pdf")), .document)
    }
}
