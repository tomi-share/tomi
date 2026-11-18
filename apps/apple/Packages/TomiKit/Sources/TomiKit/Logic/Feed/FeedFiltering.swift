import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/FeedFiltering.kt.

/// `archives` is the only filter that shows archived items; they are absent from every other one.
public enum FeedFilter: Sendable, Hashable, CaseIterable {
    case all, links, text, files, secrets, archives
}

extension TomiItem {
    /// Whether the UI treats the item as archived. Pin and archive exclude each other, so a core never reports both; if
    /// two devices ever converge on both, the pin wins and the item stays
    /// visible.
    public var isShownAsArchived: Bool { archived && !pinned }
}

public enum FeedFiltering {
    /// Archived items belong to `.archives` only; every other filter (`.all` included) leaves them out, so they do not
    /// appear in "Recent" nor in the typed filters.
    public static func matches(_ item: TomiItem, _ filter: FeedFilter) -> Bool {
        if filter == .archives { return item.isShownAsArchived }
        if item.isShownAsArchived { return false }
        switch filter {
        case .all: return true
        // A hidden item stays under "Masqués" only: classing it by its content would tell what it holds.
        case .links: return !item.secret && item.url != nil
        case .text: return !item.secret && item.text != nil
        case .files: return !item.secret && !item.attachments.isEmpty
        case .secrets: return item.secret
        case .archives: return false
        }
    }

    /// Local, case-insensitive search. A hidden ("Masqué") item is searched by its source device only: neither its
    /// title nor its content (text, link, file names) is ever searched, so typing in the search field cannot be used to
    /// guess or to confirm anything about a hidden item, whether it is currently shown in clear or not.
    public static func matchesQuery(_ item: TomiItem, _ query: String) -> Bool {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if q.isEmpty { return true }
        var fields: [String?] = [item.sourceDevice.name]
        if !item.secret {
            fields.append(item.title)
            fields.append(item.url)
            fields.append(item.text?.plain)
            for attachment in item.attachments { fields.append(attachment.name) }
        }
        return fields.contains { field in
            guard let field else { return false }
            return field.range(of: q, options: .caseInsensitive) != nil
        }
    }

    /// Filter then search. An archived item is "findable": while the user types a search under `.all`, archived items
    /// are searched too (the card carries the archive badge). With an empty search they only show under `.archives`.
    public static func apply(_ items: [TomiItem], _ filter: FeedFilter, _ query: String) -> [TomiItem] {
        let searchingEverywhere = filter == .all && !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        return items.filter { item in
            (matches(item, filter) || (searchingEverywhere && item.isShownAsArchived)) && matchesQuery(item, query)
        }
    }
}
