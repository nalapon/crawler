import AppKit

private struct TrashSnapshot: Sendable, Equatable {
  var items: [FileItem] = []
  var failures: [String] = []

  // Resolve each volume's trash without creating directories or changing its contents.
  static func read() -> Self {
    let manager = FileManager.default
    var result = Self()
    let volumes = manager.mountedVolumeURLs(includingResourceValuesForKeys: [.volumeIsBrowsableKey],
      options: [.skipHiddenVolumes]) ?? []
    var folders = Set<URL>()
    for volume in [manager.homeDirectoryForCurrentUser] + volumes {
      if Task.isCancelled { return result }
      if volume.path.hasPrefix("/System/Volumes/") { continue }
      do {
        let trash = try manager.url(for: .trashDirectory, in: .userDomainMask,
          appropriateFor: volume, create: false).standardizedFileURL
        folders.insert(trash)
      } catch {
        let error = error as NSError
        if error.domain == NSCocoaErrorDomain && (error.code == NSFileNoSuchFileError || error.code == NSFileReadNoSuchFileError) { continue }
        result.failures.append("\(volume.lastPathComponent): \(error.localizedDescription)")
      }
    }
    for folder in folders.sorted(by: { $0.path < $1.path }) {
      if Task.isCancelled { return result }
      do {
        let urls = try manager.contentsOfDirectory(at: folder,
          includingPropertiesForKeys: Array(DirectoryReader.keys), options: [])
        for url in urls where url.lastPathComponent != ".DS_Store" {
          if Task.isCancelled { return result }
          let item = (try? DirectoryReader.item(at: url)) ?? FileItem(
            id: ItemID(rawValue: url.standardizedFileURL.path), name: url.lastPathComponent,
            parent: FolderID(folder.path), kind: .document(.forExtension(url.pathExtension)),
            bytes: nil, modified: nil, created: nil)
          result.items.append(item)
        }
      } catch {
        let error = error as NSError
        if error.domain == NSCocoaErrorDomain && (error.code == NSFileNoSuchFileError || error.code == NSFileReadNoSuchFileError) { continue }
        result.failures.append("\(folder.path): \(error.localizedDescription)")
      }
    }
    result.items.sort(by: DirectoryReader.nameOrder)
    return result
  }
}

@MainActor
final class TrashView: Surface {
  private let title = label("Trash", size: 25, weight: .semibold)
  private let status = label("Loading Trash…", size: 12, color: Design.muted)
  private let failure = label("", size: 12, color: Design.muted)
  let pane = FilePane()
  private var snapshot = TrashSnapshot()
  private var selected: ItemID?
  private var field: SortField = .name
  private var ascending = true
  private var active = false
  private var loaded = false
  private var task: Task<Void, Never>?
  private var generation = UUID()
  private var timer: Timer?
  private lazy var refresh = ActionButton("Refresh", symbol: "arrow.clockwise") { [weak self] in self?.reload() }

  override init(frame: NSRect) {
    super.init(frame: frame)
    [title, status, failure, refresh, pane].forEach { addSubview($0) }
    failure.maximumNumberOfLines = 3
    failure.lineBreakMode = .byWordWrapping
    pane.table.setAccessibilityLabel("Files in Trash")
    pane.selectionChanged = { [weak self] id, _, _ in self?.selected = id }
    pane.sortChanged = { [weak self] field in
      guard let self else { return }
      self.ascending = self.field == field ? !self.ascending : true
      self.field = field
      self.render()
    }
  }
  required init?(coder: NSCoder) { nil }
  isolated deinit { task?.cancel(); timer?.invalidate() }

  func setActive(_ value: Bool) {
    guard active != value else { return }
    active = value
    if value {
      render()
      reload()
      // Also discovers newly mounted volumes and Trash directories created by Finder.
      timer = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
        Task { @MainActor in self?.reload() }
      }
    } else {
      timer?.invalidate(); timer = nil
      task?.cancel(); task = nil
      generation = UUID()
    }
  }

  private func reload() {
    guard active, task == nil else { return }
    let generation = generation
    task = Task { [weak self] in
      let worker = Task.detached(priority: .utility) { TrashSnapshot.read() }
      let snapshot = await withTaskCancellationHandler { await worker.value } onCancel: { worker.cancel() }
      guard let self, !Task.isCancelled, self.generation == generation else { return }
      let changed = !self.loaded || self.snapshot != snapshot
      self.snapshot = snapshot
      self.loaded = true
      self.task = nil
      if changed { self.render() }
    }
  }

  func render() {
    title.font = Design.font(25, .semibold)
    title.textColor = Design.text
    status.textColor = Design.muted
    failure.textColor = Design.text
    let rows = snapshot.items.sorted { a, b in
      let order: ComparisonResult
      switch field {
      case .name: order = a.name.localizedStandardCompare(b.name)
      case .kind: order = a.kind.label.localizedStandardCompare(b.kind.label)
      case .size: order = (a.bytes ?? -1) == (b.bytes ?? -1) ? .orderedSame : (a.bytes ?? -1) < (b.bytes ?? -1) ? .orderedAscending : .orderedDescending
      case .date: order = (a.modified ?? .distantPast).compare(b.modified ?? .distantPast)
      case .created: order = (a.created ?? .distantPast).compare(b.created ?? .distantPast)
      }
      if order == .orderedSame { return a.id.rawValue < b.id.rawValue }
      return ascending ? order == .orderedAscending : order == .orderedDescending
    }
    if !rows.contains(where: { $0.id == selected }) { selected = rows.first?.id }
    pane.configure(title: "Trash", location: nil,
      rows: rows.map { DisplayRow(item: $0, pending: false, dimmed: false, marked: false, locationText: $0.date) },
      selected: selected, active: true, detailed: true, context: "trash")
    pane.empty.stringValue = !loaded ? "Loading Trash…" : snapshot.failures.isEmpty ? "Trash is empty" : "Trash could not be fully read"
    status.stringValue = !loaded ? "Loading Trash…" : "\(rows.count) items\(snapshot.failures.isEmpty ? "" : " · Some locations could not be read")"
    failure.stringValue = snapshot.failures.isEmpty ? "" : "macOS could not read part of the Trash. Check Crawler’s access in System Settings → Privacy & Security → Full Disk Access, then refresh.\n" + snapshot.failures.joined(separator: "\n")
    failure.toolTip = snapshot.failures.joined(separator: "\n")
    failure.isHidden = snapshot.failures.isEmpty
    needsLayout = true
  }

  override func layout() {
    super.layout()
    title.frame = NSRect(x: 32, y: 24, width: max(0, bounds.width - 180), height: 36)
    refresh.frame = NSRect(x: bounds.width - 124, y: 28, width: 92, height: 30)
    status.frame = NSRect(x: 32, y: 68, width: max(0, bounds.width - 64), height: 20)
    failure.frame = NSRect(x: 32, y: 100, width: max(0, bounds.width - 64), height: 62)
    let top: CGFloat = failure.isHidden ? 108 : 178
    pane.frame = NSRect(x: 24, y: top, width: max(0, bounds.width - 48), height: max(0, bounds.height - top - 24))
  }

  func handleKey(_ event: NSEvent, vim: Bool) -> Bool {
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    if event.modifierFlags.contains(.command) {
      if key == "r" { reload(); return true }
      return false
    }
    guard !event.modifierFlags.contains(.control), !event.modifierFlags.contains(.option) else { return false }
    let down = event.keyCode == 125 || (vim && key == "j")
    let up = event.keyCode == 126 || (vim && key == "k")
    if down || up {
      let rows = pane.rows
      guard !rows.isEmpty else { return true }
      let current = rows.firstIndex(where: { $0.item.id == selected }) ?? (down ? -1 : rows.count)
      selected = rows[min(rows.count - 1, max(0, current + (down ? 1 : -1)))].item.id
      pane.updateSelection(selected, marked: [])
      pane.scrollSelectionVisible()
      return true
    }
    return false
  }
}
