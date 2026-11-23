#!/usr/bin/env swift
// Generates the app icon of the Mac app: TomiMac/Assets.xcassets/MacAppIcon.appiconset.
//
// The icon of the charter (design/tomi/logo-app.svg: night tile, sun symbol at 0.7) on the macOS icon grid: a
// 1024 canvas, the tile 824 wide in its centre with rounded corners and a soft shadow, as the system icons.
// The smallest size (16 px) is the pixel drawing of the charter (design/tomi/export/tomi-16.png, charte 2.4: never
// the big symbol scaled down under 32 px); from 32 px on, every size is drawn on the same grid.
//
// Usage: swift apps/apple/tools/mac-app-icon.swift   (deterministic output; run again if the charter changes)

import AppKit

let tools = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let root = tools.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
let export = root.appendingPathComponent("design/tomi/export")
let output = tools.deletingLastPathComponent().appendingPathComponent("TomiMac/Assets.xcassets/MacAppIcon.appiconset")

let night = CGColor(srgbRed: 0x24 / 255, green: 0x32 / 255, blue: 0x4A / 255, alpha: 1)
let sun = CGColor(srgbRed: 0xFF / 255, green: 0xC8 / 255, blue: 0x5C / 255, alpha: 1)

/// The icon at `pixels` (32 and more), drawn from the geometry of the charter.
func render(_ pixels: Int) -> Data {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    let scale = CGFloat(pixels) / 1024
    // The coordinates of the SVG: origin top left, y down.
    context.translateBy(x: 0, y: CGFloat(pixels))
    context.scaleBy(x: scale, y: -scale)

    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    // 22 % of the side, as the tile of the charter.
    let corner = tile.width * 0.22
    let tilePath = CGPath(roundedRect: tile, cornerWidth: corner, cornerHeight: corner, transform: nil)
    context.saveGState()
    // A shadow under the tile (device space: y up), as the icons of the system.
    context.setShadow(offset: CGSize(width: 0, height: -10 * scale), blur: 22 * scale, color: CGColor(gray: 0, alpha: 0.3))
    context.addPath(tilePath)
    context.setFillColor(night)
    context.fillPath()
    context.restoreGState()

    // The symbol in its 256 box (logo-app.svg: translate(39.8 38.4) scale(0.7)), the box mapped on the tile.
    let box = tile.width / 256
    context.translateBy(x: tile.minX, y: tile.minY)
    context.scaleBy(x: box, y: box)
    context.translateBy(x: 39.8, y: 38.4)
    context.scaleBy(x: 0.7, y: 0.7)
    let center = CGPoint(x: 136, y: 128)
    func point(_ degrees: Double, _ radius: Double) -> CGPoint {
        let angle = degrees * .pi / 180
        return CGPoint(x: center.x + radius * cos(angle), y: center.y + radius * sin(angle))
    }
    // The open ring: outer arc from 218 to 142 degrees through the right, inner arc back (TomiSymbol.ringOutline).
    let ring = stride(from: 218.0, through: 502.0, by: 0.5).map { point($0, 94) }
        + stride(from: 502.0, through: 218.0, by: -0.5).map { point($0, 70) }
    context.setFillColor(sun)
    context.addLines(between: ring)
    context.closePath()
    context.fillPath()
    // The pearl in the opening.
    context.fillEllipse(in: CGRect(x: 54 - 32, y: 128 - 32, width: 64, height: 64))

    let bitmap = NSBitmapImageRep(cgImage: context.makeImage()!)
    return bitmap.representation(using: .png, properties: [:])!
}

try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

// (points, scale, pixels)
let slots: [(Int, Int, Int)] = [
    (16, 1, 16), (16, 2, 32), (32, 1, 32), (32, 2, 64), (128, 1, 128), (128, 2, 256),
    (256, 1, 256), (256, 2, 512), (512, 1, 512), (512, 2, 1024),
]
var images: [String] = []
var written = Set<Int>()
for (points, factor, pixels) in slots {
    let name = "icon-\(pixels).png"
    if written.insert(pixels).inserted {
        let data: Data
        if pixels < 32 {
            data = try Data(contentsOf: export.appendingPathComponent("tomi-\(pixels).png"))
        } else {
            data = render(pixels)
        }
        try data.write(to: output.appendingPathComponent(name))
    }
    images.append("""
        {
          "filename" : "\(name)",
          "idiom" : "mac",
          "scale" : "\(factor)x",
          "size" : "\(points)x\(points)"
        }
    """)
}
let contents = """
{
  "images" : [
\(images.joined(separator: ",\n"))
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}

"""
try contents.write(to: output.appendingPathComponent("Contents.json"), atomically: true, encoding: .utf8)
let catalog = output.deletingLastPathComponent().appendingPathComponent("Contents.json")
if !FileManager.default.fileExists(atPath: catalog.path) {
    try "{\n  \"info\" : { \"author\" : \"xcode\", \"version\" : 1 }\n}\n".write(to: catalog, atomically: true, encoding: .utf8)
}
print("wrote \(output.path)")
