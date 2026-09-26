import AppKit

@MainActor
final class ShortcutSheet: Surface {
  var dismiss: (() -> Void)?
  private var activity = false
  private let fileEntries = [
    ("↑ ↓  /  j k", "Select file"),
    ("← →  /  h l", "Previous / narrow scope"),
    ("⌃D / ⌃U", "Half page of results"),
    ("gg / G", "First / last result"),
    ("/ or i", "Focus search"),
    ("⌘↵", "Reveal parent folder"),
    ("⇧V", "Start / finish range selection"),
    ("x", "Mark individual item"),
    ("↵", "Open file / enter folder"),
    ("Esc Esc", "Close PDF reader"),
    ("Esc", "Cancel / close shortcuts"),
    ("Space", "Preview file"),
    ("⌘T", "New tab at current folder"),
    ("⌃Tab / ⌃⇧Tab", "Next / previous tab"),
    ("⌘⌥1…9", "Jump to tab 1–9"),
    ("⌘W", "Close current tab"),
    ("⌘⇧P", "Command palette"),
    ("⌘P", "Focus file search"),
    ("⌘R", "Refresh folder"),
    ("⌘L", "Edit folder path"),
    ("⌘B", "Toggle sidebar"),
    ("Hold ⌘", "Peek at sidebar shortcuts"),
    ("⌘1…4", "Favorites and locations"),
    ("⌘⇧1…5", "Utilities"),
    ("⌘?", "Toggle this sheet"),
  ]
  private var entries: [(String, String)] {
    guard activity else { return fileEntries }
    return [
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
      ("⌘T / ⌘W", "New / close tab"),
      ("⌃Tab / ⌃⇧Tab", "Next / previous tab"),
      ("⌘⌥1…9", "Jump to tab"),
      ("⌘⇧P", "Command palette"),
      ("⌘B", "Toggle sidebar"),
      ("⌘⇧1…5", "Utilities"),
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
