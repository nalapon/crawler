import AppKit
import OSLog

/// The same renderer is used by the panes, PDF reader, and Space window.
@MainActor
final class DocumentPreview: Surface, NSTextViewDelegate {
  let textScroll = FileScrollView()
  private let textView = NSTextView(frame: .zero)
  private let message = label("", size: 12, color: Design.muted)
  private let status = label("", size: 10, color: Design.muted)
  private let footer = Surface()
  private let artwork = DocumentArtwork()
  private var video: VideoPreview?
  var videoSeconds: Double? { video?.seconds }
  private var pdfView: NativePDFView?
  var pdf: NativePDFView? { pdfView }
  private var pdfPreparation: PDFPreparation?
  private var pdfSuspended = false
  private var pdfPresentation: PDFPresentation = .pane
  private var pdfSuspendedAt: ContinuousClock.Instant?
  private var pdfInactiveDuration = Duration.zero
  private var pdfButtons: [NSButton] = []
  private var loadTask: Task<Void, Never>?
  private var generation = 0
  private var current: FileItem?
  private var rendered: PreviewText?
  private var renderedTheme: Theme?
  private var gutter: CodeLineGutter?
  var didChange: (() -> Void)?
  private(set) var state = "empty" { didSet { didChange?() } }
  private(set) var loadMilliseconds: Double?
  private(set) var reusedPDF = false
  private var sharpPDFMilliseconds: Double?

  override init(frame: NSRect) {
    super.init(frame: frame)
    textScroll.drawsBackground = false
    textScroll.hasVerticalScroller = false
    textScroll.hasHorizontalScroller = false
    textScroll.horizontalScrollElasticity = .none
    textView.isEditable = false
    textView.isSelectable = true
    textView.isRichText = true
    textView.importsGraphics = false
    textView.drawsBackground = false
    textView.isVerticallyResizable = true
    textView.isHorizontallyResizable = false
    textView.minSize = .zero
    textView.maxSize = NSSize(
      width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    textView.autoresizingMask = [.width]
    textView.textContainer?.widthTracksTextView = true
    textView.textContainer?.containerSize = NSSize(
      width: 0, height: CGFloat.greatestFiniteMagnitude)
    textView.layoutManager?.allowsNonContiguousLayout = true
    textView.textContainerInset = NSSize(width: 14, height: 16)
    textView.delegate = self
    textView.setAccessibilityLabel("File preview")
    textScroll.documentView = textView
    for view in [artwork, textScroll, message, footer] { addSubview(view) }
    footer.addSubview(status)
    message.maximumNumberOfLines = 4
    message.lineBreakMode = .byWordWrapping
    message.alignment = .center
    textScroll.contentView.postsBoundsChangedNotifications = true
  }
  required init?(coder: NSCoder) { nil }
  deinit { loadTask?.cancel() }

  func configure(
    _ file: FileItem?, coalesceSelection: Bool = true, reusingPDF preview: DocumentPreview? = nil,
    presentation: PDFPresentation = .pane
  ) {
    fill = Design.raised
    artwork.fill = Design.raised
    artwork.needsDisplay = true
    footer.fill = Design.raised
    status.textColor = Design.muted
    message.textColor = Design.muted
    if current == file, state != "empty" {
      if let rendered, renderedTheme != Design.theme { display(rendered) }
      return
    }
    stop()
    pdfPresentation = presentation
    current = file
    rendered = nil
    loadMilliseconds = nil
    sharpPDFMilliseconds = nil
    reusedPDF = false
    textView.string = ""
    textScroll.isHidden = true
    artwork.isHidden = true
    message.isHidden = false
    status.stringValue = ""
    guard let file else {
      state = "empty"
      message.stringValue = "No file selected"
      return
    }
    guard file.kind.folder == nil else {
      state = "folder"
      message.stringValue = "Select a file to preview"
      return
    }
    let format = PreviewFormat.forURL(file.url)
    if format == .video {
      let video = VideoPreview(frame: bounds)
      self.video = video
      message.isHidden = true
      footer.isHidden = true
      let started = ContinuousClock.now
      video.didChange = { [weak self, weak video] in
        guard let self, let video else { return }
        if self.loadMilliseconds == nil, video.state.hasSuffix("-paused") {
          let elapsed = started.duration(to: .now).components
          self.loadMilliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
        }
        self.state = video.state
      }
      addSubview(video)
      video.configure(file.url)
      needsLayout = true
      return
    }
    if format == .thumbnail {
      state = "thumbnail"
      message.isHidden = true
      artwork.isHidden = false
      artwork.fill = Design.raised
      artwork.item = file
      return
    }
    state = "loading"
    message.stringValue = "Loading preview…"
    let token = generation
    let url = file.url
    if format == .pdf {
      // Share even an in-flight preparation; entering immediately must not open the PDF twice.
      let previous = preview?.current == file ? preview?.pdfPreparation : nil
      let colorSpace = window?.colorSpace?.cgColorSpace ?? NSScreen.main?.colorSpace?.cgColorSpace
      pdfPreparation = previous ?? PDFPreparation(url: url, colorSpace: colorSpace)
      reusedPDF = previous != nil
    }
    let started = ContinuousClock.now
    if format == .pdf {
      pdfInactiveDuration = .zero
      pdfSuspendedAt = pdfSuspended ? started : nil
    }
    loadTask = Task { [weak self] in
      do {
        // Coalesce rapid arrow-key selection before starting disk or parsing work.
        if coalesceSelection { try await Task.sleep(for: .milliseconds(35)) }
        if format == .pdf {
          let previous = preview?.current == file ? preview?.pdfView : nil
          guard let preparation = self?.pdfPreparation else { return }
          let source = try await preparation.load()
          try Task.checkCancellation()
          guard let self, self.generation == token else { return }
          self.display(source, reusing: previous, started: started)
          return
        } else {
          let result = try await PreviewTextLoader.shared.load(url)
          try Task.checkCancellation()
          guard let self, self.generation == token else { return }
          self.rendered = result
          self.display(result)
        }
        let elapsed = started.duration(to: .now).components
        self?.loadMilliseconds =
          Double(elapsed.attoseconds) / 1e15 + Double(elapsed.seconds) * 1_000
        self?.didChange?()
      } catch is CancellationError {} catch {
        guard let self, self.generation == token else { return }
        self.fail(error.localizedDescription)
      }
    }
    needsLayout = true
  }

  func focusPDF() { window?.makeFirstResponder(pdfView ?? self) }
  func suspendPDF() {
    if !pdfSuspended { pdfSuspendedAt = .now }
    pdfSuspended = true; pdfView?.suspend()
  }
  func resumePDF() {
    if let start = pdfSuspendedAt { pdfInactiveDuration += start.duration(to: .now) }
    pdfSuspendedAt = nil; pdfSuspended = false; pdfView?.resume()
  }
  var pdfDiagnostics: [String: Any] {
    guard let pdfView else { return [:] }
    var result: [String: Any] = ["renderer": "Core Graphics", "page": pdfView.currentPage,
      "pages": pdfView.pageCount, "cache_bytes": pdfView.cacheBytes,
      "rasters": pdfView.rasterCount, "cache_hits": pdfView.cacheHits,
      "parse_ms": pdfView.parseMilliseconds, "max_raster_ms": pdfView.maximumRasterMilliseconds,
      "max_scroll_update_ms": pdfView.scrollUpdateMilliseconds]
    if let sharpPDFMilliseconds { result["first_sharp_ms"] = sharpPDFMilliseconds }
    return result
  }

  func suspendVideo() {
    guard video != nil else { return }
    video?.didChange = nil
    video?.stop()
    video?.removeFromSuperview()
    video = nil
    state = "video-suspended"
    message.stringValue = "Video open in preview window"
    message.isHidden = false
  }
  func resumeVideo() {
    guard state == "video-suspended", let file = current else { return }
    current = nil
    configure(file)
  }

  func stop() {
    generation += 1
    loadTask?.cancel()
    loadTask = nil
    artwork.item = nil
    video?.didChange = nil
    video?.stop()
    video?.removeFromSuperview()
    video = nil
    footer.isHidden = false
    gutter?.removeFromSuperview()
    gutter = nil
    pdfView?.stop()
    pdfView?.removeFromSuperview()
    pdfView = nil
    pdfPreparation = nil
    pdfButtons.forEach { $0.removeFromSuperview() }
    pdfButtons = []
    current = nil
    rendered = nil
    textView.string = ""
    state = "empty"
  }

  private func fail(_ reason: String) {
    state = "failed"
    message.stringValue = reason
    message.isHidden = false
    status.stringValue = "Preview unavailable"
  }

  private func display(_ source: PDFSource, reusing preview: NativePDFView?, started: ContinuousClock.Instant) {
    let view = NativePDFView(source: source, reusing: preview, presentation: pdfPresentation)
    if pdfSuspended { view.suspend() }
    pdfView = view
    view.firstPageReady = { [weak self, weak view] in
      guard let self, self.pdfView === view else { return }
      let elapsed = (started.duration(to: .now) - self.pdfInactiveDuration).components
      self.loadMilliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
      PDFPerformance.log.info("PDF \(self.pdfPresentation.rawValue, privacy: .public) first bitmap: \(self.loadMilliseconds ?? 0, privacy: .public) ms, pages: \(source.pages.count, privacy: .public)")
      self.message.isHidden = true
      self.state = "pdf"
    }
    view.pageChanged = { [weak self] page, total in
      self?.status.stringValue = "Page \(page) of \(total)"
    }
    view.firstSharpPageReady = { [weak self] in
      guard let self else { return }
      let elapsed = (started.duration(to: .now) - self.pdfInactiveDuration).components
      let milliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
      self.sharpPDFMilliseconds = milliseconds
      PDFPerformance.log.info("PDF \(self.pdfPresentation.rawValue, privacy: .public) first sharp bitmap: \(milliseconds, privacy: .public) ms")
    }
    view.failed = { [weak self] reason in self?.fail(reason) }
    view.renderingFinished = { [weak self] in self?.didChange?() }
    view.annotationStateChanged = { [weak self] in self?.didChange?() }
    addSubview(view, positioned: .below, relativeTo: message)
    for (title, action) in [
      ("−", #selector(zoomOut)), ("+", #selector(zoomIn)), ("Fit", #selector(fitPDF)),
    ] {
      let button = NSButton(title: title, target: self, action: action)
      button.bezelStyle = .rounded
      button.controlSize = .small
      button.setAccessibilityLabel(
        title == "−" ? "Zoom out" : title == "+" ? "Zoom in" : "Fit PDF page")
      footer.addSubview(button)
      pdfButtons.append(button)
    }
    needsLayout = true
    layoutSubtreeIfNeeded()
  }
  @objc private func zoomIn() { pdfView?.zoomIn() }
  @objc private func zoomOut() { pdfView?.zoomOut() }
  @objc private func fitPDF() { pdfView?.fit() }

  private func display(_ content: PreviewText) {
    renderedTheme = Design.theme
    state = content.markdown ? "markdown" : "code"
    let baseFont =
      content.markdown ? Design.font(13) : NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
    let paragraph = NSMutableParagraphStyle()
    paragraph.lineSpacing = content.markdown ? 4 : 2
    paragraph.lineBreakMode = .byWordWrapping
    let attributed = NSMutableAttributedString(
      string: content.text,
      attributes: [
        .font: baseFont, .foregroundColor: Design.text, .paragraphStyle: paragraph,
      ])
    var tables: [Int: NSTextTable] = [:]
    var colors: [String: NSColor] = [:]
    for span in content.spans where NSMaxRange(span.range) <= attributed.length {
      var attributes: [NSAttributedString.Key: Any] = [:]
      switch span.style {
      case .syntax(let name):
        let color = colors[name] ?? Self.syntaxColor(name)
        colors[name] = color
        attributes[.foregroundColor] = color
      case .heading(let level):
        attributes[.font] = Design.font(CGFloat(max(14, 26 - level * 2)), .bold)
      case .strong:
        attributed.enumerateAttribute(.font, in: span.range) { value, range, _ in
          if let font = value as? NSFont {
            attributed.addAttribute(
              .font, value: NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask),
              range: range)
          }
        }
      case .emphasis:
        attributed.enumerateAttribute(.font, in: span.range) { value, range, _ in
          if let font = value as? NSFont {
            attributed.addAttribute(
              .font, value: NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask),
              range: range)
          }
        }
      case .strike: attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
      case .code:
        attributes[.font] = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        attributes[.backgroundColor] = Design.line.withAlphaComponent(0.45)
      case .quote:
        let style = NSMutableParagraphStyle()
        style.lineSpacing = paragraph.lineSpacing
        style.headIndent = 16
        style.firstLineHeadIndent = 16
        attributes[.paragraphStyle] = style
        attributes[.foregroundColor] = Design.muted
      case .link(let destination):
        if let url = URL(string: destination, relativeTo: current?.url.deletingLastPathComponent()),
          ["http", "https", "mailto", "file"].contains(url.scheme?.lowercased() ?? "")
        {
          attributes[.link] = url.absoluteURL
        }
      case .list(let depth):
        let style = NSMutableParagraphStyle()
        style.headIndent = CGFloat(max(0, depth - 2) * 8 + 18)
        style.paragraphSpacing = 2
        attributes[.paragraphStyle] = style
      case .cell(let id, let row, let column, let columns):
        let table = tables[id] ?? NSTextTable()
        table.numberOfColumns = columns
        table.layoutAlgorithm = .fixedLayoutAlgorithm
        table.setValue(100, type: .percentageValueType, for: .width)
        tables[id] = table
        let block = NSTextTableBlock(
          table: table, startingRow: row, rowSpan: 1, startingColumn: column, columnSpan: 1)
        block.setWidth(5, type: .absoluteValueType, for: .padding)
        block.setWidth(0.5, type: .absoluteValueType, for: .border)
        block.setBorderColor(Design.line)
        if row == 0 {
          block.backgroundColor = Design.line.withAlphaComponent(0.4)
          attributes[.font] = Design.font(12, .semibold)
        }
        let style = NSMutableParagraphStyle()
        style.textBlocks = [block]
        attributes[.paragraphStyle] = style
      }
      attributed.addAttributes(attributes, range: span.range)
    }
    textView.textStorage?.setAttributedString(attributed)
    textView.setSelectedRange(NSRange(location: 0, length: 0))
    status.stringValue =
      content.notice
      ?? (content.markdown
        ? "Markdown"
        : "\(content.lineStarts.count) lines · \(current?.url.pathExtension.uppercased() ?? "Text")")
    status.toolTip = status.stringValue
    gutter?.removeFromSuperview()
    gutter = nil
    if !content.markdown {
      let view = CodeLineGutter(
        textView: textView, scrollView: textScroll, starts: content.lineStarts)
      addSubview(view)
      gutter = view
    }
    textScroll.isHidden = false
    message.isHidden = true
    needsLayout = true
    layoutSubtreeIfNeeded()
    textScroll.contentView.scroll(to: .zero)
    textScroll.reflectScrolledClipView(textScroll.contentView)
  }

  private static func syntaxColor(_ capture: String) -> NSColor {
    let family = capture.split(separator: ".").first.map(String.init) ?? capture
    return switch family {
    case "comment": Design.color(Design.dark ? 0x9ba891 : 0x78836b)
    case "string", "character": Design.color(Design.dark ? 0xeda4a4 : 0x9b3434)
    case "keyword", "include", "conditional", "repeat", "exception": Design.color(Design.dark ? 0xe8a4dc : 0x9a328d)
    case "number", "float", "boolean", "constant": Design.color(Design.dark ? 0xbfa5f0 : 0x7653b5)
    case "type", "constructor", "namespace", "module": Design.color(Design.dark ? 0x85c9d7 : 0x236e80)
    case "function", "method": Design.color(Design.dark ? 0x8abaf0 : 0x2462a6)
    case "property", "field", "attribute", "tag", "label": Design.color(Design.dark ? 0xdec185 : 0x8b6518)
    case "operator", "punctuation": Design.muted
    default: Design.text
    }
  }

  func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
    if let url = link as? URL,
      ["http", "https", "mailto", "file"].contains(url.scheme?.lowercased() ?? "")
    {
      if url.isFileURL {
        FileOpening.shared.open(url) { [weak self] outcome in
          switch outcome {
          case .opened(let notice): if let notice { self?.status.stringValue = notice }
          case .failed(let message): self?.status.stringValue = message
          }
        }
      } else { NSWorkspace.shared.open(url) }
    }
    return true
  }

  override func layout() {
    super.layout()
    video?.frame = bounds
    let footerHeight: CGFloat = 30
    let content = NSRect(
      x: 0, y: 0, width: bounds.width, height: max(0, bounds.height - footerHeight))
    artwork.frame = content
    if pdfView == nil, video == nil {
      let gutterWidth: CGFloat = gutter == nil ? 0 : 44
      gutter?.frame = NSRect(x: 0, y: 0, width: gutterWidth, height: content.height)
      textScroll.frame = NSRect(
        x: gutterWidth, y: 0, width: max(0, content.width - gutterWidth), height: content.height)
      textScroll.tile()
      textView.setFrameSize(NSSize(width: textScroll.contentView.bounds.width,
        height: max(textView.frame.height, content.height)))
    }
    pdfView?.frame = content
    footer.frame = NSRect(x: 0, y: content.height, width: bounds.width, height: footerHeight)
    status.frame = NSRect(
      x: 12, y: 7, width: max(0, bounds.width - (pdfView == nil ? 24 : 130)), height: 16)
    message.frame = NSRect(
      x: 20, y: max(10, content.midY - 35), width: max(0, bounds.width - 40), height: 80)
    for (i, button) in pdfButtons.enumerated() {
      button.frame = NSRect(
        x: bounds.width - 115 + CGFloat(i * 34), y: 3, width: i == 2 ? 42 : 30, height: 24)
    }
    gutter?.needsDisplay = true
  }
}

/// Draw only visible line numbers; precomputed UTF-16 line starts avoid rescanning the file on scroll.
@MainActor
private final class CodeLineGutter: Surface {
  private weak var text: NSTextView?
  private let starts: [Int]
  init(textView: NSTextView, scrollView: NSScrollView, starts: [Int]) {
    self.text = textView
    self.starts = starts
    super.init(frame: .zero)
    NotificationCenter.default.addObserver(
      self, selector: #selector(redraw), name: NSView.boundsDidChangeNotification,
      object: scrollView.contentView)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func redraw() { needsDisplay = true }
  override func draw(_ rect: NSRect) {
    NSBezierPath(rect: bounds).addClip()
    guard let text, let layout = text.layoutManager, let container = text.textContainer else {
      return
    }
    Design.raised.setFill()
    bounds.fill()
    let origin = text.textContainerOrigin
    let visible = text.visibleRect.offsetBy(dx: -origin.x, dy: -origin.y)
    let glyphs = layout.glyphRange(forBoundingRect: visible, in: container)
    let chars = layout.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
    var low = 0
    var high = starts.count
    while low < high {
      let mid = (low + high) / 2
      if starts[mid] < chars.location { low = mid + 1 } else { high = mid }
    }
    for index in max(0, low - 1)..<starts.count {
      let offset = starts[index]
      if offset > NSMaxRange(chars) { break }
      guard offset < (text.textStorage?.length ?? 0) else { continue }
      let glyph = layout.glyphIndexForCharacter(at: offset)
      let line = layout.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
      let y = line.minY + origin.y - text.visibleRect.minY
      let value = String(index + 1) as NSString
      let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .regular),
        .foregroundColor: Design.muted,
      ]
      value.draw(
        at: NSPoint(x: bounds.width - value.size(withAttributes: attributes).width - 9, y: y + 2),
        withAttributes: attributes)
    }
  }
}

@MainActor
final class FilePreviewPanel: NSPanel {
  var didClose: (() -> Void)?
  override func sendEvent(_ event: NSEvent) {
    if event.type == .keyDown, !event.modifierFlags.contains(.command),
      event.keyCode == 53 || event.keyCode == 49
    {
      close()
      return
    }
    super.sendEvent(event)
  }
  override func close() {
    (contentView as? DocumentPreview)?.stop()
    super.close()
    didClose?()
    didClose = nil
  }
}
