import SwiftUI
import TomiDesign
import TomiKit
import TomiRuntime

/// What was detected under the fields (link, text, file, hidden) and the devices it goes to (Android `DetectionRow`).
struct DetectionRow: View {
    let model: SendViewModel

    var body: some View {
        let trimmed = model.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty || model.hasAttachment {
            VStack(alignment: .leading, spacing: TomiSpace.s2) {
                HStack(spacing: TomiSpace.s2) {
                    chip(trimmed: trimmed)
                    if let caption = caption {
                        Text(caption).tomiText(.caption).foregroundStyle(TomiColor.textMuted).lineLimit(2)
                    }
                }
                switch model.secretHint {
                case .sourceSensitive: hint("hint_secret_sensitive")
                case .heuristic: hint("hint_secret_heuristic")
                case .none: EmptyView()
                }
            }
        }
    }

    private var caption: String? {
        if model.secretHint == .heuristic { return L10n.text("detect_masked_password") }
        let recipients = model.recipients
        return recipients.isEmpty ? nil : L10n.text("detect_to_devices", recipients.joined(separator: ", "))
    }

    @ViewBuilder
    private func chip(trimmed: String) -> some View {
        if model.secret {
            DetectionChip(key: "detect_masked", icon: .masked, background: TomiColor.secretContainer, foreground: TomiColor.secret)
        } else {
            let isLink = !trimmed.isEmpty && LinkExtractor.isValidHTTPURL(trimmed)
            DetectionChip(
                key: isLink ? "detect_link" : (trimmed.isEmpty ? "detect_files" : "detect_text"),
                icon: isLink ? .link : .file,
                background: TomiColor.brandContainer, foreground: TomiColor.brand
            )
        }
    }

    private func hint(_ key: LocalizedStringKey) -> some View {
        Text(key).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
    }
}

private struct DetectionChip: View {
    let key: LocalizedStringKey
    let icon: TomiIcon
    let background: Color
    let foreground: Color

    var body: some View {
        HStack(spacing: 6) {
            icon.image.font(.system(size: 14))
            Text(key).tomiText(.label)
        }
        .foregroundStyle(foreground)
        .padding(.leading, TomiSpace.s2)
        .padding(.trailing, TomiSpace.s3)
        .padding(.vertical, 6)
        .background(background, in: Capsule())
        .fixedSize()
    }
}

/// A chosen file: its whole name in the proportional font (like the tiles of the cards), and a cross to remove it (Android `AttachmentRow`).
struct AttachmentRow: View {
    let name: String
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: TomiSpace.s2) {
            TomiIcon.file.image.font(.system(size: 14)).foregroundStyle(TomiColor.textMuted)
            // The whole name: it wraps, it is never cut.
            Text(verbatim: name)
                .tomiText(.label)
                .foregroundStyle(TomiColor.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.vertical, TomiSpace.s3)
            Button(action: onRemove) {
                TomiIcon.close.image.foregroundStyle(TomiColor.textMuted).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(L10n.text("action_remove_attachment", name)))
        }
        .padding(.leading, TomiSpace.s3)
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border))
    }
}
