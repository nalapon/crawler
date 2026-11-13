import CoreServices
import CryptoKit
import Darwin
import Foundation
import os
import FFFBridge

struct SearchSnapshot: Sendable {
  var batches: [SearchIndex]
  var warning: String?
  var changed = false
  /// The newest file-system event these batches reflect. Nil means only a full walk can bring them up to date.
  var event: IndexEvent?
}

/// An FSEvents ID and when it was current. Batches marked with it include every earlier change,
/// so the next search start can replay the events after it instead of walking the whole disk.
struct IndexEvent: Sendable {
  let id: FSEventStreamEventId
  let date: Date
}

struct SearchReloadMeasurement: Codable {
  struct Reading: Codable {
    let firstResultMS: Double
    let completeMS: Double
    let cpuMS: Double
    let footprintMiB: Double
  }
  let query: String
  let indexedPaths: Int
  let cacheLoadMS: Double
  let pathsAtFirstReloadResult: Int
  let firstReloadResultWasFinalBest: Bool
  let emptyFootprintMiB: Double
  let reloaded: Reading
  let resident: Reading
  let identicalResults: Bool
  var cached: Reading?
  var releasedFootprintMiB: Double?
  var resultDigest: String?
  var totalMatches: Int?
}

// Cache reconciliation and filesystem events can rediscover a resident path.
// Deduplicate off the main actor before publishing batches, so counts stay unique.
private actor SearchDiscoveryMerge {
  private var paths: SearchPathTable
  init(previous: [SearchIndex]) {
    paths = SearchPathTable(previous: previous)
  }
  func additions(_ index: SearchIndex) -> SearchIndex? {
    let added = paths.insert(index.entries)
    guard !added.isEmpty else { return nil }
    if added.count == index.entries.count { return index }
    let entries = added.map { index.entries[$0] }
    return SearchIndex(root: index.root, paths: entries.map { ($0.path, $0.isFolder) })
  }
}

// Directory names arrive before descendants. Dependency trees are delayed, never excluded.
// Only directories and symlinks need Foundation metadata; regular files use readdir's type
// and a relative stat for Finder’s hidden flag.
enum SearchIndexReader {
  // Publish likely files before a large legacy cache reaches their paths. This
  // reads one directory level only; the ordinary scan reconciles the same IDs.
  static func warm(_ folders: [FolderID], root: FolderID,
    batch: @escaping @Sendable (SearchIndex) async -> Void) async {
    let collector = Batches(root: root, previous: [], prefixes: nil, publish: batch)
    await withTaskGroup(of: Void.self) { group in
      for folder in Set(folders) {
        let relative = String(folder.rawValue.dropFirst())
        group.addTask {
          _ = try? await readDirectory(root, relative: relative, collector: collector)
        }
      }
    }
    _ = await collector.finish()
  }

  static func read(
    _ root: FolderID, previous: [SearchIndex] = [], changed: [String]? = nil,
    warning: String? = nil,
    excluding: @escaping @Sendable ([String]) async -> Void = { _ in },
    batch: @escaping @Sendable (SearchIndex) async -> Void
  ) async throws -> SearchSnapshot {
    var result = SearchSnapshot(batches: [], warning: changed == nil ? nil : warning)
    var directories: [String] = []
    var seeds: [(String, Bool)] = []
    var prefixes: [String]?
    let keys: Set<URLResourceKey> = [
      .isDirectoryKey, .isPackageKey, .isSymbolicLinkKey, .isHiddenKey,
    ]
    if let changed {
      var kept: [String] = []
      var excluded = Set<String>()
      var closed: [String: Ancestor] = [:]
      for prefix in outermost(changed) {
        try Task.checkCancellation()
        // A file event can arrive beneath an excluded directory without an event for its parent.
        if let ancestor = closedAncestor(of: prefix, root: root, keys: keys, memo: &closed) {
          if ancestor.hidden { excluded.insert(ancestor.path) }
          continue
        }
        kept.append(prefix)
        let url = root.url.appendingPathComponent(prefix)
        do {
          let values = try url.resourceValues(forKeys: keys)
          if values.isHidden == true {
            excluded.insert(prefix)
            continue
          }
          let target =
            values.isSymbolicLink == true
            ? try? url.resolvingSymlinksInPath().resourceValues(forKeys: keys) : values
          let folder = target?.isDirectory == true && target?.isPackage != true
          seeds.append((prefix, folder))
          if folder && values.isSymbolicLink != true { directories.append(prefix) }
        } catch let error as NSError {
          if error.domain != NSCocoaErrorDomain || error.code != NSFileReadNoSuchFileError {
            result.warning = "Some paths could not be read: \(error.localizedDescription)"
          }
        }
      }
      if !excluded.isEmpty { await excluding(Array(excluded)) }
      // Nothing indexed can change beneath excluded folders alone.
      if kept.isEmpty { return SearchSnapshot(batches: previous, warning: result.warning) }
      prefixes = kept
    } else {
      directories = [""]
    }
    let collector = Batches(root: root, previous: previous, prefixes: prefixes, publish: batch)
    await collector.add(seeds)
    // A slow volume or pending privacy prompt must not hold up unrelated folders.
    try await withThrowingTaskGroup(of: Directory.self) { group in
      var position = 0
      var running = 0
      var deferred: [String] = []
      while position < directories.count || !deferred.isEmpty || running > 0 {
        try Task.checkCancellation()
        if position == directories.count && !deferred.isEmpty {
          directories = deferred
          deferred = []
          position = 0
        }
        while running < 8 && position < directories.count {
          let path = directories[position]
          position += 1
          running += 1
          group.addTask { try await readDirectory(root, relative: path, collector: collector) }
        }
        guard let directory = try await group.next() else { break }
        running -= 1
        if !directory.excluded.isEmpty { await excluding(directory.excluded) }
        if let warning = directory.warning { result.warning = warning }
        directories.append(contentsOf: directory.children)
        deferred.append(contentsOf: directory.deferred)
        if position >= 4096 {
          directories.removeFirst(position)
          position = 0
        }
      }
    }
    try Task.checkCancellation()
    result.batches += await collector.finish()
    result.changed = await collector.changed
    return result
  }

  /// Drops paths inside another changed path: reading a changed folder covers its contents.
  private static func outermost(_ paths: [String]) -> [String] {
    var seen = Set<String>()
    var result: [String] = []
    for path in paths.sorted(by: { $0.utf8.count < $1.utf8.count }) {
      var covered = seen.contains(path)
      var end = path.endIndex
      while !covered, let slash = path[..<end].lastIndex(of: "/") {
        covered = seen.contains(String(path[..<slash]))
        end = slash
      }
      if !covered {
        seen.insert(path)
        result.append(path)
      }
    }
    return result
  }

  private enum Ancestor { case open, hidden, closed, missing }
  /// The first ancestor that keeps a changed path out of the index: a hidden folder or one the
  /// walk never enters (`hidden`, reported so later events beneath it are ignored, as after a
  /// full walk), or a package or folder symlink. `memo` is shared by the paths of one read.
  private static func closedAncestor(
    of path: String, root: FolderID, keys: Set<URLResourceKey>, memo: inout [String: Ancestor]
  ) -> (path: String, hidden: Bool)? {
    var ancestor = ""
    for component in path.split(separator: "/").dropLast() {
      ancestor = ancestor.isEmpty ? String(component) : ancestor + "/" + component
      let kind: Ancestor
      if let known = memo[ancestor] {
        kind = known
      } else if unentered(root, ancestor) {
        kind = .hidden
      } else if let values = try? root.url.appendingPathComponent(ancestor).resourceValues(forKeys: keys) {
        kind = values.isHidden == true ? .hidden
          : values.isPackage == true || values.isSymbolicLink == true ? .closed : .open
      } else {
        kind = .missing
      }
      memo[ancestor] = kind
      switch kind {
      case .open: continue
      // A removed folder keeps the path in scope, so its indexed contents are removed too.
      case .missing: return nil
      case .hidden: return (ancestor, true)
      case .closed: return (ancestor, false)
      }
    }
    return nil
  }

  /// Folders the Mac-wide index lists but never enters: device files and the duplicate mount hierarchy.
  private static func unentered(_ root: FolderID, _ relative: String) -> Bool {
    root.rawValue == "/" && (relative == "System/Volumes" || relative == "dev")
  }

  private struct Directory: Sendable {
    var excluded: [String] = []
    var children: [String] = []
    var deferred: [String] = []
    var warning: String?
  }
  private static func readDirectory(
    _ root: FolderID, relative: String, collector: Batches
  ) async throws -> Directory {
    var result = Directory()
    if unentered(root, relative) {
      result.excluded = [relative]
      return result
    }
    let url = relative.isEmpty ? root.url : root.url.appendingPathComponent(relative)
    let descriptor: Int32
    do { descriptor = try await openDirectory(url) } catch {
      if relative.isEmpty { throw error }
      // A folder that did not answer in time still exists: keep what was indexed beneath it.
      // Otherwise the saved index would lose it until the next full walk.
      if (error as NSError).code == Int(ETIMEDOUT) { await collector.keepContents(of: relative) }
      result.warning = "Some folders are unavailable: \(error.localizedDescription) · ⌘R to retry"
      return result
    }
    guard let directory = fdopendir(descriptor) else {
      let error = NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
      close(descriptor)
      if relative.isEmpty { throw error }
      if error.code == Int(ETIMEDOUT) { await collector.keepContents(of: relative) }
      result.warning = "Some folders could not be read: \(error.localizedDescription)"
      return result
    }
    defer { closedir(directory) }
    let keys: Set<URLResourceKey> = [
      .isDirectoryKey, .isPackageKey, .isSymbolicLinkKey, .isHiddenKey,
    ]
    let delayed: Set<String> = ["node_modules", "vendor", "Pods", "Carthage"]
    var paths: [(String, Bool)] = []
    var lastBatch = ContinuousClock.now
    while true {
      try Task.checkCancellation()
      errno = 0
      guard let entry = readdir(directory) else {
        if errno != 0 {
          let error = NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
          if relative.isEmpty { throw error }
          if error.code == Int(ETIMEDOUT) { await collector.keepContents(of: relative) }
          result.warning = "Some folders could not be read: \(error.localizedDescription)"
        }
        break
      }
      let name = withUnsafePointer(to: &entry.pointee.d_name) { pointer in
        pointer.withMemoryRebound(to: CChar.self, capacity: Int(entry.pointee.d_namlen) + 1) {
          String(validatingCString: $0)
        }
      }
      guard let name, !name.hasPrefix(".") else { continue }
      let path = relative.isEmpty ? name : relative + "/" + name
      if entry.pointee.d_type == DT_REG {
        var metadata = stat()
        if fstatat(dirfd(directory), name, &metadata, AT_SYMLINK_NOFOLLOW) == 0 {
          if metadata.st_flags & UInt32(UF_HIDDEN) != 0 { continue }
        } else {
          result.warning = "Some items could not be read: \(String(cString: strerror(errno)))"
          continue
        }
      }
      var folder = entry.pointee.d_type == DT_DIR
      var descend = folder
      if folder || entry.pointee.d_type == DT_LNK || entry.pointee.d_type == DT_UNKNOWN {
        do {
          let kind: (Bool, Bool)? = try autoreleasepool {
            let child = root.url.appendingPathComponent(path)
            let values = try child.resourceValues(forKeys: keys)
            if values.isHidden == true { return nil }
            let target =
              values.isSymbolicLink == true
              ? try? child.resolvingSymlinksInPath().resourceValues(forKeys: keys) : values
            let folder = target?.isDirectory == true && target?.isPackage != true
            return (folder, folder && values.isSymbolicLink != true)
          }
          guard let kind else {
            result.excluded.append(path)
            continue
          }
          folder = kind.0
          descend = kind.1
        } catch {
          result.warning = "Some items could not be read: \(error.localizedDescription)"
          continue
        }
      }
      paths.append((path, folder))
      if descend {
        if delayed.contains(name) {
          result.deferred.append(path)
        } else {
          result.children.append(path)
        }
      }
      if paths.count >= 2048 || lastBatch.duration(to: .now) >= .milliseconds(40) {
        await collector.add(paths)
        paths.removeAll(keepingCapacity: true)
        lastBatch = .now
      }
    }
    if !paths.isEmpty { await collector.add(paths) }
    return result
  }

  // A privacy prompt or disconnected volume can block open indefinitely. The timeout
  // releases the scan worker; a late OS completion closes its descriptor instead of leaking it.
  private static func openDirectory(_ url: URL) async throws -> Int32 {
    try Task.checkCancellation()
    return try await withCheckedThrowingContinuation { continuation in
      let completion = OSAllocatedUnfairLock(initialState: Optional(continuation))
      DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) {
        let waiting = completion.withLock { value in
          let waiting = value
          value = nil
          return waiting
        }
        waiting?.resume(throwing: NSError(domain: NSPOSIXErrorDomain, code: Int(ETIMEDOUT)))
      }
      DispatchQueue.global(qos: .utility).async {
        let fd = url.withUnsafeFileSystemRepresentation {
          $0.map { open($0, O_RDONLY | O_DIRECTORY | O_CLOEXEC) } ?? -1
        }
        let error = errno
        let waiting = completion.withLock { value in
          let waiting = value
          value = nil
          return waiting
        }
        if let waiting {
          if fd >= 0 {
            waiting.resume(returning: fd)
          } else {
            waiting.resume(throwing: NSError(domain: NSPOSIXErrorDomain, code: Int(error)))
          }
        } else if fd >= 0 {
          close(fd)
        }
      }
    }
  }

  private actor Batches {
    let root: FolderID
    let publish: @Sendable (SearchIndex) async -> Void
    var pending: [(String, Bool)] = []
    var indexes: [SearchIndex] = []
    var lastPublish = ContinuousClock.now
    let previous: [SearchIndex]
    let prefixes: [String]?
    var remaining: SearchPathTable
    var replaced = Set<String>()
    var unread: [String] = []
    var changed: Bool { !remaining.isEmpty || !replaced.isEmpty || !indexes.isEmpty }
    init(
      root: FolderID, previous: [SearchIndex], prefixes: [String]?,
      publish: @escaping @Sendable (SearchIndex) async -> Void
    ) {
      self.root = root
      self.publish = publish
      self.previous = previous
      self.prefixes = prefixes
      remaining = SearchPathTable(previous: previous, within: prefixes.map(SearchPathScope.init))
    }
    func add(_ paths: [(String, Bool)], flush: Bool = false) async {
      for (path, folder) in paths {
        if let old = remaining.remove(path) {
          if old == folder { continue }
          replaced.insert(path)
        }
        pending.append((path, folder))
      }
      while !pending.isEmpty
        && (flush || pending.count >= 2048 || indexes.count < 4
          || lastPublish.duration(to: .now) >= .milliseconds(40))
      {
        let count = min(2048, pending.count)
        let index = SearchIndex(root: root, paths: Array(pending.prefix(count)))
        pending.removeFirst(count)
        indexes.append(index)
        lastPublish = .now
        let newEntries = index.entries.filter { !replaced.contains($0.path) }
        if newEntries.count == index.entries.count {
          await publish(index)
        } else if !newEntries.isEmpty {
          await publish(SearchIndex(root: root, paths: newEntries.map { ($0.path, $0.isFolder) }))
        }
      }
    }
    func keepContents(of folder: String) { unread.append(folder) }
    func finish() async -> [SearchIndex] {
      await add([], flush: true)
      let unread = unread.isEmpty ? nil : SearchPathScope(unread)
      var retained: [SearchIndex] = []
      for old in previous {
        if Task.isCancelled { return [] }
        let rows = old.entries.indices.filter {
          (!remaining.contains(old.entries, row: $0) || unread?.contains(old.entries, row: $0) == true)
            && (replaced.isEmpty || !replaced.contains(old.entries[$0].path))
        }
        if rows.count == old.entries.count {
          retained.append(old)
        } else if !rows.isEmpty {
          let kept = rows.map { old.entries[$0] }
          retained.append(SearchIndex(root: root, paths: kept.map { ($0.path, $0.isFolder) }))
        }
      }
      return retained + indexes
    }
  }

}

// A versioned sequence of batches lets launch search the first cached paths immediately.
// Incomplete writes never replace the last complete snapshot. Cache failures do not fail search.
private enum SearchIndexCache {
  private struct Row: Codable {
    let path: String
    let folder: Bool
  }
  private struct Record: Codable {
    let root: String
    let rows: [Row]
  }
  private static func url(_ root: FolderID) -> URL {
    FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
      .appendingPathComponent(
        "studio.crawler.native-prototype/"
          + (root == .home ? "Search-v1.jsonl" : "Search-mac-v1.jsonl"))
  }
  private static let magic = Data("CrawlerIndex0002".utf8)
  private static func preparedURL(_ root: FolderID) -> URL {
    url(root).deletingPathExtension().appendingPathExtension("fff3.bin")
  }
  static func hasPreparedCache(_ root: FolderID) -> Bool {
    FileManager.default.fileExists(atPath: preparedURL(root).path)
  }
  // Version 4 adds the saved event to the header. Versions 3 and 4 align prepared tables.
  private static let fffMagic = Data("CrawlerFFF000004".utf8)
  private static let alignedMagic = Data("CrawlerFFF000003".utf8)
  private static let unalignedMagic = ["CrawlerFFF000002", "CrawlerFFF000001"].map { Data($0.utf8) }
  private struct Header {
    let aligned: Bool
    let event: IndexEvent?
    let blocks: Int
    let size: Int
  }
  private static func header(_ data: Data, root: FolderID) throws -> Header {
    var position = 16
    func take(_ count: Int) throws -> Range<Int> {
      guard count >= 0, position <= data.count, count <= data.count - position else {
        throw CocoaError(.fileReadCorruptFile)
      }
      let range = position..<(position + count)
      position += count
      return range
    }
    func number<T: FixedWidthInteger>(_ type: T.Type) throws -> T {
      let range = try take(MemoryLayout<T>.size)
      return data.withUnsafeBytes { T(littleEndian: $0.loadUnaligned(fromByteOffset: range.lowerBound, as: T.self)) }
    }
    let magic = data.prefix(16)
    guard data.count >= 32, magic == fffMagic || magic == alignedMagic || unalignedMagic.contains(magic) else {
      throw CocoaError(.fileReadCorruptFile)
    }
    let rootLength = Int(try number(UInt32.self)), revisionLength = Int(try number(UInt32.self))
    let blocks = Int(try number(UInt32.self))
    guard try number(UInt32.self) == 0 else { throw CocoaError(.fileReadCorruptFile) }
    var event: IndexEvent?
    if magic == fffMagic {
      let id = try number(UInt64.self), seconds = try number(UInt64.self)
      if id > 0 { event = IndexEvent(id: id, date: Date(timeIntervalSince1970: TimeInterval(seconds))) }
    }
    let rootRange = try take(rootLength), revisionRange = try take(revisionLength)
    guard String(data: data[rootRange], encoding: .utf8) == root.rawValue,
      String(data: data[revisionRange], encoding: .utf8) == FFFSearch.revision else { throw CocoaError(.fileReadCorruptFile) }
    _ = try take((16 - position % 16) % 16)
    return Header(aligned: magic == fffMagic || magic == alignedMagic, event: event, blocks: blocks, size: position)
  }
  /// The event saved with the prepared cache, read from its header alone.
  static func savedEvent(_ root: FolderID) -> IndexEvent? {
    guard let file = try? FileHandle(forReadingFrom: preparedURL(root)) else { return nil }
    defer { try? file.close() }
    guard let data = try? file.read(upToCount: 4096) else { return nil }
    return (try? header(data, root: root))?.event
  }

  /// Returns the event saved with this cache, if its format records one.
  private static func loadFFF(_ root: FolderID, from url: URL, query: String,
    batch: @Sendable (SearchIndex) async -> Void) async throws -> IndexEvent? {
    let cache = try FFFMappedCache(url: url)
    defer { withExtendedLifetime(cache) {} }
    let data = cache.data
    let header = try header(data, root: root)
    guard header.blocks <= data.count / 16 else { throw CocoaError(.fileReadCorruptFile) }
    let alignedBlocks = header.aligned, blocks = header.blocks
    var position = header.size
    func take(_ count: Int) throws -> Range<Int> {
      guard count >= 0, position <= data.count, count <= data.count - position else {
        throw CocoaError(.fileReadCorruptFile)
      }
      let range = position..<(position + count)
      position += count
      return range
    }
    func number() throws -> Int {
      let range = try take(4)
      return data.withUnsafeBytes { Int(UInt32(littleEndian: $0.loadUnaligned(fromByteOffset: range.lowerBound, as: UInt32.self))) }
    }
    let queryBytes = Array(SearchText.normalized(query).utf8)
    var queryBits = Set<Int>()
    if queryBytes.count >= 3 {
      for i in 0...(queryBytes.count - 3) where !queryBytes[i...(i + 2)].contains(32) {
        let value = UInt32(queryBytes[i]) << 16 | UInt32(queryBytes[i + 1]) << 8 | UInt32(queryBytes[i + 2])
        queryBits.insert(Int((value &* 2654435761) >> 16))
      }
    }
    struct Block {
      let count: Int
      let paths: Range<Int>
      let prepared: Range<Int>
      let score: Int
      let order: Int
    }
    var descriptors: [Block] = []
    for order in 0..<blocks {
      try Task.checkCancellation()
      let count = try number(), pathSize = try number(), preparedSize = try number()
      let signatureSize = try number()
      guard signatureSize == 0 || signatureSize == 8192, count > 0, count <= 2048 else { throw CocoaError(.fileReadCorruptFile) }
      let paths = try take(pathSize)
      if alignedBlocks { _ = try take((16 - position % 16) % 16) }
      let prepared = try take(preparedSize)
      let signature = try take(signatureSize)
      let score = signatureSize == 0 ? 0 : data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
        queryBits.reduce(0) { $0 + ((raw[signature.lowerBound + $1 / 8] & (1 << ($1 % 8))) == 0 ? 0 : 1) }
      }
      descriptors.append(Block(count: count, paths: paths, prepared: prepared, score: score, order: order))
    }
    guard position == data.count else { throw CocoaError(.fileReadCorruptFile) }
    // Hints affect arrival order only. Every snapshot is still restored and
    // scored through FFF, including typos, abbreviations and false positives.
    descriptors.sort { $0.score == $1.score ? $0.order < $1.order : $0.score > $1.score }
    for block in descriptors {
      try Task.checkCancellation()
      let count = block.count, paths = block.paths, prepared = block.prepared
      let entries = try data.withUnsafeBytes { raw in
        var entries: [SearchEntries.Record] = []
        entries.reserveCapacity(count)
        var offset = paths.lowerBound
        for _ in 0..<count {
          guard offset <= paths.upperBound, paths.upperBound - offset >= 8 else { throw CocoaError(.fileReadCorruptFile) }
          let length = Int(UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: offset, as: UInt32.self)))
          let flag = UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: offset + 4, as: UInt32.self))
          offset += 8
          guard length <= paths.upperBound - offset, flag <= 1 else { throw CocoaError(.fileReadCorruptFile) }
          let bytes = UnsafeRawBufferPointer(rebasing: raw[offset..<(offset + length)])
          let encoding = crawler_fff_validate_path(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count)
          guard encoding != 0 else { throw CocoaError(.fileReadCorruptFile) }
          entries.append(SearchEntries.Record(offset: UInt32(offset - paths.lowerBound),
            length: UInt32(length), folder: flag == 1, ascii: encoding == 1))
          offset += length
        }
        guard offset == paths.upperBound else { throw CocoaError(.fileReadCorruptFile) }
        return entries
      }
      await batch(try SearchIndex(root: root, entries: .mapped(cache, paths, entries), cache: cache, range: prepared))
    }
    return header.event
  }
  private static func validRelativePath(_ bytes: UnsafeRawBufferPointer) -> Bool {
    guard !bytes.isEmpty, bytes[0] != 47 else { return false }
    var componentStart = 0
    for i in 0...bytes.count {
      if i < bytes.count, bytes[i] == 0 { return false }
      if i == bytes.count || bytes[i] == 47 {
        let length = i - componentStart
        if length == 1, bytes[componentStart] == 46 { return false }
        if length == 2, bytes[componentStart] == 46, bytes[componentStart + 1] == 46 {
          return false
        }
        componentStart = i + 1
      }
    }
    return true
  }
  private static func loadPrepared(
    _ root: FolderID, from url: URL,
    batch: @Sendable (SearchIndex) async -> Void
  ) async throws {
    let data = try Data(contentsOf: url, options: .alwaysMapped)
    var position = 0
    func take(_ count: Int) throws -> Range<Int> {
      guard count >= 0, position <= data.count, count <= data.count - position else {
        throw CocoaError(.fileReadCorruptFile)
      }
      let range = position..<(position + count)
      position += count
      return range
    }
    func number<T: FixedWidthInteger>(_ type: T.Type) throws -> T {
      let range = try take(MemoryLayout<T>.size)
      return data.withUnsafeBytes {
        T(littleEndian: $0.loadUnaligned(fromByteOffset: range.lowerBound, as: T.self))
      }
    }
    func aligned() throws { _ = try take((16 - position % 16) % 16) }
    guard data.count >= 32, data.prefix(16) == magic else { throw CocoaError(.fileReadCorruptFile) }
    position = 16
    let rootLength = Int(try number(UInt32.self))
    let blocks = Int(try number(UInt32.self))
    _ = try number(UInt64.self)
    let rootRange = try take(rootLength)
    guard String(data: data[rootRange], encoding: .utf8) == root.rawValue else {
      throw CocoaError(.fileReadCorruptFile)
    }
    try aligned()
    guard blocks <= data.count / 16 else { throw CocoaError(.fileReadCorruptFile) }
    for _ in 0..<blocks {
      try Task.checkCancellation()
      let count = Int(try number(UInt32.self))
      let pathSize = Int(try number(UInt32.self))
      let byteSize = Int(try number(UInt32.self))
      guard try number(UInt32.self) == 0, count <= 2048, byteSize % 16 == 0 else {
        throw CocoaError(.fileReadCorruptFile)
      }
      let records = try take(count * 32)
      let paths = try take(pathSize)
      try aligned()
      _ = try take(byteSize)
      _ = try take(byteSize)
      let entries = try autoreleasepool {
        var entries: [SearchEntry] = []
        entries.reserveCapacity(count)
        for i in 0..<count {
          let record = records.lowerBound + i * 32
          let fields = data.withUnsafeBytes { raw in
            func value(_ offset: Int) -> Int {
              Int(
                UInt32(
                  littleEndian: raw.loadUnaligned(fromByteOffset: record + offset, as: UInt32.self))
              )
            }
            return (
              raw.loadUnaligned(fromByteOffset: record, as: UInt64.self).littleEndian,
              value(8), value(12), value(16), value(20), value(24), value(28)
            )
          }
          let offset = fields.1
          let length = fields.2
          let filename = fields.3
          let pathOffset = fields.4
          let pathLength = fields.5
          let flag = fields.6
          let padded = ((length + 15) / 16) * 16
          guard offset % 16 == 0, length > 0, offset <= byteSize, padded <= byteSize - offset,
            filename <= length, pathOffset <= pathSize, pathLength <= pathSize - pathOffset,
            flag <= 1,
            let path = data.withUnsafeBytes({
              let raw = UnsafeRawBufferPointer(
                rebasing: $0[
                  (paths.lowerBound + pathOffset)..<(paths.lowerBound + pathOffset + pathLength)])
              return validRelativePath(raw) ? String(bytes: raw, encoding: .utf8) : nil
            })
          else { throw CocoaError(.fileReadCorruptFile) }
          entries.append(
            SearchEntry(path: path, isFolder: flag == 1))
        }
        return entries
      }
      await batch(
        SearchIndex(root: root, paths: entries.map { ($0.path, $0.isFolder) }))
    }
    guard position == data.count else { throw CocoaError(.fileReadCorruptFile) }
  }
  /// Returns the event saved with the loaded cache; older formats and fallbacks have none.
  @discardableResult
  static func load(
    _ root: FolderID, from cacheURL: URL? = nil, query: String = "", batch: @Sendable (SearchIndex) async -> Void
  ) async throws -> IndexEvent? {
    let prepared = cacheURL ?? preparedURL(root)
    let isFFF = ["fff.bin", "fff2.bin", "fff3.bin"].contains { prepared.lastPathComponent.hasSuffix($0) }
    if isFFF, FileManager.default.fileExists(atPath: prepared.path) {
      do {
        return try await loadFFF(root, from: prepared, query: query, batch: batch)
      } catch is CancellationError { throw CancellationError() }
      catch { if cacheURL != nil { throw error } }
      // A damaged or old-version app cache falls back to its original paths.
    }
    if !isFFF, prepared.pathExtension == "bin", FileManager.default.fileExists(atPath: prepared.path) {
      try await loadPrepared(root, from: prepared, batch: batch)
      return nil
    }
    if cacheURL == nil, !FileManager.default.fileExists(atPath: prepared.path) {
      for suffix in ["fff2.bin", "fff.bin"] {
        let earlierFFF = url(root).deletingPathExtension().appendingPathExtension(suffix)
        if FileManager.default.fileExists(atPath: earlierFFF.path) {
          return try await loadFFF(root, from: earlierFFF, query: query, batch: batch)
        }
      }
      let legacy = url(root).deletingPathExtension().appendingPathExtension("bin")
      let jsonFFF = url(root).deletingPathExtension().appendingPathExtension("fff.jsonl")
      if !FileManager.default.fileExists(atPath: jsonFFF.path), FileManager.default.fileExists(atPath: legacy.path) {
        try await loadPrepared(root, from: legacy, batch: batch)
        return nil
      }
    }
    let jsonFFF = url(root).deletingPathExtension().appendingPathExtension("fff.jsonl")
    let source = cacheURL ?? (FileManager.default.fileExists(atPath: jsonFFF.path) ? jsonFFF : url(root))
    guard let file = try? FileHandle(forReadingFrom: source) else {
      if cacheURL != nil { throw CocoaError(.fileReadNoSuchFile) }
      if root.rawValue == "/" {
        try await load(.home, query: query) { index in
          let prefix = String(FolderID.home.rawValue.dropFirst()) + "/"
          await batch(SearchIndex(root: root, paths: index.entries.map { (prefix + $0.path, $0.isFolder) }))
        }
      }
      return nil
    }
    defer { try? file.close() }
    var buffer = Data()
    while let data = try file.read(upToCount: 256 * 1024), !data.isEmpty {
      try Task.checkCancellation()
      buffer.append(data)
      while let end = buffer.firstIndex(of: 10) {
        let line = buffer[..<end]
        let record = try JSONDecoder().decode(Record.self, from: line)
        guard record.root == root.rawValue,
          record.rows.allSatisfy({ row in
            !row.path.isEmpty && !row.path.hasPrefix("/")
              && !row.path.split(separator: "/").contains(where: { $0 == ".." || $0 == "." })
          })
        else { return nil }
        await batch(SearchIndex(root: root, paths: record.rows.map { ($0.path, $0.folder) }))
        buffer.removeSubrange(...end)
      }
    }
    return nil
  }
  static func save(_ snapshot: SearchSnapshot, root: FolderID) throws {
    let target = preparedURL(root)
    try FileManager.default.createDirectory(
      at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
    let temporary = target.deletingLastPathComponent().appendingPathComponent(UUID().uuidString)
    FileManager.default.createFile(atPath: temporary.path, contents: nil)
    defer { try? FileManager.default.removeItem(at: temporary) }
    let file = try FileHandle(forWritingTo: temporary)
    defer { try? file.close() }
    func appendNumber(_ value: Int, to data: inout Data) {
      var number = UInt32(value).littleEndian
      withUnsafeBytes(of: &number) { data.append(contentsOf: $0) }
    }
    var header = fffMagic
    let rootBytes = Data(root.rawValue.utf8), revision = Data(FFFSearch.revision.utf8)
    appendNumber(rootBytes.count, to: &header); appendNumber(revision.count, to: &header)
    appendNumber(snapshot.batches.count, to: &header); appendNumber(0, to: &header)
    for value in [snapshot.event?.id ?? 0, UInt64(max(0, snapshot.event?.date.timeIntervalSince1970 ?? 0))] {
      withUnsafeBytes(of: value.littleEndian) { header.append(contentsOf: $0) }
    }
    header.append(rootBytes); header.append(revision)
    header.append(contentsOf: repeatElement(UInt8(0), count: (16 - header.count % 16) % 16))
    try file.write(contentsOf: header)
    var offset = header.count
    let home = String(FolderID.home.rawValue.dropFirst()) + "/"
    func priority(_ index: SearchIndex) -> Int {
      index.entries.map { entry in
        guard entry.path.hasPrefix(home) else { return 3 }
        let local = entry.path.dropFirst(home.count)
        if local.hasPrefix("Library/") || local.hasPrefix("go/") || local.hasPrefix("golang/") { return 2 }
        return 0
      }.min() ?? 3
    }
    // Reorder whole snapshots only, preserving FFF's exact candidate groups.
    let priorities = snapshot.batches.map(priority)
    let ordered = snapshot.batches.indices.sorted {
      priorities[$0] == priorities[$1] ? $0 < $1 : priorities[$0] < priorities[$1]
    }
    for block in ordered {
      try Task.checkCancellation()
      let index = snapshot.batches[block]
      var paths = Data()
      for entry in index.entries {
        let bytes = Array(entry.path.utf8)
        appendNumber(bytes.count, to: &paths); appendNumber(entry.isFolder ? 1 : 0, to: &paths)
        paths.append(contentsOf: bytes)
      }
      let prepared = try index.preparedBytes()
      let signature = try index.signatureBytes()
      var blockHeader = Data()
      appendNumber(index.entries.count, to: &blockHeader); appendNumber(paths.count, to: &blockHeader)
      appendNumber(prepared.count, to: &blockHeader); appendNumber(signature.count, to: &blockHeader)
      try file.write(contentsOf: blockHeader)
      try file.write(contentsOf: paths)
      offset += blockHeader.count + paths.count
      let padding = (16 - offset % 16) % 16
      try file.write(contentsOf: Data(repeating: 0, count: padding))
      try file.write(contentsOf: prepared)
      try file.write(contentsOf: signature)
      offset += padding + prepared.count + signature.count
    }
    try file.synchronize()
    // POSIX rename atomically replaces only this app-owned cache.
    if rename(temporary.path, target.path) != 0 {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
  }
}

// One recursive FSEvents stream, rather than a descriptor for every directory.
// The stream uses the main queue; its callback only schedules background work.
// Started from a saved event, it first replays the changes made since then and
// calls `historyDone` once it reaches the present.
@MainActor
private final class SearchWatcher {
  private var stream: FSEventStreamRef?
  let changed: ([String]?) -> Void
  private let historyDone: () -> Void
  let root: String
  var excludedPaths = Set<String>()
  /// The newest event delivered, including ones the filters below ignore.
  private(set) var lastEventID: FSEventStreamEventId = 0
  private let cachePath = FolderID.home.url
    .appendingPathComponent("Library/Caches/studio.crawler.native-prototype").path
  init(physicalRoot: String, since: FSEventStreamEventId? = nil,
    changed: @escaping ([String]?) -> Void, historyDone: @escaping () -> Void = {}) {
    self.changed = changed
    self.historyDone = historyDone
    self.root = physicalRoot
    var context = FSEventStreamContext(
      version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
      retain: nil, release: nil, copyDescription: nil)
    let callback: FSEventStreamCallback = { _, info, count, paths, flags, ids in
      guard let info else { return }
      let watcher = Unmanaged<SearchWatcher>.fromOpaque(info).takeUnretainedValue()
      let paths = unsafeBitCast(paths, to: NSArray.self)
      MainActor.assumeIsolated {
        let needsScan = FSEventStreamEventFlags(
          kFSEventStreamEventFlagMustScanSubDirs
            | kFSEventStreamEventFlagRootChanged | kFSEventStreamEventFlagUserDropped
            | kFSEventStreamEventFlagKernelDropped)
        var changed: [String] = []
        var rescan = false
        var caughtUp = false
        for i in 0..<count {
          if flags[i] & FSEventStreamEventFlags(kFSEventStreamEventFlagHistoryDone) != 0 {
            caughtUp = true
            continue
          }
          watcher.lastEventID = max(watcher.lastEventID, ids[i])
          guard !rescan, let path = paths[i] as? String else { continue }
          if path == watcher.cachePath || path.hasPrefix(watcher.cachePath + "/") { continue }
          let rootPrefix = watcher.root == "/" ? "/" : watcher.root + "/"
          let relative = path.hasPrefix(rootPrefix) ? String(path.dropFirst(rootPrefix.count)) : ""
          if watcher.excludedPaths.contains(where: { relative.hasPrefix($0 + "/") }) {
            continue
          }
          if flags[i] & needsScan != 0 {
            rescan = true
            continue
          }
          let canonical = path.hasSuffix("/") && path != "/" ? String(path.dropLast()) : path
          let relevant = FSEventStreamEventFlags(
            kFSEventStreamEventFlagItemCreated
              | kFSEventStreamEventFlagItemRemoved | kFSEventStreamEventFlagItemRenamed
              | kFSEventStreamEventFlagItemFinderInfoMod | kFSEventStreamEventFlagItemInodeMetaMod)
          // The index contains names and kinds, so content-only writes require no rescan.
          if flags[i] & relevant == 0 { continue }
          if canonical == watcher.root {
            rescan = true
            continue
          }
          let prefix = watcher.root == "/" ? "/" : watcher.root + "/"
          if canonical.hasPrefix(prefix) {
            let relative = String(canonical.dropFirst(prefix.count))
            // Match the initial scan's hidden-file policy.
            if !relative.split(separator: "/").contains(where: { $0.hasPrefix(".") }) {
              changed.append(relative)
            }
          }
        }
        if rescan {
          watcher.changed(nil)
        } else if !changed.isEmpty {
          watcher.changed(changed)
        }
        if caughtUp { watcher.historyDone() }
      }
    }
    stream = FSEventStreamCreate(
      nil, callback, &context, [self.root] as CFArray,
      since ?? FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.2,
      FSEventStreamCreateFlags(
        kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents
          | kFSEventStreamCreateFlagWatchRoot | kFSEventStreamCreateFlagNoDefer))
    if let stream {
      FSEventStreamSetDispatchQueue(stream, .main)
      if !FSEventStreamStart(stream) {
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
        self.stream = nil
      }
    }
  }
  var active: Bool { stream != nil }
  func stop() {
    guard let stream else { return }
    FSEventStreamStop(stream)
    FSEventStreamInvalidate(stream)
    FSEventStreamRelease(stream)
    self.stream = nil
  }
}

@MainActor
final class QuickFileSearch {
  let performance = SearchPerformance()
  nonisolated static let workerLimit: Int = {
    let arguments = CommandLine.arguments
    let override = arguments.firstIndex(of: "--search-workers").flatMap { i in
      arguments.indices.contains(i + 1) ? Int(arguments[i + 1]) : nil
    }
    return max(1, min(override ?? 12, ProcessInfo.processInfo.activeProcessorCount))
  }()
  private let indexRoot = FolderID("/")
  private(set) var mode: FileSearchMode = .computer
  var isComputerSearch: Bool {
    mode == .computer && !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }
  var resultRoot: FolderID? { root == nil ? nil : indexRoot }
  func search(_ text: String, mode: FileSearchMode) {
    self.mode = mode
    search(text)
  }
  enum State {
    case idle
    case indexing(Int)
    case ready
    case failed(String)
  }
  private(set) var state: State = .idle
  private(set) var root: FolderID?
  private(set) var response = SearchResponse(hits: [], total: 0, milliseconds: 0)
  private(set) var responseIsCurrent = false
  private(set) var query = ""
  private(set) var computerScope: FolderID?
  private(set) var volumeRoot: FolderID?
  var activeSearchScope: FolderID? {
    mode == .computer ? computerScope ?? volumeRoot : scope ?? root
  }

  func setVolumeRoot(_ folder: FolderID?) {
    guard volumeRoot != folder else { return }
    volumeWatcher?.stop()
    volumeWatcher = nil
    volumeRefreshTask?.cancel()
    volumeRoot = folder
    if let folder {
      volumeWatcher = SearchWatcher(physicalRoot: folder.rawValue) { [weak self] _ in
        self?.scheduleVolumeRefresh()
      }
    }
    computerScope = nil
    startVolumeDiscovery()
    search(query)
  }

  private var volumeWatcher: SearchWatcher?
  private var volumeRefreshTask: Task<Void, Never>?

  private func scheduleVolumeRefresh() {
    volumeRefreshTask?.cancel()
    volumeRefreshTask = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(220)) } catch { return }
      guard let self, self.volumeRoot != nil else { return }
      self.startVolumeDiscovery()
      self.search(self.query)
    }
  }

  func volumesChanged() {
    startVolumeDiscovery()
    scheduleRefresh(["Volumes"])
    search(query)
  }
  private(set) var scope: FolderID?
  private var scopeCounts: [String: Int] = [:]
  private(set) var searching = false
  private(set) var indexMilliseconds: Double = 0
  private var batches: [SearchIndex] = [] { didSet { invalidateQueries() } }
  private var aliasScope: FolderID?
  private var aliasBatches: [SearchIndex] = [] { didSet { invalidateQueries() } }
  private var aliasTask: Task<Void, Never>?
  private var aliasWarning: String?
  private var volumeBatches: [SearchIndex] = [] { didSet { invalidateQueries() } }
  private var volumeTask: Task<Void, Never>?
  private var volumeWarning: String?
  private var volumeDiscovering = false
  private var volumeScanID = UUID()
  private var searchedAliasBatches = 0
  private var searchedVolumeBatches = 0

  private var warning: String?
  private var cacheLoaded = false
  private var cacheDirty = false
  private var checkpointTask: Task<Void, Never>?
  private var cacheTask: Task<Void, Never>?
  private var pendingCache: SearchSnapshot?
  private var firstResultStart: ContinuousClock.Instant?
  private(set) var firstResultMilliseconds: Double = 0
  private var queryStart = ContinuousClock.now
  private var fuzzyFallback = false
  private var searchedBatches = 0
  private var best: [SearchHit] = []
  private var matchCount = 0
  private var scanTask: Task<Void, Never>?
  private var searchTask: Task<Void, Never>?
  private var refreshTask: Task<Void, Never>?
  private var watcher: SearchWatcher?
  private var watcherTask: Task<Void, Never>?
  private var watcherID = UUID()
  private var scanID = UUID(), queryID = UUID()
  private var pendingFullScan = false
  private var pendingPaths = Set<String>()
  /// Changed paths of the running scan (nil for a full walk) and the event it brings the index up to.
  private var scanRequest: [String]?
  private var scanThrough: IndexEvent?
  /// Batches reflect every change up to this event. Saved with the cache, it lets the next
  /// start replay later events instead of walking the disk; nil until a walk completes.
  private var indexEvent: IndexEvent?
  /// The event in the cache file, so an unchanged index still refreshes a stale one.
  private var savedEvent: IndexEvent?
  /// Saved history still arriving after a resumed start, and the task enforcing its time limit.
  private var replay: (start: ContinuousClock.Instant, limit: Task<Void, Never>)?
  /// Saved events older than this cost about as much to replay as a walk.
  private static let replayAge: TimeInterval = 24 * 3600
  /// The longest wait for saved history before walking the disk instead.
  private static let replayLimit: Duration = .seconds(30)
  /// Pending changed paths beyond this are covered by one full walk, bounding memory and reconciliation time.
  private static let pendingLimit = 50_000
  /// How the index last caught up with the disk, for the state report.
  private(set) var catchUp: [String: Any] = [:]
  private var presented = false
  private var idleTask: Task<Void, Never>?
  private var measuring = false
  private struct CachedQuery {
    let text: String
    let mode: FileSearchMode
    let scope: FolderID?
    let volume: FolderID?
    let boosts: [String: Int16]
    let response: SearchResponse
    let fuzzy: Bool
  }
  private var queryCache: [CachedQuery] = []
  private var queryBoosts: [String: Int16] = [:]
  private var bypassQueryCache = false
  private(set) var queryCacheHits = 0
  private(set) var queryCacheMisses = 0
  var queryCacheCount: Int { queryCache.count }
  private func invalidateQueries() { queryCache.removeAll(keepingCapacity: true) }
  private func restoreQuery() -> Bool {
    guard !bypassQueryCache, let position = queryCache.firstIndex(where: {
      $0.text == SearchText.normalized(query) && $0.mode == mode
        && $0.scope == activeSearchScope && $0.volume == volumeRoot && $0.boosts == queryBoosts
    }) else { queryCacheMisses += 1; return false }
    let cached = queryCache.remove(at: position)
    queryCache.insert(cached, at: 0)
    queryCacheHits += 1
    fuzzyFallback = cached.fuzzy
    searchedBatches = batches.count
    searchedAliasBatches = mode == .folder ? aliasBatches.count : 0
    searchedVolumeBatches = volumeRoot == nil ? 0 : volumeBatches.count
    best = cached.response.hits
    matchCount = cached.response.total
    scopeCounts = cached.response.scopes
    let duration = queryStart.duration(to: .now).components
    let ms = Double(duration.seconds) * 1000 + Double(duration.attoseconds) / 1e15
    response = SearchResponse(hits: best, total: matchCount, milliseconds: ms, scopes: scopeCounts)
    responseIsCurrent = true
    replacingResults = false
    firstResultStart = nil
    firstResultMilliseconds = best.isEmpty ? 0 : ms
    performance.published(complete: true, cacheHit: true)
    return true
  }
  private func rememberQuery() {
    guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      !searching, aliasTask == nil, !volumeDiscovering
    else { return }
    if !cacheLoaded { guard case .ready = state else { return } }
    let text = SearchText.normalized(query)
    queryCache.removeAll { $0.text == text && $0.mode == mode && $0.scope == activeSearchScope && $0.volume == volumeRoot }
    queryCache.insert(CachedQuery(text: text, mode: mode, scope: activeSearchScope,
      volume: volumeRoot, boosts: queryBoosts, response: response, fuzzy: fuzzyFallback), at: 0)
    if queryCache.count > 8 { queryCache.removeLast() }
  }
  private var indexNeeded: Bool {
    !measuring && presented && query.utf8.count <= 256
      && !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  private func pauseIndex() {
    performance.cancel()
    searchTask?.cancel()
    searchTask = nil
    queryID = UUID()
    searching = false
    // Keep an interrupted scan's work; the next search start resumes it.
    if scanTask != nil {
      if let paths = scanRequest { pendingPaths.formUnion(paths) } else { pendingFullScan = true }
    }
    scanTask?.cancel()
    scanTask = nil
    scanID = UUID()
    watcherTask?.cancel()
    watcherTask = nil
    watcherID = UUID()
    refreshTask?.cancel()
    refreshTask = nil
    checkpointTask?.cancel()
    checkpointTask = nil
    if aliasTask != nil {
      aliasScope = nil
      aliasBatches = []
    }
    aliasTask?.cancel()
    aliasTask = nil
    volumeTask?.cancel()
    volumeTask = nil
    volumeScanID = UUID()
    volumeDiscovering = false
    volumeRefreshTask?.cancel()
    volumeRefreshTask = nil
    guard idleTask == nil else { return }
    idleTask = Task { [weak self] in
      do { try await Task.sleep(for: .seconds(60)) } catch { return }
      guard let self, !self.indexNeeded else { return }
      self.releaseIndex()
    }
  }

  private func releaseIndex() {
    pauseIndex()
    idleTask?.cancel()
    idleTask = nil
    let writer = cacheTask
    writer?.cancel()
    cacheTask = nil
    pendingCache = nil
    watcher?.stop()
    watcher = nil
    volumeWatcher?.stop()
    volumeWatcher = nil
    batches = []
    aliasBatches = []
    aliasScope = nil
    aliasWarning = nil
    volumeBatches = []
    volumeWarning = nil
    indexedCount = 0
    cacheLoaded = false
    cacheDirty = false
    warning = nil
    state = .idle
    best = []
    scopeCounts = [:]
    matchCount = 0
    searchedBatches = 0
    searchedAliasBatches = 0
    searchedVolumeBatches = 0
    // Keep bounded old rows until the next publication, without retaining any index buffer.
    response = SearchResponse(hits: response.hits, total: 0, milliseconds: 0)
    responseIsCurrent = false
    pendingPaths.removeAll()
    pendingFullScan = false
    endReplay()
    indexEvent = nil
    savedEvent = nil
    // Request return of free allocator pages only after cache writing has released its snapshot.
    Task.detached(priority: .utility) {
      await writer?.value
      malloc_zone_pressure_relief(nil, 0)
    }
    didChange?()
  }
  private var visits: [String: SearchVisit] = [:]
  var didChange: (() -> Void)?

  /// Diagnostic of the real loader and matcher without UI or discovery. Cache conversion is explicit.
  func measureCacheReload(query: String, cacheURL: URL? = nil) async throws
    -> SearchReloadMeasurement
  {
    precondition(batches.isEmpty && root == nil)
    func resources() -> (cpuMS: Double, footprintMiB: Double) {
      var usage = rusage_info_v4()
      let result = withUnsafeMutablePointer(to: &usage) { pointer in
        pointer.withMemoryRebound(to: Optional<rusage_info_t>.self, capacity: 1) {
          proc_pid_rusage(getpid(), RUSAGE_INFO_V4, $0)
        }
      }
      precondition(result == 0)
      var timebase = mach_timebase_info_data_t()
      mach_timebase_info(&timebase)
      return (
        Double(usage.ri_user_time + usage.ri_system_time) * Double(timebase.numer)
          / Double(timebase.denom) / 1_000_000,
        Double(usage.ri_phys_footprint) / 1_048_576
      )
    }
    func settled() async throws {
      let deadline = ContinuousClock.now.advanced(by: .seconds(120))
      while searching {
        guard ContinuousClock.now < deadline else { throw CocoaError(.userCancelled) }
        try await Task.sleep(for: .milliseconds(2))
      }
    }
    let empty = resources()
    measuring = true
    bypassQueryCache = true
    root = indexRoot
    presented = true
    loadVisits()
    let id = scanID
    let root = indexRoot
    let start = ContinuousClock.now
    var firstPath: String?
    var pathsAtFirstResult = 0
    didChange = { [weak self] in
      guard let self, firstPath == nil, self.firstResultMilliseconds > 0,
        let hit = self.response.hits.first
      else { return }
      firstPath = hit.entry.path
      pathsAtFirstResult = self.indexedCount
    }
    search(query)
    let loader = Task.detached(priority: .utility) { [weak self] in
      let unique = SearchDiscoveryMerge(previous: [])
      try await SearchIndexCache.load(root, from: cacheURL, query: query) { [weak self] index in
        if let addition = await unique.additions(index) {
          await self?.append(addition, id: id, fromCache: true)
        }
      }
    }
    try await loader.value
    state = .ready
    if !searching, response.total == 0 { _ = startFuzzyFallback() }
    let duration = start.duration(to: .now).components
    let loadMS = Double(duration.seconds) * 1000 + Double(duration.attoseconds) / 1e15
    guard indexedCount > 0 else { throw CocoaError(.fileReadNoSuchFile) }
    try await settled()
    let coldHits = response.hits
    let coldTotal = response.total
    didChange = nil
    let coldResources = resources()
    let cold = SearchReloadMeasurement.Reading(
      firstResultMS: firstResultMilliseconds,
      completeMS: response.milliseconds, cpuMS: coldResources.cpuMS - empty.cpuMS,
      footprintMiB: coldResources.footprintMiB)
    search(query)
    try await settled()
    let warmResources = resources()
    let warm = SearchReloadMeasurement.Reading(
      firstResultMS: firstResultMilliseconds,
      completeMS: response.milliseconds, cpuMS: warmResources.cpuMS - coldResources.cpuMS,
      footprintMiB: warmResources.footprintMiB)
    var identical =
      coldTotal == response.total && coldHits.count == response.hits.count
      && zip(coldHits, response.hits).allSatisfy {
        $0.entry.path == $1.entry.path && $0.score == $1.score && $0.highlights == $1.highlights
      }
    bypassQueryCache = false
    let hitsBeforeCache = queryCacheHits
    search(query)
    try await settled()
    let cachedResources = resources()
    let cached = SearchReloadMeasurement.Reading(firstResultMS: firstResultMilliseconds,
      completeMS: response.milliseconds, cpuMS: cachedResources.cpuMS - warmResources.cpuMS,
      footprintMiB: cachedResources.footprintMiB)
    identical = identical && queryCacheHits == hitsBeforeCache + 1
      && coldTotal == response.total && coldHits == response.hits
    let pathsCount = indexedCount
    if CommandLine.arguments.contains("--prepare-search-cache") {
      let snapshot = SearchSnapshot(batches: batches, warning: nil)
      try await Task.detached(priority: .utility) {
        try SearchIndexCache.save(snapshot, root: FolderID("/"))
      }.value
    }
    var digest = SHA256()
    for hit in coldHits {
      digest.update(data: Data(hit.entry.path.utf8))
      digest.update(data: Data("\n\(hit.score):\(hit.highlights)\n".utf8))
    }
    let resultDigest = digest.finalize().map { String(format: "%02x", $0) }.joined()
    if CommandLine.arguments.contains("--benchmark-idle-release") {
      dismiss()
      try await Task.sleep(for: .seconds(61))
      guard indexedCount == 0 else { throw CocoaError(.coderInvalidValue) }
    } else {
      releaseIndex()
    }
    try await Task.sleep(for: .milliseconds(500))
    malloc_zone_pressure_relief(nil, 0)
    return SearchReloadMeasurement(
      query: query, indexedPaths: pathsCount, cacheLoadMS: loadMS,
      pathsAtFirstReloadResult: pathsAtFirstResult,
      firstReloadResultWasFinalBest: firstPath == coldHits.first?.entry.path,
      emptyFootprintMiB: empty.footprintMiB, reloaded: cold, resident: warm,
      identicalResults: identical, cached: cached, releasedFootprintMiB: resources().footprintMiB,
      resultDigest: resultDigest, totalMatches: coldTotal)
  }

  private(set) var revision = 0
  private(set) var indexedCount = 0
  var resultsArePartial: Bool {
    if volumeRoot != nil { return volumeDiscovering || searching }
    if case .indexing = state { return true }
    return searching
  }
  var status: String {
    if query.utf8.count > 256 { return "Use a shorter search" }
    if mode == .folder, let aliasWarning { return aliasWarning }
    if let volumeRoot {
      if let volumeWarning { return volumeWarning }
      if volumeDiscovering { return "Discovering folders in \(volumeRoot.url.lastPathComponent)…" }
      if searching { return "Searching…" }
      if volumeWatcher?.active == false { return "Live updates unavailable · ⌘R to refresh" }
      if response.total > FFFSearch.resultLimit {
        return "Showing the best \(FFFSearch.resultLimit) results · Narrow your search"
      }
      return response.total == 0 ? "No matching files or folders" : ""
    }
    switch state {
    case .idle: return "Type a filename or path"
    case .indexing: return "Searching discovered paths · Still discovering folders…"
    case .failed(let error): return error + " · ⌘R to retry"
    case .ready:
      if let warning { return warning }
      if watcher?.active == false { return "Live updates unavailable · ⌘R to refresh" }
      if searching { return "Searching…" }
      if response.total > FFFSearch.resultLimit {
        return "Showing the best \(FFFSearch.resultLimit) results · Narrow your search"
      }
      if response.total == 0 {
        return indexedCount == 0 ? "This folder is empty" : "No matching files or folders"
      }
      return ""
    }
  }

  func present(in root: FolderID) {
    if self.root != root {
      self.root = root
      scope = nil
      loadVisits()
    }
    presented = true
    query = ""
    search("")
  }
  private func startDiscovery() {
    guard indexNeeded else { return }
    let root = indexRoot
    if checkpointTask == nil {
      checkpointTask = Task { [weak self] in
        var savedCount = 0
        while !Task.isCancelled {
          do { try await Task.sleep(for: .seconds(5)) } catch { return }
          guard let self else { return }
          // A partly loaded cache is never saved: it would replace the complete one.
          if self.cacheLoaded && self.cacheDirty && self.indexedCount != savedCount {
            savedCount = self.indexedCount
            self.cacheDirty = false
            self.persist(self.batches)
          }
        }
      }
    }
    if watcher == nil, watcherTask == nil {
      state = .indexing(0)
      watcherID = UUID()
      let id = watcherID
      watcherTask = Task.detached(priority: .utility) { [weak self] in
        let physicalRoot = root.rawValue.withCString { path -> String in
          guard let resolved = realpath(path, nil) else { return root.rawValue }
          defer { free(resolved) }
          return String(cString: resolved)
        }
        let saved = SearchIndexCache.savedEvent(root)
        guard !Task.isCancelled else { return }
        await self?.startWatcher(root: root, physicalRoot: physicalRoot, id: id, saved: saved)
      }
      didChange?()
    }
  }
  private func startWatcher(root: FolderID, physicalRoot: String, id: UUID, saved: IndexEvent?) {
    guard indexRoot == root, watcherID == id else { return }
    watcherTask = nil
    let reason: String
    if let saved, saved.id <= FSEventsGetCurrentEventId(), -saved.date.timeIntervalSinceNow < Self.replayAge {
      let watcher = makeWatcher(physicalRoot, since: saved.id)
      if watcher.active {
        // Load the saved cache and replay the changes made since, instead of walking the disk.
        self.watcher = watcher
        indexEvent = saved
        savedEvent = saved
        let limit = Task { [weak self] in
          do { try await Task.sleep(for: Self.replayLimit) } catch { return }
          self?.walkEverything(because: "history took too long")
        }
        replay = (.now, limit)
        catchUp = ["method": "replay", "saved_hours_ago": -saved.date.timeIntervalSinceNow / 3600]
        scan(changed: [])
        return
      }
      watcher.stop()
      reason = "history unavailable"
    } else {
      reason = saved == nil ? "no saved event" : "saved event too old"
    }
    watcher = makeWatcher(physicalRoot, since: nil)
    catchUp = ["method": "walk", "reason": reason]
    scan()
  }
  private func makeWatcher(_ physicalRoot: String, since: FSEventStreamEventId?) -> SearchWatcher {
    SearchWatcher(physicalRoot: physicalRoot, since: since) { [weak self] paths in
      self?.scheduleRefresh(paths)
    } historyDone: { [weak self] in
      self?.finishReplay()
    }
  }
  /// Saved history reached the present: reconcile its changed paths together.
  private func finishReplay() {
    guard let replay else { return }
    endReplay()
    let duration = replay.start.duration(to: .now).components
    catchUp["replay_ms"] = Double(duration.seconds) * 1000 + Double(duration.attoseconds) / 1e15
    catchUp["replayed_paths"] = pendingPaths.count
    // A running scan picks up the pending paths when it finishes.
    guard scanTask == nil else { return }
    if pendingFullScan || !pendingPaths.isEmpty || !cacheLoaded {
      scanPending()
    } else {
      state = .ready
      didChange?()
    }
  }
  private func endReplay() {
    replay?.limit.cancel()
    replay = nil
  }
  /// Gives up on saved history: watch from now on and walk the whole disk once.
  private func walkEverything(because reason: String) {
    if replay != nil {
      endReplay()
      if let old = watcher {
        old.stop()
        watcher = makeWatcher(old.root, since: nil)
      }
    }
    // Neither the index nor a running scan can claim any event until the walk completes.
    indexEvent = nil
    scanThrough = nil
    pendingPaths.removeAll()
    pendingFullScan = true
    catchUp["method"] = "walk"
    catchUp["reason"] = reason
    // A running scan starts the walk when it finishes.
    if scanTask == nil { scanPending() }
  }
  func resume() {
    presented = true
    search(query)
  }
  func dismiss() {
    presented = false
    pauseIndex()
  }

  func reset() {
    dismiss()
    idleTask?.cancel()
    idleTask = nil
    scanTask?.cancel()
    scanTask = nil
    scanID = UUID()
    refreshTask?.cancel()
    refreshTask = nil
    watcher?.stop()
    watcher = nil
    watcherTask?.cancel()
    watcherTask = nil
    watcherID = UUID()
    batches = []
    aliasTask?.cancel()
    aliasTask = nil
    aliasScope = nil
    aliasBatches = []
    aliasWarning = nil
    indexedCount = 0
    warning = nil
    cacheLoaded = false
    cacheDirty = false
    checkpointTask?.cancel()
    checkpointTask = nil
    cacheTask?.cancel()
    cacheTask = nil
    pendingCache = nil
    endReplay()
    indexEvent = nil
    savedEvent = nil
    scanRequest = nil
    scanThrough = nil
    root = nil
    volumeWatcher?.stop()
    volumeWatcher = nil
    volumeRefreshTask?.cancel()
    volumeRefreshTask = nil
    volumeRoot = nil
    startVolumeDiscovery()
    scope = nil
    scopeCounts = [:]
    state = .idle
    pendingPaths.removeAll()
    pendingFullScan = false
    response = SearchResponse(hits: [], total: 0, milliseconds: 0)
    searchedBatches = 0
    searchedAliasBatches = 0
    searchedVolumeBatches = 0
    best = []
    matchCount = 0
    firstResultMilliseconds = 0
  }
  func refresh() {
    startVolumeDiscovery()
    aliasTask?.cancel()
    aliasScope = nil
    aliasBatches = []
    search(query)
    scanTask?.cancel()
    scanTask = nil
    scan()
  }

  func setComputerScope(_ folder: FolderID?) {
    let folder = folder == indexRoot || folder == volumeRoot ? nil : folder
    if let folder, let volumeRoot,
      folder != volumeRoot && !folder.rawValue.hasPrefix(volumeRoot.rawValue + "/")
    {
      return
    }
    guard computerScope != folder else { return }
    computerScope = folder
    search(query)
  }

  func setScope(_ folder: FolderID?) {
    guard let root else { return }
    let folder = folder == root ? nil : folder
    if let folder, root.rawValue != "/", !folder.rawValue.hasPrefix(root.rawValue + "/") { return }
    guard scope != folder else { return }
    scope = folder
    search(query)
  }

  private var replacingResults = false

  func search(_ text: String, preservingResults: Bool = false) {
    if !preservingResults { performance.begin() }
    replacingResults = preservingResults && query == text && !response.hits.isEmpty
    query = text
    let now = Date()
    queryBoosts = visits.mapValues { Int16($0.boost(at: now)) }
    fuzzyFallback = false
    if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { computerScope = nil }
    responseIsCurrent = replacingResults
    searchTask?.cancel()
    searchTask = nil
    queryID = UUID()
    searchedBatches = 0
    searchedAliasBatches = 0
    searchedVolumeBatches = 0
    best = []
    matchCount = 0
    scopeCounts = [:]
    queryStart = .now
    firstResultStart = queryStart
    firstResultMilliseconds = 0
    if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
      response = SearchResponse(hits: [], total: 0, milliseconds: 0)
    }
    searching = false
    if measuring {
      // Diagnostics use the real loader and matcher without filesystem discovery.
    } else if indexNeeded {
      idleTask?.cancel()
      idleTask = nil
      startDiscovery()
      if watcher != nil, scanTask == nil { scanPending() }
      if let volumeRoot, volumeWatcher == nil {
        volumeWatcher = SearchWatcher(physicalRoot: volumeRoot.rawValue) { [weak self] _ in
          self?.scheduleVolumeRefresh()
        }
      }
      if volumeRoot != nil, volumeTask == nil { startVolumeDiscovery() }
    } else {
      pauseIndex()
    }
    guard text.utf8.count <= 256 else {
      didChange?()
      return
    }
    if mode == .folder && indexNeeded { discoverAliasScope() }
    if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && restoreQuery() {
      didChange?()
      return
    }
    continueSearch()
    didChange?()
  }

  var isHome: Bool {
    root == .home && scope == nil && query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  // Keep open-drive discovery separate from the temporary symlink subtree.
  // Both publish resident batches to the same fuzzy matcher and ranking scheduler.
  private func startVolumeDiscovery() {
    volumeTask?.cancel()
    volumeScanID = UUID()
    volumeBatches = []
    volumeWarning = nil
    volumeDiscovering = false
    guard indexNeeded, let folder = volumeRoot else { return }
    volumeDiscovering = true
    let id = volumeScanID
    volumeTask = Task.detached(priority: .utility) { [weak self] in
      do {
        let snapshot = try await SearchIndexReader.read(folder) { [weak self] index in
          let prefix = String(folder.rawValue.dropFirst()) + "/"
          let mapped = SearchIndex(
            root: FolderID("/"), paths: index.entries.map { (prefix + $0.path, $0.isFolder) })
          guard !Task.isCancelled else { return }
          await self?.appendVolume(mapped, id: id)
        }
        guard !Task.isCancelled else { return }
        await self?.finishVolume(id: id, warning: snapshot.warning)
      } catch is CancellationError {} catch {
        await self?.finishVolume(id: id, warning: error.localizedDescription)
      }
    }
  }
  private func appendVolume(_ index: SearchIndex, id: UUID) {
    guard volumeScanID == id else { return }
    volumeBatches.append(index)
    continueSearch()
    if !searching { didChange?() }
  }
  private func finishVolume(id: UUID, warning: String?) {
    guard volumeScanID == id else { return }
    volumeDiscovering = false
    volumeWarning = warning
    didChange?()
  }

  // Explicitly entered symlinks may point into hidden dependency stores. Keep only
  // the current alias subtree, retaining the entered paths and the same matcher.
  private func discoverAliasScope() {
    guard let folder = scope ?? root, aliasScope != folder else { return }
    aliasTask?.cancel()
    aliasScope = folder
    aliasBatches = []
    aliasWarning = nil
    aliasTask = Task.detached(priority: .utility) { [weak self] in
      let physicalPath = folder.rawValue.withCString { path -> String in
        guard let resolved = realpath(path, nil) else { return folder.rawValue }
        defer { free(resolved) }
        return String(cString: resolved)
      }
      // Foundation canonicalizes /private/var back to /var; realpath detects that alias too.
      guard physicalPath != folder.rawValue else {
        if !Task.isCancelled { await self?.finishAlias(folder: folder) }
        return
      }
      do {
        _ = try await SearchIndexReader.read(folder) { [weak self] index in
          let prefix = String(folder.rawValue.dropFirst()) + "/"
          let mapped = SearchIndex(
            root: FolderID("/"), paths: index.entries.map { (prefix + $0.path, $0.isFolder) })
          guard !Task.isCancelled else { return }
          await self?.appendAlias(mapped, folder: folder)
        }
        if !Task.isCancelled { await self?.finishAlias(folder: folder) }
      } catch is CancellationError {} catch {
        if !Task.isCancelled { await self?.aliasFailed(error.localizedDescription, folder: folder) }
      }
    }
  }
  private func finishAlias(folder: FolderID) {
    if aliasScope == folder { aliasTask = nil }
  }
  private func appendAlias(_ index: SearchIndex, folder: FolderID) {
    guard aliasScope == folder else { return }
    aliasBatches.append(index)
    if mode == .folder { search(query) }
  }
  private func aliasFailed(_ message: String, folder: FolderID) {
    guard aliasScope == folder else { return }
    aliasTask = nil
    aliasWarning = message
    didChange?()
  }

  // Appending a discovery batch never cancels the user's running query or rescans old batches.
  private func continueSearch() {
    guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, presented,
      searchTask == nil,
      query.utf8.count <= 256
    else { return }
    let excluded = volumeRoot.map { String($0.rawValue.dropFirst()) }
    var pending = batches.dropFirst(searchedBatches).map { (index: $0, excluding: excluded) }
    searchedBatches = batches.count
    if mode == .folder {
      pending += aliasBatches.dropFirst(searchedAliasBatches).map { (index: $0, excluding: nil) }
      searchedAliasBatches = aliasBatches.count
    }
    if volumeRoot != nil {
      pending += volumeBatches.dropFirst(searchedVolumeBatches).map { (index: $0, excluding: nil) }
      searchedVolumeBatches = volumeBatches.count
    }
    guard !pending.isEmpty else { return }
    let id = queryID
    let text = SearchText.normalized(query)
    let visits = queryBoosts
    let atomicReplacement = replacingResults
    let prior = best
    let priorCount = matchCount
    let priorScopes = scopeCounts
    let relativeScope = activeSearchScope.map { String($0.rawValue.dropFirst()) } ?? ""
    let literalWords = !fuzzyFallback
    let monitorLiteral = fuzzyFallback && resultsArePartial
    searching = true
    searchTask = Task.detached(priority: .userInitiated) { [weak self] in
      var winners = prior
      var total = priorCount
      var counts = priorScopes
      let start = ContinuousClock.now
      var lastPublish = start
      var winnerPaths = Set(winners.map(\.entry.path))
      func precedes(_ lhs: SearchHit, _ rhs: SearchHit) -> Bool {
        if lhs.score != rhs.score { return lhs.score > rhs.score }
        if lhs.entry.length != rhs.entry.length { return lhs.entry.length < rhs.entry.length }
        return lhs.entry.path < rhs.entry.path
      }
      func merge(_ hits: [SearchHit]) {
        for hit in hits {
          if winners.count == FFFSearch.resultLimit, let worst = winners.last,
            !precedes(hit, worst)
          {
            continue
          }
          if winnerPaths.contains(hit.entry.path),
            let position = winners.firstIndex(where: { $0.entry.path == hit.entry.path })
          {
            if winners[position].score >= hit.score { continue }
            winners.remove(at: position)
          }
          var lower = 0
          var upper = winners.count
          while lower < upper {
            let middle = (lower + upper) / 2
            if precedes(hit, winners[middle]) { upper = middle } else { lower = middle + 1 }
          }
          winners.insert(hit, at: lower)
          winnerPaths.insert(hit.entry.path)
          if winners.count > FFFSearch.resultLimit {
            winnerPaths.remove(winners.removeLast().entry.path)
          }
        }
      }
      enum BatchResult: Sendable { case matches(SearchResponse), literalMatch }
      let evaluate: @Sendable (SearchIndex, String?) -> BatchResult = { index, excluding in
        if monitorLiteral {
          let literal = FFFSearch.search(text, index: index, visits: visits,
            scope: relativeScope, excluding: excluding, literalWords: true)
          if literal.total > 0 { return .literalMatch }
        }
        return .matches(FFFSearch.search(text, index: index, visits: visits,
          scope: relativeScope, excluding: excluding, literalWords: literalWords))
      }
      await withTaskGroup(of: BatchResult.self) { group in
        var next = 0
        let workers = min(Self.workerLimit, pending.count)
        for _ in 0..<workers {
          let index = pending[next]
          next += 1
          group.addTask {
            evaluate(index.index, index.excluding)
          }
        }
        for await batchResult in group {
          guard !Task.isCancelled else { group.cancelAll(); return }
          guard case .matches(let result) = batchResult else {
            await self?.preferLiteral(id: id)
            group.cancelAll()
            return
          }
          merge(result.hits)
          total += result.total
          for (path, count) in result.scopes { counts[path, default: 0] += count }
          if !atomicReplacement && (lastPublish.duration(to: .now) >= .milliseconds(30)
            || (!winners.isEmpty && lastPublish == start)) {
            await self?.publish(winners.map { $0.prepared() }, count: total, scopes: counts, id: id, complete: false)
            lastPublish = .now
          }
          if next < pending.count {
            let index = pending[next]
            next += 1
            group.addTask {
              evaluate(index.index, index.excluding)
            }
          }
        }
      }
      guard !Task.isCancelled else { return }
      let hits = winners.map { $0.prepared() }
      await self?.publish(hits, count: total, scopes: counts, id: id, complete: true)
    }
  }

  private func restartQuery() {
    let first = firstResultMilliseconds
    search(query, preservingResults: true)
    firstResultMilliseconds = first
    if first > 0 { firstResultStart = nil }
  }
  private func preferLiteral(id: UUID) {
    guard queryID == id, fuzzyFallback else { return }
    restartQuery()
  }
  // Cached paths are enough to try typos; a full disk reconciliation need not
  // finish first. Later discoveries still check literal terms and take priority.
  private func startFuzzyFallback() -> Bool {
    guard presented, !fuzzyFallback, query.split(whereSeparator: \.isWhitespace).count > 1,
      aliasTask == nil, !volumeDiscovering else { return false }
    if !cacheLoaded { guard case .ready = state else { return false } }
    fuzzyFallback = true
    searchedBatches = 0
    searchedAliasBatches = 0
    searchedVolumeBatches = 0
    best = []
    matchCount = 0
    scopeCounts = [:]
    searchTask = nil
    continueSearch()
    return searchTask != nil
  }

  private func publish(
    _ hits: [SearchHit], count: Int, scopes: [String: Int], id: UUID, complete: Bool
  ) {
    guard queryID == id, presented, complete || !hits.isEmpty else { return }
    // A reconciled index replaces the same query atomically. Its first batch
    // must not shrink an already populated list back to a handful of results.
    guard !replacingResults || complete else { return }
    if complete, count == 0, startFuzzyFallback() { return }
    responseIsCurrent = true
    let duration = queryStart.duration(to: .now).components
    let milliseconds = Double(duration.seconds) * 1000 + Double(duration.attoseconds) / 1e15
    if !hits.isEmpty, firstResultStart != nil {
      firstResultMilliseconds = milliseconds
      firstResultStart = nil
    }
    response = SearchResponse(hits: hits, total: count, milliseconds: milliseconds, scopes: scopes)
    performance.published(complete: complete, cacheHit: false)
    if complete {
      replacingResults = false
      best = hits
      matchCount = count
      scopeCounts = scopes
      searchTask = nil
      searching = false
      continueSearch()
      rememberQuery()
    }
    didChange?()
  }

  private func append(_ index: SearchIndex, id: UUID, fromCache: Bool = false) {
    guard scanID == id else { return }
    batches.append(index)
    if !fromCache { cacheDirty = true }
    indexedCount += index.entries.count
    continueSearch()
    // Search publications drive the UI while visible; avoid reloading it for every batch.
    if !searching { didChange?() }
  }
  private func excludeEvents(_ paths: [String], id: UUID) {
    guard scanID == id else { return }
    watcher?.excludedPaths.formUnion(paths)
    pendingPaths = pendingPaths.filter { path in
      !paths.contains { path == $0 || path.hasPrefix($0 + "/") }
    }
  }
  private func scheduleRefresh(_ paths: [String]?) {
    if let paths, !pendingFullScan {
      // Reading a pending folder covers everything beneath it.
      for path in paths where !hasPendingAncestor(path) { pendingPaths.insert(path) }
      if pendingPaths.count > Self.pendingLimit {
        pendingFullScan = true
        pendingPaths.removeAll()
      }
    } else if paths == nil {
      pendingFullScan = true
      pendingPaths.removeAll()
    }
    if replay != nil {
      // Replayed paths are reconciled together once the history ends. A full walk makes the rest
      // of the history useless; the stream is replaced after this callback returns.
      if pendingFullScan {
        let reason = paths == nil ? "history needs a full scan" : "too many changes"
        Task { [weak self] in
          guard let self, self.replay != nil else { return }
          self.walkEverything(because: reason)
        }
      }
      return
    }
    refreshTask?.cancel()
    guard indexNeeded else { return }
    refreshTask = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(220)) } catch { return }
      guard let self, self.indexNeeded, self.scanTask == nil else { return }
      self.scanPending()
    }
  }
  private func hasPendingAncestor(_ path: String) -> Bool {
    var end = path.endIndex
    while let slash = path[..<end].lastIndex(of: "/") {
      if pendingPaths.contains(String(path[..<slash])) { return true }
      end = slash
    }
    return false
  }
  private func scanPending() {
    guard indexNeeded else { return }
    if pendingFullScan {
      pendingFullScan = false
      pendingPaths.removeAll()
      scan()
    } else if replay != nil {
      // History is still arriving: load the cache now and reconcile its paths when it ends.
      if !cacheLoaded { scan(changed: []) }
    } else if !pendingPaths.isEmpty || !cacheLoaded {
      let changes = Array(pendingPaths)
      pendingPaths.removeAll()
      scan(changed: changes)
    }
  }
  private func scan(changed: [String]? = nil) {
    guard indexNeeded else { return }
    let root = indexRoot
    scanTask?.cancel()
    scanID = UUID()
    let id = scanID
    if let changed {
      watcher?.excludedPaths.subtract(changed)
    } else {
      watcher?.excludedPaths.removeAll()
    }
    scanRequest = changed
    if changed == nil {
      // The walk starts after this event, so it includes every change up to it.
      scanThrough = watcher?.active == true ? IndexEvent(id: FSEventsGetCurrentEventId(), date: Date()) : nil
    } else if replay == nil, let indexEvent, let latest = watcher?.lastEventID, latest > indexEvent.id {
      // These paths are every change delivered so far that the index does not include yet.
      scanThrough = IndexEvent(id: latest, date: Date())
    } else {
      scanThrough = indexEvent
    }
    let previous = batches
    let initialQuery = query
    let loadCache = !cacheLoaded
    let priorityFolders = [activeSearchScope ?? self.root ?? .home,
      FolderID(FolderID.home.url.appendingPathComponent("Downloads").path),
      FolderID(FolderID.home.url.appendingPathComponent("Desktop").path)]
    state = .indexing(indexedCount)
    let previousWarning = warning
    warning = nil
    didChange?()
    scanTask = Task.detached(priority: .utility) { [weak self] in
      let start = ContinuousClock.now
      do {
        if loadCache {
          let unique = SearchDiscoveryMerge(previous: previous)
          await SearchIndexReader.warm(priorityFolders, root: root) { [weak self] index in
            if let addition = await unique.additions(index) {
              await self?.append(addition, id: id)
            }
          }
          try Task.checkCancellation()
          let saved = try? await SearchIndexCache.load(root, query: initialQuery) { [weak self] index in
            if let addition = await unique.additions(index) {
              await self?.append(addition, id: id, fromCache: true)
            }
          }
          try Task.checkCancellation()
          guard await self?.markCacheLoaded(id: id, event: saved ?? nil) == true else { return }
        }
        let loaded = await self?.residentBatches(id: id) ?? []
        let snapshot = try await SearchIndexReader.read(
          root, previous: loaded, changed: changed, warning: previousWarning,
          excluding: { [weak self] paths in await self?.excludeEvents(paths, id: id) }
        ) { [weak self] index in
          await self?.append(index, id: id)
        }
        try Task.checkCancellation()
        let duration = start.duration(to: .now).components
        await self?.finish(
          snapshot, id: id,
          milliseconds: Double(duration.seconds) * 1000 + Double(duration.attoseconds) / 1e15)
      } catch is CancellationError {} catch { await self?.fail(error.localizedDescription, id: id) }
    }
  }
  private func residentBatches(id: UUID) -> [SearchIndex] { scanID == id ? batches : [] }
  /// Returns false when the scan must stop because the loaded cache cannot be brought up to date.
  private func markCacheLoaded(id: UUID, event: IndexEvent?) -> Bool {
    guard scanID == id else { return false }
    cacheLoaded = true
    if !searching, response.total == 0, startFuzzyFallback() { didChange?() }
    // Saved history replays only onto the cache saved with it, not a damaged or replaced one.
    if scanRequest != nil, let indexEvent, event?.id != indexEvent.id {
      scanTask = nil
      walkEverything(because: "cache changed")
      return false
    }
    if !batches.isEmpty, !SearchIndexCache.hasPreparedCache(indexRoot) {
      persist(batches)
    }
    return true
  }
  private func finish(_ snapshot: SearchSnapshot, id: UUID, milliseconds: Double) {
    guard scanID == id else { return }
    revision += 1
    // Content-only events keep the same paths and FFF snapshots. Preserve cached
    // queries and the active search instead of scoring those snapshots again.
    if snapshot.changed { batches = snapshot.batches }
    indexedCount = batches.reduce(0) { $0 + $1.entries.count }
    warning = snapshot.warning
    indexMilliseconds = milliseconds
    indexEvent = scanThrough
    // A walk always saves its fresh event. An unchanged incremental scan saves only when the
    // file's event is over an hour behind, so the next start replays a short history.
    let behind = indexEvent.map { $0.date.timeIntervalSince(savedEvent?.date ?? .distantPast) } ?? 0
    if snapshot.changed || scanRequest == nil || behind > 3600 { persist(batches) }
    cacheDirty = false
    state = replay == nil ? .ready : .indexing(indexedCount)
    scanTask = nil
    scanRequest = nil
    // Reconciliation replaces cached paths, removing deleted entries and duplicate coverage.
    if presented && snapshot.changed {
      restartQuery()
    } else {
      didChange?()
    }
    scanPending()
  }
  private func persist(_ batches: [SearchIndex]) {
    pendingCache = SearchSnapshot(batches: batches, event: indexEvent)
    savedEvent = indexEvent
    guard cacheTask == nil else { return }
    cacheTask = Task { [weak self] in
      while let self, let snapshot = self.pendingCache {
        self.pendingCache = nil
        let writer = Task.detached(priority: .utility) {
          try? SearchIndexCache.save(snapshot, root: FolderID("/"))
        }
        await withTaskCancellationHandler {
          await writer.value
        } onCancel: {
          writer.cancel()
        }
        guard !Task.isCancelled else { return }
      }
      self?.cacheTask = nil
    }
  }
  private func fail(_ error: String, id: UUID) {
    guard scanID == id else { return }
    state = .failed(error)
    scanTask = nil
    scanRequest = nil
    didChange?()
  }
  func recordOpen(_ hit: SearchHit) {
    invalidateQueries()
    guard let resultRoot else { return }
    let url = resultRoot.url.appendingPathComponent(hit.entry.path).standardizedFileURL
    let prefix = "/"
    let path =
      url.path.hasPrefix(prefix) ? String(url.path.dropFirst(prefix.count)) : hit.entry.path
    visits[path] = SearchVisit(count: min(100, (visits[path]?.count ?? 0) + 1), lastOpened: Date())
    // Bound history across roots, recording file opens and folder/reveal picks.
    var stored = Self.readHistory()
    stored[url.path] = visits[path]
    if stored.count > 1000 {
      stored = Dictionary(
        uniqueKeysWithValues: stored.sorted { $0.value.lastOpened > $1.value.lastOpened }.prefix(
          1000
        ).map { ($0.key, $0.value) })
    }
    if let data = try? JSONEncoder().encode(stored) {
      UserDefaults.standard.set(data, forKey: "quickSearchVisits")
    }
  }
  private static func readHistory() -> [String: SearchVisit] {
    guard let data = UserDefaults.standard.data(forKey: "quickSearchVisits"),
      let decoded = try? JSONDecoder().decode([String: SearchVisit].self, from: data)
    else { return [:] }
    return decoded.filter { $0.value.count > 0 && $0.value.count <= 100 }
  }
  private func loadVisits() {
    let prefix = "/"
    visits = Dictionary(
      uniqueKeysWithValues: Self.readHistory().compactMap { path, visit in
        path.hasPrefix(prefix) ? (String(path.dropFirst(prefix.count)), visit) : nil
      })
  }
}
