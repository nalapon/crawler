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
  static func font(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
    if classic || theme == .terminal || theme == .linuxRetro {
      loadClassicFonts()
      let face = weight.rawValue >= NSFont.Weight.semibold.rawValue ? "SemiBold" : weight.rawValue >= NSFont.Weight.medium.rawValue ? "Medium" : "Regular"
      return NSFont(name: face == "Regular" ? "IBMPlexMono" : "IBMPlexMono-" + face, size: size) ?? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
    }
    return NSFont.systemFont(ofSize: size, weight: weight)
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
@MainActor
final class BorderOverlay: NSView {
  var active = false { didSet { needsDisplay = true } }
  override var isFlipped: Bool { true }
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func draw(_ dirtyRect: NSRect) {
    guard active else { return }
    (Design.classic ? NSColor.black : Design.accent).setStroke()
    let path = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
    path.lineWidth = 2
    path.stroke()
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
