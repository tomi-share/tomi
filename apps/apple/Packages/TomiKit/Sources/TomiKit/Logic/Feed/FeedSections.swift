import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/FeedSections.kt.

/// The feed as the user sees it: a "Pinned" section on top, then the other items newest first.
public struct FeedSections: Sendable, Equatable {
    public var pinned: [TomiItem]
    public var recent: [TomiItem]

    public init(pinned: [TomiItem], recent: [TomiItem]) {
        self.pinned = pinned
        self.recent = recent
    }

    public var isEmpty: Bool { pinned.isEmpty && recent.isEmpty }

    /// Builds the sections from the (already filtered) items of the core, which arrive newest first
    /// (spec/pin-contract.md: the core never reorders for a pin, the UI does). Pinned items are sorted by
    /// `pinnedAt`, most recently pinned first; ties and a missing `pinnedAt` fall back to the creation time. An item is
    /// in exactly one section.
    public static func split(_ items: [TomiItem]) -> FeedSections {
        var pinned: [TomiItem] = []
        var recent: [TomiItem] = []
        for item in items {
            if item.pinned { pinned.append(item) } else { recent.append(item) }
        }
        // Stable sort (ties keep the order of the core), made explicit with the original index.
        let sorted = pinned.enumerated().sorted { a, b in
            let pa = a.element.pinnedAt ?? .distantPast
            let pb = b.element.pinnedAt ?? .distantPast
            if pa != pb { return pa > pb }
            if a.element.createdAt != b.element.createdAt { return a.element.createdAt > b.element.createdAt }
            return a.offset < b.offset
        }
        return FeedSections(pinned: sorted.map(\.element), recent: recent)
    }
}
