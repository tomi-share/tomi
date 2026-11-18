import SwiftUI
import TomiDesign
import TomiKit
import UIKit

/// Swipe to the left to archive (in "Récents") or bring back (in "Archives"), in two stages like Android
/// (`SwipeToArchive.kt`, rules in `SwipeArchiveLogic`): past 88 pt the card stays open on a tappable pill; past 58 % of
/// the width the release runs the action. The decision is made at release by the distance only, never by the speed;
/// one strong haptic tick when entering the armed zone. Never a deletion; no swipe to the right. A card is wrapped only
/// when the action is allowed (`ArchiveSwipe.swipeAction`); the menu and the accessibility action stay the other ways.
struct SwipeToArchive<Content: View>: View {
    let item: TomiItem
    let action: ArchiveAction
    let model: FeedViewModel
    @ViewBuilder let content: () -> Content

    @State private var offset: CGFloat = 0
    @State private var dragStart: CGFloat?
    @State private var zone = SwipeZone.closed
    @State private var exiting = false
    @State private var width: CGFloat = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let actions = CGFloat(SwipeArchiveLogic.actionsWidthDp)
    private let pill = CGFloat(SwipeArchiveLogic.pillSizeDp)
    private var isOpen: Bool { model.openSwipeItemId == item.id && !exiting }
    private var reveal: Float { SwipeArchiveLogic.reveal(Float(offset)) }

    var body: some View {
        ZStack(alignment: .trailing) {
            if offset < 0 { underlay }
            content()
                .offset(x: offset)
                .overlay {
                    // Open: a tap on the card closes it instead of opening the item.
                    if isOpen {
                        Color.clear.contentShape(Rectangle()).offset(x: offset).onTapGesture { close() }
                    }
                }
        }
        .background(GeometryReader { geo in Color.clear.onAppear { width = geo.size.width }.onChange(of: geo.size.width) { _, w in width = w } })
        .modifier(HorizontalDrag(onChanged: dragChanged, onEnded: dragEnded))
        .onChange(of: model.openSwipeItemId) { _, id in
            if id != item.id && offset != 0 && !exiting { animate(to: 0, spring: true) }
        }
        .accessibilityAction(named: Text(LocalizedStringKey(action == .archive ? "action_archive" : "action_unarchive"))) {
            model.applyArchiveAction(item, action)
        }
    }

    // MARK: Underlay (the pill)

    private var underlay: some View {
        let r = reveal
        let arrival = CGFloat(SwipeArchiveLogic.arrival(reveal: r, actionsWidth: Float(actions)))
        let stretch = CGFloat(SwipeArchiveLogic.stretch(reveal: r, actionsWidth: Float(actions)))
        let iconShift = CGFloat(SwipeArchiveLogic.iconOffsetInPill(reveal: r, actionsWidth: Float(actions), width: Float(width)))
        return RoundedRectangle(cornerRadius: TomiRadius.medium)
            .fill(TomiColor.brandContainer)
            .overlay(alignment: .trailing) {
                Capsule()
                    .fill(TomiColor.brand)
                    .frame(width: pill + stretch, height: pill)
                    .overlay {
                        (action == .archive ? TomiIcon.archive : TomiIcon.unarchive).image
                            .font(.system(size: 20, weight: .medium))
                            .foregroundStyle(TomiColor.onBrand)
                            .offset(x: iconShift)
                    }
                    .scaleEffect(CGFloat(SwipeArchiveLogic.pillScale(Float(arrival))))
                    .opacity(arrival)
                    .offset(x: CGFloat(SwipeArchiveLogic.arrivalShiftDp) * (1 - arrival))
                    .padding(.trailing, (actions - pill) / 2)
                    .contentShape(Capsule())
                    .onTapGesture { if isOpen { commit() } }
                    .accessibilityHidden(true)
            }
            .frame(width: max(-offset, 0))
    }

    // MARK: Gesture

    /// The finger moved `translation` points horizontally since the drag began (the gesture only begins horizontally).
    private func dragChanged(_ translation: CGFloat) {
        guard !exiting, width > 0 else { return }
        if dragStart == nil {
            dragStart = offset
            if model.openSwipeItemId != item.id { model.openSwipeItemId = nil }
        }
        offset = min(0, max(-width, (dragStart ?? 0) + translation))
        let next = SwipeArchiveLogic.nextZone(
            was: zone, reveal: reveal, width: Float(width),
            openPx: Float(actions), hysteresis: SwipeArchiveLogic.hysteresisDp
        )
        if next != zone {
            if SwipeArchiveLogic.vibratesWhenEntering(next) {
                UIImpactFeedbackGenerator(style: .heavy).impactOccurred()
            }
            zone = next
        }
    }

    /// The finger was lifted (or the system took the touch: `cancelled`, the card goes back).
    private func dragEnded(cancelled: Bool) {
        defer { dragStart = nil }
        guard dragStart != nil, !exiting, width > 0 else { return }
        if cancelled { return close() }
        let target = SwipeArchiveLogic.settleTarget(
            was: zone, reveal: reveal, width: Float(width),
            openPx: Float(actions), hysteresis: SwipeArchiveLogic.hysteresisDp
        )
        switch target {
        case .out: commit()
        case .open:
            model.openSwipeItemId = item.id
            zone = .open
            animate(to: -CGFloat(SwipeArchiveLogic.openOffset(actionsWidth: Float(actions), width: Float(width))), spring: false)
        case .resting: close()
        }
    }

    private func close() {
        if model.openSwipeItemId == item.id { model.openSwipeItemId = nil }
        zone = .closed
        animate(to: 0, spring: true)
    }

    /// The card leaves to the left, then the action runs once. If the card is still there a moment later (it stays in
    /// the search results, or the action failed), it comes back.
    private func commit() {
        guard !exiting else { return }
        exiting = true
        let duration = Double(SwipeArchiveLogic.stretchMs) / 1000
        if reduceMotion { offset = -width } else { withAnimation(.easeOut(duration: duration)) { offset = -width } }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(reduceMotion ? 0 : SwipeArchiveLogic.stretchMs))
            model.applyArchiveAction(item, action)
            try? await Task.sleep(for: .milliseconds(Int(SwipeArchiveLogic.restoreDelayMs)))
            exiting = false
            zone = .closed
            animate(to: 0, spring: true)
        }
    }

    private func animate(to target: CGFloat, spring: Bool) {
        if reduceMotion {
            offset = target
        } else if spring {
            withAnimation(.interpolatingSpring(stiffness: Double(SwipeArchiveLogic.returnStiffness), damping: 2 * Double(SwipeArchiveLogic.returnDampingRatio) * Double(SwipeArchiveLogic.returnStiffness).squareRoot())) {
                offset = target
            }
        } else {
            withAnimation(.interpolatingSpring(stiffness: Double(SwipeArchiveLogic.returnStiffness), damping: 2 * Double(SwipeArchiveLogic.returnStiffness).squareRoot())) {
                offset = target
            }
        }
    }
}

/// A horizontal drag that never takes a vertical one from the list. From iOS 18 a SwiftUI `DragGesture` on a card
/// blocks the scroll of the `ScrollView` around it, so the drag is a UIKit pan that only begins when the finger moves
/// more sideways than up or down; on iOS 17 the SwiftUI drag cohabits with the scroll and stays.
private struct HorizontalDrag: ViewModifier {
    let onChanged: (CGFloat) -> Void
    let onEnded: (_ cancelled: Bool) -> Void
    @State private var horizontal: Bool?
    /// True while the SwiftUI drag runs; it resets by itself even when the scroll takes the touch (no `onEnded` then).
    @GestureState private var dragging = false

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.gesture(HorizontalPan(onChanged: onChanged, onEnded: onEnded))
        } else {
            content.simultaneousGesture(
                DragGesture(minimumDistance: 12, coordinateSpace: .local)
                    .updating($dragging) { _, state, _ in state = true }
                    .onChanged { value in
                        if horizontal == nil {
                            horizontal = abs(value.translation.width) > abs(value.translation.height)
                        }
                        if horizontal == true { onChanged(value.translation.width) }
                    }
                    .onEnded { _ in
                        if horizontal == true { onEnded(false) }
                        horizontal = nil
                    }
            )
            .onChange(of: dragging) { _, now in
                // Cancelled without `onEnded`: the card goes back and the next drag starts afresh.
                if !now, horizontal != nil {
                    if horizontal == true { onEnded(true) }
                    horizontal = nil
                }
            }
        }
    }
}

@available(iOS 18.0, *)
private struct HorizontalPan: UIGestureRecognizerRepresentable {
    let onChanged: (CGFloat) -> Void
    let onEnded: (_ cancelled: Bool) -> Void

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.delegate = context.coordinator
        pan.maximumNumberOfTouches = 1
        return pan
    }

    func handleUIGestureRecognizerAction(_ recognizer: UIPanGestureRecognizer, context: Context) {
        switch recognizer.state {
        case .began, .changed: onChanged(recognizer.translation(in: recognizer.view).x)
        case .ended: onEnded(false)
        case .cancelled, .failed: onEnded(true)
        default: break
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        /// Begins only when the movement is mostly horizontal; a vertical one is left to the scroll.
        func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
            guard let pan = recognizer as? UIPanGestureRecognizer else { return false }
            let velocity = pan.velocity(in: pan.view)
            let translation = pan.translation(in: pan.view)
            let dx = abs(velocity.x) + abs(translation.x)
            let dy = abs(velocity.y) + abs(translation.y)
            return dx > dy * 1.2
        }
    }
}
