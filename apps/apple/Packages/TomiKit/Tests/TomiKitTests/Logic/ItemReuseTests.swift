import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/core/ItemReuseTest.kt. Kotlin checks the identity of the
// instances; a Swift value has none, so these tests check the storage: the text and the thumbnail of a reused item
// are the buffers of the previous read.
final class ItemReuseTests: XCTestCase {
    private let office = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_794_391_200)

    /// New storage for everything at each call, as a new read of the core gives.
    private func note(_ id: String, text: String? = nil, read: Bool = false, thumbnail: UInt8? = nil) -> TomiItem {
        // Long enough to live on the heap (a short string is stored inline and has no buffer to share).
        let body = String(decoding: Array((text ?? "contenu de \(id), assez long pour ne pas tenir en ligne").utf8), as: UTF8.self)
        return TomiItem.readable(
            id: id, sourceDevice: office, createdAt: now,
            title: String(decoding: Array("un titre assez long pour ne pas tenir en ligne".utf8), as: UTF8.self),
            text: RichText(plain: body, markdown: String(decoding: Array(body.utf8), as: UTF8.self)),
            attachments: thumbnail.map { [Attachment(name: "photo.jpg", mime: "image/jpeg", size: 3, thumbnail: Data([$0] + [UInt8](repeating: 2, count: 64)))] } ?? [],
            read: read
        )
    }

    private func address(_ text: String?) -> UnsafeRawPointer? {
        text?.utf8.withContiguousStorageIfAvailable { UnsafeRawPointer($0.baseAddress) } ?? nil
    }

    private func address(_ data: Data?) -> UnsafeRawPointer? {
        data?.withUnsafeBytes { $0.baseAddress }
    }

    private func assertSameStorage(_ a: TomiItem, _ b: TomiItem, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(a, b, file: file, line: line)
        XCTAssertNotNil(address(a.text?.plain), file: file, line: line)
        XCTAssertEqual(address(a.text?.plain), address(b.text?.plain), file: file, line: line)
        XCTAssertEqual(address(a.text?.markdown), address(b.text?.markdown), file: file, line: line)
        XCTAssertEqual(address(a.attachments.first?.thumbnail), address(b.attachments.first?.thumbnail), file: file, line: line)
    }

    func testAnUnchangedItemKeepsItsStorage() {
        let previous = [note("a"), note("b", thumbnail: 1)]
        let fresh = [note("a"), note("b", thumbnail: 1)]
        XCTAssertNotEqual(address(previous[0].text?.plain), address(fresh[0].text?.plain))
        let result = ItemReuse.reuseUnchanged(previous: previous, fresh: fresh)
        XCTAssertEqual(fresh, result)
        assertSameStorage(previous[0], result[0])
        assertSameStorage(previous[1], result[1])
    }

    func testAChangeOfMetadataKeepsTheStorageOfTheContent() {
        let previous = [note("a", thumbnail: 1)]
        let fresh = [note("a", read: true, thumbnail: 1)]
        let result = ItemReuse.reuseUnchanged(previous: previous, fresh: fresh)
        XCTAssertEqual(fresh, result)
        XCTAssertNotEqual(previous[0], result[0])
        XCTAssertEqual(address(previous[0].text?.plain), address(result[0].text?.plain))
        // The thumbnail is the same buffer: what was decoded from it stays valid.
        XCTAssertEqual(address(previous[0].attachments[0].thumbnail), address(result[0].attachments[0].thumbnail))
    }

    func testAChangedContentIsTheFreshOne() {
        let previous = [note("a", text: "avant, assez long pour ne pas tenir en ligne"), note("b", thumbnail: 1)]
        let fresh = [note("a", text: "après, assez long pour ne pas tenir en ligne"), note("b", thumbnail: 9)]
        let result = ItemReuse.reuseUnchanged(previous: previous, fresh: fresh)
        assertSameStorage(fresh[0], result[0])
        assertSameStorage(fresh[1], result[1])
    }

    func testTheResultFollowsTheFreshReadOnly() {
        let previous = [note("gone"), note("kept")]
        let fresh = [note("new"), note("kept")]
        let result = ItemReuse.reuseUnchanged(previous: previous, fresh: fresh)
        XCTAssertEqual(["new", "kept"], result.map(\.id))
        assertSameStorage(fresh[0], result[0])
        assertSameStorage(previous[1], result[1])
    }

    func testUnreadableItemsAndEmptyReadsAreHandled() {
        func unreadable() -> TomiItem {
            TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: office, createdAt: now)
        }
        let previous = [unreadable()]
        XCTAssertEqual(previous, ItemReuse.reuseUnchanged(previous: previous, fresh: [unreadable()]))
        XCTAssertEqual([], ItemReuse.reuseUnchanged(previous: previous, fresh: []))
        let fresh = [note("a")]
        assertSameStorage(fresh[0], ItemReuse.reuseUnchanged(previous: [], fresh: fresh)[0])
    }
}
