import AppKit

// Draws Resources/Crawler.png: a "C" made of six retro stripes on night blue, ending in a found
// point. Run from the repository root, then package it with make-icon.swift (see Resources/ICON.md).
let output = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "Resources/Crawler.png")
let size: CGFloat = 1024
guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
  samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
  let context = NSGraphicsContext(bitmapImageRep: rep) else { fatalError("Cannot create bitmap") }
NSGraphicsContext.current = context
let cg = context.cgContext
cg.translateBy(x: 0, y: size); cg.scaleBy(x: 1, y: -1) // y grows downward

func color(_ hex: UInt32) -> CGColor {
  CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: 1)
}
func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }

let night = (top: UInt32(0x1b2340), bottom: UInt32(0x0f1428)), cream: UInt32 = 0xf3ecdc
let stripes: [UInt32] = [0x61bb46, 0xfdb827, 0xf5821f, 0xe03a3e, 0x963d97, 0x009ddc]
let band: CGFloat = 30, radius: CGFloat = 250, center = point(512, 512), gap: CGFloat = 0.75

// macOS icon grid: an 824 pt tile with a soft drop shadow, filled with a night-blue gradient.
let tile = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824), cornerWidth: 186, cornerHeight: 186, transform: nil)
cg.saveGState()
cg.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: CGColor(gray: 0, alpha: 0.3))
cg.addPath(tile); cg.setFillColor(color(night.bottom)); cg.fillPath()
cg.restoreGState()
cg.saveGState()
cg.addPath(tile); cg.clip()
let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [color(night.top), color(night.bottom)] as CFArray, locations: [0, 1])!
cg.drawLinearGradient(gradient, start: point(0, 100), end: point(0, 924), options: [])

// The C: concentric arcs, one per stripe, open on the right.
for (i, stripe) in stripes.enumerated() {
  let r = radius + (CGFloat(stripes.count - 1) / 2 - CGFloat(i)) * band
  let arc = CGMutablePath()
  arc.addArc(center: center, radius: r, startAngle: -gap, endAngle: gap, clockwise: true)
  cg.addPath(arc); cg.setStrokeColor(color(stripe)); cg.setLineWidth(band + 1.5); cg.setLineCap(.butt); cg.strokePath()
}

// The found point, just past the C's lower end.
let dot = point(center.x + cos(gap) * radius + 40, center.y + sin(gap) * radius + 40), dotSize: CGFloat = 120
cg.saveGState()
cg.setShadow(offset: CGSize(width: 0, height: -6), blur: 14, color: CGColor(gray: 0, alpha: 0.28))
cg.addEllipse(in: CGRect(x: dot.x - dotSize / 2, y: dot.y - dotSize / 2, width: dotSize, height: dotSize))
cg.setFillColor(color(cream)); cg.fillPath()
cg.restoreGState()
let hole = dotSize * 0.46
cg.addEllipse(in: CGRect(x: dot.x - hole / 2, y: dot.y - hole / 2, width: hole, height: hole))
cg.setFillColor(color(night.bottom)); cg.fillPath()
cg.restoreGState()

NSGraphicsContext.current = nil
guard let data = rep.representation(using: .png, properties: [:]) else { fatalError("Cannot encode PNG") }
try data.write(to: output)
print(output.path)
