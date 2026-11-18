import SwiftUI
import TomiDesign

/// How long a confirmation stays on screen (Android `FEEDBACK_DURATION_MS`): long enough to be read, and independent of
/// the 3 s undo window of the send banner (`SendCoordinator.undoWindowMs`).
let feedbackDuration: Duration = .seconds(5)

/// One message of the feedback banner (Android snackbar): text, optional action, `feedbackDuration`.
struct FeedbackMessage: Identifiable, Equatable {
    let id = UUID()
    let text: String
    var actionTitle: String?
    var action: (() -> Void)?

    static func == (a: FeedbackMessage, b: FeedbackMessage) -> Bool { a.id == b.id }
}

/// Shows one banner at a time; a new message replaces the current one at once (Android `showFeedback`).
@MainActor
@Observable
final class FeedbackCenter {
    private(set) var current: FeedbackMessage?
    private var dismissTask: Task<Void, Never>?

    func show(_ text: String, actionTitle: String? = nil, action: (() -> Void)? = nil) {
        let message = FeedbackMessage(text: text, actionTitle: actionTitle, action: action)
        current = message
        dismissTask?.cancel()
        dismissTask = Task { [weak self] in
            try? await Task.sleep(for: feedbackDuration)
            guard !Task.isCancelled, self?.current?.id == message.id else { return }
            self?.current = nil
        }
    }

    func dismiss() {
        dismissTask?.cancel()
        current = nil
    }
}

/// The banner, at the bottom above the floating toolbar.
struct FeedbackHost: View {
    let center: FeedbackCenter
    var bottomPadding: CGFloat = 96

    var body: some View {
        VStack {
            Spacer()
            if let message = center.current {
                TomiFeedbackBanner {
                    HStack {
                        Text(message.text)
                        Spacer(minLength: TomiSpace.s3)
                        if let title = message.actionTitle, let action = message.action {
                            Button(title) {
                                action()
                                center.dismiss()
                            }
                            .tomiText(.label)
                            .foregroundStyle(TomiColor.inverseSecondary)
                        }
                    }
                }
                .padding(.bottom, bottomPadding)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .id(message.id)
            }
        }
        .animation(.easeOut(duration: TomiMotion.medium), value: center.current)
        .allowsHitTesting(center.current != nil)
    }
}
