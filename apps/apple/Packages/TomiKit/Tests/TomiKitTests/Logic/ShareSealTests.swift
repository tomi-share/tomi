import Foundation
import XCTest
@testable import TomiKit

/// The seal of the share inbox: what the extension puts in the app group opens only for the app, whole, in place.
final class ShareSealTests: XCTestCase {
    private var dir: URL!
    private let keys = ShareSeal.makeKeyPair()
    private let block = 64 * 1024

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("seal-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func makeSealer(_ id: String = "share-1") throws -> ShareSeal.Sealer {
        try ShareSeal.Sealer(recipient: keys.publicKey, shareId: id)
    }

    private func makeOpener(for sealer: ShareSeal.Sealer, id: String = "share-1", key: Data? = nil) throws -> ShareSeal.Opener {
        try ShareSeal.Opener(header: sealer.header, privateKey: key ?? keys.privateKey, shareId: id)
    }

    private func bytes(_ count: Int) -> Data {
        Data((0..<count).map { UInt8(truncatingIfNeeded: $0 &* 31 &+ 7) })
    }

    private func write(_ name: String, _ data: Data) throws -> URL {
        let url = dir.appendingPathComponent(name)
        try data.write(to: url)
        return url
    }

    private func assertRefused<T>(
        _ expression: @autoclosure () throws -> T, _ error: ShareSealError, file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertThrowsError(try expression(), file: file, line: line) {
            XCTAssertEqual($0 as? ShareSealError, error, file: file, line: line)
        }
    }

    func testASmallPartRoundTrips() throws {
        let sealer = try makeSealer()
        let plain = Data("{\"title\":\"mot de passe\"}".utf8)
        let sealed = try sealer.seal(plain, purpose: "manifest")
        XCTAssertNil(sealed.range(of: Data("mot de passe".utf8)))
        XCTAssertEqual(try makeOpener(for: sealer).open(sealed, purpose: "manifest"), plain)
    }

    func testFilesOfEverySizeRoundTripAndTakeOnlyATagPerBlock() throws {
        let sealer = try makeSealer()
        let opener = try makeOpener(for: sealer)
        for size in [0, 1, 15, 16, block - 1, block, block + 1, 2 * block, 3 * block + 5] {
            let plain = bytes(size)
            let source = try write("plain-\(size)", plain)
            let sealed = dir.appendingPathComponent("sealed-\(size)")
            let back = dir.appendingPathComponent("back-\(size)")
            try sealer.seal(file: source, to: sealed, purpose: "file/0")
            try opener.open(file: sealed, to: back, purpose: "file/0")
            XCTAssertEqual(try Data(contentsOf: back), plain, "size \(size)")
            let blocks = max(1, (size + block - 1) / block)
            XCTAssertEqual(try Data(contentsOf: sealed).count, size + 16 * blocks, "size \(size)")
            XCTAssertEqual(try Data(contentsOf: source), plain, "the source is left as it was")
        }
    }

    func testNothingOfThePlaintextIsInASealedFile() throws {
        let sealer = try makeSealer()
        let phrase = Data("phrase secrète de test".utf8)
        let source = try write("p", phrase + bytes(10_000))
        let sealed = dir.appendingPathComponent("s")
        try sealer.seal(file: source, to: sealed, purpose: "file/0")
        XCTAssertNil(try Data(contentsOf: sealed).range(of: phrase))
    }

    func testAFileCutAtABlockBoundaryIsRefused() throws {
        let sealer = try makeSealer()
        let opener = try makeOpener(for: sealer)
        let source = try write("p", bytes(3 * block))
        let sealed = dir.appendingPathComponent("s")
        let target = dir.appendingPathComponent("b")
        try sealer.seal(file: source, to: sealed, purpose: "file/0")
        let whole = try Data(contentsOf: sealed)
        // Whole blocks only: the first two of three. The end of the file is no longer what was sealed as its end.
        try Data(whole.prefix(2 * (block + 16))).write(to: sealed)
        assertRefused(try opener.open(file: sealed, to: target, purpose: "file/0"), .authentication)
        // A cut inside a tag too.
        try Data(whole.prefix(whole.count - 1)).write(to: sealed)
        assertRefused(try opener.open(file: sealed, to: target, purpose: "file/0"), .authentication)
    }

    func testReorderedAndAlteredBlocksAreRefused() throws {
        let sealer = try makeSealer()
        let opener = try makeOpener(for: sealer)
        let source = try write("p", bytes(3 * block))
        let sealed = dir.appendingPathComponent("s")
        let target = dir.appendingPathComponent("b")
        try sealer.seal(file: source, to: sealed, purpose: "file/0")
        let whole = try Data(contentsOf: sealed)
        let size = block + 16
        var swapped = Data()
        swapped.append(whole.subdata(in: size..<(2 * size)))
        swapped.append(whole.subdata(in: 0..<size))
        swapped.append(whole.subdata(in: (2 * size)..<whole.count))
        try swapped.write(to: sealed)
        assertRefused(try opener.open(file: sealed, to: target, purpose: "file/0"), .authentication)
        var altered = whole
        altered[100] ^= 1
        try altered.write(to: sealed)
        assertRefused(try opener.open(file: sealed, to: target, purpose: "file/0"), .authentication)
    }

    func testAnotherKeyAnotherShareOrAnotherPartDoesNotOpen() throws {
        let sealer = try makeSealer()
        let sealed = try sealer.seal(Data("x".utf8), purpose: "manifest")
        let stranger = ShareSeal.makeKeyPair().privateKey
        assertRefused(try makeOpener(for: sealer, key: stranger).open(sealed, purpose: "manifest"), .authentication)
        assertRefused(try makeOpener(for: sealer, id: "share-2").open(sealed, purpose: "manifest"), .authentication)
        assertRefused(try makeOpener(for: sealer).open(sealed, purpose: "file/0"), .authentication)
        XCTAssertEqual(try makeOpener(for: sealer).open(sealed, purpose: "manifest"), Data("x".utf8))
    }

    func testTwoSealsOfTheSameShareNeverShareAKey() throws {
        let first = try makeSealer()
        let second = try makeSealer()
        XCTAssertNotEqual(first.header, second.header, "a new ephemeral key every time")
        XCTAssertEqual(first.header.count, ShareSeal.headerLength)
        XCTAssertEqual(Data(first.header.prefix(4)), ShareSeal.magic)
    }

    func testAHeaderThatIsNotOursOrAKeyThatIsNotAKeyIsRefused() throws {
        assertRefused(try ShareSeal.Opener(header: Data(count: ShareSeal.headerLength), privateKey: keys.privateKey, shareId: "s"), .malformed)
        assertRefused(try ShareSeal.Opener(header: Data("TSI1".utf8), privateKey: keys.privateKey, shareId: "s"), .malformed)
        assertRefused(try ShareSeal.Sealer(recipient: Data([1, 2, 3]), shareId: "s"), .badKey)
        assertRefused(try ShareSeal.publicKey(of: Data([1])), .badKey)
        XCTAssertEqual(try ShareSeal.publicKey(of: keys.privateKey), keys.publicKey)
    }

    func testTheNonceNeverRepeatsAcrossBlocksNorFlagsTheLastOneEarly() throws {
        var seen = Set<Data>()
        for counter in 0..<4 {
            for last in [false, true] {
                let nonce = try ShareSeal.nonce(UInt64(counter), last: last)
                XCTAssertTrue(seen.insert(Data(nonce)).inserted)
            }
        }
        XCTAssertEqual(Data(try ShareSeal.nonce(1, last: false)), Data([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0]))
        XCTAssertEqual(Data(try ShareSeal.nonce(0, last: true)), Data([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]))
    }
}
