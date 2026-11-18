import SwiftUI
import TomiDesign

/// At the top of the feed while the demo entered from the real core is on screen: the data is made up, and "Quitter
/// la démo" goes back to the welcome screen.
struct DemoBanner: View {
    let onLeave: () -> Void

    var body: some View {
        HStack(spacing: TomiSpace.s3) {
            Text("demo_banner")
                .tomiText(.caption)
                .foregroundStyle(TomiColor.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(L10n.text("demo_leave"), action: onLeave)
                .buttonStyle(.tomiText)
        }
        .padding(.horizontal, TomiSpace.s4)
        .padding(.vertical, TomiSpace.s2)
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
    }
}
