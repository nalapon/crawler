import Foundation
import SwiftTreeSitter
import TreeSitterBash
import TreeSitterGo
import TreeSitterJSON
import TreeSitterJavaScript
import TreeSitterPython
import TreeSitterSql
import TreeSitterSwift
import TreeSitterTSX
import TreeSitterTypeScript
import TreeSitterYAML
import UniformTypeIdentifiers

/// UTF-16 ranges throughout: Tree-sitter and AppKit use the same coordinate system.
struct PreviewSpan: Sendable {
  enum Style: Sendable {
    case syntax(String)
    case heading(Int)
    case strong, emphasis, strike, code, quote
    case link(String)
    case list(Int)
    case cell(table: Int, row: Int, column: Int, columns: Int)
  }
  let range: NSRange
  let style: Style
}

struct PreviewText: Sendable {
  var text: String
  var spans: [PreviewSpan] = []
  var notice: String?
  var markdown = false
  var lineStarts: [Int] = [0]
}

enum PreviewFormat {
  case pdf, markdown, text, video, thumbnail
  static func isGoModuleFile(_ url: URL) -> Bool {
    ["go.mod", "go.sum", "go.work", "go.work.sum"].contains(url.lastPathComponent.lowercased())
  }
  static func forURL(_ url: URL) -> Self {
    if isGoModuleFile(url) { return .text }
    let ext = url.pathExtension.lowercased()
    if ["mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "mts", "m2ts", "ogv", "wmv", "flv"].contains(ext)
      || UTType(filenameExtension: ext)?.conforms(to: .movie) == true { return .video }
    if ext == "pdf" { return .pdf }
    if ["md", "markdown", "mdown", "mkd"].contains(ext) { return .markdown }
    if CodeLanguage.detect(ext) != nil { return .text }
    if [
      "txt", "log", "csv", "tsv", "xml", "html", "css", "scss", "toml", "ini", "conf", "cfg", "rs",
      "c", "h", "cpp", "hpp", "java", "rb", "sql", "dockerfile", "gitignore", "editorconfig", "env",
    ].contains(ext)
      || ["makefile", "dockerfile", "license", "readme", ".gitignore", ".editorconfig", ".env"]
        .contains(url.lastPathComponent.lowercased())
    {
      return .text
    }
    if let type = UTType(filenameExtension: ext), type.conforms(to: .text) { return .text }
    return .thumbnail
  }
}

enum CodeLanguage: String, CaseIterable, Sendable {
  case swift, javascript, typescript, tsx, go, python, json, yaml, bash, sql
  static func detect(_ name: String) -> Self? {
    switch name.lowercased() {
    case "sql": .sql
    case "swift": .swift
    case "js", "jsx", "mjs", "cjs", "javascript": .javascript
    case "ts", "mts", "cts", "typescript": .typescript
    case "tsx": .tsx
    case "go", "golang": .go
    case "py", "pyw", "python": .python
    case "json", "jsonc": .json
    case "yml", "yaml": .yaml
    case "sh", "bash", "zsh", "shell": .bash
    default: nil
    }
  }
  var parserLanguage: Language {
    switch self {
    case .sql: Language(tree_sitter_sql())
    case .swift: Language(tree_sitter_swift())
    case .javascript: Language(tree_sitter_javascript())
    case .typescript: Language(tree_sitter_typescript())
    case .tsx: Language(tree_sitter_tsx())
    case .go: Language(tree_sitter_go())
    case .python: Language(tree_sitter_python())
    case .json: Language(tree_sitter_json())
    case .yaml: Language(tree_sitter_yaml())
    case .bash: Language(tree_sitter_bash())
    }
  }
  var bundleName: String {
    switch self {
    case .sql: "TreeSitterSql_TreeSitterSql"
    case .swift: "TreeSitterSwift_TreeSitterSwift"
    case .javascript: "TreeSitterJavaScript_TreeSitterJavaScript"
    case .typescript: "TreeSitterTypeScript_TreeSitterTypeScript"
    case .tsx: "TreeSitterTypeScript_TreeSitterTSX"
    case .go: "TreeSitterGo_TreeSitterGo"
    case .python: "TreeSitterPython_TreeSitterPython"
    case .json: "TreeSitterJSON_TreeSitterJSON"
    case .yaml: "TreeSitterYAML_TreeSitterYAML"
    case .bash: "TreeSitterBash_TreeSitterBash"
    }
  }
}

/// One background owner for compiled queries and a bounded, metadata-validated text cache.
/// No AppKit objects or mutable parsers cross the actor boundary.
actor PreviewTextLoader {
  static let shared = PreviewTextLoader()
  static let byteLimit = 256 * 1024
  private struct CacheKey: Equatable {
    let path: String
    let size: Int64
    let modified: Date?
  }
  private var cache: [(CacheKey, PreviewText)] = []
  private var queries: [CodeLanguage: Query] = [:]

  func load(_ url: URL) throws -> PreviewText {
    try Task.checkCancellation()
    let values = try url.resolvingSymlinksInPath().resourceValues(forKeys: [
      .isRegularFileKey, .fileSizeKey, .contentModificationDateKey,
    ])
    guard values.isRegularFile == true else { throw PreviewReadError.notRegular }
    let key = CacheKey(
      path: url.path, size: Int64(values.fileSize ?? 0), modified: values.contentModificationDate)
    if let index = cache.firstIndex(where: { $0.0 == key }) {
      let hit = cache.remove(at: index)
      cache.append(hit)
      return hit.1
    }
    // O_NONBLOCK plus fstat also prevents a file replaced by a FIFO from hanging a worker.
    let descriptor = open(url.path, O_RDONLY | O_NONBLOCK | O_CLOEXEC)
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    defer { try? handle.close() }
    var info = stat()
    guard fstat(descriptor, &info) == 0, info.st_mode & S_IFMT == S_IFREG else {
      throw PreviewReadError.notRegular
    }
    var data = Data()
    while data.count <= Self.byteLimit {
      try Task.checkCancellation()
      let chunk =
        try handle.read(upToCount: min(64 * 1024, Self.byteLimit + 1 - data.count)) ?? Data()
      if chunk.isEmpty { break }
      data.append(chunk)
    }
    var truncated = data.count > Self.byteLimit
    if truncated { data = data.prefix(Self.byteLimit) }
    let text: String
    if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) {
      if data.count % 2 != 0 { data.removeLast() }
      var decoded = String(data: data, encoding: .utf16)
      if decoded == nil, truncated, data.count >= 4 {
        data.removeLast(2)
        decoded = String(data: data, encoding: .utf16)
      }
      guard let decoded else { throw PreviewReadError.encoding }
      text = decoded
    } else {
      if data.starts(with: [0xEF, 0xBB, 0xBF]) { data.removeFirst(3) }
      guard !data.contains(0) else { throw PreviewReadError.encoding }
      // A byte cap can bisect the final UTF-8 scalar, but invalid data elsewhere is an error.
      var decoded = String(data: data, encoding: .utf8)
      if truncated {
        for _ in 0..<3 where decoded == nil && !data.isEmpty {
          data.removeLast()
          decoded = String(data: data, encoding: .utf8)
        }
      }
      guard let decoded else { throw PreviewReadError.encoding }
      text = decoded
    }
    // Bound line layout as well as bytes, including minified single-line source.
    var end = text.endIndex
    var lines = 1
    var column = 0
    for index in text.indices {
      if text[index] == "\n" {
        lines += 1
        column = 0
      } else {
        column += 1
      }
      if lines > 5_000 || column > 4_096 {
        end = index
        truncated = true
        break
      }
    }
    var result = PreviewText(text: String(text[..<end]))
    if truncated {
      result.notice =
        "Partial preview · limited to 256 KB, 5,000 lines and 4,096 characters per line"
    }
    try Task.checkCancellation()
    if PreviewFormat.forURL(url) == .markdown {
      result = try renderMarkdown(result)
    } else if let language = CodeLanguage.detect(url.pathExtension) {
      do {
        result.spans = try highlight(result.text, language: language)
      } catch is CancellationError { throw CancellationError() } catch {
        result.notice = [result.notice, "Syntax colors unavailable: \(error.localizedDescription)"]
          .compactMap { $0 }.joined(separator: " · ")
      }
    }
    result.lineStarts = [0]
    for (index, unit) in result.text.utf16.enumerated() where unit == 10 {
      result.lineStarts.append(index + 1)
    }
    try Task.checkCancellation()
    cache.removeAll { $0.0.path == key.path }
    cache.append((key, result))
    // Each entry is bounded above; cap aggregate text/span storage as well as entry count.
    while cache.count > 8
      || cache.reduce(0, { $0 + $1.1.text.utf8.count + $1.1.spans.count * 128 }) > 8 * 1024 * 1024
    {
      cache.removeFirst()
    }
    return result
  }

  func highlight(_ text: String, language: CodeLanguage) throws -> [PreviewSpan] {
    try Task.checkCancellation()
    guard !text.isEmpty else { return [] }
    let query: Query
    if let existing = queries[language] {
      query = existing
    } else {
      var source = ""
      // TypeScript extends JavaScript's grammar and highlight rules.
      if language == .typescript || language == .tsx {
        source += try queryText(.javascript, file: "highlights")
      }
      source += try queryText(language, file: "highlights")
      if language == .javascript || language == .tsx {
        source += try queryText(.javascript, file: "highlights-jsx")
      }
      if language == .sql {
        // Upstream SQL queries use Lua digit classes and a spell-check annotation.
        source = source.replacingOccurrences(of: "%d", with: "[0-9]")
          .replacingOccurrences(of: " @spell", with: "")
      }
      query = try Query(language: language.parserLanguage, data: Data(source.utf8))
      queries[language] = query
    }
    let parser = Parser()
    try parser.setLanguage(language.parserLanguage)
    parser.timeout = 0.075
    guard let tree = parser.parse(text) else { throw PreviewReadError.highlightBudget }
    try Task.checkCancellation()
    let cursor = query.execute(in: tree)
    cursor.matchLimit = 1024
    let deadline = ContinuousClock.now + .milliseconds(75)
    var captures: [QueryCapture] = []
    for match in cursor.resolve(with: .init(string: text)) {
      try Task.checkCancellation()
      guard captures.count + match.captures.count < 20_000, ContinuousClock.now < deadline else {
        throw PreviewReadError.highlightBudget
      }
      captures.append(contentsOf: match.captures)
    }
    return captures.sorted().compactMap { capture in
      guard let name = capture.name, !name.hasPrefix("_") else { return nil }
      return PreviewSpan(range: capture.range, style: .syntax(name))
    }
  }

  private func queryText(_ language: CodeLanguage, file: String) throws -> String {
    let root = Bundle.main.resourceURL ?? Bundle.main.bundleURL
    let bundle = root.appendingPathComponent(language.bundleName + ".bundle")
    let url = bundle.appendingPathComponent("Contents/Resources/queries/\(file).scm")
    return try String(contentsOf: url, encoding: .utf8) + "\n"
  }
}

enum PreviewReadError: LocalizedError {
  case notRegular, encoding, highlightBudget
  var errorDescription: String? {
    switch self {
    case .notRegular: "This item is not a regular file."
    case .encoding: "This file is binary or uses an unsupported text encoding."
    case .highlightBudget: "Highlighting exceeded the preview time or token limit."
    }
  }
}
