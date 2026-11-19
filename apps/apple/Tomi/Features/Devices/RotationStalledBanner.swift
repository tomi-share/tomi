import SwiftUI
import TomiDesign

/// Under the header of the feed and on the devices screen while the removal of a device is not finished: the key
/// rotation that follows it keeps failing (`CoreState.rotationStalled`), so the removed device may still hold the
/// current key. Calm on purpose (nothing to do, Tomi keeps trying): no error color, no action, no secret content.
struct RotationStalledBanner: View {
    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s1) {
            Text("ios_rotation_stalled_title")
                .tomiText(.label)
                .foregroundStyle(TomiColor.text)
            Text("ios_rotation_stalled_body")
                .tomiText(.caption)
                .foregroundStyle(TomiColor.textMuted)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, TomiSpace.s4)
        .padding(.vertical, TomiSpace.s3)
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
        .accessibilityElement(children: .combine)
    }
}
