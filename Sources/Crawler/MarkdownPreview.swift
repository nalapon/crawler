import AppKit
import Markdown

extension PreviewTextLoader {
  /// Render the Markdown tree into plain text plus semantic style ranges. Raw HTML never runs:
  /// inline tags are dropped, comments hidden and other HTML blocks shown as source. Images
  /// remain labeled links here, so selection never triggers network requests.
  func renderMarkdown(_ source: PreviewText, highlighting budget: Duration) throws -> PreviewText {
    let document = Document(parsing: source.text)
    var result = PreviewText(text: "", notice: source.notice, markdown: true)
    var offset = 0
    var tableID = 0
    var listLevel = 0
    let highlightDeadline = ContinuousClock.now + budget
    func append(_ text: String) {
      result.text += text
      offset += text.utf16.count
    }
    /// Blocks end with exactly one newline; paragraph styles own the space between blocks.
    func endBlock() {
      if !result.text.isEmpty, !result.text.hasSuffix("\n") { append("\n") }
    }
    /// Inserting at the node's pre-order position lets inner styles refine their container's.
    func span(_ style: PreviewSpan.Style, from start: Int, to end: Int? = nil, at index: Int) {
      let end = end ?? offset
      guard end > start else { return }
      result.spans.insert(
        .init(range: NSRange(location: start, length: end - start), style: style), at: index)
    }
    func marker(_ item: ListItem, _ position: Int, _ ordered: OrderedList?) -> String {
      if let ordered { return "\(ordered.startIndex + UInt(position))." }
      if let checkbox = item.checkbox { return checkbox == .checked ? "☑" : "☐" }
      return listLevel == 1 ? "•" : listLevel == 2 ? "◦" : "▪"
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
      let index = result.spans.count
      switch node {
      case let text as Markdown.Text: append(text.string)
      case is SoftBreak: append(" ")
      // A line separator keeps hard breaks inside the paragraph and its spacing.
      case is LineBreak: append("\u{2028}")
      case let heading as Heading:
        try children(heading, depth)
        endBlock()
        span(.heading(heading.level), from: start, at: index)
      case is Paragraph:
        try children(node, depth)
        endBlock()
      case is Strong:
        try children(node, depth)
        span(.strong, from: start, at: index)
      case is Emphasis:
        try children(node, depth)
        span(.emphasis, from: start, at: index)
      case is Strikethrough:
        try children(node, depth)
        span(.strike, from: start, at: index)
      case let code as InlineCode:
        append(code.code)
        span(.code, from: start, at: index)
      case let code as CodeBlock:
        append(code.code)
        endBlock()
        span(.codeBlock, from: start, at: index)
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
      case let link as Link:
        try children(link, depth)
        if let destination = link.destination { span(.link(destination), from: start, at: index) }
      case let image as Markdown.Image:
        append("Image: ")
        try children(image, depth)
        if let source = image.source, !source.isEmpty { span(.image(source), from: start, at: index) }
      case is BlockQuote:
        try children(node, depth)
        span(.quote, from: start, at: index)
      case let list as any ListItemContainer:
        listLevel += 1
        defer { listLevel -= 1 }
        for (position, item) in list.listItems.enumerated() {
          let itemStart = offset
          let itemIndex = result.spans.count
          append(marker(item, position, list as? OrderedList) + "\t")
          let markerEnd = offset
          try children(item, depth + 1)
          endBlock()
          span(.listMarker, from: itemStart, to: markerEnd, at: itemIndex)
          span(.listItem(level: listLevel, first: listLevel == 1 && position == 0), from: itemStart, at: itemIndex)
        }
      case let table as Markdown.Table:
        tableID += 1
        let id = tableID
        let rows: [any Markup] = [table.head] + Array(table.body.children)
        let alignments: [PreviewSpan.ColumnAlignment?] = table.columnAlignments.map {
          switch $0 {
          case .left: .left
          case .center: .center
          case .right: .right
          case nil: nil
          }
        }
        for (rowIndex, row) in rows.enumerated() {
          for (column, cell) in row.children.enumerated() {
            let cellStart = offset
            let cellIndex = result.spans.count
            try children(cell, depth)
            append("\n")
            let alignment = alignments.indices.contains(column) ? alignments[column] : nil
            span(
              .cell(table: id, row: rowIndex, column: column, columns: table.maxColumnCount, alignment: alignment),
              from: cellStart, at: cellIndex)
          }
        }
      case is ThematicBreak:
        append(" \n")
        span(.rule, from: start, at: index)
      case let html as HTMLBlock:
        let raw = html.rawHTML.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty, !(raw.hasPrefix("<!--") && raw.hasSuffix("-->")) else { return }
        append(raw)
        endBlock()
        span(.codeBlock, from: start, at: index)
      case let html as InlineHTML:
        if html.rawHTML.lowercased().hasPrefix("<br") { append("\u{2028}") }
      default: try children(node, depth)
      }
    }
    try visit(document, 0)
    return result
  }
}

/// Sizes and spacing for rendered Markdown: compact in panes, generous in the reader.
struct MarkdownTypography {
  let body: CGFloat
  /// Extra space below each line.
  let leading: CGFloat
  /// Space above an ordinary block.
  let spacing: CGFloat
  static let pane = Self(body: 13, leading: 3, spacing: 8)
  static func reader(scale: CGFloat) -> Self {
    Self(body: (17 * scale).rounded(), leading: (7 * scale).rounded(), spacing: (16 * scale).rounded())
  }
  var code: CGFloat { (body * 0.88).rounded() }
  var cell: CGFloat { (body * 0.92).rounded() }
  var cellPadding: CGFloat { (body * 0.6).rounded() }
  var indent: CGFloat { (body * 1.7).rounded() }
  func heading(_ level: Int) -> CGFloat {
    (body * [1.9, 1.5, 1.25, 1.1, 1, 0.92][min(max(level, 1), 6) - 1]).rounded()
  }
}

extension NSAttributedString.Key {
  /// GitHub-style heading slug, used to resolve `#fragment` links.
  static let markdownAnchor = Self("CrawlerMarkdownAnchor")
}

@MainActor
enum MarkdownStyle {
  /// Apply native text styles to rendered Markdown. `images` maps an image source, as written
  /// in the document, to an already decoded image that replaces its "Image: alt" text. Images
  /// and tables fit within `columnWidth`; an unknown width lets tables fill the text view.
  static func attributedString(
    _ content: PreviewText, _ type: MarkdownTypography, base: URL?,
    images: [String: NSImage] = [:], columnWidth: CGFloat = .greatestFiniteMagnitude
  ) -> NSMutableAttributedString {
    let plain = NSMutableParagraphStyle()
    plain.lineSpacing = type.leading
    plain.paragraphSpacingBefore = type.spacing
    plain.lineBreakMode = .byWordWrapping
    let text = NSMutableAttributedString(
      string: content.text,
      attributes: [.font: Design.font(type.body), .foregroundColor: Design.text, .paragraphStyle: plain])
    let string = text.string as NSString
    let mono = NSFont.monospacedSystemFont(ofSize: type.code, weight: .regular)
    var tables: [Int: NSTextTable] = [:]
    let tableWidths = Self.tableWidths(content, string, type, available: columnWidth)
    var colors: [String: NSColor] = [:]
    var anchors: [String: Int] = [:]
    var attachments: [(range: NSRange, image: NSImage)] = []
    /// Edit whole paragraphs, keeping what enclosing quotes and lists already set.
    func paragraphs(_ range: NSRange, _ edit: (NSMutableParagraphStyle) -> Void) {
      let range = string.paragraphRange(for: range)
      text.enumerateAttribute(.paragraphStyle, in: range) { value, run, _ in
        let style = NSMutableParagraphStyle()
        style.setParagraphStyle((value as? NSParagraphStyle) ?? plain)
        edit(style)
        text.addAttribute(.paragraphStyle, value: style, range: run)
      }
    }
    func fonts(_ range: NSRange, _ trait: NSFontTraitMask) {
      text.enumerateAttribute(.font, in: range) { value, run, _ in
        guard let font = value as? NSFont else { return }
        text.addAttribute(.font, value: NSFontManager.shared.convert(font, toHaveTrait: trait), range: run)
      }
    }
    /// Move inherited list indentation into the block margin so backgrounds and borders follow it.
    func enter(_ block: NSTextBlock, _ style: NSMutableParagraphStyle) {
      if !(block is NSTextTableBlock) { block.setValue(100, type: .percentageValueType, for: .width) }
      block.setWidth(style.headIndent, type: .absoluteValueType, for: .margin, edge: .minX)
      style.textBlocks.append(block)
      style.headIndent = 0
      style.firstLineHeadIndent = 0
    }
    for span in content.spans where NSMaxRange(span.range) <= text.length {
      let range = span.range
      switch span.style {
      case .syntax(let name):
        let color = colors[name] ?? Design.syntaxColor(name)
        colors[name] = color
        text.addAttribute(.foregroundColor, value: color, range: range)
      case .heading(let level):
        text.addAttributes([
          .font: Design.font(type.heading(level), level <= 2 ? .bold : .semibold),
          .foregroundColor: level == 6 ? Design.muted : Design.text,
        ], range: range)
        paragraphs(range) {
          $0.paragraphSpacingBefore = type.spacing * (level <= 2 ? 1.9 : 1.5)
          $0.lineSpacing = type.leading * 0.4
        }
        let anchor = Self.slug(string.substring(with: range))
        let count = anchors[anchor, default: 0]
        anchors[anchor] = count + 1
        text.addAttribute(.markdownAnchor, value: count == 0 ? anchor : "\(anchor)-\(count)", range: range)
      case .strong: fonts(range, .boldFontMask)
      case .emphasis: fonts(range, .italicFontMask)
      case .strike: text.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: range)
      case .code:
        text.addAttributes([.font: mono, .backgroundColor: Design.line.withAlphaComponent(0.45)], range: range)
      case .codeBlock:
        let block = NSTextBlock()
        block.backgroundColor = Design.line.withAlphaComponent(0.32)
        block.setWidth((type.body * 0.9).rounded(), type: .absoluteValueType, for: .padding)
        // AppKit paints a filled block's margins, so a transparent wrapper holds the space above.
        let spacer = NSTextBlock()
        spacer.setWidth(type.spacing, type: .absoluteValueType, for: .margin, edge: .minY)
        text.addAttribute(.font, value: mono, range: range)
        paragraphs(range) {
          enter(spacer, $0)
          enter(block, $0)
          $0.paragraphSpacingBefore = 0
          $0.lineSpacing = type.leading * 0.5
          $0.tabStops = []
          $0.defaultTabInterval = type.code * 2.4
        }
      case .quote:
        let block = NSTextBlock()
        block.setWidth(3, type: .absoluteValueType, for: .border, edge: .minX)
        block.setBorderColor(Design.line, for: .minX)
        block.setWidth(type.body * 0.9, type: .absoluteValueType, for: .padding, edge: .minX)
        block.setWidth(type.spacing, type: .absoluteValueType, for: .margin, edge: .minY)
        text.addAttribute(.foregroundColor, value: Design.muted, range: range)
        paragraphs(range) {
          enter(block, $0)
          $0.paragraphSpacingBefore = type.spacing * 0.6
        }
        paragraphs(NSRange(location: range.location, length: 0)) { $0.paragraphSpacingBefore = 0 }
      case .rule:
        let block = NSTextBlock()
        block.setWidth(1, type: .absoluteValueType, for: .border, edge: .maxY)
        block.setBorderColor(Design.line, for: .maxY)
        block.setWidth(type.spacing, type: .absoluteValueType, for: .margin, edge: .minY)
        text.addAttributes([.font: Design.font(4), .foregroundColor: NSColor.clear], range: range)
        paragraphs(range) {
          enter(block, $0)
          $0.paragraphSpacingBefore = 0
          $0.lineSpacing = 0
        }
      case .link(let destination):
        if let url = Self.url(destination, base: base) { text.addAttribute(.link, value: url, range: range) }
      case .image(let source):
        // An image inside a link keeps the outer destination.
        if text.attribute(.link, at: range.location, effectiveRange: nil) == nil,
          let url = Self.url(source, base: base)
        {
          text.addAttribute(.link, value: url, range: range)
        }
        if let image = images[source] { attachments.append((range, image)) }
      case .listItem(let level, let first):
        paragraphs(range) {
          $0.headIndent = CGFloat(level) * type.indent
          $0.firstLineHeadIndent = $0.headIndent
          $0.paragraphSpacingBefore = first ? type.spacing : type.spacing * 0.35
        }
      case .listMarker:
        text.addAttribute(.foregroundColor, value: Design.muted, range: range)
        paragraphs(range) {
          $0.firstLineHeadIndent = $0.headIndent - type.indent
          $0.tabStops = [NSTextTab(textAlignment: .left, location: $0.headIndent)]
        }
      case .cell(let id, let row, let column, let columns, let alignment):
        let table = tables[id] ?? NSTextTable()
        if tables[id] == nil {
          table.numberOfColumns = columns
          // Column widths come from `tableWidths`, so AppKit need not measure every row.
          table.layoutAlgorithm = .fixedLayoutAlgorithm
          table.collapsesBorders = true
          if let width = tableWidths[id]?.table {
            table.setValue(width, type: .absoluteValueType, for: .width)
            table.setValue(100, type: .percentageValueType, for: .maximumWidth)
          }
          tables[id] = table
          // AppKit subtracts a table's margins from its width, even vertical ones, and wrapping
          // the table in another block crashes its background drawing; space the paragraph above.
          if range.location > 0 {
            paragraphs(NSRange(location: range.location - 1, length: 0)) {
              $0.paragraphSpacing = max($0.paragraphSpacing, type.spacing)
            }
          }
        }
        let block = NSTextTableBlock(
          table: table, startingRow: row, rowSpan: 1, startingColumn: column, columnSpan: 1)
        block.setWidth((type.body * 0.35).rounded(), type: .absoluteValueType, for: .padding, edge: .minY)
        block.setWidth((type.body * 0.35).rounded(), type: .absoluteValueType, for: .padding, edge: .maxY)
        block.setWidth(type.cellPadding, type: .absoluteValueType, for: .padding, edge: .minX)
        block.setWidth(type.cellPadding, type: .absoluteValueType, for: .padding, edge: .maxX)
        if let share = tableWidths[id]?.columns[column] {
          block.setValue(share, type: .percentageValueType, for: .width)
        }
        block.setWidth(1, type: .absoluteValueType, for: .border)
        block.setBorderColor(Design.line)
        text.addAttribute(.font, value: Design.font(type.cell, row == 0 ? .semibold : .regular), range: range)
        if row == 0 { block.backgroundColor = Design.line.withAlphaComponent(0.4) }
        paragraphs(range) {
          enter(block, $0)
          $0.paragraphSpacingBefore = 0
          $0.lineSpacing = type.leading * 0.5
          $0.alignment = switch alignment {
          case .left: .left
          case .center: .center
          case .right: .right
          case nil: .natural
          }
        }
      }
    }
    // Replace from the end so earlier ranges remain valid.
    for (range, image) in attachments.reversed() {
      let attachment = NSTextAttachment()
      attachment.image = image
      let width = min(image.size.width, columnWidth)
      attachment.bounds = NSRect(
        x: 0, y: 0, width: width, height: image.size.height * width / max(1, image.size.width))
      let replacement = NSMutableAttributedString(attachment: attachment)
      replacement.addAttributes(
        text.attributes(at: range.location, effectiveRange: nil),
        range: NSRange(location: 0, length: replacement.length))
      text.replaceCharacters(in: range, with: replacement)
    }
    return text
  }

  /// Resolve against the document file itself, so `#section` stays within the same document.
  /// Each table's width in points, and its cells' content widths as percentages of the whole
  /// table, sized like an HTML automatic table: a table that fits keeps its content width;
  /// otherwise every column keeps its longest word and the remaining width follows each column's
  /// longest line. Callers cap the table at the text width, which can shrink after rendering.
  private static func tableWidths(
    _ content: PreviewText, _ string: NSString, _ type: MarkdownTypography, available: CGFloat
  ) -> [Int: (table: CGFloat, columns: [CGFloat])] {
    // Cell padding and borders sit outside a cell's content percentage. The content also keeps
    // 10 pt of slack: AppKit rounds line fragments and laid-out text runs a little wider than
    // `NSString.size` measures.
    let box = 2 * type.cellPadding + 2
    let chrome = box + 10
    var measured: [Int: [(word: CGFloat, line: CGFloat)]] = [:]
    for span in content.spans where NSMaxRange(span.range) <= string.length {
      guard case .cell(let id, let row, let column, let columns, _) = span.style, column < columns else { continue }
      let attributes: [NSAttributedString.Key: Any] = [.font: Design.font(type.cell, row == 0 ? .semibold : .regular)]
      func widest(_ parts: [Substring]) -> CGFloat {
        parts.map { (String($0) as NSString).size(withAttributes: attributes).width }.max() ?? 0
      }
      let cell = string.substring(with: span.range)
      let word = widest(cell.split(whereSeparator: \.isWhitespace)) + chrome
      let line = max(word, widest(cell.split(whereSeparator: \.isNewline)) + chrome)
      var widths = measured[id] ?? Array(repeating: (chrome, chrome), count: columns)
      widths[column] = (max(widths[column].word, word), max(widths[column].line, line))
      measured[id] = widths
    }
    return measured.mapValues { widths in
      let lines = widths.map(\.line)
      let natural = lines.reduce(0, +)
      func shares(_ columns: [CGFloat]) -> [CGFloat] {
        let total = columns.reduce(0, +)
        return columns.map { ($0 - box) / total * 100 }
      }
      guard available < natural else { return (natural, shares(lines)) }
      let words = widths.map(\.word)
      let extra = max(0, available - words.reduce(0, +))
      let flexible = max(1, natural - words.reduce(0, +))
      return (available, shares(zip(words, lines).map { $0 + extra * ($1 - $0) / flexible }))
    }
  }

  nonisolated static func url(_ destination: String, base: URL?) -> URL? {
    let encoded = destination.replacingOccurrences(of: " ", with: "%20")
    guard let url = URL(string: encoded, relativeTo: base)?.absoluteURL,
      ["http", "https", "mailto", "file"].contains(url.scheme?.lowercased() ?? "")
    else { return nil }
    return url
  }

  nonisolated static func slug(_ title: String) -> String {
    String(
      title.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().compactMap {
        $0 == " " ? "-" : $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : nil
      })
  }
}
