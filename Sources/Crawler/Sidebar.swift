import AppKit

struct SidebarDestination {
  let name: String
  let icon: SidebarIcon
  let location: Location
  let key: String
}
@MainActor
final class SidebarView: Surface {
  static func groups(pinned: FolderID?, volumes: [MountedVolume] = []) -> [(String, [SidebarDestination])] {
    var favorites: [SidebarDestination] = [
      .init(name: "Downloads", icon: .downloads, location: .folder(.downloads), key: "1"),
      .init(name: "Desktop", icon: .desktop, location: .folder(.desktop), key: "2"),
    ]
    if let pinned {
      favorites.append(.init(name: pinned.url.lastPathComponent, icon: .pin, location: .folder(pinned), key: "3"))
    }
    return [
      ("Favorites", favorites),
      ("Locations", volumes.enumerated().map { index, volume in
        .init(name: volume.name, icon: .drive, location: .folder(volume.folder), key: index == 0 ? "4" : "")
      }),
      ("Utilities", [
        .init(name: "AirDrop", icon: .airDrop, location: .airDrop, key: "⇧1"),
        .init(name: "Activity Monitor", icon: .activity, location: .activity, key: "⇧2"),
        .init(name: "System Settings", icon: .systemSettings, location: .systemSettings, key: "⇧3"),
        .init(name: "Crawler Settings", icon: .preferences, location: .preferences, key: "⇧4"),
        .init(name: "Trash", icon: .trash, location: .trash, key: "⇧5"),
      ]),
    ]
  }
  let scroll = NSScrollView(), content = SidebarDropArea(), storage = Surface()
  var eject: ((MountedVolume) -> Void)?
  /// Files dropped on a folder or drive row, to be copied into that folder.
  var drop: (([URL], FolderID) -> Void)? {
    get { content.drop }
    set { content.drop = newValue }
  }
  var choosePin: (() -> Void)?
  var navigate: ((Location) -> Void)?, openSaved: ((Int) -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.documentView = content
    addSubview(scroll)
    addSubview(storage)
  }
  required init?(coder: NSCoder) { nil }
  func configure(location: Location, commandHeld: Bool, saved: [SavedSearch], pinned: FolderID?, volumes: [MountedVolume]) {
    fill = Design.sidebar
    content.fill = Design.sidebar
    storage.fill = Design.classic ? Design.sidebar : .clear
    content.fill = Design.classic ? Design.sidebar : .clear
    applyMaterial(.sidebar)
    content.subviews.forEach { $0.removeFromSuperview() }
    storage.subviews.forEach { $0.removeFromSuperview() }
    content.targets = []
    var y: CGFloat = 17
    func groupTitle(_ title: String) {
      let field = label(title, size: 10, weight: .semibold, color: Design.muted)
      field.frame = NSRect(x: 22, y: y, width: 160, height: 16)
      content.addSubview(field)
      y += 23
    }
    for (name, entries) in Self.groups(pinned: pinned, volumes: volumes) {
      groupTitle(name)
      if name == "Locations" && entries.isEmpty {
        let empty = label("No connected drives", size: 11, color: Design.muted)
        empty.frame = NSRect(x: 22, y: y, width: bounds.width - 32, height: 24)
        content.addSubview(empty)
        y += 25
      }
      for entry in entries {
        let volume = volumes.first { entry.location == .folder($0.folder) }
        let selected = entry.location == location || (volume.map { volume in location.folder.map(volume.contains) ?? false } ?? false)
        let button = ActionButton(
          entry.name,
          action: { [weak self] in self?.navigate?(entry.location) })
        button.font = Design.font(12, selected ? .medium : .regular)
        button.alignment = .left
        button.frame = NSRect(x: 10, y: y, width: bounds.width - 20, height: 30)
        button.imageHugsTitle = true
        button.setSelected(selected)
        button.image = SidebarIcons.image(entry.icon, selected: selected)
        button.imagePosition = .imageLeft
        button.toolTip = volume?.folder.rawValue
        if let volume, volume.ejectable {
          button.frame.size.width -= 26
          let eject = ActionButton("", symbol: "eject", help: "Eject \(volume.name)") { [weak self] in self?.eject?(volume) }
          eject.image = SidebarIcons.image(.eject)
          eject.setAccessibilityLabel("Eject \(volume.name)")
          eject.frame = NSRect(x: bounds.width - 36, y: y + 3, width: 24, height: 24)
          content.addSubview(eject)
        }
        content.addSubview(button)
        if case .folder(let folder) = entry.location { content.targets.append((button, folder)) }
        if commandHeld && !entry.key.isEmpty {
          let hint = label(entry.key, size: 10, color: Design.muted)
          hint.alignment = .center
          hint.wantsLayer = true
          hint.layer?.backgroundColor = Design.background.cgColor
          hint.layer?.cornerRadius = 3
          hint.frame = NSRect(x: bounds.width - 36, y: y + 7, width: 21, height: 16)
          content.addSubview(hint)
        }
        y += 31
      }
      if name == "Favorites" {
        let pin = ActionButton(pinned == nil ? "Pin a folder…" : "Change pinned folder…", symbol: "plus") { [weak self] in self?.choosePin?() }
        pin.image = SidebarIcons.image(.add)
        pin.alignment = .left
        pin.font = Design.font(11)
        pin.frame = NSRect(x: 10, y: y, width: bounds.width - 20, height: 30)
        content.addSubview(pin)
        y += 31
      }
      y += 20
      if name == "Locations" && !saved.isEmpty {
        groupTitle("Saved searches")
        for (index, search) in saved.enumerated() {
          let button = ActionButton(
            search.name,
            action: { [weak self] in self?.openSaved?(index) })
          button.image = SidebarIcons.image(.search)
          button.imagePosition = .imageLeft
          button.alignment = .left
          button.frame = NSRect(x: 10, y: y, width: bounds.width - 20, height: 30)
          content.addSubview(button)
          y += 31
        }
        y += 20
      }
    }
    content.frame = NSRect(x: 0, y: 0, width: bounds.width, height: y)
    needsDisplay = true
    needsLayout = true
  }
  override func layout() {
    super.layout()
    scroll.frame = NSRect(x: 0, y: 0, width: bounds.width, height: bounds.height)
    storage.isHidden = true
  }
}

/// Accepts files dragged onto folder and drive rows and outlines the row under the pointer.
@MainActor
final class SidebarDropArea: Surface {
  var targets: [(button: ActionButton, folder: FolderID)] = []
  var drop: (([URL], FolderID) -> Void)?
  private var highlighted: ActionButton? {
    didSet {
      guard oldValue !== highlighted else { return }
      oldValue?.layer?.borderWidth = 0
      guard let highlighted else { return }
      highlighted.wantsLayer = true
      highlighted.layer?.cornerRadius = Design.square ? 0 : 5
      highlighted.layer?.borderColor = Design.accent.cgColor
      highlighted.layer?.borderWidth = 2
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
  /// The folder row under the pointer, unless every dragged file already lives there.
  private func target(_ sender: NSDraggingInfo) -> (button: ActionButton, folder: FolderID)? {
    let point = convert(sender.draggingLocation, from: nil)
    guard let target = targets.first(where: { $0.button.frame.contains(point) }) else { return nil }
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
