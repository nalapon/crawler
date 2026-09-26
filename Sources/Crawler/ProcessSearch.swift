import Foundation

// Process-table filtering is independent of the FFF file index.
/// Text recognition shared by file ranking and process filtering, without result limits or filesystem state.
struct FuzzyQuery: Sendable {
  fileprivate let patterns: [SearchPattern]
  init?(_ text: String) {
    let patterns = text.split(whereSeparator: \.isWhitespace).map(SearchPattern.init)
    guard patterns.count <= 16, patterns.allSatisfy({ $0.bytes.count <= 256 }) else { return nil }
    self.patterns = patterns
  }
  func matches(_ text: FuzzySearchText) -> Bool {
    patterns.allSatisfy { $0.matches(text.bytes[...], signature: text.signature) }
  }
}

struct FuzzySearchText: Sendable {
  fileprivate let bytes: [UInt8]
  fileprivate let signature: UInt64
  init(_ text: String) {
    bytes = SearchText.bytes(text)
    signature = bytes.reduce(0) { $0 | (UInt64(1) << ($1 & 63)) }
  }
}

private struct SearchPattern: Sendable {
  let bytes: [UInt8]
  let masks: [UInt64]
  let signature: UInt64
  let typos: Int
  init(_ text: Substring) {
    bytes = SearchText.bytes(String(text))
    var masks = [UInt64](repeating: 0, count: 256)
    for (index, byte) in bytes.enumerated() where index < 63 { masks[Int(byte)] |= 1 << index }
    self.masks = masks
    signature = bytes.reduce(0) { $0 | (UInt64(1) << ($1 & 63)) }
    // Avoid treating individual bytes of a Unicode scalar as misspellings.
    typos = bytes.allSatisfy { $0 < 128 } ? min(2, bytes.count / 4) : 0
  }

  func matches<C: Collection>(_ haystack: C, signature: UInt64) -> Bool where C.Element == UInt8 {
    (self.signature & ~signature).nonzeroBitCount <= typos && accepts(haystack)
  }

  // Bit-parallel subsequence filter. Each state allows one more omitted query
  // character, covering substitutions and adjacent transpositions as well.
  func accepts<C: Collection>(_ haystack: C) -> Bool where C.Element == UInt8 {
    guard bytes.count < 64 else {
      var i = 0
      for byte in haystack where byte == bytes[i] {
        i += 1
        if i == bytes.count { return true }
      }
      return false
    }
    let goal = UInt64(1) << bytes.count
    var zero: UInt64 = 1
    var one: UInt64 = 3
    var two: UInt64 = 7
    for byte in haystack {
      let mask = masks[Int(byte)]
      zero |= (zero & mask) << 1
      if typos == 0 {
        if zero & goal != 0 { return true }
        continue
      }
      one |= ((one & mask) << 1) | (zero << 1)
      if typos == 1 {
        if one & goal != 0 { return true }
        continue
      }
      two |= ((two & mask) << 1) | (one << 1)
      if two & goal != 0 { return true }
    }
    return false
  }
}
