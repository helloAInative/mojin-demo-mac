#!/usr/bin/env swift
import AppKit
import Foundation

let outDir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "."
let size = 1024
let img = NSImage(size: NSSize(width: size, height: size))
img.lockFocus()
NSColor(calibratedRed: 0.07, green: 0.11, blue: 0.18, alpha: 1).setFill()
NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: size, height: size), xRadius: 220, yRadius: 220).fill()

let grad = NSGradient(colors: [
    NSColor(calibratedRed: 1, green: 0.75, blue: 0.15, alpha: 1),
    NSColor(calibratedRed: 1, green: 0.35, blue: 0.2, alpha: 1)
])!
grad.draw(in: NSBezierPath(ovalIn: NSRect(x: 220, y: 220, width: 584, height: 584)), angle: -45)

let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center
let attrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 420, weight: .black),
    .foregroundColor: NSColor.white,
    .paragraphStyle: paragraph
]
let text = "金" as NSString
text.draw(in: NSRect(x: 0, y: 240, width: size, height: 520), withAttributes: attrs)
img.unlockFocus()

guard let tiff = img.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else {
    fputs("failed to render\n", stderr)
    exit(1)
}

let iconset = (outDir as NSString).appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(atPath: iconset)
try FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)

func save(_ name: String, _ side: Int) throws {
    let target = NSImage(size: NSSize(width: side, height: side))
    target.lockFocus()
    img.draw(in: NSRect(x: 0, y: 0, width: side, height: side),
             from: .zero, operation: .copy, fraction: 1)
    target.unlockFocus()
    guard let t = target.tiffRepresentation,
          let r = NSBitmapImageRep(data: t),
          let p = r.representation(using: .png, properties: [:]) else { return }
    try p.write(to: URL(fileURLWithPath: (iconset as NSString).appendingPathComponent(name)))
}

let map: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024)
]
for (n, s) in map { try save(n, s) }

let icns = (outDir as NSString).appendingPathComponent("AppIcon.icns")
let proc = Process()
proc.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
proc.arguments = ["-c", "icns", iconset, "-o", icns]
try proc.run()
proc.waitUntilExit()
guard proc.terminationStatus == 0 else {
    fputs("iconutil failed\n", stderr)
    exit(1)
}
print(icns)
