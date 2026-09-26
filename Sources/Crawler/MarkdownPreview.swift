import Foundation
import Markdown

extension PreviewTextLoader {
  /// Render the Markdown tree into native text and style ranges. Raw HTML is inert text;
  /// images remain labeled links, so selection never triggers network requests.
  func renderMarkdown(_ source: PreviewText) throws -> PreviewText {
    let document = Document(parsing: source.text)
    var result = PreviewText(text: "", notice: source.notice, markdown: true)
    var offset = 0
    var tableID = 0
    let highlightDeadline = ContinuousClock.now + .milliseconds(150)
    func append(_ text: String) {
      result.text += text
      offset += text.utf16.count
    }
    func span(_ start: Int, _ style: PreviewSpan.Style) {
      if offset > start {
        result.spans.append(
          .init(range: NSRange(location: start, length: offset - start), style: style))
      }
    }
    func children(_ node: any Markup, _ depth: Int) throws {
      for child in node.children { try visit(child, depth + 1) }
    }
    func visit(_ node: any Markup, _ depth: Int) throws {
      try Task.checkCancellation()
      guard depth < 128 else {
        append("…")
        return
      }
      let start = offset
      switch node {
      case let text as Markdown.Text: append(text.string)
      case is SoftBreak: append(" ")
      case is LineBreak: append("\n")
      case let heading as Heading:
        try children(heading, depth)
        append("\n")
        span(start, .heading(heading.level))
        append("\n")
      case is Paragraph:
        try children(node, depth)
        append("\n\n")
      case is Strong:
        try children(node, depth)
        span(start, .strong)
      case is Emphasis:
        try children(node, depth)
        span(start, .emphasis)
      case is Strikethrough:
        try children(node, depth)
        span(start, .strike)
      case let code as InlineCode:
        append(code.code)
        span(start, .code)
      case let code as CodeBlock:
        append(code.code)
        if !result.text.hasSuffix("\n") { append("\n") }
        span(start, .code)
        if let language = code.language.flatMap(CodeLanguage.detect),
          ContinuousClock.now < highlightDeadline
        {
          do {
            result.spans += try highlight(code.code, language: language).map {
              PreviewSpan(
                range: NSRange(location: start + $0.range.location, length: $0.range.length),
                style: $0.style)
            }
          } catch is CancellationError { throw CancellationError() } catch {
            result.notice = result.notice ?? "Some code blocks are shown without syntax colors"
          }
        }
        append("\n")
      case let link as Link:
        try children(link, depth)
        if let destination = link.destination { span(start, .link(destination)) }
      case let image as Markdown.Image:
        append("Image: ")
        try children(image, depth)
        if let destination = image.source { span(start, .link(destination)) }
      case is BlockQuote:
        try children(node, depth)
        span(start, .quote)
      case let list as OrderedList:
        for (i, child) in list.children.enumerated() {
          append("\(list.startIndex + UInt(i)). ")
          try visit(child, depth + 1)
        }
      case is UnorderedList:
        for child in node.children {
          if let item = child as? ListItem, let checkbox = item.checkbox {
            append(checkbox == .checked ? "☑ " : "☐ ")
          } else {
            append("• ")
          }
          try visit(child, depth + 1)
        }
      case is ListItem:
        try children(node, depth)
        span(start, .list(depth))
      case let table as Markdown.Table:
        tableID += 1
        let id = tableID
        let rows: [any Markup] = [table.head] + Array(table.body.children)
        for (rowIndex, row) in rows.enumerated() {
          for (column, cell) in row.children.enumerated() {
            let cellStart = offset
            try children(cell, depth)
            append("\n")
            span(
              cellStart,
              .cell(table: id, row: rowIndex, column: column, columns: table.maxColumnCount))
          }
        }
        append("\n")
      case is ThematicBreak: append("────────────\n\n")
      case let html as HTMLBlock:
        append(html.rawHTML)
        append("\n")
      case let html as InlineHTML: append(html.rawHTML)
      default: try children(node, depth)
      }
    }
    try visit(document, 0)
    return result
  }
}
