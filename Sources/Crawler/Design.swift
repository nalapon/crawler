import AppKit
import UniformTypeIdentifiers
import CoreText
import QuartzCore

@MainActor
enum Design {
  static var theme: Theme = UserDefaults.standard.string(forKey: "theme").flatMap(Theme.init(rawValue:)) ?? .cupertino
  static var classic: Bool { theme == .system7 }
  static func color(_ hex: UInt32, alpha: CGFloat = 1) -> NSColor {
    NSColor(
      srgbRed: CGFloat((hex >> 16) & 255) / 255, green: CGFloat((hex >> 8) & 255) / 255,
      blue: CGFloat(hex & 255) / 255, alpha: alpha)
  }
  static var palette: ThemePalette { theme.palette }
  static var dark: Bool { palette.dark }
  static var square: Bool { theme == .system7 || theme == .windowsRetro || theme == .linuxRetro || theme == .terminal }
  /// Windows Retro and Linux Retro draw controls with raised and pressed bevels.
  static var bevelled: Bool { theme == .windowsRetro || theme == .linuxRetro }
  /// Draws a two-pixel classic bevel inside `rect` of a flipped view. Sunken swaps light and dark edges.
  static func drawBevel(_ rect: NSRect, sunken: Bool) {
    let unix = theme == .linuxRetro
    let light = color(unix ? 0xf5f4ee : 0xffffff), innerLight = color(unix ? 0xdddcd3 : 0xdfdfdf)
    let dark = color(unix ? 0x4c5249 : 0x0a0a0a), innerDark = color(unix ? 0x8f948a : 0x808080)
    func edges(_ r: NSRect, _ topLeft: NSColor, _ bottomRight: NSColor) {
      topLeft.setFill()
      NSRect(x: r.minX, y: r.minY, width: r.width, height: 1).fill()
      NSRect(x: r.minX, y: r.minY, width: 1, height: r.height).fill()
      bottomRight.setFill()
      NSRect(x: r.minX, y: r.maxY - 1, width: r.width, height: 1).fill()
      NSRect(x: r.maxX - 1, y: r.minY, width: 1, height: r.height).fill()
    }
    edges(rect, sunken ? dark : light, sunken ? light : dark)
    edges(rect.insetBy(dx: 1, dy: 1), sunken ? innerDark : innerLight, sunken ? innerLight : innerDark)
  }
  static var background: NSColor { color(palette.background) }
  static var text: NSColor { color(palette.text) }
  static var muted: NSColor { color(palette.muted) }
  static var line: NSColor { color(palette.line) }
  static var raised: NSColor { color(palette.raised) }
  static var sidebar: NSColor { color(palette.sidebar) }
  static var accent: NSColor { color(palette.accent) }
  static var selection: NSColor { color(palette.selection) }
  static var selectionText: NSColor { color(palette.selectionText) }
  static var accentText: NSColor { color(palette.accentText) }
  static func applyAppearance() {
    NSApp.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    for window in NSApp.windows { window.backgroundColor = background }
  }
  private static var classicFontsLoaded = false
  static func loadClassicFonts() {
    guard !classicFontsLoaded else { return }
    for weight in ["Regular", "Medium", "SemiBold"] {
      if let url = Bundle.main.url(forResource: "IBMPlexMono-" + weight, withExtension: "ttf", subdirectory: "Fonts") {
        CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
      }
    }
    classicFontsLoaded = true
  }
  /// The interface face. Retro themes use the system faces of their era: Geneva for System 7,
  /// Tahoma for Windows Retro and Helvetica for Linux Retro. Terminal uses the bundled IBM Plex Mono.
  static func font(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
    let bold = weight.rawValue >= NSFont.Weight.semibold.rawValue
    switch theme {
    case .terminal:
      loadClassicFonts()
      let face = bold ? "SemiBold" : weight.rawValue >= NSFont.Weight.medium.rawValue ? "Medium" : "Regular"
      return NSFont(name: face == "Regular" ? "IBMPlexMono" : "IBMPlexMono-" + face, size: size) ?? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
    case .system7: return NSFont(name: "Geneva", size: size) ?? NSFont.systemFont(ofSize: size, weight: weight)
    case .windowsRetro: return NSFont(name: bold ? "Tahoma-Bold" : "Tahoma", size: size) ?? NSFont.systemFont(ofSize: size, weight: weight)
    case .linuxRetro: return NSFont(name: bold ? "Helvetica-Bold" : "Helvetica", size: size) ?? NSFont.systemFont(ofSize: size, weight: weight)
    default: return NSFont.systemFont(ofSize: size, weight: weight)
    }
  }
  /// The face for document text in previews and the reader: New York for System 7, Verdana for
  /// Windows Retro and Charter for Linux Retro; other themes read in their interface face.
  static func readingFont(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
    let bold = weight.rawValue >= NSFont.Weight.semibold.rawValue
    switch theme {
    case .system7:
      let system = NSFont.systemFont(ofSize: size, weight: weight)
      return system.fontDescriptor.withDesign(.serif).flatMap { NSFont(descriptor: $0, size: size) } ?? system
    case .windowsRetro: return NSFont(name: bold ? "Verdana-Bold" : "Verdana", size: size) ?? font(size, weight)
    case .linuxRetro: return NSFont(name: bold ? "Charter-Bold" : "Charter-Roman", size: size) ?? font(size, weight)
    default: return font(size, weight)
    }
  }
  static func symbol(_ name: String, size: CGFloat = 14) -> NSImage? {
    NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(
      .init(pointSize: size, weight: .regular))
  }
}

@MainActor
class Surface: NSView {
  var fill: NSColor = Design.background { didSet { needsDisplay = true } }
  var bottomLine = false
  private var visibilityTarget: Bool?
  private var visibilityGeneration = 0
  var isPresented: Bool { visibilityTarget ?? !isHidden }
  func transitionVisible(_ visible: Bool) {
    visibilityTarget = visible
    visibilityGeneration += 1
    let generation = visibilityGeneration
    wantsLayer = true
    if visible { isHidden = false }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0.12
      context.timingFunction = CAMediaTimingFunction(name: .easeOut)
      if visible && alphaValue == 1 { alphaValue = 0 }
      animator().alphaValue = visible ? 1 : 0
    } completionHandler: { [weak self] in
      Task { @MainActor [weak self] in
        guard let self, self.visibilityGeneration == generation else { return }
        self.isHidden = !visible
        self.visibilityTarget = nil
      }
    }
  }
  private var materialView: NSVisualEffectView?
  func applyMaterial(_ material: NSVisualEffectView.Material,
    blendingMode: NSVisualEffectView.BlendingMode = .behindWindow) {
    guard Design.theme == .cupertino else {
      materialView?.removeFromSuperview()
      materialView = nil
      return
    }
    let view = materialView ?? NSVisualEffectView()
    view.material = material
    view.blendingMode = blendingMode
    view.state = .followsWindowActiveState
    view.frame = bounds
    view.autoresizingMask = [.width, .height]
    if view.superview !== self { addSubview(view, positioned: .below, relativeTo: nil) }
    materialView = view
    fill = .clear
  }
  override var isFlipped: Bool { true }
  override func draw(_ dirtyRect: NSRect) {
    fill.setFill()
    bounds.fill()
    if bottomLine {
      Design.line.setFill()
      NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }
  }
}
/// The title strip of a pane. The active pane, where the keyboard goes, is marked the way each
/// theme marks an active window: pinstripes in System 7, a navy or green bar in the retro themes,
/// an inverted bar in Terminal and an accent dot elsewhere.
@MainActor
final class PaneTitle: NSView {
  static let height: CGFloat = 30
  var title = "" { didSet { needsDisplay = true; setAccessibilityLabel(title) } }
  var subtitle = "" { didSet { needsDisplay = true } }
  var active = false { didSet { needsDisplay = true } }
  /// Clicking the strip, for example to browse the folder a pane shows.
  var click: (() -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    setAccessibilityElement(true)
    setAccessibilityRole(.staticText)
  }
  required init?(coder: NSCoder) { nil }
  override var isFlipped: Bool { true }
  override func mouseUp(with event: NSEvent) {
    if bounds.contains(convert(event.locationInWindow, from: nil)) { click?() }
  }
  override func draw(_ dirtyRect: NSRect) {
    let theme = Design.theme
    var background = Design.raised, foreground = active ? Design.text : Design.muted
    switch theme {
    case .system7: background = .white; foreground = active ? .black : Design.color(0x808080)
    case .windowsRetro: background = Design.color(0x808080); foreground = active ? .white : Design.color(0xd4d0c8)
    case .linuxRetro: background = active ? Design.accent : Design.raised; foreground = active ? .white : Design.muted
    case .terminal: background = active ? Design.accent : Design.sidebar; foreground = active ? Design.background : Design.muted
    default: break
    }
    background.setFill()
    bounds.fill()
    if theme == .windowsRetro && active {
      NSGradient(starting: Design.color(0x000080), ending: Design.color(0x1084d0))?.draw(in: bounds, angle: 0)
    }
    let attributes: [NSAttributedString.Key: Any] = [.font: Design.font(12, .semibold)]
    let titleWidth = ceil((title as NSString).size(withAttributes: attributes).width)
    let subtitleWidth = subtitle.isEmpty ? 0 : ceil((subtitle as NSString).size(withAttributes: [.font: Design.font(11)]).width) + 8
    var x: CGFloat = 12
    if !Design.square {
      (active ? Design.accent : Design.line).setFill()
      NSBezierPath(ovalIn: NSRect(x: x, y: bounds.midY - 3, width: 6, height: 6)).fill()
      x += 14
    }
    let available = max(0, bounds.width - x - 12)
    let titleRoom = min(titleWidth, available)
    let subtitleRoom = min(subtitleWidth, max(0, available - titleRoom))
    if theme == .system7 && active {
      NSColor.black.setFill()
      for y in stride(from: CGFloat(8), to: bounds.height - 7, by: 2) {
        NSRect(x: 4, y: y, width: bounds.width - 8, height: 1).fill()
      }
      NSColor.white.setFill()
      NSRect(x: 4, y: 4, width: x - 4 + titleRoom + subtitleRoom + 6, height: bounds.height - 8).fill()
    }
    let y = (bounds.height - 16) / 2
    drawText(title, in: NSRect(x: x, y: y, width: titleRoom, height: 16), size: 12, color: foreground, weight: .semibold)
    if subtitleRoom > 8 {
      drawText(subtitle, in: NSRect(x: x + titleRoom + 8, y: y + 1, width: subtitleRoom - 8, height: 16), size: 11,
        color: foreground.withAlphaComponent(0.72))
    }
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
  }
}
@MainActor
func label(
  _ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor? = nil
) -> NSTextField {
  let field = NSTextField(labelWithString: text)
  field.font = Design.font(size, weight)
  field.textColor = color ?? Design.text
  field.lineBreakMode = .byTruncatingTail
  field.maximumNumberOfLines = 1
  return field
}
@MainActor
func drawText(
  _ text: String, in rect: NSRect, size: CGFloat, color: NSColor, weight: NSFont.Weight = .regular,
  align: NSTextAlignment = .left
) {
  let p = NSMutableParagraphStyle()
  p.lineBreakMode = .byTruncatingTail
  // Truncate instead of squeezing glyphs; tightening overlaps letters in the retro faces.
  p.tighteningFactorForTruncation = 0
  p.alignment = align
  (text as NSString).draw(
    in: rect,
    withAttributes: [.font: Design.font(size, weight), .foregroundColor: color, .paragraphStyle: p])
}
@MainActor
final class ActionButton: NSButton {
  var invoke: (() -> Void)?
  private var hoverArea: NSTrackingArea?
  private var hovered = false
  private var selectedFill: NSColor?
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let hoverArea { removeTrackingArea(hoverArea) }
    let area = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
      owner: self)
    addTrackingArea(area)
    hoverArea = area
  }
  override func mouseEntered(with event: NSEvent) {
    hovered = true
    needsDisplay = true
  }
  override func mouseExited(with event: NSEvent) {
    hovered = false
    needsDisplay = true
  }
  override func draw(_ dirtyRect: NSRect) {
    if let selectedFill {
      selectedFill.setFill()
      NSBezierPath(roundedRect: bounds, xRadius: Design.square ? 0 : 4,
        yRadius: Design.square ? 0 : 4).fill()
    }
    if isEnabled && (hovered || isHighlighted) {
      Design.text.withAlphaComponent(isHighlighted ? 0.13 : 0.065).setFill()
      NSBezierPath(
        roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: Design.square ? 0 : 5,
        yRadius: Design.square ? 0 : 5
      ).fill()
    }
    super.draw(dirtyRect)
  }
  init(
    _ title: String = "", symbol: String? = nil, help: String? = nil, action: @escaping () -> Void
  ) {
    super.init(frame: .zero)
    self.title = title
    invoke = action
    self.target = self
    self.action = #selector(performAction)
    isBordered = false
    bezelStyle = .regularSquare
    font = Design.font(11)
    contentTintColor = Design.muted
    alignment = .center
    cell?.wraps = false
    cell?.lineBreakMode = .byTruncatingTail
    if let symbol {
      image = Design.symbol(symbol)
      imagePosition = title.isEmpty ? .imageOnly : .imageLeft
      imageHugsTitle = true
    }
    toolTip = help ?? (title.isEmpty ? symbol : title)
    setAccessibilityLabel(help ?? title)
    focusRingType = .none
  }
  required init?(coder: NSCoder) { nil }
  @objc private func performAction() { invoke?() }
  func setSelected(_ selected: Bool, primary: Bool = false) {
    wantsLayer = true
    layer?.cornerRadius = Design.square ? 0 : 4
    selectedFill = selected ? (primary ? Design.accent : Design.selection) : nil
    layer?.backgroundColor = NSColor.clear.cgColor
    needsDisplay = true
    contentTintColor = selected ? (primary ? Design.accentText : Design.selectionText) : Design.muted
  }
}
