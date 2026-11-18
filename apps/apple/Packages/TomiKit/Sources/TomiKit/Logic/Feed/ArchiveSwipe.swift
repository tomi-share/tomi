import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/ArchiveSwipe.kt.

/// What the swipe of a card (or the "Archive" entry of its menu) asks the core to do.
public enum ArchiveAction: Sendable, Hashable {
    case archive
    case unarchive

    /// The action that cancels this one (the "Undo" of the feedback banner).
    public var inverse: ArchiveAction { self == .archive ? .unarchive : .archive }
}

/// Pure rules of the archive gesture. The UI only goes through
/// `TomiCore.archive` / `TomiCore.unarchive`: there is no local archive state in the app.
public enum ArchiveSwipe {
    /// An outgoing item that is still queued or failed stays where it is: a failed send must remain visible (section
    /// 2.4), and the queue entry belongs to the send flow.
    public static func canArchive(_ item: TomiItem, localDeviceId: String) -> Bool {
        !(item.isOutgoing(localDeviceId: localDeviceId)
            && item.deliveries.contains { $0.state == .queued || $0.state == .failed })
    }

    /// The action of the menu entry: "Unarchive" for an archived item, "Archive" for any other.
    public static func menuAction(_ item: TomiItem) -> ArchiveAction {
        item.isShownAsArchived ? .unarchive : .archive
    }

    /// The action of the swipe to the left on `item`, or nil when the card does not swipe. A pinned card never swipes
    /// (the user wanted it on top; the menu offers "Unpin" and "Archive"), nor does an item that cannot be archived.
    /// Everywhere else the swipe archives, except on an archived card (Archives view, or a search) where it un-archives.
    public static func swipeAction(_ item: TomiItem, localDeviceId: String) -> ArchiveAction? {
        if item.pinned { return nil }
        if !canArchive(item, localDeviceId: localDeviceId) { return nil }
        return menuAction(item)
    }

    /// Runs `action` on the core. The one place that maps an `ArchiveAction` to the contract.
    @MainActor
    public static func perform(_ core: any TomiCore, _ action: ArchiveAction, itemId: String) async throws {
        switch action {
        case .archive: try await core.archive(id: itemId)
        case .unarchive: try await core.unarchive(id: itemId)
        }
    }

    /// Undoes an `action` done on an item (as it was BEFORE the action). Un-archiving brings the item back to "Recent";
    /// if the archive had unpinned it, the pin is restored too.
    @MainActor
    public static func undo(_ core: any TomiCore, _ action: ArchiveAction, itemId: String, wasPinned: Bool) async throws {
        try await perform(core, action.inverse, itemId: itemId)
        if action == .archive && wasPinned { try await core.pin(id: itemId) }
    }
}
