import Foundation

// Spotlight supplies indexed, live filename results without recursively scanning the disk.
@MainActor
final class FileSearch: NSObject {
  private var query = NSMetadataQuery()
  private var pending: Task<Void, Never>?
  private var text = ""
  private(set) var rows: [FileItem] = []
  private(set) var gathering = false
  private(set) var limited = false
  private(set) var failure: String?
  var didChange: (() -> Void)?
  override init() {
    super.init()
    query.searchScopes = [NSMetadataQueryLocalComputerScope]
    query.notificationBatchingInterval = 0.15
    for name in [NSNotification.Name.NSMetadataQueryDidFinishGathering, NSNotification.Name.NSMetadataQueryDidUpdate] {
      NotificationCenter.default.addObserver(self, selector: #selector(updated), name: name, object: nil)
    }
  }
  func search(_ text: String) {
    guard text != self.text else { return }
    self.text = text
    pending?.cancel()
    query.stop()
    failure = nil
    rows = []; limited = false; gathering = !text.isEmpty
    guard !text.isEmpty else { return }
    pending = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(180)) } catch { return }
      guard let self else { return }
      self.pending = nil
      // Each search owns a fresh query; stopped queries can still deliver notifications.
      self.query = NSMetadataQuery()
      self.query.searchScopes = [NSMetadataQueryLocalComputerScope]
      self.query.notificationBatchingInterval = 0.15
      let words = text.split(whereSeparator: { $0.isWhitespace })
      let predicates = words.map { NSPredicate(format: "%K CONTAINS[cd] %@", NSMetadataItemFSNameKey, String($0)) }
      guard let first = predicates.first else { self.gathering = false; self.didChange?(); return }
      // Spotlight rejects an AND compound containing only one predicate.
      self.query.predicate = predicates.count == 1 ? first : NSCompoundPredicate(andPredicateWithSubpredicates: predicates)
      if !self.query.start() { self.failure = "Spotlight search could not start"; self.gathering = false; self.didChange?() }
    }
  }
  @objc private func updated(_ notification: Notification) {
    guard let source = notification.object as? NSMetadataQuery, source === query, pending == nil, !text.isEmpty else { return }
    query.disableUpdates()
    defer { query.enableUpdates() }
    gathering = query.isGathering
    limited = query.resultCount > 5000
    rows = (0..<min(query.resultCount, 5000)).compactMap { index in
      guard let metadata = query.result(at: index) as? NSMetadataItem,
        let path = metadata.value(forAttribute: NSMetadataItemPathKey) as? String,
        let name = metadata.value(forAttribute: NSMetadataItemFSNameKey) as? String else { return nil }
      let url = URL(fileURLWithPath: path)
      let types = metadata.value(forAttribute: "kMDItemContentTypeTree") as? [String] ?? []
      let folder = types.contains("public.folder") && !types.contains("com.apple.package")
      return FileItem(id: ItemID(rawValue: url.standardizedFileURL.path), name: name,
        parent: FolderID(url.deletingLastPathComponent().path),
        kind: folder ? .folder(FolderID(path)) : .document(.forExtension(url.pathExtension)),
        bytes: (metadata.value(forAttribute: NSMetadataItemFSSizeKey) as? NSNumber)?.int64Value,
        modified: metadata.value(forAttribute: NSMetadataItemFSContentChangeDateKey) as? Date,
        created: metadata.value(forAttribute: NSMetadataItemFSCreationDateKey) as? Date)
    }.sorted(by: DirectoryReader.nameOrder)
    didChange?()
  }
}
