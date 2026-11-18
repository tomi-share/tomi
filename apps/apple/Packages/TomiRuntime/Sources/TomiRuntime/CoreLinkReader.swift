import Foundation
import TomiCore
import TomiKit

/// `TomiKit.LinkReading` over the Rust core: the single reading of pairing links, creation links and installation tokens
/// that the three platforms share (`parse_pair_link`, `parse_setup_link`, `is_install_token_shape` of
/// `crates/core/src/ffi.rs`). Pure functions of the core: no network, nothing stored, no state.
///
/// The methods are named differently from the free functions of the core (`readPairLink` / `parsePairLink`...) so that a
/// call inside this type never resolves to the method itself.
public struct CoreLinkReader: LinkReading {
    public init() {}

    public func readPairLink(_ text: String) -> ParsedPairLink? {
        parsePairLink(text: text).map { ParsedPairLink(relayUrl: $0.relayUrl, code: $0.code) }
    }

    public func readSetupLink(_ text: String) -> ParsedSetupLink? {
        parseSetupLink(text: text).map { ParsedSetupLink(relayUrl: $0.relayUrl, token: $0.token) }
    }

    public func hasTokenShape(_ text: String) -> Bool {
        isInstallTokenShape(text: text)
    }
}
