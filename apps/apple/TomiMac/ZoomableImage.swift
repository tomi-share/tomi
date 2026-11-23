import SwiftUI
import TomiKit

/// One image of the viewer on the Mac (the iOS counterpart is a `UIScrollView`): fitted in the window, zoomed with
/// the trackpad pinch (1 to 6) or a double click (2.5, or back to the fitted size), moved by dragging once zoomed.
/// A single click shows or hides the controls.
struct ZoomableImage: View {
    let image: PlatformImage
    let description: String
    let onTap: () -> Void
    /// iOS gesture (pulling the fitted image down closes the viewer): unused on the Mac, where Escape and the close
    /// button do it.
    var onPullDown: (CGFloat, Bool) -> Void = { _, _ in }
    /// A hidden item's image: the window is kept out of captures while it shows (`captureProtected`).
    var protected = false

    @State private var scale: CGFloat = 1
    @State private var pinchStart: CGFloat?
    @State private var offset = CGSize.zero
    @State private var dragStart: CGSize?

    private static let maxScale: CGFloat = 6
    private static let doubleClickScale: CGFloat = 2.5

    var body: some View {
        GeometryReader { proxy in
            Image(platformImage: image)
                .resizable()
                .scaledToFit()
                .scaleEffect(scale)
                .offset(offset)
                .frame(width: proxy.size.width, height: proxy.size.height)
                .contentShape(Rectangle())
                .accessibilityLabel(Text(verbatim: description))
                .gesture(
                    MagnifyGesture()
                        .onChanged { value in
                            let start = pinchStart ?? scale
                            pinchStart = start
                            scale = min(Self.maxScale, max(1, start * value.magnification))
                            offset = clamped(offset, in: proxy.size)
                        }
                        .onEnded { _ in
                            pinchStart = nil
                            if scale <= 1 { offset = .zero }
                        }
                )
                .simultaneousGesture(
                    DragGesture(minimumDistance: 4)
                        .onChanged { value in
                            guard scale > 1 else { return }
                            let start = dragStart ?? offset
                            dragStart = start
                            offset = clamped(
                                CGSize(width: start.width + value.translation.width, height: start.height + value.translation.height),
                                in: proxy.size
                            )
                        }
                        .onEnded { _ in dragStart = nil }
                )
                .onTapGesture(count: 2) {
                    withAnimation(.easeOut(duration: 0.2)) {
                        if scale > 1 {
                            scale = 1
                            offset = .zero
                        } else {
                            scale = Self.doubleClickScale
                        }
                    }
                }
                .onTapGesture(perform: onTap)
        }
        .captureProtected(protected)
    }

    /// Keeps the zoomed image over the window: it never leaves more than its own overflow on a side.
    private func clamped(_ offset: CGSize, in size: CGSize) -> CGSize {
        let limitX = max(0, (size.width * scale - size.width) / 2)
        let limitY = max(0, (size.height * scale - size.height) / 2)
        return CGSize(
            width: min(limitX, max(-limitX, offset.width)),
            height: min(limitY, max(-limitY, offset.height))
        )
    }
}
