import SwiftUI
import TomiDesign
import TomiKit

// Shared by the send screen and the share extension (TomiShare compiles this file too).

/// The expiry choices allowed for this content, "Jamais" (the default) first (Android `ExpiryChips`).
struct ExpiryChips: View {
    let options: [ExpiryChoice]
    let selected: ExpiryChoice
    let onSelect: (ExpiryChoice) -> Void

    var body: some View {
        FlowLayout(spacing: TomiSpace.s2) {
            ForEach(options, id: \.self) { choice in
                TomiChip(LocalizedStringKey(choice.labelKey), icon: choice == selected ? .sent : nil, selected: choice == selected) {
                    onSelect(choice)
                }
            }
        }
    }
}

extension ExpiryChoice {
    var labelKey: String {
        switch self {
        case .never: "expiry_never"
        case .hour1: "expiry_hour1"
        case .day1: "expiry_day1"
        case .day7: "expiry_day7"
        case .day30: "expiry_day30"
        case .afterFirstRead: "expiry_after_read"
        }
    }
}

/// "Envoyé · Annuler" (Android `SentBanner`). The bar follows the deadline on the monotonic clock, the one the scheduler
/// waits for (never an animation, which Reduce Motion or a slow device would stretch), refreshed every 50 ms; the end of
/// the window belongs to the screen. "Options" gives the draft back like "Annuler", to change the switch or the
/// expiry.
struct SentBanner: View {
    let deadline: UndoDeadline
    var skippedFiles = 0
    var undoing = false
    let onUndo: () -> Void
    var onOptions: (() -> Void)?

    var body: some View {
        TomiFeedbackBanner {
            VStack(alignment: .leading, spacing: TomiSpace.s2) {
                TimelineView(.periodic(from: .now, by: 0.05)) { _ in
                    ProgressBar(fraction: Double(deadline.fraction(MonotonicClock.nowMs())))
                }
                HStack(spacing: TomiSpace.s2) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("banner_sent")
                        if skippedFiles > 0 {
                            Text(L10n.text("banner_files_skipped", skippedFiles)).tomiText(.caption)
                        }
                    }
                    Spacer(minLength: TomiSpace.s2)
                    Group {
                        Button(L10n.text("action_undo"), action: onUndo)
                            .frame(minHeight: 44)
                        Button(L10n.text("action_options"), action: onOptions ?? onUndo)
                            .frame(minHeight: 44)
                    }
                    .tomiText(.label)
                    .foregroundStyle(TomiColor.inverseBrand)
                    .disabled(undoing)
                    .opacity(undoing ? 0.5 : 1)
                }
                .buttonStyle(.plain)
                .tomiText(.body)
                .foregroundStyle(TomiColor.onInverseSurface)
            }
        }
    }

    private struct ProgressBar: View {
        let fraction: Double

        var body: some View {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(TomiColor.onInverseSurface.opacity(0.2))
                    Capsule().fill(TomiColor.inverseBrand).frame(width: geo.size.width * fraction)
                }
            }
            .frame(height: 4)
            .accessibilityHidden(true)
        }
    }
}

/// Lays its children in rows, wrapping to the next row when the width is full.
struct FlowLayout: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(width: proposal.width ?? .infinity, subviews: subviews)
        let width = rows.map { $0.width }.max() ?? 0
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(rows.count - 1, 0))
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(width: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var indices: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            let needed = rows[rows.count - 1].indices.isEmpty ? size.width : rows[rows.count - 1].width + spacing + size.width
            if needed > width && !rows[rows.count - 1].indices.isEmpty {
                rows.append(Row())
            }
            var row = rows[rows.count - 1]
            row.width = row.indices.isEmpty ? size.width : row.width + spacing + size.width
            row.height = max(row.height, size.height)
            row.indices.append(index)
            rows[rows.count - 1] = row
        }
        return rows.filter { !$0.indices.isEmpty }
    }
}
