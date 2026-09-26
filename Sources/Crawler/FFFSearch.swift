import Foundation
import FFFBridge
import os

struct SearchEntry: Sendable, Equatable {
  let path: String
  let isFolder: Bool
  var length: Int { path.utf8.count }
}

// Metadata and Rust path storage read the same inode, even during atomic cache
// replacement. Rust snapshots retain their own Arc after this reader is released.
final class FFFMappedCache: @unchecked Sendable {
  fileprivate let handle: OpaquePointer
  let data: Data
  private let bytes: UnsafeRawBufferPointer
  init(url: URL) throws {
    guard let handle = url.path.withCString({ crawler_fff_cache_open($0) })
    else { throw CocoaError(.fileReadCorruptFile) }
    let view = crawler_fff_cache_view(handle)
    guard let bytes = view.data, view.count > 0 else {
      crawler_fff_cache_destroy(handle)
      throw CocoaError(.fileReadCorruptFile)
    }
    self.handle = handle
    self.bytes = UnsafeRawBufferPointer(start: bytes, count: view.count)
    data = Data(bytesNoCopy: UnsafeMutableRawPointer(mutating: bytes),
      count: view.count, deallocator: .none)
  }
  func withBytes<T>(in range: Range<Int>, _ body: (UnsafeRawBufferPointer) -> T) -> T {
    precondition(range.lowerBound >= 0 && range.upperBound <= bytes.count)
    return body(UnsafeRawBufferPointer(rebasing: bytes[range]))
  }
  deinit { crawler_fff_cache_destroy(handle) }
}

// An index owns a frozen Rust snapshot. Concurrent workers only borrow it;
// the reference remains alive until every query using it has returned.
private final class FFFSnapshot: @unchecked Sendable {
  let handle: OpaquePointer
  private let historyCache = OSAllocatedUnfairLock(initialState: (key: [String: Int16](), values: [Int16]()))
  init(paths: [String]) {
    var bytes: [UInt8] = []
    bytes.reserveCapacity(paths.reduce(0) { $0 + $1.utf8.count + 1 })
    for path in paths {
      bytes.append(contentsOf: path.utf8)
      bytes.append(0)
    }
    guard let handle = bytes.withUnsafeBufferPointer({
      crawler_fff_create($0.baseAddress, $0.count, paths.count)
    }) else { preconditionFailure("FFF rejected a validated path snapshot") }
    self.handle = handle
  }
  init(cache: FFFMappedCache, range: Range<Int>, count: Int) throws {
    guard let handle = crawler_fff_restore_mapped(cache.handle, range.lowerBound, range.count, count)
    else { throw CocoaError(.fileReadCorruptFile) }
    self.handle = handle
  }
  func serialized() throws -> Data {
    guard let bytes = crawler_fff_serialize(handle) else { throw CocoaError(.fileWriteUnknown) }
    defer { crawler_fff_free_bytes(bytes) }
    return Data(bytes: bytes.pointee.data, count: bytes.pointee.count)
  }
  func signature() throws -> Data {
    guard let bytes = crawler_fff_signature(handle) else { throw CocoaError(.fileWriteUnknown) }
    defer { crawler_fff_free_bytes(bytes) }
    return Data(bytes: bytes.pointee.data, count: bytes.pointee.count)
  }
  func boosts(entries: SearchEntries, history: [String: Int16]) -> [Int16] {
    historyCache.withLock { cache in
      if cache.key != history {
        cache.key = history
        // Match the bounded history against mapped paths, not vice versa.
        let fingerprints = Dictionary(grouping: history.keys, by: SearchEntries.fingerprint)
        let values = entries.indices.map { i -> Int16 in
          guard let paths = fingerprints[entries.fingerprint(at: i)] else { return 0 }
          return paths.first(where: { entries.equal(at: i, to: $0) }).flatMap { history[$0] } ?? 0
        }
        cache.values = values.contains(where: { $0 != 0 }) ? values : []
      }
      return cache.values
    }
  }
  deinit { crawler_fff_destroy(handle) }
}

private enum RootScope: Sendable {
  case uniform(String?), mixed
  init(entries: SearchEntries) {
    guard !entries.isEmpty else { self = .uniform(nil); return }
    if let name = entries.childScope(at: 0, prefix: "") {
      let prefix = name + "/"
      self = entries.indices.allSatisfy { entries.hasPrefix(prefix, at: $0) } ? .uniform(name) : .mixed
    } else {
      self = entries.indices.allSatisfy { entries.childScope(at: $0, prefix: "") == nil } ? .uniform(nil) : .mixed
    }
  }
}

struct SearchIndex: Sendable {
  let root: FolderID
  let entries: SearchEntries
  let warning: String?
  fileprivate let snapshot: FFFSnapshot
  fileprivate let rootScope: RootScope

  init(root: FolderID, paths: [(String, Bool)], warning: String? = nil) {
    self.root = root
    self.warning = warning
    entries = .owned(paths.map { SearchEntry(path: $0.0, isFolder: $0.1) })
    rootScope = RootScope(entries: entries)
    snapshot = FFFSnapshot(paths: paths.map { SearchText.normalized($0.0) })
  }
  init(root: FolderID, entries: SearchEntries, cache: FFFMappedCache, range: Range<Int>) throws {
    self.root = root
    self.entries = entries
    warning = nil
    rootScope = RootScope(entries: entries)
    snapshot = try FFFSnapshot(cache: cache, range: range, count: entries.count)
  }
  func preparedBytes() throws -> Data { try snapshot.serialized() }
  func signatureBytes() throws -> Data { try snapshot.signature() }
}

enum SearchText {
  static func normalized(_ string: String) -> String {
    string.folding(options: [.caseInsensitive, .diacriticInsensitive],
      locale: Locale(identifier: "en_US_POSIX"))
  }
  static func bytes(_ string: String) -> [UInt8] { Array(normalized(string).utf8) }
}

struct SearchHit: Sendable, Equatable {
  let entry: SearchEntry
  let score: Int
  // UTF-16 ranges in the original displayed path, including decomposed accents.
  private enum Matches: Sendable, Equatable { case bytes([NSRange]), text([NSRange]) }
  private let matches: Matches
  var highlights: [NSRange] {
    switch matches {
    case .bytes(let ranges): FFFSearch.originalHighlights(ranges, in: entry.path)
    case .text(let ranges): ranges
    }
  }
  init(entry: SearchEntry, score: Int, highlights: [NSRange]) {
    self.entry = entry; self.score = score; matches = .text(highlights)
  }
  fileprivate init(entry: SearchEntry, score: Int, bytes: [NSRange]) {
    self.entry = entry; self.score = score; matches = .bytes(bytes)
  }
  func prepared() -> SearchHit {
    if case .text = matches { return self }
    return SearchHit(entry: entry, score: score, highlights: highlights)
  }
}

struct SearchResponse: Sendable {
  let hits: [SearchHit]
  let total: Int
  let milliseconds: Double
  var scopes: [String: Int] = [:]
}

struct SearchVisit: Codable, Sendable {
  var count: Int
  var lastOpened: Date
  func boost(at now: Date) -> Int {
    let days = max(0, now.timeIntervalSince(lastOpened) / 86_400)
    return Int(min(60, Double(count) * 6) / (1 + days / 7))
  }
}

enum FFFSearch {
  static let resultLimit = 100
  static var revision: String { String(cString: crawler_fff_revision()) }

  static func search(_ normalizedQuery: String, index: SearchIndex, visits: [String: Int16],
    scope: String = "", excluding: String? = nil, literalWords: Bool = true) -> SearchResponse {
    let start = ContinuousClock.now
    let prefix = scope.isEmpty ? "" : scope + "/"
    let excluded = excluding.map { $0 + "/" }
    let allPaths = scope.isEmpty && excluding == nil
    var candidates: [UInt32] = []
    if !allPaths {
      for id in index.entries.indices {
        if id & 127 == 0 && Task.isCancelled { return SearchResponse(hits: [], total: 0, milliseconds: 0) }
        guard scope.isEmpty || index.entries.hasPrefix(prefix, at: id) else { continue }
        if let excluded, index.entries.hasPrefix(excluded, at: id) { continue }
        candidates.append(UInt32(id))
      }
    }
    let count = allPaths ? index.entries.count : candidates.count
    guard count > 0, !Task.isCancelled else { return SearchResponse(hits: [], total: 0, milliseconds: 0) }
    let boosts = index.snapshot.boosts(entries: index.entries, history: visits)
    let frecency = boosts.isEmpty || allPaths ? boosts : candidates.map { boosts[Int($0)] }
    let result = withExtendedLifetime(index.snapshot) {
      normalizedQuery.withCString { query in
        candidates.withUnsafeBufferPointer { ids in
          frecency.withUnsafeBufferPointer { boosts in
            crawler_fff_search(index.snapshot.handle, query, allPaths ? nil : ids.baseAddress,
              boosts.isEmpty ? nil : boosts.baseAddress, count, resultLimit, literalWords ? 1 : 0)
          }
        }
      }
    }
    guard let result else { preconditionFailure("FFF query failed at the native boundary") }
    defer { crawler_fff_free_result(result) }
    let response = result.pointee
    let ranges = UnsafeBufferPointer(start: response.ranges, count: response.range_count)
    let hits = UnsafeBufferPointer(start: response.hits, count: response.hit_count).map { hit in
      let entry = index.entries[Int(hit.index)]
      let offsets = ranges[Int(hit.range_offset)..<Int(hit.range_offset + hit.range_count)]
      return SearchHit(entry: entry, score: Int(hit.score),
        bytes: offsets.map { NSRange(location: Int($0.start), length: Int($0.end - $0.start)) })
    }
    var scopes: [String: Int] = [:]
    // Most bounded batches belong to one root branch. Count once rather than
    // re-reading and hashing the same prefix for every matching file.
    if scope.isEmpty, case .uniform(let name) = index.rootScope {
      if let name, response.total > 0 { scopes[name] = response.total }
    } else {
      for id in UnsafeBufferPointer(start: response.matching_indices, count: response.total) {
        if let child = index.entries.childScope(at: Int(id), prefix: scope.isEmpty ? "" : prefix) {
          scopes[child, default: 0] += 1
        }
      }
    }
    let elapsed = start.duration(to: .now).components
    return SearchResponse(hits: hits, total: response.total,
      milliseconds: Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15,
      scopes: scopes)
  }

  // FFF returns offsets into folded UTF-8. Convert whole Characters back to the
  // original UTF-16 title so accents and emoji never split when highlighted.
  static func originalHighlights(_ offsets: [NSRange], in path: String) -> [NSRange] {
    guard !offsets.isEmpty else { return [] }
    if path.utf8.allSatisfy({ $0 < 128 }) { return offsets }
    var byteOffset = 0
    var utf16Offset = 0
    var result: [NSRange] = []
    for character in path {
      let text = String(character)
      let bytes = SearchText.normalized(text).utf8.count
      if offsets.contains(where: { $0.location < byteOffset + bytes && NSMaxRange($0) > byteOffset }) {
        let range = NSRange(location: utf16Offset, length: text.utf16.count)
        if let last = result.last, NSMaxRange(last) == range.location {
          result[result.count - 1].length += range.length
        } else { result.append(range) }
      }
      byteOffset += bytes
      utf16Offset += text.utf16.count
    }
    return result
  }
}
