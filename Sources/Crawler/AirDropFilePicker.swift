import AppKit

/// Crawler's immediate-directory browser, presented as a file selection sheet.
@MainActor
final class AirDropFilePicker: NSObject, NSSearchFieldDelegate, NSWindowDelegate {
  var choose: (([URL]) -> Void)?
  private let library = FileSystemLibrary()
  private var folder = FolderID.downloads
  private var selected: Set<ItemID> = []
  private var focused: ItemID?
  private let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 760, height: 560),
    styleMask: [.titled, .closable], backing: .buffered, defer: false)
  private let root = Surface()
  private let pane = FilePane()
  private let preview = PreviewPane(frame: .zero)
  private let path = NSTextField()
  private let filter = NSSearchField()
  private let summary = label("Select files or folders", size: 12)
  private lazy var add = ActionButton("Use selected files", symbol: "checkmark") { [weak self] in self?.finish() }
  init(initialFolder: FolderID = .downloads) {
    folder = initialFolder
    super.init()
    panel.title = "Choose files to send"
    panel.isReleasedWhenClosed = false
    panel.delegate = self
    panel.contentView = root
    root.frame = NSRect(x: 0, y: 0, width: 760, height: 560)
    let up = ActionButton("Parent", symbol: "chevron.up") { [weak self] in
      guard let self else { return }
      self.navigate(FolderID(self.folder.url.deletingLastPathComponent().path))
    }
    let downloads = ActionButton("Downloads", symbol: "arrow.down.circle") { [weak self] in self?.navigate(.downloads) }
    let desktop = ActionButton("Desktop", symbol: "desktopcomputer") { [weak self] in self?.navigate(.desktop) }
    let home = ActionButton("Home", symbol: "house") { [weak self] in self?.navigate(.home) }
    let cancel = ActionButton("Cancel") { [weak self] in self?.dismiss() }
    cancel.keyEquivalent = "\u{1b}"
    add.keyEquivalent = "\r"
    for view in [up, downloads, desktop, home, path, filter, pane, preview, summary, cancel, add] { root.addSubview(view) }
    up.frame = NSRect(x: 16, y: 12, width: 80, height: 30)
    downloads.frame = NSRect(x: 106, y: 12, width: 120, height: 30)
    desktop.frame = NSRect(x: 238, y: 12, width: 104, height: 30)
    home.frame = NSRect(x: 354, y: 12, width: 90, height: 30)
    path.frame = NSRect(x: 20, y: 54, width: 720, height: 26)
    path.font = Design.font(12)
    path.target = self; path.action = #selector(commitPath)
    path.setAccessibilityLabel("Folder path")
    filter.frame = NSRect(x: 20, y: 92, width: 720, height: 28)
    filter.placeholderString = "Filter this folder…"
    filter.delegate = self
    pane.frame = NSRect(x: 20, y: 134, width: 410, height: 366)
    preview.frame = NSRect(x: 444, y: 134, width: 296, height: 366)
    summary.frame = NSRect(x: 20, y: 523, width: 430, height: 22)
    cancel.frame = NSRect(x: 466, y: 514, width: 92, height: 32)
    add.frame = NSRect(x: 570, y: 514, width: 170, height: 32)
    pane.selectionChanged = { [weak self] id, _, flags in
      guard let self else { return }
      self.focused = id
      if flags.contains(.command) {
        if self.selected.contains(id) { self.selected.remove(id) } else { self.selected.insert(id) }
      } else { self.selected = [id] }
      self.refresh()
    }
    pane.openItem = { [weak self] id in
      guard let self, let item = self.library.children(self.folder).first(where: { $0.id == id }) else { return }
      if let folder = item.kind.folder { self.navigate(folder) }
      else { self.selected = [id]; self.finish() }
    }
    library.didChange = { [weak self] folder in
      guard let self, self.folder == folder else { return }
      self.refresh()
    }
    refresh()
  }
  func present(on window: NSWindow) {
    window.beginSheet(panel)
    panel.makeFirstResponder(pane.table)
  }
  private func navigate(_ folder: FolderID) {
    self.folder = folder
    selected = []; focused = nil; filter.stringValue = ""
    refresh()
  }
  private func refresh() {
    path.stringValue = folder.rawValue
    let children = library.children(folder)
    let available = Set(children.map(\.id))
    selected.formIntersection(available)
    let query = filter.stringValue
    let rows = children.filter { query.isEmpty || $0.name.localizedStandardContains(query) }
      .map { DisplayRow(item: $0, pending: false, dimmed: false, marked: selected.contains($0.id), locationText: "") }
    pane.configure(title: folder.url.lastPathComponent, location: folder, rows: rows, selected: focused, active: true)
    pane.empty.stringValue = library.state(folder).message ?? (query.isEmpty ? "Empty folder" : "No matching files")
    preview.configure(children.first { $0.id == focused })
    summary.stringValue = selected.isEmpty ? "Select files or folders. Command-click to select more." : "\(selected.count) selected"
    add.isEnabled = !selected.isEmpty
    add.setSelected(add.isEnabled, primary: true)
    library.watch([folder])
  }
  func controlTextDidChange(_ obj: Notification) { refresh() }
  @objc private func commitPath() { navigate(FolderID((path.stringValue as NSString).expandingTildeInPath)) }
  private func finish() {
    guard !selected.isEmpty else { return }
    let urls = library.children(folder).filter { selected.contains($0.id) }.map(\.url)
    dismiss()
    choose?(urls)
  }
  private func dismiss() {
    library.watch([])
    preview.configure(nil)
    panel.sheetParent?.endSheet(panel)
    panel.orderOut(nil)
  }
  func windowShouldClose(_ sender: NSWindow) -> Bool { dismiss(); return false }
}
