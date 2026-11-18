import Foundation

// Port of `ThumbnailSize` and of the thumbnail budget of `OutgoingDraft.toFfi` (apps/android/app/src/main/java/eu/shaffe/
// tomi/send/DraftThumbnails.kt, core/RealCoreMapping.kt).

/// Framing of a thumbnail: the rows `top ..< top + cropHeight` of the image (all its width), scaled by `scale` (never
/// above 1). A tall image is cut to its centre at `maxTallRatio` (height / width); the result fits in `maxWidth` x
/// `maxHeight`. Large enough for a card across the whole width of a phone (a smaller one was visibly blurred there).
public enum ThumbnailSize {
    public static let maxWidth = 600
    public static let maxHeight = 800
    public static let maxTallRatio = 4.0 / 3.0
    /// Each thumbnail travels inside the item (in base64), which the relay caps at about 1 MB.
    public static let maxBytes = 120 * 1024
    /// JPEG qualities tried in turn until the thumbnail fits in `maxBytes`.
    public static let jpegQualities: [Double] = [0.80, 0.65, 0.50]
    /// Decoding size: enough for a `maxWidth` px wide crop of a tall screenshot, without decoding a 50 Mpx photo.
    public static let decodeMaxEdge = 2400

    public struct Plan: Equatable, Sendable {
        public let top: Int
        public let cropHeight: Int
        public let scale: Double

        public func isIdentity(width: Int, height: Int) -> Bool { top == 0 && cropHeight == height && scale == 1 }

        /// Size of the result, each side at least 1 px.
        public func size(width: Int) -> (width: Int, height: Int) {
            (max(1, Int((Double(width) * scale).rounded())), max(1, Int((Double(cropHeight) * scale).rounded())))
        }
    }

    public static func plan(width: Int, height: Int) -> Plan {
        let cropHeight = min(height, max(1, Int((Double(width) * maxTallRatio).rounded(.up))))
        let top = (height - cropHeight) / 2
        let scale = min(1.0, Double(maxWidth) / Double(width), Double(maxHeight) / Double(cropHeight))
        return Plan(top: top, cropHeight: cropHeight, scale: scale)
    }
}

/// The thumbnails of one item. A hidden item carries none: its images are only shown after the biometric gate, never
/// as a preview. Together they stay under `budgetBytes`: the next ones are left out.
public enum ThumbnailBudget {
    /// All the thumbnails of one item, in bytes (base64 adds a third): well under the ~1 MB the relay takes for an item.
    public static let budgetBytes = 384 * 1024

    /// `thumbnails[i]` is the thumbnail of attachment `i` (nil: none); the result keeps those that may travel.
    public static func apply(_ thumbnails: [Data?], secret: Bool) -> [Data?] {
        var budget = budgetBytes
        return thumbnails.map { thumbnail in
            guard !secret, budget > 0, let thumbnail, thumbnail.count <= budget else { return nil }
            budget -= thumbnail.count
            return thumbnail
        }
    }
}
