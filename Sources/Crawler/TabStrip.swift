import AppKit

@MainActor
final class TabStrip: NSScrollView {
  private let content = Surface()
  private var cards: [TabCard] = []
  private var selectedIndex = 0
  private var revealSelection = false
  var select: ((Int) -> Void)?
  var close: ((Int) -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    drawsBackground = false
    borderType = .noBorder
    automaticallyAdjustsContentInsets = false
    contentInsets = .init()
    hasHorizontalScroller = false
    hasVerticalScroller = false
    horizontalScrollElasticity = .allowed
    content.fill = .clear
    documentView = content
  }
  required init?(coder: NSCoder) { nil }
  func configure(titles: [String], selected: Int) {
    selectedIndex = selected
    content.subviews.forEach { $0.removeFromSuperview() }
    cards = titles.enumerated().map { index, title in
      let card = TabCard(title: title, selected: index == selected,
                         select: { [weak self] in self?.select?(index) },
                         close: titles.count > 1 ? { [weak self] in self?.close?(index) } : nil)
      card.frame = NSRect(x: CGFloat(index) * 184 + 4, y: 0, width: 176, height: 46)
      content.addSubview(card)
      return card
    }
    content.frame = NSRect(x: 0, y: 0, width: max(bounds.width, CGFloat(titles.count) * 184 + 4), height: 46)
    revealSelection = true
    needsLayout = true
  }
  override func layout() {
    super.layout()
    if revealSelection, cards.indices.contains(selectedIndex) {
      cards[selectedIndex].scrollToVisible(cards[selectedIndex].bounds.insetBy(dx: -4, dy: 0))
      revealSelection = false
    }
  }
}

@MainActor
private final class TabCard: Surface {
  private let selected: Bool
  init(title: String, selected: Bool, select: @escaping () -> Void, close: (() -> Void)?) {
    self.selected = selected
    super.init(frame: .zero)
    fill = .clear
    let button = ActionButton(title, symbol: "folder", help: title, action: select)
    button.alignment = .left
    button.font = Design.font(12, selected ? .semibold : .regular)
    button.contentTintColor = selected ? Design.text : Design.muted
    button.frame = NSRect(x: 14, y: selected ? 9 : 12, width: close == nil ? 148 : 124, height: 26)
    button.setAccessibilityValue(selected ? "selected" : "")
    addSubview(button)
    if let close {
      let closeButton = ActionButton("", symbol: "xmark", help: "Close \(title) tab", action: close)
      closeButton.image = Design.symbol("xmark", size: 9)
      closeButton.frame = NSRect(x: 143, y: selected ? 10 : 13, width: 23, height: 24)
      addSubview(closeButton)
    }
  }
  required init?(coder: NSCoder) { nil }
  override func draw(_ dirtyRect: NSRect) {
    guard selected else { return }
    let rect = NSRect(x: 1, y: 5, width: bounds.width - 2, height: 35)
    let path = NSBezierPath(roundedRect: rect, xRadius: Design.square ? 0 : 8, yRadius: Design.square ? 0 : 8)
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.14)
    shadow.shadowBlurRadius = 3
    shadow.shadowOffset = NSSize(width: 0, height: -1)
    shadow.set()
    Design.background.setFill()
    path.fill()
    NSGraphicsContext.restoreGraphicsState()
    Design.line.setStroke()
    path.lineWidth = 0.5
    path.stroke()
  }
}
