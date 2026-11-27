import CryptoKit
import Foundation

/// What can go wrong sealing or opening a share of the inbox.
public enum ShareSealError: Error, Equatable {
    /// No key to seal to (the app has not published one yet) or to open with (the app could not read its own).
    case noKey
    /// A key of the wrong length or shape.
    case badKey
    /// Not something this code sealed: a wrong header, a block too short.
    case malformed
    /// Altered, cut short, moved to another share, or sealed to another key.
    case authentication
}

/// Seals the shares that the share extension puts in the inbox of the app group to the key of the app, so that what
/// waits there (up to `ShareInbox.maxAge`) is never plaintext at rest, secrets and files included.
///
/// The extension never opens the core and holds no secret: it only knows the **public** key the app published in the
/// app group. For each share it makes an ephemeral X25519 key, derives with the app's public key a master key (HKDF-SHA256,
/// bound to both public keys and to the id of the share), and from it one key per part (`manifest`, `file/0`, `file/1`...).
/// Only the app, which keeps the private key in its Keychain, can open them. This protects the **confidentiality** of
/// what waits in the inbox; it does not authenticate who put it there (anything able to write in the group and to read its
/// public key can deposit a share, as before).
///
/// A manifest is `header || ChaChaPoly(combined)`, where the header is `"TSI1" || ephemeral public key (32)`. A file is a
/// stream of blocks of 64 KiB (the last one shorter), each `ciphertext || tag`, with the nonce `0x000000 || counter (8, big
/// endian) || last (1 if this is the final block)`: cutting a file at a block boundary, reordering or repeating blocks
/// fails the authentication, and a file is never read whole in memory (an extension has little of it).
public enum ShareSeal {
    public static let magic = Data("TSI1".utf8)
    public static let publicKeyLength = 32
    /// `magic` then the ephemeral public key.
    public static let headerLength = 4 + 32

    static let chunkSize = 64 * 1024
    static let tagLength = 16
    private static let info = Data("tomi/share-inbox/v1".utf8)

    /// The long-lived X25519 key of the app: the private half stays in its Keychain, the public half goes to the group.
    public struct KeyPair: Sendable {
        public let privateKey: Data
        public let publicKey: Data
    }

    public static func makeKeyPair() -> KeyPair {
        let key = Curve25519.KeyAgreement.PrivateKey()
        return KeyPair(privateKey: key.rawRepresentation, publicKey: key.publicKey.rawRepresentation)
    }

    /// The public half of a private key, or `badKey`.
    public static func publicKey(of privateKey: Data) throws -> Data {
        guard let key = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKey) else { throw ShareSealError.badKey }
        return key.publicKey.rawRepresentation
    }

    // MARK: Extension side

    /// Seals the parts of one share. Made once per share, from the public key of the app.
    public struct Sealer {
        /// To write once, in front of the sealed manifest.
        public let header: Data
        private let master: SymmetricKey
        private let aad: Data

        public init(recipient: Data, shareId: String) throws {
            guard recipient.count == ShareSeal.publicKeyLength,
                  let recipientKey = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: recipient)
            else { throw ShareSealError.badKey }
            let ephemeral = Curve25519.KeyAgreement.PrivateKey()
            let ephemeralPublic = ephemeral.publicKey.rawRepresentation
            let secret = try ephemeral.sharedSecretFromKeyAgreement(with: recipientKey)
            header = ShareSeal.magic + ephemeralPublic
            master = ShareSeal.master(secret, ephemeralPublic: ephemeralPublic, recipientPublic: recipient, shareId: shareId)
            aad = Data(shareId.utf8)
        }

        /// A small part (the manifest) in one piece: `ChaChaPoly` combined (nonce, ciphertext, tag).
        public func seal(_ plaintext: Data, purpose: String) throws -> Data {
            let box = try ChaChaPoly.seal(plaintext, using: ShareSeal.subkey(master, purpose: purpose), authenticating: aad)
            return box.combined
        }

        /// A file, block by block: `target` is created (or emptied) and filled, `source` is left as it is.
        public func seal(file source: URL, to target: URL, purpose: String) throws {
            let key = ShareSeal.subkey(master, purpose: purpose)
            let input = try FileHandle(forReadingFrom: source)
            defer { try? input.close() }
            guard FileManager.default.createFile(atPath: target.path, contents: nil) else { throw CocoaError(.fileWriteUnknown) }
            let output = try FileHandle(forWritingTo: target)
            defer { try? output.close() }
            var counter: UInt64 = 0
            var current = try ShareSeal.readFull(input, count: ShareSeal.chunkSize)
            while true {
                // One block ahead, to know which one is the last (an empty file is one empty last block).
                let next = try ShareSeal.readFull(input, count: ShareSeal.chunkSize)
                let last = next.isEmpty
                let box = try ChaChaPoly.seal(
                    current, using: key, nonce: ShareSeal.nonce(counter, last: last), authenticating: aad
                )
                try output.write(contentsOf: box.ciphertext)
                try output.write(contentsOf: box.tag)
                if last { break }
                current = next
                counter += 1
            }
        }
    }

    // MARK: App side

    /// Opens the parts of one share. Made from its header, the private key of the app and the id of the share (a part
    /// moved into another share does not open).
    public struct Opener {
        private let master: SymmetricKey
        private let aad: Data

        public init(header: Data, privateKey: Data, shareId: String) throws {
            guard header.count == ShareSeal.headerLength, header.prefix(ShareSeal.magic.count) == ShareSeal.magic else {
                throw ShareSealError.malformed
            }
            guard let key = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKey) else { throw ShareSealError.badKey }
            let ephemeralPublic = Data(header.suffix(ShareSeal.publicKeyLength))
            guard let peer = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: ephemeralPublic) else {
                throw ShareSealError.malformed
            }
            let secret = try key.sharedSecretFromKeyAgreement(with: peer)
            master = ShareSeal.master(
                secret, ephemeralPublic: ephemeralPublic, recipientPublic: key.publicKey.rawRepresentation, shareId: shareId
            )
            aad = Data(shareId.utf8)
        }

        public func open(_ sealed: Data, purpose: String) throws -> Data {
            do {
                let box = try ChaChaPoly.SealedBox(combined: sealed)
                return try ChaChaPoly.open(box, using: ShareSeal.subkey(master, purpose: purpose), authenticating: aad)
            } catch {
                throw ShareSealError.authentication
            }
        }

        /// A sealed file, block by block, into `target` (created or emptied).
        public func open(file source: URL, to target: URL, purpose: String) throws {
            let key = ShareSeal.subkey(master, purpose: purpose)
            let input = try FileHandle(forReadingFrom: source)
            defer { try? input.close() }
            guard FileManager.default.createFile(atPath: target.path, contents: nil) else { throw CocoaError(.fileWriteUnknown) }
            let output = try FileHandle(forWritingTo: target)
            defer { try? output.close() }
            let block = ShareSeal.chunkSize + ShareSeal.tagLength
            var counter: UInt64 = 0
            var current = try ShareSeal.readFull(input, count: block)
            while true {
                guard current.count >= ShareSeal.tagLength else { throw ShareSealError.malformed }
                let next = try ShareSeal.readFull(input, count: block)
                let last = next.isEmpty
                let plain: Data
                do {
                    let box = try ChaChaPoly.SealedBox(
                        nonce: ShareSeal.nonce(counter, last: last),
                        ciphertext: current.prefix(current.count - ShareSeal.tagLength),
                        tag: current.suffix(ShareSeal.tagLength)
                    )
                    plain = try ChaChaPoly.open(box, using: key, authenticating: aad)
                } catch {
                    throw ShareSealError.authentication
                }
                try output.write(contentsOf: plain)
                if last { break }
                current = next
                counter += 1
            }
        }
    }

    // MARK: Shared

    private static func master(_ secret: SharedSecret, ephemeralPublic: Data, recipientPublic: Data, shareId: String) -> SymmetricKey {
        secret.hkdfDerivedSymmetricKey(
            using: SHA256.self, salt: ephemeralPublic + recipientPublic + Data(shareId.utf8), sharedInfo: info, outputByteCount: 32
        )
    }

    /// One key per part: a nonce never meets another part's key.
    private static func subkey(_ master: SymmetricKey, purpose: String) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(inputKeyMaterial: master, salt: Data(), info: Data(purpose.utf8), outputByteCount: 32)
    }

    static func nonce(_ counter: UInt64, last: Bool) throws -> ChaChaPoly.Nonce {
        var bytes = [UInt8](repeating: 0, count: 12)
        var value = counter
        for index in stride(from: 10, through: 3, by: -1) {
            bytes[index] = UInt8(truncatingIfNeeded: value)
            value >>= 8
        }
        bytes[11] = last ? 1 : 0
        return try ChaChaPoly.Nonce(data: Data(bytes))
    }

    /// Up to `count` bytes, fewer only at the end of the file (a read may come back short).
    static func readFull(_ handle: FileHandle, count: Int) throws -> Data {
        var data = Data()
        while data.count < count {
            guard let part = try handle.read(upToCount: count - data.count), !part.isEmpty else { break }
            data.append(part)
        }
        return data
    }
}
