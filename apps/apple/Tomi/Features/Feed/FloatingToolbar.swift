import SwiftUI
import TomiDesign
import TomiKit

/// The floating toolbar of the feed (Android Material 3 Expressive floating toolbar): search, filters, devices, and the
/// square "Envoyer" button in `brand` (radius 12, the one send button style). Never a tab bar. Horizontal at the bottom,
/// or vertical in the side strip of iPhone Duo (`ActionBarScaffold`).
struct FloatingToolbar: View {
    @Bindable var model: FeedViewModel
    @Binding var searching: Bool
    let axis: Axis
    let onDevices: () -> Void
    let onSend: () -> Void

    var body: some View {
        FloatingActionBar(axis: axis) {
            FloatingBarButton(icon: .search, label: "action_search", selected: searching) {
                if searching {
                    model.setQuery("")
                    searching = false
                } else {
                    searching = true
                }
            }
            // While searching, the chips under the field do the filtering: no filter button (the filter stays active).
            if !searching { filterMenu }
            FloatingBarButton(icon: .devices, label: "devices_title", action: onDevices)
        } primary: {
            FloatingPrimaryButton(icon: .send, label: "action_send", action: onSend)
        }
        .animation(.easeOut(duration: TomiMotion.short), value: searching)
    }

    private var filterMenu: some View {
        Menu {
            Picker(selection: $model.filter) {
                ForEach(FeedFilter.allCases, id: \.self) { filter in
                    Label(L10n.text(filter.labelKey), systemImage: filter.icon.systemName).tag(filter)
                }
            } label: {
                EmptyView()
            }
        } label: {
            FloatingBarIcon(icon: .filter, selected: model.filter != .all)
                .overlay(alignment: .topTrailing) {
                    if model.filter != .all {
                        Circle().fill(TomiColor.brand).frame(width: 8, height: 8).offset(x: -6, y: 6)
                    }
                }
        }
        // "Tous" first even when the menu opens upwards from the bar.
        .menuOrder(.fixed)
        .accessibilityLabel(Text("action_filters"))
    }
}

extension FeedFilter {
    var labelKey: String {
        switch self {
        case .all: "filter_all"
        case .links: "filter_links"
        case .text: "filter_text"
        case .files: "filter_files"
        case .secrets: "filter_secrets"
        case .archives: "filter_archives"
        }
    }

    var icon: TomiIcon {
        switch self {
        case .all: .devices
        case .links: .link
        case .text: .text
        case .files: .file
        case .secrets: .masked
        case .archives: .archive
        }
    }
}
