import Foundation

/// Converts Word Open XML documents (.docx, .docm, .dotx) to Markdown so the Markdown reader can
/// show them with its own typography, zoom, column and find. Keeps headings, bold, italics,
/// strikethrough, lists, tables and links; fonts, colors, images and page layout are dropped.
enum WordDocument {
  enum Failure: LocalizedError {
    case unreadable
    var errorDescription: String? { "This Word document could not be read." }
  }

  private static let w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  private static let r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

  static func markdown(_ url: URL) throws -> String {
    guard let body = try part("word/document.xml", of: url)?.rootElement()?.child("body", w) else {
      throw Failure.unreadable
    }
    try Task.checkCancellation()
    var converter = Converter(
      headings: headingLevels(try part("word/styles.xml", of: url)),
      bulletLists: bulletLists(try part("word/numbering.xml", of: url)),
      links: links(try part("word/_rels/document.xml.rels", of: url)))
    converter.blocks(body)
    return converter.output
  }

  /// Read one XML part of the zip package with the system `unzip`; a missing part is nil.
  private static func part(_ name: String, of url: URL) throws -> XMLDocument? {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
    process.arguments = ["-p", url.path, name]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0, !data.isEmpty else { return nil }
    return try XMLDocument(data: data)
  }

  /// Heading level per paragraph style: "Title" is 1 and "heading N" follows it.
  private static func headingLevels(_ styles: XMLDocument?) -> [String: Int] {
    var levels: [String: Int] = [:]
    for style in styles?.rootElement()?.children("style", w) ?? [] {
      guard let id = style.value("styleId", w) else { continue }
      let name = style.child("name", w)?.value("val", w)?.lowercased() ?? id.lowercased()
      if name == "title" {
        levels[id] = 1
      } else if name.hasPrefix("heading "), let level = Int(name.dropFirst(8)) {
        levels[id] = min(6, level + 1)
      } else if let outline = style.child("pPr", w)?.child("outlineLvl", w)?.value("val", w).flatMap(Int.init),
        outline < 9
      {
        levels[id] = min(6, outline + 2)
      }
    }
    return levels
  }

  /// Numbering IDs and levels drawn as bullets; every other list is ordered.
  private static func bulletLists(_ numbering: XMLDocument?) -> Set<String> {
    guard let root = numbering?.rootElement() else { return [] }
    var abstract: [String: Set<Int>] = [:]
    for definition in root.children("abstractNum", w) {
      guard let id = definition.value("abstractNumId", w) else { continue }
      abstract[id] = Set(definition.children("lvl", w).compactMap { level in
        level.child("numFmt", w)?.value("val", w) == "bullet" ? level.value("ilvl", w).flatMap(Int.init) : nil
      })
    }
    var bullets: Set<String> = []
    for number in root.children("num", w) {
      guard let id = number.value("numId", w),
        let levels = number.child("abstractNumId", w)?.value("val", w).flatMap({ abstract[$0] })
      else { continue }
      for level in levels { bullets.insert("\(id):\(level)") }
    }
    return bullets
  }

  private static func links(_ relationships: XMLDocument?) -> [String: String] {
    var links: [String: String] = [:]
    for relationship in relationships?.rootElement()?.children ?? [] {
      guard let element = relationship as? XMLElement, element.attribute(forName: "TargetMode")?.stringValue == "External",
        let id = element.attribute(forName: "Id")?.stringValue,
        let target = element.attribute(forName: "Target")?.stringValue
      else { continue }
      links[id] = target
    }
    return links
  }

  private struct Run {
    var text: String
    var strong = false
    var emphasis = false
    var strike = false
    var link: String?
    func sameStyle(_ other: Run) -> Bool {
      strong == other.strong && emphasis == other.emphasis && strike == other.strike && link == other.link
    }
  }

  private struct Converter {
    let headings: [String: Int]
    let bulletLists: Set<String>
    let links: [String: String]
    var output = ""
    /// Whether the previous block was a list item, so consecutive items stay one list.
    private var inList = false

    init(headings: [String: Int], bulletLists: Set<String>, links: [String: String]) {
      self.headings = headings
      self.bulletLists = bulletLists
      self.links = links
    }

    mutating func blocks(_ container: XMLElement) {
      for case let element as XMLElement in container.children ?? [] {
        switch element.localName {
        case "p": paragraph(element)
        case "tbl": table(element)
        case "sdt": if let content = element.child("sdtContent", w) { blocks(content) }
        default: break
        }
      }
    }

    private mutating func emit(_ block: String, listItem: Bool) {
      if !output.isEmpty { output += listItem && inList ? "\n" : "\n\n" }
      output += block
      inList = listItem
    }

    private mutating func paragraph(_ element: XMLElement) {
      let text = WordDocument.inline(runs(element), hardBreak: "\\\n")
      guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
      let properties = element.child("pPr", w)
      if let style = properties?.child("pStyle", w)?.value("val", w), let level = headings[style] {
        return emit(String(repeating: "#", count: level) + " " + text, listItem: false)
      }
      if let numbering = properties?.child("numPr", w),
        let id = numbering.child("numId", w)?.value("val", w), id != "0"
      {
        let level = numbering.child("ilvl", w)?.value("val", w).flatMap(Int.init) ?? 0
        let marker = bulletLists.contains("\(id):\(level)") ? "-" : "1."
        return emit(String(repeating: "    ", count: level) + marker + " " + text, listItem: true)
      }
      emit(WordDocument.escapeBlockStart(text), listItem: false)
    }

    private mutating func table(_ element: XMLElement) {
      var rows: [[String]] = []
      for row in element.children("tr", w) {
        var cells: [String] = []
        for cell in row.children("tc", w) {
          let text = cell.elements(named: "p", w).map { WordDocument.inline(runs($0), hardBreak: " ") }
            .filter { !$0.isEmpty }.joined(separator: " ")
          cells.append(text.replacingOccurrences(of: "\n", with: " "))
          let span = cell.child("tcPr", w)?.child("gridSpan", w)?.value("val", w).flatMap(Int.init) ?? 1
          cells += Array(repeating: "", count: max(0, span - 1))
        }
        rows.append(cells)
      }
      let columns = rows.map(\.count).max() ?? 0
      guard columns > 0 else { return }
      let lines = rows.map { row in
        "| " + (row + Array(repeating: "", count: columns - row.count)).joined(separator: " | ") + " |"
      }
      let separator = "|" + String(repeating: " --- |", count: columns)
      emit(([lines[0], separator] + lines.dropFirst()).joined(separator: "\n"), listItem: false)
    }

    /// Text runs in document order, following hyperlinks, insertions and content controls.
    private func runs(_ element: XMLElement, link: String? = nil) -> [Run] {
      var result: [Run] = []
      for case let child as XMLElement in element.children ?? [] {
        switch child.localName {
        case "r": result += run(child, link: link)
        case "hyperlink":
          let target = child.attribute(forLocalName: "id", uri: r)?.stringValue.flatMap { links[$0] }
          result += runs(child, link: target ?? link)
        case "pPr", "rPr", "del", "tbl", "p": break
        default: result += runs(child, link: link)
        }
      }
      return result
    }

    private func run(_ element: XMLElement, link: String?) -> [Run] {
      let properties = element.child("rPr", w)
      func on(_ name: String) -> Bool {
        guard let flag = properties?.child(name, w) else { return false }
        return !["0", "false", "off"].contains(flag.value("val", w) ?? "true")
      }
      var text = ""
      for case let child as XMLElement in element.children ?? [] {
        switch child.localName {
        case "t": text += child.stringValue ?? ""
        case "tab": text += " "
        case "br", "cr": text += "\n"
        case "noBreakHyphen": text += "-"
        default: break
        }
      }
      guard !text.isEmpty else { return [] }
      return [Run(text: text, strong: on("b"), emphasis: on("i"), strike: on("strike") || on("dstrike"), link: link)]
    }
  }

  /// Join runs into Markdown, merging neighbors with the same style and keeping spaces outside
  /// emphasis markers so CommonMark still recognizes them.
  private static func inline(_ runs: [Run], hardBreak: String) -> String {
    var merged: [Run] = []
    for run in runs {
      if let last = merged.last, last.sameStyle(run) { merged[merged.count - 1].text += run.text } else { merged.append(run) }
    }
    var result = ""
    for run in merged {
      let core = run.text.trimmingCharacters(in: .whitespaces)
      guard !core.isEmpty else { result += run.text.isEmpty ? "" : " "; continue }
      let leading = run.text.prefix { $0 == " " }
      let trailing = String(run.text.reversed().prefix { $0 == " " })
      var text = escape(core).replacingOccurrences(of: "\n", with: hardBreak)
      if run.strike { text = "~~\(text)~~" }
      if run.emphasis { text = "*\(text)*" }
      if run.strong { text = "**\(text)**" }
      if let link = run.link { text = "[\(text)](<\(link)>)" }
      result += leading + text + trailing
    }
    return result.trimmingCharacters(in: .whitespaces)
  }

  private static func escape(_ text: String) -> String {
    var result = ""
    for character in text {
      if "\\`*_[]<>|~".contains(character) { result.append("\\") }
      result.append(character)
    }
    return result
  }

  /// Keep a paragraph that starts like a heading, quote, list or rule as plain text.
  private static func escapeBlockStart(_ text: String) -> String {
    guard let first = text.first else { return text }
    if "#>-+=".contains(first) { return "\\" + text }
    let digits = text.prefix { $0.isNumber }
    if !digits.isEmpty, let next = text.dropFirst(digits.count).first, next == "." || next == ")" {
      return digits + "\\" + text.dropFirst(digits.count)
    }
    return text
  }
}

private extension XMLElement {
  func children(_ name: String, _ namespace: String) -> [XMLElement] {
    elements(forLocalName: name, uri: namespace)
  }
  func child(_ name: String, _ namespace: String) -> XMLElement? {
    children(name, namespace).first
  }
  func value(_ name: String, _ namespace: String) -> String? {
    attribute(forLocalName: name, uri: namespace)?.stringValue
  }
  /// Descendant elements named `name`, in document order.
  func elements(named name: String, _ namespace: String) -> [XMLElement] {
    var result: [XMLElement] = []
    for case let child as XMLElement in children ?? [] {
      if child.localName == name, child.uri == namespace { result.append(child) } else {
        result += child.elements(named: name, namespace)
      }
    }
    return result
  }
}
