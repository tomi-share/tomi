import SwiftUI
import TomiKit

/// Shows `content` up to `maxHeight` (top-aligned, the rest clipped) and reports whether something was cut (Android
/// `HeightLimited` + `ClipDetection`, 2 pt of tolerance). With `maxHeight` nil the content is shown whole.
struct ClippedContent<Content: View>: View {
    let maxHeight: CGFloat?
    @Binding var clipped: Bool
    @ViewBuilder let content: () -> Content
    @State private var naturalHeight: CGFloat = 0

    var body: some View {
        if let maxHeight {
            content()
                .fixedSize(horizontal: false, vertical: true)
                .background(GeometryReader { geo in
                    Color.clear.preference(key: ClippedContentHeightKey.self, value: geo.size.height)
                })
                .onPreferenceChange(ClippedContentHeightKey.self) { height in
                    naturalHeight = height
                    let cut = ClipDetection.isClipped(contentHeight: Int(height.rounded()), visibleHeight: Int(maxHeight.rounded()))
                    if cut != clipped { clipped = cut }
                }
                .frame(maxHeight: min(maxHeight, max(naturalHeight, 1)), alignment: .top)
                .clipped()
        } else {
            content()
        }
    }
}

private struct ClippedContentHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}
