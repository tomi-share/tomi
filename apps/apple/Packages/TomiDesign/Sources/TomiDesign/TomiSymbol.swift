import SwiftUI

/// The Tomi symbol, "la boucle" (charter §2.3): an open ring and a pearl in its opening, drawn
/// from the reference geometry (viewBox 256: ring centre 136,128, radii 70 and 94, opening from
/// 142° to 218° with straight radial cuts; pearl centre 54,128, radius 32). Fill it with
/// `TomiColor.logo` (or `sun` on `night`). Valid from 32 pt: smaller sizes have pixel drawings
/// in design/tomi/ (logo-16/20/24.svg), never a scaled-down symbol (charter §2.4).
///
/// Never a loading indicator and never an action icon (charter §2.5, §6.3).
public struct TomiSymbol: Shape {
    public init() {}

    /// Points of the outline in the 256-unit box: outer arc 218° -> 142° through the right,
    /// cut, inner arc back, cut. Sampled every degree (error far below a pixel at any size).
    static func ringOutline() -> [CGPoint] {
        let center = CGPoint(x: 136, y: 128)
        func point(_ degrees: Double, _ radius: Double) -> CGPoint {
            let a = degrees * .pi / 180
            return CGPoint(x: center.x + radius * cos(a), y: center.y + radius * sin(a))
        }
        let outer = stride(from: 218.0, through: 502.0, by: 1).map { point($0, 94) }
        let inner = stride(from: 502.0, through: 218.0, by: -1).map { point($0, 70) }
        return outer + inner
    }

    public func path(in rect: CGRect) -> Path {
        let scale = min(rect.width, rect.height) / 256
        let origin = CGPoint(x: rect.midX - 128 * scale, y: rect.midY - 128 * scale)
        let transform = CGAffineTransform(translationX: origin.x, y: origin.y).scaledBy(x: scale, y: scale)
        var path = Path()
        path.addLines(Self.ringOutline())
        path.closeSubpath()
        path.addEllipse(in: CGRect(x: 54 - 32, y: 128 - 32, width: 64, height: 64))
        return path.applying(transform)
    }
}
