import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/core/ItemReuse.kt.
//
// Deviation from Kotlin: Swift values have no identity. "The same instance" is here "the same storage": the strings,
// the thumbnails and the arrays of a reused item are the buffers of the previous read, so comparing it with the item
// on screen stops at the first pointer instead of reading every text.

public enum ItemReuse {
    /// `fresh` (a new read of the whole feed) with what did not change taken from `previous` (the read before): an
    /// item that is equal is the previous one, and an item whose metadata only changed (read, deliveries, pin,
    /// archive) keeps the previous content (texts, attachments, thumbnails).
    ///
    /// The core hands out new values for the whole feed at every change it announces (a sync announces many). With
    /// the previous storage kept, a card whose item did not change is equal to itself at once (it is not drawn again,
    /// its texts are not compared byte by byte), and the buffers of the new read are freed. Equal by value in every
    /// case: only the storage differs from `fresh`.
    public static func reuseUnchanged(previous: [TomiItem], fresh: [TomiItem]) -> [TomiItem] {
        if previous.isEmpty || fresh.isEmpty { return fresh }
        var known = [String: Int](minimumCapacity: previous.count)
        for (index, item) in previous.enumerated() { known[item.id] = index }
        return fresh.map { item in
            guard let index = known[item.id] else { return item }
            let old = previous[index]
            if old.content != item.content { return item }
            // Same content: the comparison of the rest is then cheap (the content is the same storage).
            var shared = item
            shared.content = old.content
            return shared == old ? old : shared
        }
    }
}
