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
    case strong, emphasis, strike, code, codeBlock, quote, rule
    case link(String)
    /// Covers the "Image: alt" fallback text; the reader replaces it with local image files.
    case image(String)
    /// `first` marks the opening item of a top-level list, which keeps full block spacing.
    case listItem(level: Int, first: Bool)
    case listMarker
    /// `alignment` is the column's Markdown alignment; nil keeps the natural text direction.
    case cell(table: Int, row: Int, column: Int, columns: Int, alignment: ColumnAlignment?)
  }
  enum ColumnAlignment: Sendable { case left, center, right }
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

/// Read limits: panes stay small and fast; the full-window reader accepts whole documents.
struct PreviewBudget: Equatable, Sendable {
  let bytes: Int
  let lines: Int
  let columns: Int
  let highlighting: Duration
  let notice: String
  static let preview = Self(
    bytes: 256 * 1024, lines: 5_000, columns: 4_096, highlighting: .milliseconds(150),
    notice: "Partial preview · limited to 256 KB, 5,000 lines and 4,096 characters per line")
  static let reader = Self(
    bytes: 4 * 1024 * 1024, lines: 100_000, columns: .max, highlighting: .milliseconds(600),
    notice: "Partial document · limited to 4 MB and 100,000 lines")
}

enum PreviewFormat {
  /// `document` is a Word Open XML file: panes show its Quick Look thumbnail and Enter
  /// converts it to Markdown for the read-only Markdown reader.
  case pdf, markdown, text, video, document, thumbnail
  /// Formats that Enter opens inside the Crawler window instead of another app.
  var opensInReader: Bool { self == .pdf || self == .markdown || self == .document }
  /// Formats previewed through Quick Look thumbnails rather than a native renderer.
  var usesQuickLook: Bool { self == .document || self == .thumbnail }
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
    if ["docx", "docm", "dotx"].contains(ext) { return .document }
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
  private struct CacheKey: Equatable {
    let path: String
    let size: Int64
    let modified: Date?
  }
  private struct CacheEntry {
    let key: CacheKey
    let budget: PreviewBudget
    let truncated: Bool
    let text: PreviewText
  }
  private var cache: [CacheEntry] = []
  private var queries: [CodeLanguage: Query] = [:]

  func load(_ url: URL, budget: PreviewBudget = .preview) throws -> PreviewText {
    try Task.checkCancellation()
    let values = try url.resolvingSymlinksInPath().resourceValues(forKeys: [
      .isRegularFileKey, .fileSizeKey, .contentModificationDateKey,
    ])
    guard values.isRegularFile == true else { throw PreviewReadError.notRegular }
    let key = CacheKey(
      path: url.path, size: Int64(values.fileSize ?? 0), modified: values.contentModificationDate)
    // A complete read is valid for any budget; a truncated one only for its own.
    if let index = cache.firstIndex(where: { $0.key == key && (!$0.truncated || $0.budget == budget) }) {
      let hit = cache.remove(at: index)
      cache.append(hit)
      return hit.text
    }
    // Word documents become Markdown so the Markdown reader can show them.
    let format = PreviewFormat.forURL(url)
    let read = format == .document
      ? (text: try WordDocument.markdown(url), truncated: false) : try readText(url, budget: budget)
    let text = read.text
    var truncated = read.truncated
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
      if lines > budget.lines || column > budget.columns {
        end = index
        truncated = true
        break
      }
    }
    var result = PreviewText(text: String(text[..<end]))
    if truncated { result.notice = budget.notice }
    try Task.checkCancellation()
    if format == .markdown || format == .document {
      result = try renderMarkdown(result, highlighting: budget.highlighting)
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
    cache.removeAll { $0.key.path == key.path }
    cache.append(CacheEntry(key: key, budget: budget, truncated: truncated, text: result))
    // Each entry is bounded above; cap aggregate text/span storage as well as entry count.
    while cache.count > 8
      || cache.reduce(0, { $0 + $1.text.text.utf8.count + $1.text.spans.count * 128 }) > 8 * 1024 * 1024
    {
      cache.removeFirst()
    }
    return result
  }

  /// Read at most `budget.bytes` of a text file, decoding UTF-8 or UTF-16.
  private func readText(_ url: URL, budget: PreviewBudget) throws -> (text: String, truncated: Bool) {
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
    while data.count <= budget.bytes {
      try Task.checkCancellation()
      let chunk =
        try handle.read(upToCount: min(64 * 1024, budget.bytes + 1 - data.count)) ?? Data()
      if chunk.isEmpty { break }
      data.append(chunk)
    }
    let truncated = data.count > budget.bytes
    if truncated { data = data.prefix(budget.bytes) }
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
    return (text, truncated)
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
