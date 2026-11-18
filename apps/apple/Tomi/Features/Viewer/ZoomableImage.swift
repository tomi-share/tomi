import SwiftUI
import TomiKit
import UIKit

/// One image of the viewer, fitted in the screen, with pinch zoom (1 to 6), double tap (2.5 around the point, or back
/// to the fitted size) and pan once zoomed (Android `ZoomableImage`, `ZoomMath`). A `UIScrollView`: at the fitted size it
/// leaves the horizontal swipe to the pager, once zoomed it keeps it. A single tap shows or hides the bars.
struct ZoomableImage: UIViewRepresentable {
    let image: UIImage
    let description: String
    let onTap: () -> Void
    /// Pulling the fitted image down (Photos): the distance while it moves, then whether it was let go.
    var onPullDown: (CGFloat, Bool) -> Void = { _, _ in }
    /// A hidden item's image: drawn in the layer that captures leave out (screenshots, recordings, mirroring).
    var protected = false

    func makeUIView(context: Context) -> ZoomScrollView {
        let view = ZoomScrollView(protected: protected)
        view.onTap = onTap
        view.onPullDown = onPullDown
        view.image = image
        view.imageView.accessibilityLabel = description
        return view
    }

    func updateUIView(_ view: ZoomScrollView, context: Context) {
        view.onTap = onTap
        view.onPullDown = onPullDown
        if view.image !== image { view.image = image }
        view.imageView.accessibilityLabel = description
    }
}

final class ZoomScrollView: UIScrollView, UIScrollViewDelegate {
    let imageView = UIImageView()
    /// What zooms: the image itself, or the protected layer that holds it.
    private let zoomed: UIView
    var onTap: (() -> Void)?
    var onPullDown: ((CGFloat, Bool) -> Void)?
    private lazy var pull = UIPanGestureRecognizer(target: self, action: #selector(pulled(_:)))
    /// The delegate of the pull alone: the scroll view already is, privately, the delegate of its own pan and pinch.
    private lazy var pullDelegate = PullDelegate(view: self)
    var image: UIImage? {
        didSet {
            imageView.image = image
            fittedFor = .zero
            setNeedsLayout()
        }
    }

    /// The size the image was last fitted to: a new size (rotation, unfolding) fits it again at scale 1.
    private var fittedFor = CGSize.zero

    init(protected: Bool) {
        zoomed = protected ? CaptureProtectedLayer.make() : imageView
        super.init(frame: .zero)
        delegate = self
        minimumZoomScale = ZoomMath.minScale
        maximumZoomScale = ZoomMath.maxScale
        showsHorizontalScrollIndicator = false
        showsVerticalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        decelerationRate = .fast
        bouncesZoom = true
        backgroundColor = .clear
        imageView.contentMode = .scaleAspectFit
        imageView.isAccessibilityElement = true
        imageView.accessibilityTraits = .image
        if zoomed !== imageView {
            imageView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            zoomed.addSubview(imageView)
        }
        addSubview(zoomed)

        let double = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
        double.numberOfTapsRequired = 2
        addGestureRecognizer(double)
        let single = UITapGestureRecognizer(target: self, action: #selector(tapped))
        single.require(toFail: double)
        addGestureRecognizer(single)
        pull.delegate = pullDelegate
        // One finger: two are a pinch, which the pull must never swallow.
        pull.maximumNumberOfTouches = 1
        addGestureRecognizer(pull)
    }

    /// The pull starts only on the fitted image and downwards: once zoomed the drag moves the image, and a sideways
    /// swipe turns the page.
    fileprivate func pullShouldBegin() -> Bool {
        guard !ZoomMath.isZoomed(Placement(scale: zoomScale)) else { return false }
        let velocity = pull.velocity(in: self)
        return velocity.y > 0 && abs(velocity.y) > abs(velocity.x) * 1.5
    }

    @objc private func pulled(_ recognizer: UIPanGestureRecognizer) {
        // In the window: the image shrinks while pulled, the finger is followed on screen.
        let distance = max(0, recognizer.translation(in: nil).y)
        switch recognizer.state {
        case .changed: onPullDown?(distance, false)
        case .ended:
            // A quick flick counts as far as a long pull.
            let flick = recognizer.velocity(in: self).y > 900
            onPullDown?(flick ? .greatestFiniteMagnitude : distance, true)
        case .cancelled, .failed: onPullDown?(0, true)
        default: break
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.size != fittedFor, bounds.width > 0, bounds.height > 0 {
            fittedFor = bounds.size
            fit()
        }
        center()
    }

    private func fit() {
        zoomScale = 1
        guard let image else { return }
        let size = ZoomMath.fitSize(
            contentWidth: image.size.width, contentHeight: image.size.height,
            viewWidth: bounds.width, viewHeight: bounds.height
        )
        zoomed.frame = CGRect(x: 0, y: 0, width: size.width, height: size.height)
        imageView.frame = zoomed.bounds
        contentSize = zoomed.frame.size
    }

    /// Keeps the image centred while it is smaller than the screen.
    private func center() {
        let x = max(0, (bounds.width - contentSize.width) / 2)
        let y = max(0, (bounds.height - contentSize.height) / 2)
        contentInset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { zoomed }

    func scrollViewDidZoom(_ scrollView: UIScrollView) { center() }

    @objc private func tapped() { onTap?() }

    @objc private func doubleTapped(_ recognizer: UITapGestureRecognizer) {
        if ZoomMath.isZoomed(Placement(scale: zoomScale)) {
            setZoomScale(ZoomMath.minScale, animated: true)
            return
        }
        let point = recognizer.location(in: imageView)
        let scale = ZoomMath.doubleTapScale
        let size = CGSize(width: bounds.width / scale, height: bounds.height / scale)
        zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: true)
    }
}

/// Decides for the pull of a `ZoomScrollView`: it begins only on a downward drag of the fitted image, and the pager's
/// own pan waits for it to fail (it fails at once on a sideways swipe, so the page turns as before).
@MainActor
private final class PullDelegate: NSObject, UIGestureRecognizerDelegate {
    weak var view: ZoomScrollView?

    init(view: ZoomScrollView) { self.view = view }

    func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
        view?.pullShouldBegin() ?? false
    }

    func gestureRecognizer(_ recognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
        guard let view, let scroll = other.view as? UIScrollView, scroll !== view else { return false }
        return other === scroll.panGestureRecognizer && view.isDescendant(of: scroll)
    }
}
