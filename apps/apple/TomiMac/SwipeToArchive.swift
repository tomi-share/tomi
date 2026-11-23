import SwiftUI
import TomiKit

/// The Mac counterpart of the swipe of the iOS feed: no swipe on a Mac (as on Windows), the card is shown as it is.
/// Archiving stays in the context menu of the card, in the bar of the detail, and as an accessibility action.
struct SwipeToArchive<Content: View>: View {
    let item: TomiItem
    let action: ArchiveAction
    let model: FeedViewModel
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .accessibilityAction(named: Text(LocalizedStringKey(action == .archive ? "action_archive" : "action_unarchive"))) {
                model.applyArchiveAction(item, action)
            }
    }
}
