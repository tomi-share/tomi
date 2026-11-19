import SwiftUI
import TomiDesign

/// At the top of the feed while a demo is on screen: the data is made up. When the demo was entered from the real core,
/// "Quitter la démo" goes back to the welcome screen (`onLeave`); a core opened as a demo (debug builds) has no way out,
/// but is labelled all the same.
struct DemoBanner: View {
    let onLeave: (() -> Void)?

    var body: some View {
        HStack(spacing: TomiSpace.s3) {
            Text("demo_banner")
                .tomiText(.caption)
                .foregroundStyle(TomiColor.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let onLeave {
                Button(L10n.text("demo_leave"), action: onLeave)
                    .buttonStyle(.tomiText)
            }
        }
        .padding(.horizontal, TomiSpace.s4)
        .padding(.vertical, TomiSpace.s2)
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
    }
}
