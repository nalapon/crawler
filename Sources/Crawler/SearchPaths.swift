import Foundation
import FFFBridge

// Cache rows keep byte offsets, not 1.4 million allocated Strings. Only hits
// escaping the index become SearchEntry values, so retained UI rows do not pin it.
enum SearchEntries: RandomAccessCollection, Sendable {
  struct Record: Sendable {
    let offset: UInt32
    let length: UInt32
    let folder: Bool
    let ascii: Bool
  }
  case owned([SearchEntry])
  case mapped(FFFMappedCache, Range<Int>, [Record])
  var startIndex: Int { 0 }
  var endIndex: Int {
    switch self { case .owned(let rows): rows.count; case .mapped(_, _, let rows): rows.count }
  }
  subscript(i: Int) -> SearchEntry {
    switch self {
    case .owned(let rows): rows[i]
    case .mapped(_, _, let rows):
      SearchEntry(path: withBytes(at: i) { String(decoding: $0, as: UTF8.self) }, isFolder: rows[i].folder)
    }
  }
  func isFolder(at i: Int) -> Bool {
    switch self { case .owned(let rows): rows[i].isFolder; case .mapped(_, _, let rows): rows[i].folder }
  }
  func withBytes<T>(at i: Int, _ body: (UnsafeRawBufferPointer) -> T) -> T {
    switch self {
    case .owned(let rows):
      var path = rows[i].path
      return path.withUTF8 { body(UnsafeRawBufferPointer($0)) }
    case .mapped(let cache, let block, let rows):
      let row = rows[i], start = block.lowerBound + Int(row.offset)
      return cache.withBytes(in: start..<(start + Int(row.length)), body)
    }
  }
  private func isASCII(at i: Int) -> Bool {
    switch self {
    case .owned: withBytes(at: i) { $0.allSatisfy { $0 < 128 } }
    case .mapped(_, _, let rows): rows[i].ascii
    }
  }
  func hasPrefix(_ prefix: String, at i: Int) -> Bool {
    guard isASCII(at: i), prefix.utf8.allSatisfy({ $0 < 128 }) else { return self[i].path.hasPrefix(prefix) }
    return withBytes(at: i) { $0.starts(with: prefix.utf8) }
  }
  func isAncestor(of path: String, at i: Int) -> Bool {
    guard isASCII(at: i), path.utf8.allSatisfy({ $0 < 128 }) else { return path.hasPrefix(self[i].path + "/") }
    var path = path
    return path.withUTF8 { candidate in
      withBytes(at: i) { bytes in
        candidate.count > bytes.count && candidate[bytes.count] == 47
          && candidate.starts(with: bytes)
      }
    }
  }
  func equal(at i: Int, to other: SearchEntries, at j: Int) -> Bool {
    if isASCII(at: i) && other.isASCII(at: j) {
      return withBytes(at: i) { a in other.withBytes(at: j) { a.elementsEqual($0) } }
    }
    return self[i].path == other[j].path
  }
  func equal(at i: Int, to path: String) -> Bool {
    guard isASCII(at: i), path.utf8.allSatisfy({ $0 < 128 }) else { return self[i].path == path }
    return withBytes(at: i) { $0.elementsEqual(path.utf8) }
  }
  // Hasher's process seed is sufficient here. A fingerprint is only a lookup
  // accelerator: collisions always compare the complete, canonically equal path.
  func fingerprint(at i: Int) -> Int {
    if isASCII(at: i) { return withBytes(at: i) { Self.hash($0) } }
    return Self.fingerprint(self[i].path)
  }
  static func fingerprint(_ path: String) -> Int {
    var path = path
    if let ascii = path.withUTF8({ bytes in
      bytes.allSatisfy { $0 < 128 } ? hash(UnsafeRawBufferPointer(bytes)) : nil
    }) { return ascii }
    var canonical = path.precomposedStringWithCanonicalMapping
    return canonical.withUTF8 { hash(UnsafeRawBufferPointer($0)) }
  }
  private static func hash(_ bytes: UnsafeRawBufferPointer) -> Int {
    var hasher = Hasher()
    hasher.combine(bytes: bytes)
    return hasher.finalize()
  }
  func childScope(at i: Int, prefix: String) -> String? {
    guard isASCII(at: i), prefix.utf8.allSatisfy({ $0 < 128 }) else {
      let path = self[i].path, relative = path.dropFirst(prefix.count)
      return relative.firstIndex(of: "/").map { String(path[..<$0]) }
    }
    return withBytes(at: i) { bytes in
      guard prefix.utf8.count <= bytes.count,
        let slash = bytes.dropFirst(prefix.utf8.count).firstIndex(of: 47) else { return nil }
      return String(decoding: bytes[..<slash], as: UTF8.self)
    }
  }
}

// Used for discovery deduplication and reconciliation. References are bounded
// batch/row IDs; the table does not materialize or retain every original path.
struct SearchPathTable {
  private struct Location {
    let batch: UInt32
    let row: UInt32
  }
  private var sources: [SearchEntries] = []
  private var locations: [Int: Location] = [:]
  private var collisions: [Int: [Location]] = [:]
  private(set) var count = 0
  var isEmpty: Bool { count == 0 }
  init(previous: [SearchIndex], prefixes: [String]? = nil) {
    for index in previous { _ = insert(index.entries, prefixes: prefixes) }
  }
  private func matches(_ location: Location, entries: SearchEntries, row: Int) -> Bool {
    sources[Int(location.batch)].equal(at: Int(location.row), to: entries, at: row)
  }
  func contains(_ entries: SearchEntries, row: Int) -> Bool {
    let key = entries.fingerprint(at: row)
    if let location = locations[key], matches(location, entries: entries, row: row) { return true }
    return collisions[key]?.contains { matches($0, entries: entries, row: row) } ?? false
  }
  mutating func insert(_ entries: SearchEntries, prefixes: [String]? = nil) -> [Int] {
    let batch = UInt32(sources.count)
    sources.append(entries)
    var added: [Int] = []
    for row in entries.indices {
      if let prefixes, !prefixes.contains(where: {
        entries.equal(at: row, to: $0) || entries.hasPrefix($0 + "/", at: row)
      }) { continue }
      let key = entries.fingerprint(at: row)
      if let location = locations[key] {
        if matches(location, entries: entries, row: row)
          || collisions[key]?.contains(where: { matches($0, entries: entries, row: row) }) == true { continue }
        collisions[key, default: []].append(Location(batch: batch, row: UInt32(row)))
      } else { locations[key] = Location(batch: batch, row: UInt32(row)) }
      added.append(row)
      count += 1
    }
    return added
  }
  mutating func remove(_ path: String) -> Bool? {
    let key = SearchEntries.fingerprint(path)
    guard let first = locations[key] else { return nil }
    func equal(_ location: Location) -> Bool {
      sources[Int(location.batch)].equal(at: Int(location.row), to: path)
    }
    let removed: Location
    if equal(first) {
      removed = first
      if var bucket = collisions[key], let next = bucket.popLast() {
        locations[key] = next
        collisions[key] = bucket.isEmpty ? nil : bucket
      } else { locations[key] = nil }
    } else {
      guard var bucket = collisions[key], let position = bucket.firstIndex(where: equal) else { return nil }
      removed = bucket.remove(at: position)
      collisions[key] = bucket.isEmpty ? nil : bucket
    }
    count -= 1
    return sources[Int(removed.batch)].isFolder(at: Int(removed.row))
  }
}
