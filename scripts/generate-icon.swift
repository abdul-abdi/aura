#!/usr/bin/swift
// generate-icon.swift — Creates Aura's app icon as an .icns file
// Design: Dark surface with a luminous green orb and radiating glow
// Run: swift scripts/generate-icon.swift <output-dir>

import AppKit
import CoreGraphics
import Foundation

func renderIcon(size: Int) -> NSImage {
    let s = CGFloat(size)
    let img = NSImage(size: NSSize(width: s, height: s))
    img.lockFocus()

    guard NSGraphicsContext.current?.cgContext != nil else {
        img.unlockFocus()
        return img
    }

    // ── Background: deep dark blue-black ──
    let bgDark = NSColor(red: 0.05, green: 0.06, blue: 0.10, alpha: 1.0)
    let bgMid = NSColor(red: 0.08, green: 0.09, blue: 0.16, alpha: 1.0)

    // macOS-style continuous rounded rectangle (squircle)
    let cornerRadius = s * 0.22
    let iconRect = NSRect(x: 0, y: 0, width: s, height: s)
    let path = NSBezierPath(roundedRect: iconRect, xRadius: cornerRadius, yRadius: cornerRadius)
    path.addClip()

    // Radial gradient background: slightly lighter at center
    let bgGradient = NSGradient(
        colorsAndLocations:
            (bgMid, 0.0),
            (bgDark, 0.7),
            (NSColor(red: 0.03, green: 0.03, blue: 0.06, alpha: 1.0), 1.0)
    )!
    bgGradient.draw(
        fromCenter: NSPoint(x: s * 0.5, y: s * 0.5), radius: 0,
        toCenter: NSPoint(x: s * 0.5, y: s * 0.5), radius: s * 0.7,
        options: [.drawsBeforeStartingLocation, .drawsAfterEndingLocation]
    )

    // ── Subtle grid/texture overlay ──
    let gridColor = NSColor(white: 1.0, alpha: 0.012)
    gridColor.setStroke()
    let gridSpacing = s / 16
    for i in 1..<16 {
        let p = CGFloat(i) * gridSpacing
        let hLine = NSBezierPath()
        hLine.move(to: NSPoint(x: 0, y: p))
        hLine.line(to: NSPoint(x: s, y: p))
        hLine.lineWidth = 0.5
        hLine.stroke()
        let vLine = NSBezierPath()
        vLine.move(to: NSPoint(x: p, y: 0))
        vLine.line(to: NSPoint(x: p, y: s))
        vLine.lineWidth = 0.5
        vLine.stroke()
    }

    let center = NSPoint(x: s * 0.5, y: s * 0.5)
    let orbRadius = s * 0.15

    // ── Outer aura rings (3 concentric, very faint) ──
    for i in 0..<3 {
        let ringRadius = orbRadius * (2.5 + CGFloat(i) * 1.2)
        let alpha = 0.06 - CGFloat(i) * 0.018
        let ringColor = NSColor(red: 0.30, green: 0.88, blue: 0.52, alpha: alpha)
        ringColor.setStroke()
        let ring = NSBezierPath(
            ovalIn: NSRect(
                x: center.x - ringRadius, y: center.y - ringRadius,
                width: ringRadius * 2, height: ringRadius * 2
            )
        )
        ring.lineWidth = s * 0.003
        ring.stroke()
    }

    // ── Wide ambient glow (large soft radial) ──
    let glowGradient = NSGradient(
        colorsAndLocations:
            (NSColor(red: 0.25, green: 0.85, blue: 0.48, alpha: 0.18), 0.0),
            (NSColor(red: 0.20, green: 0.70, blue: 0.40, alpha: 0.08), 0.3),
            (NSColor(red: 0.15, green: 0.50, blue: 0.30, alpha: 0.02), 0.6),
            (NSColor(red: 0.10, green: 0.30, blue: 0.20, alpha: 0.0), 1.0)
    )!
    glowGradient.draw(
        fromCenter: center, radius: 0,
        toCenter: center, radius: s * 0.45,
        options: []
    )

    // ── Inner bright glow ──
    let innerGlow = NSGradient(
        colorsAndLocations:
            (NSColor(red: 0.50, green: 1.0, blue: 0.70, alpha: 0.35), 0.0),
            (NSColor(red: 0.30, green: 0.88, blue: 0.52, alpha: 0.15), 0.5),
            (NSColor(red: 0.30, green: 0.88, blue: 0.52, alpha: 0.0), 1.0)
    )!
    innerGlow.draw(
        fromCenter: center, radius: 0,
        toCenter: center, radius: orbRadius * 2.5,
        options: []
    )

    // ── The orb: solid green circle with bright center ──
    let orbGradient = NSGradient(
        colorsAndLocations:
            (NSColor(red: 0.65, green: 1.0, blue: 0.78, alpha: 1.0), 0.0),   // bright center
            (NSColor(red: 0.35, green: 0.92, blue: 0.55, alpha: 1.0), 0.4),   // main green
            (NSColor(red: 0.25, green: 0.80, blue: 0.45, alpha: 1.0), 0.8),   // edge
            (NSColor(red: 0.20, green: 0.70, blue: 0.38, alpha: 0.9), 1.0)    // soft edge
    )!
    orbGradient.draw(
        fromCenter: NSPoint(x: center.x - orbRadius * 0.2, y: center.y + orbRadius * 0.2),
        radius: 0,
        toCenter: center,
        radius: orbRadius,
        options: []
    )

    // ── Specular highlight on orb (top-left) ──
    let specGradient = NSGradient(
        colorsAndLocations:
            (NSColor(white: 1.0, alpha: 0.45), 0.0),
            (NSColor(white: 1.0, alpha: 0.0), 1.0)
    )!
    let specCenter = NSPoint(x: center.x - orbRadius * 0.3, y: center.y + orbRadius * 0.3)
    specGradient.draw(
        fromCenter: specCenter, radius: 0,
        toCenter: specCenter, radius: orbRadius * 0.6,
        options: []
    )

    // ── Subtle bottom edge reflection ──
    let reflectionY = s * 0.12
    let reflGradient = NSGradient(
        colorsAndLocations:
            (NSColor(red: 0.30, green: 0.88, blue: 0.52, alpha: 0.04), 0.0),
            (NSColor(red: 0.30, green: 0.88, blue: 0.52, alpha: 0.0), 1.0)
    )!
    reflGradient.draw(
        fromCenter: NSPoint(x: center.x, y: reflectionY), radius: 0,
        toCenter: NSPoint(x: center.x, y: reflectionY), radius: s * 0.3,
        options: []
    )

    img.unlockFocus()
    return img
}

func savePNG(_ image: NSImage, to path: String, pixelSize: Int) {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: pixelSize,
        pixelsHigh: pixelSize,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    )!
    rep.size = image.size

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    image.draw(in: NSRect(x: 0, y: 0, width: image.size.width, height: image.size.height))
    NSGraphicsContext.restoreGraphicsState()

    let data = rep.representation(using: .png, properties: [:])!
    try! data.write(to: URL(fileURLWithPath: path))
}

// ── Main ──
let args = CommandLine.arguments
guard args.count > 1 else {
    print("Usage: swift generate-icon.swift <output-directory>")
    exit(1)
}

let outputDir = args[1]
let iconsetDir = outputDir + "/AppIcon.iconset"
try! FileManager.default.createDirectory(atPath: iconsetDir, withIntermediateDirectories: true)

let sizes: [(Int, Int)] = [
    (16, 1), (16, 2),
    (32, 1), (32, 2),
    (128, 1), (128, 2),
    (256, 1), (256, 2),
    (512, 1), (512, 2),
]

for (size, scale) in sizes {
    let px = size * scale
    let suffix = scale == 2 ? "@2x" : ""
    let filename = "icon_\(size)x\(size)\(suffix).png"
    let image = renderIcon(size: px)
    savePNG(image, to: iconsetDir + "/" + filename, pixelSize: px)
    print("  Generated \(filename) (\(px)x\(px))")
}

// Convert iconset to icns
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconsetDir, "-o", outputDir + "/AppIcon.icns"]
try! task.run()
task.waitUntilExit()

// Clean up iconset
try? FileManager.default.removeItem(atPath: iconsetDir)

print("  Icon saved to \(outputDir)/AppIcon.icns")
