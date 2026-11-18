import Foundation

/// How Tomi shows a file of an item (iOS: Android and Windows hand anything but images and code to another app).
public enum FilePreviewKind: Sendable, Equatable {
    /// The image viewer.
    case image
    /// The code screen (mono, search).
    case code
    /// Audio or video iOS plays itself: the system player.
    case media
    /// Anything else: Quick Look when it can draw it (PDF, Office, iWork, plain text, CSV as a table...), else a page that
    /// offers to open it in another app.
    case document
}

public enum FilePreview {
    /// Formats AVFoundation plays on iOS. MKV, WebM, AVI, WMV, FLV and Ogg are not among them: they get the page that
    /// offers another app (VLC, Infuse...).
    private static let mediaExtensions: Set<String> = [
        "mp4", "m4v", "mov", "3gp", "3g2", "mp3", "m4a", "m4b", "aac", "wav", "aif", "aiff", "aifc", "caf", "flac", "amr",
    ]

    private static let mediaTypes: Set<String> = [
        "video/mp4", "video/quicktime", "video/x-m4v", "video/3gpp", "audio/mpeg", "audio/mp3", "audio/mp4", "audio/x-m4a",
        "audio/aac", "audio/wav", "audio/x-wav", "audio/aiff", "audio/x-aiff", "audio/flac", "audio/x-flac", "audio/amr",
    ]

    public static func kind(_ attachment: Attachment) -> FilePreviewKind {
        if attachment.isImage { return .image }
        if CodeFiles.isCodeFile(attachment) { return .code }
        if let ext = CodeLanguages.extensionOf(attachment.name) {
            return mediaExtensions.contains(ext) ? .media : .document
        }
        return mediaTypes.contains(attachment.mime.lowercased()) ? .media : .document
    }
}
