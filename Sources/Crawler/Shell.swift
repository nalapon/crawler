import AppKit

/// The five top-level sections in the title bar. Search and Folders share the scope funnel;
/// each keeps its own funnel state while the other is visible.
enum Section: String, CaseIterable {
  case search = "Search", folders = "Folders", activity = "Activity", transfers = "Transfers", settings = "Settings"

  var icon: ShellIcon {
    switch self {
    case .search: .search
    case .folders: .folder
    case .activity: .activity
    case .transfers: .airDrop
    case .settings: .preferences
    }
  }
  /// The key that opens the section with Command, as shown in the View menu.
  var key: String {
    switch self {
    case .search: "1"
    case .folders: "2"
    case .activity: "3"
    case .transfers: "4"
    case .settings: ","
    }
  }
  static func forKey(_ key: String) -> Section? { allCases.first { $0.key == key } }
}

/// A labelled toolbar button drawn in the current theme: a tinted block in modern themes, an
/// inverted block in System 7 and a raised or pressed bevel in the Windows and Unix retro themes.
@MainActor
final class ShellButton: NSView {
  var invoke: () -> Void
  var selected = false {
    didSet { needsDisplay = true; setAccessibilitySelected(selected) }
  }
  /// Outlines the button while files are dragged over it.
  var dropTarget = false { didSet { needsDisplay = true } }
  let title: String
  private let icon: ShellIcon?
  private var hovered = false, pressed = false
  private var font: NSFont { Design.font(12, selected ? .semibold : .medium) }

  init(_ title: String, icon: ShellIcon?, help: String, invoke: @escaping () -> Void) {
    self.title = title
    self.icon = icon
    self.invoke = invoke
    super.init(frame: .zero)
    toolTip = help
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel(title)
    setAccessibilityHelp(help)
  }
  required init?(coder: NSCoder) { nil }
  override var isFlipped: Bool { true }
  override var mouseDownCanMoveWindow: Bool { false }
  var preferredWidth: CGFloat {
    ceil((title as NSString).size(withAttributes: [.font: Design.font(12, .semibold)]).width) + (icon == nil ? 22 : 44)
  }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
  }
  override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
  override func mouseExited(with event: NSEvent) { hovered = false; pressed = false; needsDisplay = true }
  override func mouseDown(with event: NSEvent) { pressed = true; needsDisplay = true }
  override func mouseUp(with event: NSEvent) {
    let inside = bounds.contains(convert(event.locationInWindow, from: nil))
    pressed = false
    needsDisplay = true
    if inside { invoke() }
  }
  override func accessibilityPerformPress() -> Bool { invoke(); return true }

  override func draw(_ dirtyRect: NSRect) {
    let radius: CGFloat = Design.square ? 0 : 6
    let fill: NSColor? = selected ? Design.selection
      : Design.bevelled ? Design.raised
      : hovered || pressed ? Design.text.withAlphaComponent(pressed ? 0.12 : 0.06) : nil
    if let fill {
      fill.setFill()
      NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
    }
    if Design.bevelled { Design.drawBevel(bounds, sunken: selected || pressed) }
    if dropTarget {
      Design.accent.setStroke()
      let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
      outline.lineWidth = 2
      outline.stroke()
    }
    let color = selected ? Design.selectionText : hovered || Design.square ? Design.text : Design.muted
    var x: CGFloat = 11
    if let icon {
      ShellIcons.image(icon, selected: selected).draw(in: NSRect(x: x - 1, y: (bounds.height - 18) / 2, width: 18, height: 18))
      x += 22
    }
    let height = font.ascender - font.descender
    drawText(title, in: NSRect(x: x, y: (bounds.height - height) / 2 - 1, width: bounds.width - x - 8, height: height + 2),
      size: 12, color: color, weight: selected ? .semibold : .medium)
  }
}

/// The title bar: section buttons after the window controls, then the Open menu,
/// the command palette and keyboard help on the right.
@MainActor
final class SectionBar: Surface {
  var choose: ((Section) -> Void)?
  var showCommands: (() -> Void)?
  var showHelp: (() -> Void)?
  let openButton = EditorOpenButton(frame: .zero)
  private var buttons: [(section: Section, button: ShellButton)] = []
  private let commands = ActionButton("", symbol: "command", help: "Commands · ⌘K") {}
  private let help = ActionButton("", symbol: "questionmark.circle", help: "Keyboard shortcuts · ⌘?") {}

  override init(frame: NSRect) {
    super.init(frame: frame)
    for section in Section.allCases {
      let button = ShellButton(section.rawValue, icon: section.icon, help: "\(section.rawValue) · ⌘\(section.key)") { [weak self] in
        self?.choose?(section)
      }
      buttons.append((section, button))
      addSubview(button)
    }
    commands.invoke = { [weak self] in self?.showCommands?() }
    help.invoke = { [weak self] in self?.showHelp?() }
    for view in [openButton, commands, help] { addSubview(view) }
    setAccessibilityElement(true)
    setAccessibilityRole(.tabGroup)
    setAccessibilityLabel("Sections")
  }
  required init?(coder: NSCoder) { nil }

  func configure(section: Section, selectedFile: FileItem?) {
    fill = Design.sidebar
    applyMaterial(.titlebar)
    for (candidate, button) in buttons { button.selected = candidate == section }
    openButton.isHidden = section != .search && section != .folders
    openButton.configure(selectedFile)
    for button in [commands, help] { button.contentTintColor = Design.muted }
    needsDisplay = true
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
  }
  override func layout() {
    super.layout()
    // The window's traffic lights occupy the first 80 points.
    var x: CGFloat = 86
    for (_, button) in buttons {
      button.frame = NSRect(x: x, y: 9, width: button.preferredWidth, height: 28)
      x += button.preferredWidth + (Design.bevelled ? 4 : 2)
    }
    help.frame = NSRect(x: bounds.width - 38, y: 10, width: 26, height: 26)
    commands.frame = NSRect(x: bounds.width - 68, y: 10, width: 26, height: 26)
    openButton.frame = NSRect(x: bounds.width - 178, y: 9, width: 100, height: 28)
  }
}

/// Accepts files dragged onto its folder targets and outlines the target under the pointer.
@MainActor
class FolderDropArea: Surface {
  var targets: [(button: ShellButton, folder: FolderID)] = []
  var drop: (([URL], FolderID) -> Void)?
  private var highlighted: ShellButton? {
    didSet {
      guard oldValue !== highlighted else { return }
      oldValue?.dropTarget = false
      highlighted?.dropTarget = true
    }
  }
  override init(frame: NSRect) {
    super.init(frame: frame)
    registerForDraggedTypes([.fileURL])
  }
  required init?(coder: NSCoder) { nil }

  private func urls(_ sender: NSDraggingInfo) -> [URL] {
    (sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
  }
  /// The target under the pointer, unless every dragged file already lives in its folder.
  private func target(_ sender: NSDraggingInfo) -> (button: ShellButton, folder: FolderID)? {
    let point = convert(sender.draggingLocation, from: nil)
    guard let target = targets.first(where: { !$0.button.isHidden && $0.button.frame.contains(point) }) else { return nil }
    let urls = urls(sender)
    guard !urls.isEmpty, !urls.allSatisfy({ FolderID($0.deletingLastPathComponent().path) == target.folder }) else { return nil }
    return target
  }
  override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { draggingUpdated(sender) }
  override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
    highlighted = target(sender)?.button
    return highlighted == nil ? [] : .copy
  }
  override func draggingExited(_ sender: NSDraggingInfo?) { highlighted = nil }
  override func draggingEnded(_ sender: NSDraggingInfo) { highlighted = nil }
  override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    highlighted = nil
    guard let target = target(sender) else { return false }
    drop?(urls(sender), target.folder)
    return true
  }
}

/// The Folders toolbar: Back and Forward, the places (Home, Downloads, Desktop, the pinned
/// folder and connected drives), and actions for the current folder. Places accept dropped
/// files and copy them there. ⌘L swaps the places for a path field.
@MainActor
final class PlacesBar: FolderDropArea {
  var navigate: ((FolderID) -> Void)?
  var step: ((Int) -> Void)?
  var eject: ((MountedVolume) -> Void)?
  var pin: (() -> Void)?
  var refresh: (() -> Void)?
  let pathEditor = NSTextField()
  private let back = ActionButton("", symbol: "chevron.left", help: "Back · ⌘[") {}
  private let forward = ActionButton("", symbol: "chevron.right", help: "Forward · ⌘]") {}
  private let reload = ActionButton("", symbol: "arrow.clockwise", help: "Refresh · ⌘R") {}
  private let pinButton = ActionButton("Pin folder", symbol: "pin") {}
  private var ejects: [(button: ActionButton, after: ShellButton)] = []
  private var editing = false

  override init(frame: NSRect) {
    super.init(frame: frame)
    back.invoke = { [weak self] in self?.step?(-1) }
    forward.invoke = { [weak self] in self?.step?(1) }
    reload.invoke = { [weak self] in self?.refresh?() }
    pinButton.invoke = { [weak self] in self?.pin?() }
    pathEditor.setAccessibilityLabel("Folder path")
    pathEditor.placeholderString = "Type a folder path, such as ~/Projects"
    for view in [back, forward, reload, pinButton, pathEditor] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }

  func configure(folder: FolderID, canGoBack: Bool, canGoForward: Bool, pinned: FolderID?,
                 volumes: [MountedVolume], editingPath: Bool) {
    fill = Design.bevelled ? Design.sidebar : Design.background
    targets.forEach { $0.button.removeFromSuperview() }
    ejects.forEach { $0.button.removeFromSuperview() }
    targets = []
    ejects = []
    var places: [(String, ShellIcon, FolderID)] = [("Home", .home, .home), ("Downloads", .downloads, .downloads), ("Desktop", .desktop, .desktop)]
    if let pinned { places.append((pinned.url.lastPathComponent, .pin, pinned)) }
    places += volumes.map { ($0.name, .drive, $0.folder) }
    // The most specific place that contains the folder is the selected one.
    let current = places.map(\.2).filter { folder == $0 || folder.rawValue.hasPrefix($0.rawValue + "/") }
      .max { $0.rawValue.count < $1.rawValue.count }
    for (name, icon, target) in places {
      let button = ShellButton(name, icon: icon, help: target.rawValue) { [weak self] in self?.navigate?(target) }
      button.selected = target == current
      addSubview(button)
      targets.append((button, target))
      if let volume = volumes.first(where: { $0.folder == target }), volume.ejectable {
        let eject = ActionButton("", symbol: "eject", help: "Eject \(volume.name)") { [weak self] in self?.eject?(volume) }
        eject.image = ShellIcons.image(.eject)
        eject.setAccessibilityLabel("Eject \(volume.name)")
        addSubview(eject)
        ejects.append((eject, button))
      }
    }
    back.isEnabled = canGoBack
    forward.isEnabled = canGoForward
    for button in [back, forward, reload, pinButton] {
      button.contentTintColor = button.isEnabled ? Design.text : Design.muted.withAlphaComponent(0.45)
    }
    let isPinned = pinned == folder
    pinButton.title = isPinned ? "Pinned" : "Pin folder"
    pinButton.isEnabled = !isPinned
    pinButton.toolTip = isPinned ? "This is your pinned folder" : "Pin this folder to the places · replaces the current pin"
    pathEditor.font = Design.font(12)
    editing = editingPath
    pathEditor.isHidden = !editingPath
    targets.forEach { $0.button.isHidden = editingPath }
    ejects.forEach { $0.button.isHidden = editingPath }
    needsLayout = true
    needsDisplay = true
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
  }
  override func layout() {
    super.layout()
    back.frame = NSRect(x: 12, y: 9, width: 28, height: 26)
    forward.frame = NSRect(x: 42, y: 9, width: 28, height: 26)
    pinButton.frame = NSRect(x: bounds.width - 112, y: 9, width: 100, height: 26)
    reload.frame = NSRect(x: bounds.width - 144, y: 9, width: 28, height: 26)
    pathEditor.frame = NSRect(x: 84, y: 10, width: max(120, bounds.width - 244), height: 24)
    var x: CGFloat = 84
    let limit = bounds.width - 156
    for (button, _) in targets {
      let width = min(button.preferredWidth, 190)
      button.frame = NSRect(x: x, y: 8, width: width, height: 28)
      // Places that do not fit stay reachable from the command palette.
      button.isHidden = editing || x + width > limit
      x += width + (Design.bevelled ? 4 : 2)
      if let eject = ejects.first(where: { $0.after === button })?.button {
        eject.frame = NSRect(x: x - 2, y: 10, width: 24, height: 24)
        eject.isHidden = button.isHidden
        x += 24
      }
    }
  }
}

/// A determinate bar drawn in the theme: blocks in Windows Retro, a framed fill in System 7,
/// a sunken bevel in Linux Retro and a rounded fill elsewhere.
@MainActor
final class ProgressBar: NSView {
  var value: Double = 0 { didSet { needsDisplay = true } }
  override var isFlipped: Bool { true }
  override func draw(_ dirtyRect: NSRect) {
    let radius: CGFloat = Design.square ? 0 : bounds.height / 2
    (Design.bevelled ? NSColor.white : Design.classic ? .white : Design.text.withAlphaComponent(0.1)).setFill()
    NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
    if Design.bevelled { Design.drawBevel(bounds, sunken: true) }
    if Design.classic {
      NSColor.black.setStroke()
      NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
    }
    let inner = Design.bevelled ? bounds.insetBy(dx: 3, dy: 3) : Design.classic ? bounds.insetBy(dx: 2, dy: 2) : bounds
    let width = inner.width * CGFloat(min(1, max(0, value)))
    (Design.classic ? NSColor.black : Design.accent).setFill()
    if Design.theme == .windowsRetro {
      for x in stride(from: inner.minX, to: inner.minX + width, by: 9) {
        NSRect(x: x, y: inner.minY, width: min(7, inner.maxX - x), height: inner.height).fill()
      }
    } else {
      NSBezierPath(roundedRect: NSRect(x: inner.minX, y: inner.minY, width: width, height: inner.height), xRadius: radius, yRadius: radius).fill()
    }
  }
}

/// The window's status bar: the section's status and key hints on the left, and the running
/// copy on the right, visible from every section. Clicking the copy opens Transfers.
@MainActor
final class StatusBar: Surface {
  static let height: CGFloat = 26
  let cancel = ActionButton("Cancel", action: {})
  var showTransfers: (() -> Void)?
  private let text = label("", size: 11)
  private let copyText = label("", size: 11)
  private let bar = ProgressBar()
  private var copying = false

  override init(frame: NSRect) {
    super.init(frame: frame)
    copyText.lineBreakMode = .byTruncatingMiddle
    for view in [text, copyText, bar, cancel] { addSubview(view) }
    showCopy(nil)
  }
  required init?(coder: NSCoder) { nil }

  func configure(text value: String) {
    fill = Design.sidebar
    text.stringValue = value
    for field in [text, copyText] { field.font = Design.font(11); field.textColor = Design.muted }
    cancel.font = Design.font(11)
    cancel.contentTintColor = Design.text
    needsDisplay = true
  }
  /// Shows a copy's progress, or hides the copy area when `progress` is nil.
  func showCopy(_ progress: FileCopyProgress?, place: String = "") {
    copying = progress != nil
    for view in [copyText, bar, cancel] as [NSView] { view.isHidden = !copying }
    if let progress {
      let copied = progress.copied, total = max(progress.total, 1)
      let bytes = { ByteCountFormatter.string(fromByteCount: $0, countStyle: .file) }
      copyText.stringValue = "Copying \(progress.name) to \(place) · \(bytes(min(copied, total))) of \(bytes(total))"
      copyText.toolTip = "Show Transfers"
      bar.value = min(1, Double(copied) / Double(total))
    }
    needsLayout = true
    needsDisplay = true
  }
  private var copyArea: NSRect { NSRect(x: bounds.width - 560, y: 0, width: 480, height: bounds.height) }
  override func mouseUp(with event: NSEvent) {
    if copying && copyArea.contains(convert(event.locationInWindow, from: nil)) { showTransfers?() }
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
  }
  override func layout() {
    super.layout()
    let right = copying ? copyArea.minX - 12 : bounds.width - 12
    text.frame = NSRect(x: 12, y: 5, width: max(0, right - 12), height: 16)
    cancel.frame = NSRect(x: bounds.width - 72, y: 2, width: 62, height: 22)
    bar.frame = NSRect(x: bounds.width - 212, y: Design.square ? 7 : 9, width: 130, height: Design.square ? 12 : 8)
    copyText.frame = NSRect(x: copyArea.minX, y: 5, width: 330, height: 16)
  }
}
