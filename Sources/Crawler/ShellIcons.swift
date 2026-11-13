import AppKit

// Sections and places identify what an icon means; the theme owns how it looks.
enum ShellIcon: CaseIterable {
  case home, folder, downloads, desktop, pin, drive, airDrop, activity, preferences
  case add, search, eject

  var symbol: String {
    switch self {
    case .home: "house"
    case .folder: "folder"
    case .downloads: "arrow.down.circle"
    case .desktop: "desktopcomputer"
    case .pin: "pin"
    case .drive: "externaldrive"
    case .airDrop: "airplayaudio"
    case .activity: "waveform.path.ecg"
    case .preferences: "slider.horizontal.3"
    case .add: "plus"
    case .search: "magnifyingglass"
    case .eject: "eject"
    }
  }

  var filledSymbol: String {
    switch self {
    case .downloads: "arrow.down.circle.fill"
    case .pin: "pin.fill"
    case .drive: "externaldrive.fill"
    case .home: "house.fill"
    case .folder: "folder.fill"
    case .activity: "waveform.path.ecg.rectangle.fill"
    default: symbol
    }
  }
}

@MainActor
enum ShellIcons {
  private struct Key: Hashable {
    let icon: ShellIcon
    let theme: Theme
    let selected: Bool
  }
  // Bounded by the twelve roles, eleven themes, and two selection states.
  private static var images: [Key: NSImage] = [:]

  static func image(_ icon: ShellIcon, theme: Theme = Design.theme, selected: Bool = false) -> NSImage {
    let key = Key(icon: icon, theme: theme, selected: selected)
    if let image = images[key] { return image }
    let image = NSImage(size: NSSize(width: 20, height: 20))
    let drawing = Drawing(theme: theme, selected: selected)
    // Supply both display scales so pixel artwork stays crisp and vectors stay smooth.
    for scale in [1, 2] {
      guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 20 * scale,
        pixelsHigh: 20 * scale, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
        isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
        let context = NSGraphicsContext(bitmapImageRep: bitmap) else { continue }
      NSGraphicsContext.saveGraphicsState()
      NSGraphicsContext.current = context
      context.cgContext.scaleBy(x: CGFloat(scale) * 20 / 24, y: CGFloat(scale) * 20 / 24)
      context.cgContext.translateBy(x: 0, y: 24)
      context.cgContext.scaleBy(x: 1, y: -1)
      drawing.draw(icon)
      NSGraphicsContext.restoreGraphicsState()
      bitmap.size = image.size
      image.addRepresentation(bitmap)
    }
    image.isTemplate = false
    images[key] = image
    return image
  }

  @MainActor
  private struct Drawing {
    let theme: Theme
    let selected: Bool
    var retro: Bool { theme == .system7 || theme == .windowsRetro || theme == .linuxRetro }
    var outlineOnly: Bool { theme == .github || theme == .vercel || theme == .terminal }
    var ink: NSColor { Design.color(selected ? theme.palette.selectionText : theme.palette.text) }
    var accent: NSColor { Design.color(selected ? theme.palette.selectionText : theme.palette.accent) }
    var interiorInk: NSColor { selected || outlineOnly ? ink : Design.color(theme.palette.dark ? theme.palette.background : theme.palette.text) }
    var background: NSColor { Design.color(selected ? theme.palette.selection : theme.palette.sidebar) }
    var width: CGFloat { theme == .vercel ? 1.2 : retro ? 1 : 1.6 }
    var radius: CGFloat { retro || theme == .terminal ? 0 : theme == .catppuccin ? 3 : 1.5 }

    func stroke(_ points: [NSPoint], color: NSColor? = nil, close: Bool = false, fill: NSColor? = nil) {
      guard let first = points.first else { return }
      let path = NSBezierPath()
      path.move(to: first)
      for point in points.dropFirst() { path.line(to: point) }
      if close { path.close() }
      if let fill { fill.setFill(); path.fill() }
      (color ?? ink).setStroke()
      path.lineWidth = width
      path.lineJoinStyle = retro || theme == .terminal ? .miter : .round
      path.lineCapStyle = retro || theme == .terminal ? .butt : .round
      path.stroke()
    }
    func line(_ coordinates: [(CGFloat, CGFloat)], color: NSColor? = nil) {
      stroke(coordinates.map { NSPoint(x: $0.0, y: $0.1) }, color: color)
    }
    func box(_ rect: NSRect, fill: NSColor? = nil, color: NSColor? = nil, rounding: CGFloat? = nil) {
      let path = NSBezierPath(roundedRect: rect, xRadius: rounding ?? radius, yRadius: rounding ?? radius)
      if let fill { fill.setFill(); path.fill() }
      (color ?? ink).setStroke(); path.lineWidth = width; path.stroke()
    }
    func circle(_ rect: NSRect, fill: NSColor? = nil, color: NSColor? = nil) {
      let path = NSBezierPath(ovalIn: rect)
      if let fill { fill.setFill(); path.fill() }
      (color ?? ink).setStroke(); path.lineWidth = width; path.stroke()
    }
    func color(_ icon: ShellIcon) -> NSColor {
      if selected { return ink }
      switch theme {
      case .catppuccin:
        switch icon {
        case .downloads, .pin: return Design.color(0xf9e2af)
        case .desktop, .drive: return Design.color(0x89b4fa)
        case .airDrop: return Design.color(0x94e2d5)
        case .activity: return Design.color(0xa6e3a1)
        default: return Design.color(0xcba6f7)
        }
      case .windows, .windowsRetro: return Design.color(icon == .downloads || icon == .pin ? 0xe2b537 : 0x167ac6)
      case .linux: return Design.color(icon == .activity ? 0x8cda91 : 0xffa077)
      case .linuxRetro: return Design.color(icon == .downloads || icon == .pin ? 0xc29d59 : 0x405b56)
      case .system7: return .black
      default: return accent
      }
    }
    func fill(_ icon: ShellIcon) -> NSColor? {
      if outlineOnly { return nil }
      if selected { return background }
      if theme == .system7 { return .white }
      return color(icon)
    }
    func bevel(_ rect: NSRect) {
      guard theme == .windowsRetro || theme == .linuxRetro else { return }
      line([(rect.minX + 1, rect.maxY - 1), (rect.minX + 1, rect.minY + 1), (rect.maxX - 1, rect.minY + 1)], color: .white)
      line([(rect.maxX - 1, rect.minY + 2), (rect.maxX - 1, rect.maxY - 1), (rect.minX + 2, rect.maxY - 1)], color: Design.color(0x555555))
    }
    func draw(_ icon: ShellIcon) {
      NSGraphicsContext.current?.shouldAntialias = !retro && theme != .terminal
      if theme == .cupertino || theme == .dark {
        drawNative(icon)
        return
      }
      if theme == .linuxRetro {
        box(NSRect(x: 1, y: 1, width: 22, height: 22), fill: selected ? background : Design.color(0xb8b9af), rounding: 0)
        bevel(NSRect(x: 1, y: 1, width: 22, height: 22))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current?.cgContext.translateBy(x: 3, y: 3)
        NSGraphicsContext.current?.cgContext.scaleBy(x: 0.75, y: 0.75)
      }
      switch icon {
      case .home:
        let body = NSRect(x: 5, y: 10, width: 14, height: 11)
        box(body, fill: fill(icon), rounding: 0); bevel(body)
        stroke([NSPoint(x: 2, y: 12), NSPoint(x: 12, y: 3), NSPoint(x: 22, y: 12)], color: color(icon))
        box(NSRect(x: 10, y: 14, width: 4, height: 7), fill: background, rounding: 0)
      case .folder: folder(icon)
      case .downloads: drawDownloads()
      case .desktop:
        let rect = NSRect(x: 3, y: 4, width: 18, height: retro ? 13 : 12)
        box(rect, fill: fill(icon)); bevel(rect)
        if !outlineOnly { box(NSRect(x: 5, y: 6, width: 14, height: 8), fill: theme == .system7 ? .white : background, color: ink, rounding: 0) }
        line([(12, 17), (12, 20)]); line([(7, 21), (17, 21)])
        if theme == .windows { line([(6, 8), (10, 8)], color: color(icon)) }
      case .pin:
        if theme == .catppuccin || theme == .windows || theme == .linux || retro {
          folder(icon)
          line([(12, 10), (12, 18)], color: interiorInk)
          line([(9, 12), (15, 12)], color: interiorInk)
        } else {
          stroke([.init(x: 8, y: 3), .init(x: 17, y: 3), .init(x: 16, y: 9), .init(x: 19, y: 13), .init(x: 6, y: 13), .init(x: 9, y: 9)], close: true)
          line([(12, 13), (12, 21)])
        }
      case .drive:
        let rect = NSRect(x: 3, y: 5, width: 18, height: 15)
        box(rect, fill: fill(icon)); bevel(rect)
        line([(5, 15), (19, 15)], color: interiorInk)
        box(NSRect(x: 16, y: 17, width: 2, height: 1), fill: interiorInk, color: interiorInk, rounding: 0)
        if retro { line([(6, 8), (16, 8)]); line([(6, 10), (16, 10)]) }
      case .airDrop:
        if retro || theme == .terminal {
          line([(3, 11), (3, 7), (7, 3), (17, 3), (21, 7), (21, 11)], color: accent)
          line([(7, 12), (7, 9), (10, 6), (14, 6), (17, 9), (17, 12)], color: accent)
        } else if theme == .vercel {
          stroke([.init(x: 3, y: 18), .init(x: 12, y: 3), .init(x: 21, y: 18)], close: true)
        } else {
          circle(NSRect(x: 3, y: 3, width: 18, height: 18), color: color(icon))
          circle(NSRect(x: 7, y: 7, width: 10, height: 10), color: color(icon))
        }
        stroke([.init(x: 12, y: 12), .init(x: 7, y: 21), .init(x: 17, y: 21)], close: true, fill: outlineOnly ? background : color(icon))
      case .activity:
        if theme != .vercel { box(NSRect(x: 2, y: 4, width: 20, height: 16), fill: outlineOnly ? nil : background); bevel(NSRect(x: 2, y: 4, width: 20, height: 16)) }
        line([(4, 13), (7, 13), (10, 8), (13, 17), (16, 11), (20, 11)], color: color(icon))
      case .preferences:
        for (x, y): (CGFloat, CGFloat) in [(5, 8), (12, 16), (19, 10)] {
          line([(x, 3), (x, 21)], color: outlineOnly ? ink : color(icon))
          box(NSRect(x: x - 2, y: y - 2, width: 4, height: 4), fill: outlineOnly ? background : color(icon), rounding: theme == .catppuccin ? 2 : 0)
        }
      case .add:
        if theme == .catppuccin || theme == .linux { circle(NSRect(x: 3, y: 3, width: 18, height: 18), fill: background, color: color(icon)) }
        line([(12, 6), (12, 18)], color: color(icon)); line([(6, 12), (18, 12)], color: color(icon))
      case .search:
        if retro || theme == .terminal { box(NSRect(x: 3, y: 3, width: 12, height: 12), fill: fill(icon)) }
        else { circle(NSRect(x: 3, y: 3, width: 12, height: 12), fill: outlineOnly ? nil : background, color: color(icon)) }
        line([(14, 14), (21, 21)], color: color(icon))
      case .eject:
        stroke([.init(x: 4, y: 14), .init(x: 12, y: 4), .init(x: 20, y: 14)], close: true, fill: fill(icon))
        box(NSRect(x: 4, y: 18, width: 16, height: 3), fill: fill(icon), rounding: 0)
      }
      if theme == .linuxRetro { NSGraphicsContext.restoreGraphicsState() }
    }

    func folder(_ icon: ShellIcon) {
      let tone = color(icon)
      if theme == .catppuccin || theme == .linux {
        box(NSRect(x: 3, y: 4, width: 9, height: 6), fill: tone, color: tone)
        box(NSRect(x: 2, y: 7, width: 20, height: 14), fill: selected ? background : tone, color: tone)
      } else {
        stroke([(2, 20), (2, 4), (10, 4), (12, 7), (22, 7), (22, 20)].map { NSPoint(x: $0.0, y: $0.1) }, close: true, fill: fill(icon))
        if theme == .windows { stroke([.init(x: 2, y: 10), .init(x: 22, y: 10), .init(x: 20, y: 20), .init(x: 2, y: 20)], color: ink, close: true, fill: fill(icon)) }
        else { line([(3, 8), (21, 8)]); bevel(NSRect(x: 2, y: 8, width: 20, height: 12)) }
        if theme == .system7 {
          for x in stride(from: 4, through: 20, by: 2) { NSColor.black.setFill(); NSRect(x: x, y: 19, width: 1, height: 1).fill() }
        }
      }
    }
    func drawDownloads() {
      if theme == .github {
        line([(3, 14), (3, 21), (21, 21), (21, 14)])
      } else if theme == .vercel {
        line([(5, 21), (19, 21)])
      } else if theme == .terminal {
        line([(3, 7), (3, 3), (8, 3)], color: accent)
        line([(16, 3), (21, 3), (21, 7)], color: accent)
        line([(3, 17), (3, 21), (21, 21), (21, 17)], color: accent)
      } else { folder(.downloads) }
      let foreground = outlineOnly ? accent : selected ? ink : theme == .catppuccin || theme == .linux ? Design.color(theme.palette.background) : ink
      line([(12, outlineOnly ? 5 : 10), (12, 17)], color: foreground)
      line([(8, 13), (12, 17), (16, 13)], color: foreground)
    }
    func drawNative(_ icon: ShellIcon) {
      if theme == .dark, icon == .downloads {
        circle(NSRect(x: 2, y: 2, width: 20, height: 20), fill: accent, color: accent)
        line([(12, 6), (12, 17)], color: background)
        line([(8, 13), (12, 17), (16, 13)], color: background)
        return
      }
      let symbol = theme == .dark ? icon.filledSymbol : icon.symbol
      guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
        .withSymbolConfiguration(.init(pointSize: theme == .dark ? 17 : 18, weight: theme == .dark ? .semibold : .regular))?
        .withSymbolConfiguration(.init(paletteColors: [selected ? ink : theme == .dark ? Design.color(0x80b6ff) : Design.color(0x086cdc)])) else { return }
      // Symbols draw upright inside our top-down icon coordinate system.
      NSGraphicsContext.saveGraphicsState()
      NSGraphicsContext.current?.cgContext.translateBy(x: 0, y: 24)
      NSGraphicsContext.current?.cgContext.scaleBy(x: 1, y: -1)
      let size = image.size
      let factor = min(20 / size.width, 20 / size.height)
      image.draw(in: NSRect(x: (24 - size.width * factor) / 2, y: (24 - size.height * factor) / 2,
        width: size.width * factor, height: size.height * factor))
      NSGraphicsContext.restoreGraphicsState()
    }
  }
}
