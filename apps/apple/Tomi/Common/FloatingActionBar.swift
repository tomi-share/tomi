import SwiftUI
import TomiDesign
import TomiKit

/// Where a screen's controls go, worked out from the screen: the floating action bar (`placement`), and the top band
/// to share with the status bar when it only fills a corner (`topBand`, unfolded iPhone Duo in portrait).
struct ScreenLayout {
    let placement: ActionBarPlacement
    let topBand: TopCornerBand?
    /// Centre line of the top row, from the top of the safe area (`TopCornerBand.headerCenter`), nil on a classic iPhone.
    let headerCenter: CGFloat?

    /// Room above a top row `rowHeight` points high so that it lines up with the status content (0 on a classic iPhone).
    func topPadding(rowHeight: CGFloat) -> CGFloat {
        headerCenter.map { max(0, $0 - rowHeight / 2) } ?? 0
    }
}

/// A screen with a floating action bar (feed, detail). The bar is horizontal at the bottom on a classic iPhone, and
/// vertical at the bottom of the side strip on iPhone Duo (see `ActionBarPlacement`). `content` gets the layout to
/// leave room at the bottom only when the bar is there, and to use the top band when there is one.
struct ActionBarScaffold<Content: View, Bar: View>: View {
    @ViewBuilder let content: (ScreenLayout) -> Content
    @ViewBuilder let bar: (Axis) -> Bar

    var body: some View {
        SystemBarEdgeReader { systemEdge in
            GeometryReader { proxy in
                let insets = proxy.safeAreaInsets
                let placement = ActionBarPlacement.resolve(
                    systemEdge: systemEdge, leading: Double(insets.leading), trailing: Double(insets.trailing)
                )
                ZStack {
                    let band = Self.topBand(proxy)
                    let center = TopCornerBand.headerCenter(band: band, safeTop: Double(insets.top)).map { CGFloat($0) }
                    content(ScreenLayout(placement: placement, topBand: band, headerCenter: center))
                    switch placement {
                    case .bottom:
                        bar(.horizontal)
                            .padding(.bottom, TomiSpace.s4)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
                    case let .side(edge, width):
                        // In the strip of the system bars, under its status bar and back button, bottom-aligned, above
                        // the camera when it is at the bottom of the strip (iPhone Duo folded in landscape).
                        bar(.vertical)
                            .frame(width: CGFloat(width))
                            .padding(.bottom, CGFloat(ActionBarPlacement.sideBottomClearance(
                                safeBottom: Double(insets.bottom), occupiedCorner: Self.bottomCornerHeight(proxy, edge: edge)
                            )) + TomiSpace.s4)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: edge == .trailing ? .bottomTrailing : .bottomLeading)
                            .ignoresSafeArea()
                    }
                }
            }
        }
    }

    /// Height of the occupied bottom corner on the side of the strip (`containerCornerInsets`, iOS 26), 0 before.
    private static func bottomCornerHeight(_ proxy: GeometryProxy, edge: SideEdge) -> Double {
        #if os(iOS)
        guard #available(iOS 26.0, *) else { return 0 }
        let corners = proxy.containerCornerInsets
        return Double(edge == .trailing ? corners.bottomTrailing.height : corners.bottomLeading.height)
        #else
        // A Mac window has no corner taken by the system.
        return 0
        #endif
    }

    /// The occupied corners are known from iOS 26 (`containerCornerInsets`); before, there is no band.
    private static func topBand(_ proxy: GeometryProxy) -> TopCornerBand? {
        #if os(macOS)
        return nil
        #else
        guard #available(iOS 26.0, *) else { return nil }
        let insets = proxy.safeAreaInsets
        let corners = proxy.containerCornerInsets
        return TopCornerBand.resolve(
            width: Double(proxy.size.width + insets.leading + insets.trailing),
            safeTop: Double(insets.top), safeLeading: Double(insets.leading), safeTrailing: Double(insets.trailing),
            topLeadingCorner: (width: Double(corners.topLeading.width), height: Double(corners.topLeading.height)),
            topTrailingCorner: (width: Double(corners.topTrailing.width), height: Double(corners.topTrailing.height))
        )
        #endif
    }
}

/// The side of the system vertical bar (`toolbarVerticalEdge`, iOS 27.1), nil before or where there is none.
private struct SystemBarEdgeReader<Content: View>: View {
    @ViewBuilder let content: (SideEdge?) -> Content

    var body: some View {
        #if os(iOS)
        if #available(iOS 27.1, *) {
            VerticalEdgeProbe(content: content)
        } else {
            content(nil)
        }
        #else
        // No system bar on a side of a Mac window: the action bar is at the bottom.
        content(nil)
        #endif
    }
}

#if os(iOS)
@available(iOS 27.1, *)
private struct VerticalEdgeProbe<Content: View>: View {
    @Environment(\.toolbarVerticalEdge) private var edge
    let content: (SideEdge?) -> Content

    var body: some View {
        content(edge.map { $0 == .leading ? SideEdge.leading : .trailing })
    }
}
#endif

/// The capsule of secondary actions plus the square primary button (`brand`, radius 12), laid along `axis`. On iOS 26
/// and later both are Liquid Glass (the material of the system bars, interactive, merged in one container); before, the
/// opaque surface of the charter.
struct FloatingActionBar<Items: View, Primary: View>: View {
    let axis: Axis
    @ViewBuilder let items: () -> Items
    @ViewBuilder let primary: () -> Primary

    var body: some View {
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: TomiSpace.s2) { bar }
        } else {
            bar
        }
    }

    private var bar: some View {
        let outer = axis == .horizontal ? AnyLayout(HStackLayout(spacing: TomiSpace.s2)) : AnyLayout(VStackLayout(spacing: TomiSpace.s2))
        let inner = axis == .horizontal ? AnyLayout(HStackLayout(spacing: TomiSpace.s1)) : AnyLayout(VStackLayout(spacing: TomiSpace.s1))
        return outer {
            inner { items() }
                .padding(TomiSpace.s1 + 2)
                .tomiGlass(in: Capsule())
            primary()
        }
    }
}

extension View {
    /// Liquid Glass in `shape` from iOS 26 (the material of the system bars, interactive: it reacts to the touch);
    /// before, `fallback` when given, else the opaque surface of the charter with its border and shadow. The glass is never
    /// tinted to imitate Android: iOS keeps its own Liquid Glass design.
    func tomiGlass<S: Shape>(in shape: S, fallback: Color? = nil) -> some View {
        modifier(GlassSurface(shape: shape, fallback: fallback))
    }
}

private struct GlassSurface<S: Shape>: ViewModifier {
    let shape: S
    let fallback: Color?

    func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content.glassEffect(.regular.interactive(), in: shape)
        } else if let fallback {
            content.background(fallback, in: shape)
        } else {
            content
                .background(TomiColor.surface, in: shape)
                .overlay(shape.stroke(TomiColor.border, lineWidth: 1))
                .shadow(color: .black.opacity(0.12), radius: 10, y: 4)
        }
    }
}

/// A round icon of the capsule. Selected (search open, a filter applied): a `brand` icon on a pill of `brand` at 24 %
/// (`barSelected`, as on Android), so that the current state reads at a glance, like the selected tab of a system tab bar.
/// `brandContainer` is too close to the colour of the bar in dark.
struct FloatingBarIcon: View {
    let icon: TomiIcon
    var selected = false

    var body: some View {
        icon.image
            .font(.system(size: 20, weight: selected ? .semibold : .regular))
            .foregroundStyle(selected ? TomiColor.brand : TomiColor.text)
            .frame(width: 48, height: 48)
            .background(selected ? TomiColor.barSelected : Color.clear, in: Circle())
            .contentShape(Circle())
            .animation(.easeOut(duration: TomiMotion.short), value: selected)
    }
}

/// A button of the capsule.
struct FloatingBarButton: View {
    let icon: TomiIcon
    let label: LocalizedStringKey
    var selected = false
    let action: () -> Void

    var body: some View {
        Button(action: action) { FloatingBarIcon(icon: icon, selected: selected) }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(label))
            .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// The square primary button ("Envoyer" on the feed, "Copier" on the detail): `brand` glass on iOS 26 and later (like
/// `.glassProminent`, but square with the radius 12 of the charter), plain `brand` before.
struct FloatingPrimaryButton: View {
    let icon: TomiIcon
    let label: LocalizedStringKey
    /// Off (nothing to send yet): plain glass and a muted icon, so it does not call for a touch.
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            let glyph = icon.image
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(enabled ? TomiColor.onBrand : TomiColor.textMuted)
                .frame(width: 56, height: 56)
            if #available(iOS 26.0, *) {
                glyph.glassEffect(
                    enabled ? .regular.tint(TomiColor.brand).interactive() : .regular,
                    in: RoundedRectangle(cornerRadius: TomiRadius.large)
                )
            } else {
                glyph
                    .background(enabled ? TomiColor.brand : TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.large))
                    .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
            }
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(Text(label))
        .animation(.easeOut(duration: TomiMotion.short), value: enabled)
    }
}
