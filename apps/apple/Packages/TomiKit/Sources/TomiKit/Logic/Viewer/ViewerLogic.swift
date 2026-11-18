import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/viewer/ViewerLogic.kt.
// Pure logic of the internal image viewer: which attachments are pages, what a page index means for the core, what is
// allowed to be on screen, how EXIF orientation and zoom are computed.

/// One page of the viewer: an image attachment and its index in the attachment list of the item (what the core is asked for).
public struct ViewerImage: Hashable, Sendable {
    public let attachmentIndex: Int
    public let attachment: Attachment

    public init(attachmentIndex: Int, attachment: Attachment) {
        self.attachmentIndex = attachmentIndex
        self.attachment = attachment
    }
}

public enum ViewerImages {
    /// The image attachments of `item`, in order. A mixed item (image + PDF) only pages through its images; each page keeps
    /// the index of its attachment in the whole list, because `TomiCore.attachmentUri(itemId, index)` counts every attachment.
    public static func of(_ item: TomiItem) -> [ViewerImage] {
        item.attachments.enumerated().compactMap { index, attachment in
            attachment.isImage ? ViewerImage(attachmentIndex: index, attachment: attachment) : nil
        }
    }

    /// Page showing the attachment `attachmentIndex`; the first page when it is not an image of the item.
    public static func pageFor(_ images: [ViewerImage], attachmentIndex: Int) -> Int {
        max(0, images.firstIndex { $0.attachmentIndex == attachmentIndex } ?? -1)
    }

    /// Attachment index of `page` (coerced into range), or nil when the item has no image.
    public static func attachmentIndexAt(_ images: [ViewerImage], page: Int) -> Int? {
        if images.isEmpty { return nil }
        return images[min(max(page, 0), images.count - 1)].attachmentIndex
    }

    /// "2/5" for the page index 1 of 5.
    public static func positionLabel(page: Int, total: Int) -> String {
        "\(min(max(page, 0), max(0, total - 1)) + 1)/\(total)"
    }
}

/// When the "2/5" position counter of a multi-image viewer is on screen: with the bars, or shortly after an interaction.
public enum ViewerCounter {
    /// Idle time without interaction (page change, swipe, tap) after which the counter fades out while the bars are hidden.
    public static let idleMs: Int64 = 2500

    /// Visible while the bars are shown, or while the user was recently active.
    public static func isVisible(barsVisible: Bool, recentlyActive: Bool) -> Bool { barsVisible || recentlyActive }
}

/// What the viewer is allowed to put on screen. A hidden ("Masqué") item shows nothing until the biometric gate passed.
public enum ViewerContent: Hashable, Sendable {
    /// The item is not known yet.
    case loading
    /// The item is gone (deleted, expired) or has no image.
    case empty
    /// A hidden item whose authentication has not succeeded: no thumbnail, no decrypted file, no name.
    case locked
    case pages([ViewerImage])
}

public enum ViewerGate {
    public static func content(item: TomiItem?, loaded: Bool, unlocked: Bool) -> ViewerContent {
        guard let item else { return loaded ? .empty : .loading }
        if item.secret && !unlocked { return .locked }
        let images = ViewerImages.of(item)
        return images.isEmpty ? .empty : .pages(images)
    }
}

/// Which actions of the viewer menu exist for an item.
public enum ViewerActions {
    /// "Enregistrer dans la galerie" (a permanent clear copy in the shared storage) is not offered for a hidden ("Masqué") item
    /// (product decision). Sharing a hidden image stays available: it goes through the biometric gate.
    public static func canSaveToGallery(_ item: TomiItem) -> Bool { !item.secret }
}

/// EXIF `Orientation` (tag 0x0112) as a rotation (degrees clockwise) followed by an optional horizontal mirror.
public enum ExifOrientation {
    public struct Transform: Hashable, Sendable {
        public let rotationDegrees: Int
        public let flipHorizontal: Bool

        public init(rotationDegrees: Int, flipHorizontal: Bool) {
            self.rotationDegrees = rotationDegrees
            self.flipHorizontal = flipHorizontal
        }

        public var isIdentity: Bool { rotationDegrees == 0 && !flipHorizontal }

        /// True when width and height are exchanged by the transform (quarter turns).
        public var swapsAxes: Bool { rotationDegrees % 180 != 0 }
    }

    private static let identity = Transform(rotationDegrees: 0, flipHorizontal: false)

    /// Same table as ExifInterface / Glide: 2 mirror, 3 rotate 180, 4 flip vertical, 5 transpose, 6 rotate 90, 7 transverse, 8 rotate 270.
    public static func transformFor(_ orientation: Int) -> Transform {
        switch orientation {
        case 2: Transform(rotationDegrees: 0, flipHorizontal: true)
        case 3: Transform(rotationDegrees: 180, flipHorizontal: false)
        case 4: Transform(rotationDegrees: 180, flipHorizontal: true)
        case 5: Transform(rotationDegrees: 90, flipHorizontal: true)
        case 6: Transform(rotationDegrees: 90, flipHorizontal: false)
        case 7: Transform(rotationDegrees: -90, flipHorizontal: true)
        case 8: Transform(rotationDegrees: -90, flipHorizontal: false)
        default: identity
        }
    }

    /// Size of the image as displayed, given the stored size.
    public static func displayedSize(width: Int, height: Int, transform: Transform) -> (width: Int, height: Int) {
        transform.swapsAxes ? (height, width) : (width, height)
    }
}

public enum ImageSampling {
    /// Power-of-two sample size that brings the image under `maxEdge` pixels on its longest side and under `maxPixels`
    /// in total, so that a 50 MP photo does not exhaust memory nor exceed the GPU texture size (iOS: 4096 px / 12 MP).
    public static func sampleSize(width: Int, height: Int, maxEdge: Int, maxPixels: Int64) -> Int {
        if width <= 0 || height <= 0 { return 1 }
        var sample = 1
        while max(width, height) / sample > maxEdge || Int64(width / sample) * Int64(height / sample) > maxPixels {
            sample *= 2
        }
        return sample
    }
}

/// Scale and translation of the image inside the viewer: the image is drawn at `scale`, then moved by (`x`, `y`) points from the centre.
public struct Placement: Hashable, Sendable {
    public var scale: Double
    public var x: Double
    public var y: Double

    public init(scale: Double = 1, x: Double = 0, y: Double = 0) {
        self.scale = scale
        self.x = x
        self.y = y
    }
}

public enum ZoomMath {
    public static let minScale = 1.0
    public static let maxScale = 6.0
    public static let doubleTapScale = 2.5

    /// Zoom level (as a share of the fitted image) above which the image counts as zoomed in.
    private static let zoomed = 1.05

    private static func coerce(_ value: Double, _ low: Double, _ high: Double) -> Double { min(max(value, low), high) }

    /// Size of the image once fitted (letterboxed) into the view, at scale 1.
    public static func fitSize(contentWidth: Double, contentHeight: Double, viewWidth: Double, viewHeight: Double) -> (width: Double, height: Double) {
        if contentWidth <= 0 || contentHeight <= 0 { return (0, 0) }
        let factor = min(viewWidth / contentWidth, viewHeight / contentHeight)
        return (contentWidth * factor, contentHeight * factor)
    }

    /// Keeps the scale within bounds and the image from leaving the view: no empty border once it is larger than the view.
    public static func clamp(_ p: Placement, fitWidth: Double, fitHeight: Double, viewWidth: Double, viewHeight: Double) -> Placement {
        let scale = coerce(p.scale, minScale, maxScale)
        let maxX = max(0, (fitWidth * scale - viewWidth) / 2)
        let maxY = max(0, (fitHeight * scale - viewHeight) / 2)
        return Placement(scale: scale, x: coerce(p.x, -maxX, maxX), y: coerce(p.y, -maxY, maxY))
    }

    /// Pinch / drag step: multiplies the zoom by `zoom` keeping the point under the fingers (`centroidX`, `centroidY`, relative
    /// to the centre of the view) in place, then moves by (`panX`, `panY`).
    public static func transform(
        _ p: Placement, zoom: Double, panX: Double, panY: Double, centroidX: Double, centroidY: Double,
        fitWidth: Double, fitHeight: Double, viewWidth: Double, viewHeight: Double
    ) -> Placement {
        let scale = coerce(p.scale * zoom, minScale, maxScale)
        let ratio = scale / p.scale
        let moved = Placement(
            scale: scale,
            x: (p.x - centroidX) * ratio + centroidX + panX,
            y: (p.y - centroidY) * ratio + centroidY + panY
        )
        return clamp(moved, fitWidth: fitWidth, fitHeight: fitHeight, viewWidth: viewWidth, viewHeight: viewHeight)
    }

    public static func isZoomed(_ p: Placement) -> Bool { p.scale > zoomed }

    /// Double tap: back to the fitted image when zoomed in, otherwise zoom to `doubleTapScale` around the tapped point.
    public static func doubleTap(
        _ p: Placement, tapX: Double, tapY: Double, fitWidth: Double, fitHeight: Double, viewWidth: Double, viewHeight: Double
    ) -> Placement {
        if isZoomed(p) { return Placement() }
        return transform(
            p, zoom: doubleTapScale / p.scale, panX: 0, panY: 0, centroidX: tapX, centroidY: tapY,
            fitWidth: fitWidth, fitHeight: fitHeight, viewWidth: viewWidth, viewHeight: viewHeight
        )
    }
}

/// File name given to an image saved to the gallery: no path characters, an extension that matches the MIME type.
public enum GalleryName {
    private static let maxLength = 100
    private static let maxUTF8Bytes = 240
    private static let fallback = "tomi-image"
    private static let forbidden: Set<Unicode.Scalar> = ["\\", "/", ":", "*", "?", "\"", "<", ">", "|"]

    public static func of(_ name: String, mime: String) -> String {
        let ext = extensionFor(mime)
        var last = Substring(name)
        if let slash = last.lastIndex(of: "/") { last = last[last.index(after: slash)...] }
        if let backslash = last.lastIndex(of: "\\") { last = last[last.index(after: backslash)...] }
        var scalars = String.UnicodeScalarView()
        for scalar in last.unicodeScalars {
            scalars.append(forbidden.contains(scalar) || scalar.value < 0x20 ? "_" : scalar)
        }
        var clean = String(scalars).trimmingCharacters(in: .whitespacesAndNewlines)
        while clean.hasPrefix(".") { clean.removeFirst() }
        while clean.hasSuffix(".") { clean.removeLast() }
        var base = String((clean.isEmpty ? fallback : clean).prefix(maxLength))
        // File systems count bytes (255 at most): keep room for the extension.
        while base.utf8.count > maxUTF8Bytes { base.removeLast() }
        let lower = base.lowercased()
        if knownExtensions[ext, default: []].contains(where: { lower.hasSuffix(".\($0)") }) { return base }
        var stem = base
        if let dot = base.lastIndex(of: ".") { stem = String(base[..<dot]) }
        return "\(stem.isEmpty ? fallback : stem).\(ext)"
    }

    private static let knownExtensions: [String: [String]] = [
        "jpg": ["jpg", "jpeg"],
        "png": ["png"],
        "webp": ["webp"],
        "gif": ["gif"],
        "heic": ["heic", "heif"],
        "avif": ["avif"],
        "bmp": ["bmp"],
    ]

    private static func extensionFor(_ mime: String) -> String {
        switch mime.lowercased() {
        case "image/jpeg", "image/jpg", "image/pjpeg": "jpg"
        case "image/png": "png"
        case "image/webp": "webp"
        case "image/gif": "gif"
        case "image/heic", "image/heif": "heic"
        case "image/avif": "avif"
        case "image/bmp": "bmp"
        default: "jpg"
        }
    }
}
