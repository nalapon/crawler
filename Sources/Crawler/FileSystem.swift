import Foundation
import Darwin

// Reads and sorts only the requested directory. Never walks the disk at launch.
enum DirectoryReader {
  static let keys: Set<URLResourceKey> = [
    .isDirectoryKey, .isPackageKey, .isSymbolicLinkKey, .fileSizeKey,
    .contentModificationDateKey, .creationDateKey,
  ]
  static func read(_ folder: FolderID) throws -> [FileItem] {
    // Foundation refuses a directory symlink as the enumeration root. Read its target,
    // but keep the entered path in every item so back/forward navigation stays stable.
    let isLink = try folder.url.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink == true
    let readURL = isLink ? folder.url.resolvingSymlinksInPath() : folder.url
    let urls = try FileManager.default.contentsOfDirectory(
      at: readURL, includingPropertiesForKeys: Array(keys), options: [.skipsHiddenFiles])
    return urls.map { child in
      let url = isLink ? folder.url.appendingPathComponent(child.lastPathComponent) : child
      return (try? item(at: url)) ?? FileItem(id: ItemID(rawValue: url.standardizedFileURL.path),
        name: url.lastPathComponent, parent: folder, kind: .document(.forExtension(url.pathExtension)),
        bytes: nil, modified: nil, created: nil)
    }.sorted(by: nameOrder)
  }
  static func item(at url: URL) throws -> FileItem {
    let values = try url.resourceValues(forKeys: keys)
    let target = values.isSymbolicLink == true
      ? try? url.resolvingSymlinksInPath().resourceValues(forKeys: [.isDirectoryKey, .isPackageKey]) : values
    let isFolder = target?.isDirectory == true && target?.isPackage != true
    return FileItem(
      id: ItemID(rawValue: url.standardizedFileURL.path), name: url.lastPathComponent,
      parent: FolderID(url.deletingLastPathComponent().path),
      kind: isFolder ? .folder(FolderID(url.path)) : .document(.forExtension(url.pathExtension)),
      bytes: values.isDirectory == true ? nil : values.fileSize.map(Int64.init),
      modified: values.contentModificationDate, created: values.creationDate)
  }
  // Context needs a few neighboring entries, not a second complete directory listing.
  static func nearby(_ folder: FolderID, limit: Int = 80) throws -> (rows: [FileItem], hasMore: Bool) {
    var failure: Error?
    guard let iterator = FileManager.default.enumerator(at: folder.url, includingPropertiesForKeys: nil,
      options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants, .skipsPackageDescendants],
      errorHandler: { _, error in failure = error; return false }) else {
      throw CocoaError(.fileReadUnknown)
    }
    var rows: [FileItem] = []
    var visited = 0
    while let url = iterator.nextObject() as? URL {
      try Task.checkCancellation()
      if visited == limit { return (rows.sorted(by: nameOrder), true) }
      visited += 1
      if let file = try? item(at: url) { rows.append(file) }
    }
    if let failure { throw failure }
    return (rows.sorted(by: nameOrder), false)
  }
  // Logical file bytes, including hidden contents and packages. Do not follow symlinks.
  static func totalSize(_ url: URL) -> Int64? {
    let keys: Set<URLResourceKey> = [.isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey]
    guard let root = try? url.resourceValues(forKeys: keys) else { return nil }
    guard root.isSymbolicLink != true else { return root.fileSize.map(Int64.init) }
    guard root.isDirectory == true else { return root.fileSize.map(Int64.init) }
    var failed = false
    guard let entries = FileManager.default.enumerator(at: url, includingPropertiesForKeys: Array(keys),
      options: [], errorHandler: { _, _ in failed = true; return false }) else { return nil }
    var total: Int64 = 0
    for case let child as URL in entries {
      guard !Task.isCancelled else { return nil }
      guard let values = try? child.resourceValues(forKeys: keys) else { return nil }
      if values.isSymbolicLink == true { entries.skipDescendants() }
      if values.isDirectory != true {
        guard let bytes = values.fileSize else { return nil }
        let sum = total.addingReportingOverflow(Int64(bytes))
        guard !sum.overflow else { return nil }
        total = sum.partialValue
      }
    }
    return failed ? nil : total
  }
  static func nameOrder(_ a: FileItem, _ b: FileItem) -> Bool {
    if (a.kind.folder != nil) != (b.kind.folder != nil) { return a.kind.folder != nil }
    return a.name.localizedStandardCompare(b.name) == .orderedAscending
  }
}

enum DirectoryState {
  case loading([FileItem])
  case ready([FileItem])
  case failed(String)
  var rows: [FileItem] {
    switch self { case .loading(let rows), .ready(let rows): rows; case .failed: [] }
  }
  var message: String? {
    switch self {
    case .loading: "Loading folder…"
    case .ready(let rows): rows.isEmpty ? "Empty folder" : nil
    case .failed(let message): message
    }
  }
}

@MainActor
final class FileSystemLibrary {
  private var directories: [FolderID: DirectoryState] = [:]
  private var accessOrder: [FolderID] = []
  private var requests: [FolderID: Task<Void, Never>] = [:]
  private var watcherRequests: [FolderID: Task<Void, Never>] = [:]
  private var watchers: [FolderID: DispatchSourceFileSystemObject] = [:]
  private var reloads: [FolderID: Task<Void, Never>] = [:]
  private var visible: Set<FolderID> = []
  var didChange: ((FolderID) -> Void)?
  var didUpdateSize: ((FileItem) -> Void)?
  private var sizeTask: Task<Void, Never>?
  private var sizeFolder: FolderID?
  private var sizeGeneration = UUID()

  // One cancellable scan for the active folder only, never its parent columns.
  func calculateSizes(in folder: FolderID?) {
    guard folder != sizeFolder else { return }
    sizeTask?.cancel()
    sizeFolder = folder
    sizeGeneration = UUID()
    guard let folder, case .ready(let rows) = directories[folder] else {
      sizeFolder = nil
      return
    }
    let generation = sizeGeneration
    sizeTask = Task.detached(priority: .utility) { [weak self] in
      for item in rows where item.bytes == nil {
        guard !Task.isCancelled else { return }
        guard let bytes = DirectoryReader.totalSize(item.url) else { continue }
        guard !Task.isCancelled else { return }
        await self?.publishSize(bytes, for: item, generation: generation)
      }
    }
  }
  private func publishSize(_ bytes: Int64, for item: FileItem, generation: UUID) {
    guard generation == sizeGeneration, case .ready(var rows) = directories[item.parent],
      let index = rows.firstIndex(where: { $0.id == item.id }) else { return }
    rows[index].bytes = bytes
    directories[item.parent] = .ready(rows)
    didUpdateSize?(rows[index])
  }
  private(set) var pinned: FolderID?
  init() {
    if let path = UserDefaults.standard.string(forKey: "pinnedFolder") { pinned = FolderID(path) }
  }
  func pin(_ folder: FolderID) {
    pinned = folder
    UserDefaults.standard.set(folder.rawValue, forKey: "pinnedFolder")
  }
  func path(_ folder: FolderID) -> String { folder.rawValue }
  func parent(_ folder: FolderID) -> FolderID? {
    folder.rawValue == "/" ? nil : FolderID(folder.url.deletingLastPathComponent().path)
  }
  func ancestors(_ folder: FolderID) -> [FolderID] {
    var result = [folder]
    while let parent = parent(result[0]) { result.insert(parent, at: 0) }
    return result
  }
  func item(_ id: ItemID?) -> FileItem? {
    guard let id else { return nil }
    let parent = FolderID(URL(fileURLWithPath: id.rawValue).deletingLastPathComponent().path)
    return directories[parent]?.rows.first { $0.id == id }
  }
  func owner(_ folder: FolderID) -> FileItem? { item(ItemID(rawValue: folder.rawValue)) }
  func state(_ folder: FolderID) -> DirectoryState { directories[folder] ?? .loading([]) }
  func children(_ folder: FolderID) -> [FileItem] {
    if directories[folder] == nil { load(folder) }
    return directories[folder]?.rows ?? []
  }
  func load(_ folder: FolderID, refresh: Bool = false) {
    guard requests[folder] == nil, refresh || directories[folder] == nil else { return }
    if sizeFolder == folder {
      sizeTask?.cancel()
      sizeFolder = nil
      sizeGeneration = UUID()
    }
    let previous = directories[folder]
    if case .ready = previous {} else { directories[folder] = .loading(previous?.rows ?? []) }
    accessOrder.removeAll { $0 == folder }
    accessOrder.append(folder)
    requests[folder] = Task { [weak self] in
      let result = await Task.detached(priority: .userInitiated) {
        do { return Result<[FileItem], Error>.success(try DirectoryReader.read(folder)) }
        catch { return .failure(error) }
      }.value
      guard let self else { return }
      switch result {
      case .success(let rows): self.directories[folder] = .ready(rows)
      case .failure(let error): self.directories[folder] = .failed(error.localizedDescription)
      }
      self.requests[folder] = nil
      self.evict()
      if case .ready(let before) = previous, case .ready(let after) = self.directories[folder], before == after { return }
      self.didChange?(folder)
    }
  }
  // Watch just the visible columns. Revisited cache entries refresh in the background.
  func watch(_ folders: Set<FolderID>) {
    let entered = folders.subtracting(visible)
    visible = folders
    for folder in Array(watchers.keys) where !folders.contains(folder) {
      watchers.removeValue(forKey: folder)?.cancel()
      reloads.removeValue(forKey: folder)?.cancel()
    }
    for folder in entered {
      accessOrder.removeAll { $0 == folder }
      accessOrder.append(folder)
      load(folder, refresh: true)
      guard watchers[folder] == nil, watcherRequests[folder] == nil else { continue }
      watcherRequests[folder] = Task { [weak self] in
        // Opening a protected or remote directory can block just like enumeration.
        let descriptor = await Task.detached(priority: .userInitiated) {
          Darwin.open(folder.rawValue, O_EVTONLY)
        }.value
        guard let self else { if descriptor >= 0 { Darwin.close(descriptor) }; return }
        self.watcherRequests[folder] = nil
        guard descriptor >= 0 else { return }
        guard self.visible.contains(folder) else { Darwin.close(descriptor); return }
        self.attachWatcher(descriptor, to: folder)
      }
    }
  }
  private func attachWatcher(_ descriptor: Int32, to folder: FolderID) {
      let source = DispatchSource.makeFileSystemObjectSource(
        fileDescriptor: descriptor, eventMask: [.write, .delete, .rename, .attrib, .extend, .link, .revoke], queue: .main)
      source.setEventHandler { [weak self] in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.reloads[folder]?.cancel()
          self.reloads[folder] = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(120)) } catch { return }
            guard let self else { return }
            // An event during enumeration must trigger one more read after it finishes.
            await self.requests[folder]?.value
            guard !Task.isCancelled else { return }
            self.load(folder, refresh: true)
          }
        }
      }
      source.setCancelHandler { Darwin.close(descriptor) }
      watchers[folder] = source
      source.resume()
  }
  private func evict() {
    while directories.count > 32,
      let candidate = accessOrder.first(where: { !visible.contains($0) && requests[$0] == nil }) {
      directories[candidate] = nil
      accessOrder.removeAll { $0 == candidate }
    }
  }
}
