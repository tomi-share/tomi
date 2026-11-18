import SwiftUI
import UIKit

/// The layer iOS leaves out of every capture: screenshots, screen recordings, mirroring and screen sharing show it
/// empty, while the person holding the phone sees it as usual (the closest iOS has to Android `FLAG_SECURE`). It is the
/// layer of a secure text field (`UITextField.isSecureTextEntry`), which iOS draws only on the device's own screen.
///
/// Not a documented API: if a future iOS no longer gives a secure text field that layer, `make()` gives an ordinary
/// view and the content is drawn as before (the hide-after-screenshot and the capture cover still apply).
enum CaptureProtectedLayer {
    /// A view whose subviews are left out of captures (an ordinary view when iOS does not give the layer).
    @MainActor
    static func make() -> UIView {
        let field = UITextField()
        field.isSecureTextEntry = true
        field.layoutIfNeeded()
        // The secure canvas is the first subview of a secure text field (iOS 15 to 27).
        // Checked on iOS 27: `_UITextLayoutCanvasView`, its layer marked to be left out of captures. Anything else (a
        // future iOS) is not reused: an ordinary view, the content drawn as usual.
        guard let canvas = field.subviews.first, NSStringFromClass(type(of: canvas)).contains("CanvasView") else {
            #if DEBUG
            print("CaptureProtectedLayer: no secure canvas in this iOS, content not protected from captures")
            #endif
            return UIView()
        }
        canvas.subviews.forEach { $0.removeFromSuperview() }
        canvas.removeFromSuperview()
        canvas.isUserInteractionEnabled = true
        canvas.backgroundColor = .clear
        return canvas
    }
}

/// SwiftUI content drawn in the protected layer: what a capture shows is the empty place. The content keeps the
/// environment of where it is placed (colours, Dynamic Type, the app's models); taps, menus and buttons work.
struct CaptureShield<Content: View>: UIViewRepresentable {
    @ViewBuilder let content: Content
    @Environment(\.self) private var environment

    func makeUIView(context: Context) -> ShieldView {
        ShieldView(root: AnyView(content.environment(\.self, environment)))
    }

    func updateUIView(_ view: ShieldView, context: Context) {
        view.host.rootView = AnyView(content.environment(\.self, environment))
        view.invalidateIntrinsicContentSize()
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: ShieldView, context: Context) -> CGSize? {
        let width = proposal.width ?? .greatestFiniteMagnitude
        let height = proposal.height ?? .greatestFiniteMagnitude
        return uiView.host.sizeThatFits(in: CGSize(width: width, height: height))
    }

    final class ShieldView: UIView {
        let host: SizeReportingHost

        init(root: AnyView) {
            host = SizeReportingHost(rootView: root)
            super.init(frame: .zero)
            backgroundColor = .clear
            host.view.backgroundColor = .clear
            host.safeAreaRegions = []
            // A change inside (a QR made on appearance, a measured title) reaches the layout outside.
            host.sizingOptions = [.preferredContentSize]
            host.onSizeChange = { [weak self] in self?.invalidateIntrinsicContentSize() }
            let layer = CaptureProtectedLayer.make()
            for (child, parent) in [(layer, self as UIView), (host.view!, layer)] {
                child.translatesAutoresizingMaskIntoConstraints = false
                parent.addSubview(child)
                NSLayoutConstraint.activate([
                    child.leadingAnchor.constraint(equalTo: parent.leadingAnchor),
                    child.trailingAnchor.constraint(equalTo: parent.trailingAnchor),
                    child.topAnchor.constraint(equalTo: parent.topAnchor),
                    child.bottomAnchor.constraint(equalTo: parent.bottomAnchor),
                ])
            }
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    }
}

/// Tells when the size of its content changes (`preferredContentSize`, kept up to date by `sizingOptions`).
final class SizeReportingHost: UIHostingController<AnyView> {
    var onSizeChange: (() -> Void)?

    override var preferredContentSize: CGSize {
        didSet { if preferredContentSize != oldValue { onSizeChange?() } }
    }
}

extension View {
    /// Left out of screenshots, recordings and mirroring when `active` (a hidden item shown, a pairing code).
    @ViewBuilder
    func captureProtected(_ active: Bool = true) -> some View {
        if active {
            CaptureShield { self }
        } else {
            self
        }
    }
}
