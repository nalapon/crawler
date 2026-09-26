import AppKit

@MainActor
final class CommandPalette: Surface, NSTextFieldDelegate {
  struct Command {
    let id: String
    let title: String
    var aliases = ""
    var shortcut = ""
    var symbol = "command"
    let action: () -> Void
  }
  private enum Page: Equatable { case commands, themes, choices(String) }
  var commands: [Command] = []
  private var childCommands: [Command] = []
  private var recent: [String] { UserDefaults.standard.stringArray(forKey: "recentCommands") ?? [] }
  private var matches: [Command] {
    let source = page == .commands ? commands : childCommands
    let query = input.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    return source.enumerated().compactMap { index, command -> (Int, Int, Command)? in
      guard let score = Self.score(query, in: command.title).map({ $0 + 100 })
        ?? Self.score(query, in: command.aliases) else { return nil }
      let rank = query.isEmpty && page == .commands ? (recent.firstIndex(of: command.id) ?? (recent.count + index)) : -score
      return (rank, index, command)
    }.sorted { $0.0 == $1.0 ? $0.1 < $1.1 : $0.0 < $1.0 }.map { $0.2 }
  }
  // Small command catalogues need only subsequence matching, with contiguous matches preferred.
  private static func score(_ query: String, in title: String) -> Int? {
    let query = query.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current)
    let title = title.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current)
    if query.isEmpty { return 0 }
    if title.hasPrefix(query) { return 1000 }
    if title.contains(query) { return 500 }
    var cursor = title.startIndex
    for character in query where !character.isWhitespace {
      guard let index = title[cursor...].firstIndex(of: character) else { return nil }
      cursor = title.index(after: index)
    }
    return 100
  }
  func showChoices(_ title: String, commands: [Command]) {
    page = .choices(title)
    childCommands = commands
    input.stringValue = ""
    selection = 0
    refresh()
  }
  func showThemes() {
    page = .themes
    input.stringValue = ""
    selection = Theme.allCases.firstIndex(of: Design.theme) ?? 0
    refresh()
  }
  private func remember(_ id: String) {
    UserDefaults.standard.set(Array(([id] + recent.filter { $0 != id }).prefix(12)), forKey: "recentCommands")
  }
  func finish(_ action: () -> Void) {
    originalTheme = nil
    dismiss?()
    action()
  }
  private var page: Page = .commands
  private var selection = 0
  private let input = NSTextField()
  private let results = PaletteResults()
  private let scroll = NSScrollView()
  private var originalTheme: Theme?
  private let topRule = Surface(), bottomRule = Surface()
  private let caption = label("Commands", size: 11, weight: .medium)
  private let hint = label("Open command   ↵", size: 11, weight: .medium)
  private var choices: [Theme] {
    Theme.allCases.filter { Self.score(input.stringValue, in: $0.rawValue + " " + $0.detail) != nil }
  }
  var previewTheme: ((Theme) -> Void)?
  var confirmTheme: ((Theme) -> Void)?
  var dismiss: (() -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    layer?.cornerRadius = 18
    layer?.masksToBounds = true
    layer?.borderWidth = 1
    input.isBordered = false
    input.drawsBackground = false
    input.focusRingType = .none
    input.delegate = self
    input.setAccessibilityLabel("Command palette")
    results.hoverRow = { [weak self] index in
      guard let self, (self.page == .themes ? self.choices.count : self.matches.count) > index, self.selection != index else { return }
      self.selection = index
      self.previewSelection()
    }
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.hasHorizontalScroller = false
    scroll.horizontalScrollElasticity = .none
    scroll.documentView = results
    for view in [input, caption, scroll, hint, topRule, bottomRule] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }
  func present() {
    originalTheme = Design.theme
    Design.loadClassicFonts()
    page = .commands
    input.stringValue = ""
    selection = 0
    transitionVisible(true)
    refresh()
    window?.makeFirstResponder(input)
  }
  func refresh() {
    fill = Design.background
    applyMaterial(.popover)
    layer?.borderColor = Design.line.cgColor
    layer?.cornerRadius = Design.square ? 0 : 18
    input.font = Design.font(21)
    input.textColor = Design.text
    input.placeholderAttributedString = NSAttributedString(
      string: page == .commands ? "Type a command…" : "Filter choices…",
      attributes: [.foregroundColor: Design.muted, .font: Design.font(21)])
    switch page {
    case .commands: caption.stringValue = input.stringValue.isEmpty && !recent.isEmpty ? "Commands · recently used first" : "Commands"
    case .themes: caption.stringValue = "Themes · Esc to go back"
    case .choices(let title): caption.stringValue = title + " · Esc to go back"
    }
    caption.font = Design.font(11, .medium)
    caption.textColor = Design.muted
    hint.stringValue = page == .themes ? "Apply theme   ↵" : "Run command   ↵"
    hint.font = Design.font(11, .medium)
    hint.textColor = Design.text
    topRule.fill = Design.line
    bottomRule.fill = Design.line
    results.fill = .clear
    results.subviews.forEach { $0.removeFromSuperview() }
    let visible = matches
    let titles = page == .themes ? choices.map(\.rawValue) : visible.map(\.title)
    selection = min(selection, max(0, titles.count - 1))
    for (index, title) in titles.enumerated() {
      let row = ActionButton(title, symbol: page == .themes ? "circle.lefthalf.filled" : visible[index].symbol) { [weak self] in
        self?.selection = index
        self?.activate()
      }
      row.alignment = .left
      row.font = Design.font(15, .medium)
      row.image = Design.symbol(page == .themes ? "circle.lefthalf.filled" : visible[index].symbol, size: 19)
      row.setSelected(index == selection)
      row.contentTintColor = index == selection ? Design.selectionText : Design.text
      row.frame = NSRect(x: 0, y: CGFloat(index) * 46, width: bounds.width - 32, height: 42)
      results.addSubview(row)
      let detail = label(page == .themes ? (choices[index] == originalTheme ? "Current" : choices[index].detail) : visible[index].shortcut, size: 11, color: index == selection ? Design.selectionText : Design.muted)
      detail.alignment = .right
      detail.frame = NSRect(x: bounds.width - 196, y: CGFloat(index) * 46 + 13, width: 148, height: 18)
      results.addSubview(detail)
    }
    if titles.isEmpty {
      let message: String
      switch page {
      case .choices("Connected drives") where childCommands.isEmpty: message = "No connected drives"
      case .choices("Choose editor") where childCommands.isEmpty: message = "No installed editors support opening this folder."
      case .choices("Open with") where childCommands.isEmpty: message = "No installed apps support this file."
      default: message = "No matches"
      }
      let empty = label(message, size: 13, color: Design.muted)
      empty.frame = NSRect(x: 12, y: 12, width: bounds.width - 64, height: 22)
      results.addSubview(empty)
    }
    results.frame.size = NSSize(width: bounds.width - 32, height: CGFloat(max(1, titles.count)) * 46)
    needsLayout = true
    layoutSubtreeIfNeeded()
    results.scrollToVisible(NSRect(x: 0, y: CGFloat(selection) * 46, width: 1, height: 46))
    needsDisplay = true
  }
  override func layout() {
    super.layout()
    topRule.frame = NSRect(x: 0, y: 65, width: bounds.width, height: 1)
    bottomRule.frame = NSRect(x: 0, y: bounds.height - 42, width: bounds.width, height: 1)
    input.frame = NSRect(x: 24, y: 20, width: bounds.width - 48, height: 34)
    caption.frame = NSRect(x: 24, y: 77, width: bounds.width - 48, height: 20)
    scroll.frame = NSRect(x: 16, y: 103, width: bounds.width - 32, height: max(46, bounds.height - 153))
    hint.alignment = .right
    hint.frame = NSRect(x: 24, y: bounds.height - 31, width: bounds.width - 48, height: 20)
  }
  func close() {
    let restore = originalTheme
    originalTheme = nil
    if let restore { previewTheme?(restore) }
    dismiss?()
  }
  private func previewSelection() {
    selection = min(selection, max(0, (page == .themes ? choices.count : matches.count) - 1))
    if page == .themes, choices.indices.contains(selection), choices[selection] != Design.theme {
      previewTheme?(choices[selection])
    } else {
      refresh()
    }
  }
  func controlTextDidChange(_ obj: Notification) { selection = 0; previewSelection() }
  func handleKey(_ event: NSEvent) -> Bool {
    switch event.keyCode {
    case 53:
      if page == .commands { close() }
      else {
        page = .commands
        input.stringValue = ""
        selection = 0
        if let originalTheme { previewTheme?(originalTheme) }
        refresh()
      }
      return true
    case 36, 76: activate(); return true
    case 125: selection += 1; previewSelection(); return true
    case 126: selection = max(0, selection - 1); previewSelection(); return true
    default: return false
    }
  }
  private func activate() {
    if page == .themes {
      guard choices.indices.contains(selection) else { return }
      remember("theme")
      confirmTheme?(choices[selection])
      originalTheme = nil
      dismiss?()
    } else {
      let visible = matches
      guard visible.indices.contains(selection) else { return }
      let command = visible[selection]
      if page == .commands { remember(command.id) }
      command.action()
    }
  }
}

// Track actual pointer movement on the stable container, so rebuilding theme rows
// cannot override keyboard selection while the mouse stays still.
@MainActor
private final class PaletteResults: Surface {
  var hoverRow: ((Int) -> Void)?
  private var tracking: NSTrackingArea?
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let tracking { removeTrackingArea(tracking) }
    let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
    addTrackingArea(area)
    tracking = area
  }
  override func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }
  override func mouseMoved(with event: NSEvent) {
    let point = convert(event.locationInWindow, from: nil)
    if bounds.contains(point) { hoverRow?(Int(point.y / 46)) }
  }
}
