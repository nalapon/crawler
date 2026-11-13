import AppKit

/// Crawler's own settings: the theme and the default text editor. Changes apply at once and persist.
@MainActor
final class SettingsView: Surface {
  var chooseTheme: ((Theme) -> Void)?
  private let scroll = NSScrollView()
  private let content = Surface()
  private var editorPicker: EditorPicker?
  private var editorStatus: NSTextField?

  override init(frame: NSRect) {
    super.init(frame: frame)
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.horizontalScrollElasticity = .none
    scroll.documentView = content
    addSubview(scroll)
  }
  required init?(coder: NSCoder) { nil }

  func render() {
    fill = Design.background
    content.fill = Design.background
    content.subviews.forEach { $0.removeFromSuperview() }
    let width = max(560, min(900, bounds.width - 72))
    var y: CGFloat = 32
    func heading(_ title: String, _ detail: String) {
      let field = label(title, size: 15, weight: .semibold)
      field.frame = NSRect(x: 36, y: y, width: width, height: 22)
      content.addSubview(field)
      let text = label(detail, size: 12, color: Design.muted)
      text.frame = NSRect(x: 36, y: y + 26, width: width, height: 18)
      content.addSubview(text)
      y += 58
    }
    heading("Theme", "Colours, controls and lines for the whole window. Retro themes use square edges and classic bevels.")
    let columns = max(2, Int((width + 12) / 212))
    let tileWidth = (width - CGFloat(columns - 1) * 12) / CGFloat(columns)
    for (index, theme) in Theme.allCases.enumerated() {
      let tile = ThemeTile(theme: theme, selected: theme == Design.theme) { [weak self] in self?.chooseTheme?(theme) }
      tile.frame = NSRect(x: 36 + CGFloat(index % columns) * (tileWidth + 12), y: y + CGFloat(index / columns) * 76,
        width: tileWidth, height: 64)
      content.addSubview(tile)
    }
    y += CGFloat((Theme.allCases.count + columns - 1) / columns) * 76 + 30
    heading("Default text editor", "Code and plain text open with this app, as do files you send from the reader with E.")
    let picker = EditorPicker()
    picker.frame = NSRect(x: 32, y: y, width: 300, height: 30)
    content.addSubview(picker)
    editorPicker = picker
    let refresh = ActionButton("Refresh editors", symbol: "arrow.clockwise") { FileOpening.shared.refresh(force: true) }
    refresh.frame = NSRect(x: 344, y: y, width: 136, height: 30)
    content.addSubview(refresh)
    let status = label("", size: 11, color: Design.muted)
    status.frame = NSRect(x: 36, y: y + 42, width: width, height: 34)
    status.maximumNumberOfLines = 2
    status.lineBreakMode = .byWordWrapping
    content.addSubview(status)
    editorStatus = status
    y += 104
    heading("PDFs, Markdown and Word", "PDFs, Markdown and Word documents open in Crawler's reader. Images and video use the macOS default apps.")
    content.frame = NSRect(x: 0, y: 0, width: bounds.width, height: max(bounds.height, y + 24))
    updateEditors()
    FileOpening.shared.refresh()
  }
  func updateEditors() {
    editorPicker?.refresh()
    let opening = FileOpening.shared
    editorStatus?.stringValue = opening.scanning ? "Looking for installed text editors…"
      : opening.unavailablePreference ? "Your saved editor is unavailable. Using \(opening.selected?.name ?? "no editor"). Choose another editor above."
      : opening.editors.isEmpty ? "No text editor found. Install an editor and refresh the list."
      : "\(opening.editors.count) installed editors · This choice applies only in Crawler."
  }
  override func layout() {
    super.layout()
    scroll.frame = bounds
    content.frame.size.width = bounds.width
  }
}

/// A theme choice drawn with that theme's own palette: a miniature window beside its name.
@MainActor
private final class ThemeTile: NSView {
  private let theme: Theme
  private let selected: Bool
  private let choose: () -> Void
  private var hovered = false

  init(theme: Theme, selected: Bool, choose: @escaping () -> Void) {
    self.theme = theme
    self.selected = selected
    self.choose = choose
    super.init(frame: .zero)
    toolTip = "\(theme.rawValue) · \(theme.detail)"
    setAccessibilityElement(true)
    setAccessibilityRole(.radioButton)
    setAccessibilityLabel(theme.rawValue)
    setAccessibilityValue(selected)
  }
  required init?(coder: NSCoder) { nil }
  override var isFlipped: Bool { true }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
  }
  override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
  override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
  override func mouseUp(with event: NSEvent) {
    if bounds.contains(convert(event.locationInWindow, from: nil)) { choose() }
  }
  override func accessibilityPerformPress() -> Bool { choose(); return true }

  override func draw(_ dirtyRect: NSRect) {
    let radius: CGFloat = Design.square ? 0 : 8
    if selected || hovered {
      (selected ? Design.selection : Design.text.withAlphaComponent(0.05)).setFill()
      NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
    }
    if selected {
      (Design.classic ? NSColor.black : Design.accent).setStroke()
      let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
      outline.lineWidth = 2
      outline.stroke()
    }
    // The miniature: title bar, a list with a selected row and a reading pane.
    let p = theme.palette
    let mini = NSRect(x: 10, y: 10, width: 72, height: bounds.height - 20)
    let square = theme == .system7 || theme == .windowsRetro || theme == .linuxRetro || theme == .terminal
    let corner: CGFloat = square ? 0 : 4
    let window = NSBezierPath(roundedRect: mini, xRadius: corner, yRadius: corner)
    Design.color(p.background).setFill(); window.fill()
    NSGraphicsContext.saveGraphicsState()
    window.addClip()
    Design.color(p.sidebar).setFill()
    NSRect(x: mini.minX, y: mini.minY, width: mini.width, height: 10).fill()
    Design.color(p.selection).setFill()
    NSRect(x: mini.minX + 5, y: mini.minY + 3, width: 14, height: 5).fill()
    Design.color(p.line).setFill()
    NSRect(x: mini.minX, y: mini.minY + 10, width: mini.width, height: 1).fill()
    NSRect(x: mini.minX + 28, y: mini.minY + 11, width: 1, height: mini.height - 11).fill()
    Design.color(p.selection).setFill()
    NSRect(x: mini.minX + 3, y: mini.minY + 15, width: 22, height: 6).fill()
    Design.color(p.muted).withAlphaComponent(0.55).setFill()
    for row in 0..<3 { NSRect(x: mini.minX + 4, y: mini.minY + 25 + CGFloat(row) * 6, width: 18, height: 2).fill() }
    Design.color(p.text).withAlphaComponent(0.7).setFill()
    for row in 0..<4 { NSRect(x: mini.minX + 33, y: mini.minY + 16 + CGFloat(row) * 6, width: row == 3 ? 22 : 34, height: 2).fill() }
    Design.color(p.accent).setFill()
    NSRect(x: mini.minX + 33, y: mini.minY + 40, width: 12, height: 3).fill()
    NSGraphicsContext.restoreGraphicsState()
    (theme == .system7 ? NSColor.black : Design.color(p.line)).setStroke()
    window.lineWidth = 1
    window.stroke()
    let textColor = selected ? Design.selectionText : Design.text
    drawText(theme.rawValue, in: NSRect(x: 94, y: bounds.height / 2 - 18, width: bounds.width - 100, height: 18), size: 12, color: textColor, weight: .semibold)
    drawText(theme.detail, in: NSRect(x: 94, y: bounds.height / 2 + 1, width: bounds.width - 100, height: 16), size: 11,
      color: selected ? Design.selectionText.withAlphaComponent(0.8) : Design.muted)
  }
}
