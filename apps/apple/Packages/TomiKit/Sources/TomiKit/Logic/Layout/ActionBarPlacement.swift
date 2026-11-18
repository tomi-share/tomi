import Foundation

/// Side of the screen, independent of SwiftUI (`HorizontalEdge` in the app).
public enum SideEdge: Sendable, Hashable {
    case leading, trailing
}

/// Where the floating action bars of a screen go (feed: search, filters, devices and "Envoyer"; detail: pin, archive,
/// delete, more and "Copier").
///
/// On a classic iPhone they float at the bottom, horizontal, like on Android. Wherever the system lays its own bars
/// vertically in a strip along the side (iPhone Duo folded, and unfolded in landscape: status bar and back button in an
/// 84 pt strip on the right), they go into that strip, vertical and bottom-aligned, as Apple asks for bottom toolbars
/// ("Design for iPhone Duo"): the content keeps the whole height. Unfolded in portrait there is no strip (the status bar
/// is at the top) and they stay at the bottom.
public enum ActionBarPlacement: Sendable, Hashable {
    case bottom
    /// In the strip on `edge`, `width` points wide (the safe-area inset on that side).
    case side(SideEdge, width: Double)

    /// Narrowest strip that holds the vertical bar: a 48 pt button plus its padding.
    public static let minimumSideWidth: Double = 64

    /// - Parameters:
    ///   - systemEdge: the side of the system vertical bar (`toolbarVerticalEdge`, iOS 27.1), nil when the system lays
    ///     its bars horizontally or the API is not available.
    ///   - leading, trailing: safe-area insets of the screen, in points.
    ///
    /// The system edge wins whenever its strip is wide enough. Without it, only a strip on one side with nothing on the
    /// other counts: a classic iPhone in landscape has the same inset on both sides and keeps its bar at the bottom.
    public static func resolve(systemEdge: SideEdge?, leading: Double, trailing: Double) -> ActionBarPlacement {
        let leading = validInset(leading)
        let trailing = validInset(trailing)
        if let systemEdge {
            let width = systemEdge == .trailing ? trailing : leading
            if width >= minimumSideWidth { return .side(systemEdge, width: width) }
            return .bottom
        }
        if trailing >= minimumSideWidth && leading < 1 { return .side(.trailing, width: trailing) }
        if leading >= minimumSideWidth && trailing < 1 { return .side(.leading, width: leading) }
        return .bottom
    }

    /// Room to leave under the vertical bar, from the bottom of the screen: the home indicator (`safeBottom`), or the
    /// occupied corner at the bottom of the strip when it is taller (`containerCornerInsets`: iPhone Duo folded in
    /// landscape has its camera there, 84 × 82 pt).
    public static func sideBottomClearance(safeBottom: Double, occupiedCorner: Double) -> Double {
        max(validInset(safeBottom), validInset(occupiedCorner))
    }

    public var isSide: Bool {
        if case .side = self { return true }
        return false
    }
}

/// The top band of a screen whose status bar only fills a corner of it: on the unfolded iPhone Duo in portrait the
/// system keeps 82 pt at the top, but the time, the camera and the Wi-Fi sit in a 144 × 82 pt block at the top right
/// (`containerCornerInsets`, iOS 26); the rest of the band is empty. The top of the screen (logotype and menu, or the
/// search field and its filters) then goes into the band, beside the block instead of under it.
public struct TopCornerBand: Sendable, Hashable {
    /// Height of the band (the top safe-area inset).
    public let height: Double
    /// Room to leave free from each edge for the occupied corners, in points (0 when the corner is free).
    public let leading: Double
    public let trailing: Double

    /// Narrowest free middle that still holds the header (logotype and menu, or the search field).
    public static let minimumFreeWidth: Double = 280
    /// Shortest top inset that is a band of its own (below, it is the margin of a classic screen).
    public static let minimumHeight: Double = 24
    /// Centre line of the status content at the top of an iPhone Duo screen, in points from the top edge: the time,
    /// the camera and the Wi-Fi are drawn there, folded or not (measured on the simulator, as Safari aligns its buttons).
    public static let statusRowCenter: Double = 48

    /// Centre line of the status content inside the band: the system draws it between the first third (the clearance
    /// of the rounded screen corner) and the last sixth (its touch margin) of the occupied corner, 47.8 pt in 82 pt.
    public var rowCenter: Double { height * 7 / 12 }

    /// Height of the glass behind the status content: the content (half the band) plus 7 pt above and below.
    public var backdropHeight: Double { height / 2 + 14 }

    /// Where the centre of the top row of a screen goes, in points from the top of its safe area, so that the row lines
    /// up with the status content: in the band when there is one (the page then starts at the top of the screen), at
    /// `statusRowCenter` when the screen has no top inset (iPhone Duo folded, or unfolded in landscape: the status is in
    /// the side strip), nil on a classic iPhone (the row sits under the status bar, as usual).
    public static func headerCenter(band: TopCornerBand?, safeTop: Double) -> Double? {
        if let band { return band.rowCenter }
        let top = validInset(safeTop)
        guard top < minimumHeight else { return nil }
        return max(statusRowCenter - top, 0)
    }

    public init(height: Double, leading: Double, trailing: Double) {
        self.height = height
        self.leading = leading
        self.trailing = trailing
    }

    /// - Parameters:
    ///   - width: width of the screen, in points.
    ///   - safeTop, safeLeading, safeTrailing: safe-area insets of the screen.
    ///   - topLeadingCorner, topTrailingCorner: the occupied corners (`containerCornerInsets`), width and height.
    ///
    /// A band only when the system bars are at the top (no side strip) and an occupied corner spans its whole height:
    /// then nothing else is in it. A classic iPhone (Dynamic Island in the middle, rounded corners shorter than the
    /// band) and the side strip of iPhone Duo keep the usual layout.
    public static func resolve(
        width: Double, safeTop: Double, safeLeading: Double, safeTrailing: Double,
        topLeadingCorner: (width: Double, height: Double), topTrailingCorner: (width: Double, height: Double)
    ) -> TopCornerBand? {
        let width = validInset(width)
        let top = validInset(safeTop)
        guard top >= minimumHeight, validInset(safeLeading) < 1, validInset(safeTrailing) < 1 else { return nil }
        let corners = [topLeadingCorner, topTrailingCorner].map { (width: validInset($0.width), height: validInset($0.height)) }
        guard corners.contains(where: { $0.width > 0 && $0.height >= top - 1 }) else { return nil }
        let band = TopCornerBand(height: top, leading: corners[0].width, trailing: corners[1].width)
        guard width - band.leading - band.trailing >= minimumFreeWidth else { return nil }
        return band
    }
}

/// A finite, non-negative inset (NaN, infinite and negative values count as none).
private func validInset(_ value: Double) -> Double { value.isFinite ? max(value, 0) : 0 }
