import CoreGraphics
import Foundation
import ImageIO
import QuickLookThumbnailing
import TomiKit
import UniformTypeIdentifiers

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/send/DraftThumbnails.kt, extended to the other files iOS can
// draw (PDF first page, a frame of a video, Office and iWork documents) through Quick Look.

/// A thumbnail ready to travel inside the item.
public struct DraftThumbnail: Sendable, Equatable {
    public let data: Data
    public let mime: String
}

/// The thumbnail an outgoing file carries (spec/format-v1.md: "vignette"), shown in the feed and the notification of
/// every device without downloading the file. Made from the staged copy, upright, framed by `ThumbnailSize.plan` (a tall
/// image is cut to its centre, as the cards crop it anyway), at most `ThumbnailSize.maxBytes`: JPEG, or PNG when the
/// image has transparency and the PNG stays small enough (otherwise JPEG on a white ground). No metadata is copied (the
/// place a photo was taken stays on this device). Anything that cannot be drawn or made small enough has none.
public enum DraftThumbnails {
    /// How long a send waits for Quick Look on one file before it goes without that thumbnail.
    static let quickLookTimeout: Duration = .seconds(3)

    public static func make(_ url: URL, name: String, mime: String) async -> DraftThumbnail? {
        if mime.hasPrefix("image/") {
            return await Task.detached(priority: .userInitiated) {
                decodeImage(url).flatMap { frameAndEncode($0.image, transparent: $0.transparent) }
            }.value
        }
        guard drawnByQuickLook(name: name, mime: mime), let image = await quickLookImage(url) else { return nil }
        return await Task.detached(priority: .userInitiated) { frameAndEncode(image, transparent: false) }.value
    }

    /// The files whose Quick Look picture says something: a page of a PDF or a document, a frame of a video. Not text,
    /// code or audio (an image of their text, or a cover, would take the budget of the item from its real images).
    static func drawnByQuickLook(name: String, mime: String) -> Bool {
        let ext = (name as NSString).pathExtension
        guard let type = (ext.isEmpty ? nil : UTType(filenameExtension: ext)) ?? UTType(mimeType: mime) else { return false }
        if [UTType.pdf, .movie, .presentation, .spreadsheet].contains(where: type.conforms(to:)) { return true }
        return documentTypes.contains(type.identifier)
    }

    /// Word and Pages documents (no public parent type covers them).
    private static let documentTypes: Set<String> = [
        "org.openxmlformats.wordprocessingml.document", "com.microsoft.word.doc", "com.apple.iwork.pages.sffpages",
        "com.apple.iwork.pages.pages", "org.oasis-open.opendocument.text", "public.rtf",
    ]

    /// The image, upright, at most `ThumbnailSize.decodeMaxEdge` on its longest side, and whether the file has
    /// transparency (a decoded image often has an alpha channel even when the file has none).
    static func decodeImage(_ url: URL) -> (image: CGImage, transparent: Bool)? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary) else {
            return nil
        }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let transparent = (properties?[kCGImagePropertyHasAlpha] as? Bool) ?? false
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: ThumbnailSize.decodeMaxEdge,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return (image, transparent)
    }

    /// A first page, a frame or an icon-free preview of another file; nil when Quick Look has no thumbnail for it.
    static func quickLookImage(_ url: URL) async -> CGImage? {
        let request = QLThumbnailGenerator.Request(
            fileAt: url,
            size: CGSize(width: ThumbnailSize.maxWidth, height: ThumbnailSize.maxHeight),
            scale: 1,
            representationTypes: .thumbnail
        )
        // Cancelled after `quickLookTimeout`: the handler then comes back with an error, once.
        let timeout = Task {
            try? await Task.sleep(for: quickLookTimeout)
            if !Task.isCancelled { QLThumbnailGenerator.shared.cancel(request) }
        }
        defer { timeout.cancel() }
        return await withCheckedContinuation { continuation in
            QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { representation, _ in
                continuation.resume(returning: representation?.cgImage)
            }
        }
    }

    /// A Quick Look thumbnail is drawn opaque (a page, a frame): only an image file may keep its transparency.
    static func frameAndEncode(_ image: CGImage, transparent: Bool) -> DraftThumbnail? {
        guard let framed = frame(image, transparent: transparent) else { return nil }
        return encode(framed, transparent: transparent)
    }

    /// The crop and the scale of `ThumbnailSize.plan`, always redrawn in sRGB (a CMYK or wide-gamut file would reach
    /// other devices as such), on white when the image has no transparency.
    static func frame(_ image: CGImage, transparent: Bool) -> CGImage? {
        let plan = ThumbnailSize.plan(width: image.width, height: image.height)
        guard let cropped = image.cropping(to: CGRect(x: 0, y: plan.top, width: image.width, height: plan.cropHeight)) else {
            return nil
        }
        let size = plan.size(width: image.width)
        let alpha = transparent ? CGImageAlphaInfo.premultipliedLast : .noneSkipLast
        guard let context = CGContext(
            data: nil, width: size.width, height: size.height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: alpha.rawValue
        ) else { return nil }
        if !transparent {
            context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
            context.fill(CGRect(x: 0, y: 0, width: size.width, height: size.height))
        }
        context.interpolationQuality = .high
        context.draw(cropped, in: CGRect(x: 0, y: 0, width: size.width, height: size.height))
        return context.makeImage()
    }

    static func encode(_ image: CGImage, transparent: Bool) -> DraftThumbnail? {
        if transparent, let png = data(image, type: .png, quality: nil), png.count <= ThumbnailSize.maxBytes {
            return DraftThumbnail(data: png, mime: "image/png")
        }
        guard let opaque = hasAlpha(image) ? flatten(image) : image else { return nil }
        for quality in ThumbnailSize.jpegQualities {
            if let jpeg = data(opaque, type: .jpeg, quality: quality), jpeg.count <= ThumbnailSize.maxBytes {
                return DraftThumbnail(data: jpeg, mime: "image/jpeg")
            }
        }
        return nil
    }

    private static func hasAlpha(_ image: CGImage) -> Bool {
        switch image.alphaInfo {
        case .none, .noneSkipFirst, .noneSkipLast: false
        default: true
        }
    }

    /// A transparent image drawn on white (JPEG has no alpha: it would turn black).
    private static func flatten(_ image: CGImage) -> CGImage? {
        guard let context = CGContext(
            data: nil, width: image.width, height: image.height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { return nil }
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return context.makeImage()
    }

    private static func data(_ image: CGImage, type: UTType, quality: Double?) -> Data? {
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, type.identifier as CFString, 1, nil) else { return nil }
        let properties = quality.map { [kCGImageDestinationLossyCompressionQuality: $0] as CFDictionary }
        CGImageDestinationAddImage(destination, image, properties)
        return CGImageDestinationFinalize(destination) ? data as Data : nil
    }
}
