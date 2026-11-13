import AppKit

@MainActor
final class ShortcutSheet: Surface {
  var dismiss: (() -> Void)?
  private var activity = false
  private let sectionEntries = [
    ("⌘1 … ⌘4", "Search, Folders, Activity, Transfers"),
    ("⌘,", "Settings"),
    ("⌘K / ⌘⇧P", "Commands"),
  ]
  private let fileEntries = [
    ("⌘P", "Search This Mac"),
    ("↑ ↓  /  j k", "Select file"),
    ("← →  /  h l", "Parent folder / open folder"),
    ("⇧↑ ⇧↓  /  ⇧J ⇧K", "Extend selection in Folders"),
    ("⌘A", "Select all in Folders"),
    ("⌃D / ⌃U", "Half page of results"),
    ("gg / G", "First / last result"),
    ("/ or i", "Focus search"),
    ("$", "Search the current folder"),
    ("↵", "Open file / enter folder"),
    ("⌘↵", "Show in Folders"),
    ("Space", "Preview file"),
    ("Esc Esc", "Close the reader"),
    ("⌘[ / ⌘]", "Back / forward in Folders"),
    ("⌘L", "Go to a folder path"),
    ("⌘R", "Refresh"),
    ("⌘?", "Toggle this sheet"),
  ]
  private var entries: [(String, String)] {
    guard activity else { return sectionEntries + fileEntries }
    return sectionEntries + [
      ("↑ ↓ / j k", "Select process or box"),
      ("Tab", "Switch process list / map"),
      ("↵ / l / →", "Expand group / explore box"),
      ("h / ←", "Collapse group / go back"),
      ("gg / G", "First / last item"),
      ("⌃D / ⌃U", "Half page of items"),
      ("/ or i", "Focus process filter"),
      ("Esc", "Leave filter / go back"),
      ("Space", "Pause / resume sampling"),
      ("1 / 2 / 3", "Map memory / CPU / disk"),
      ("m / c / d", "Sort memory / CPU / disk"),
      ("Repeat sort key", "Reverse sort order"),
      ("⌘R", "Refresh sample"),
      ("⌘I", "Inspect process / group"),
      ("⌘⌫", "Quit selected process…"),
      ("⌘?", "Toggle this sheet"),
    ]
  }
  var preferredHeight: CGFloat { 66 + CGFloat(entries.count) * 21 }
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    layer?.cornerRadius = 12
    layer?.masksToBounds = true
    setAccessibilityLabel("Keyboard shortcuts")
  }
  required init?(coder: NSCoder) { nil }
  func refreshTheme(activity: Bool = false) {
    self.activity = activity
    subviews.filter { !($0 is NSVisualEffectView) }.forEach { $0.removeFromSuperview() }
    fill = Design.background
    applyMaterial(.popover)
    layer?.borderWidth = 1
    layer?.borderColor = Design.line.cgColor
    let title = label("Keyboard shortcuts", size: 14, weight: .semibold)
    title.frame = NSRect(x: 18, y: 17, width: 260, height: 22)
    addSubview(title)
    let close = ActionButton("", symbol: "xmark", help: "Close shortcuts") { [weak self] in
      self?.dismiss?()
    }
    close.frame = NSRect(x: 302, y: 13, width: 28, height: 28)
    addSubview(close)
    for (index, entry) in entries.enumerated() {
      let y = 53 + CGFloat(index) * 21
      let keys = label(entry.0, size: 11, weight: .medium)
      keys.frame = NSRect(x: 18, y: y, width: 104, height: 18)
      let action = label(entry.1, size: 11, color: Design.muted)
      action.frame = NSRect(x: 126, y: y, width: 205, height: 18)
      addSubview(keys)
      addSubview(action)
    }
  }
}
