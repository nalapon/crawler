import AppKit
import ImageIO

/// Full-window Markdown reading view, also used read-only for Word documents converted by
/// `WordDocument`. Like `PDFReader`, it covers the workspace without replacing its tabs,
/// search, selection or scroll views.
@MainActor
final class MarkdownReader: Surface, WorkspaceReader, NSTextViewDelegate {
  /// Where to place a document once it has rendered.
  enum Position {
    case top
    case anchor(String)
    case character(Int)
  }
  var close: (() -> Void)?
  var didChange: (() -> Void)?
  private(set) var url: URL
  private(set) var state = "loading" { didSet { didChange?() } }
  private(set) var loadMilliseconds: Double?
  let textView = PinchTextView(frame: .zero)
  private let scroll = NSScrollView()
  private let header = Surface()
  private let title = label("", size: 12, weight: .medium)
  private let progress = label("", size: 11, color: Design.muted)
  private let hint = label("Esc twice to return", size: 11, color: Design.muted)
  private let message = label("Loading…", size: 12, color: Design.muted)
  private let backButton = ActionButton("", symbol: "chevron.left", help: "Back · ⌘[", action: {})
  private let editButton = ActionButton(
    "Open in editor", symbol: "square.and.pencil", help: "Open in your text editor · E", action: {})
  private let closeButton = ActionButton("", symbol: "xmark", help: "Close · Esc twice", action: {})
  private let zoomOutButton = ActionButton("", symbol: "minus.magnifyingglass", help: "Zoom out · ⌘−", action: {})
  private let zoomResetButton = ActionButton("100%", help: "Actual size · ⌘0", action: {})
  private let zoomInButton = ActionButton("", symbol: "plus.magnifyingglass", help: "Zoom in · ⌘+", action: {})
  /// Pinch distance not yet turned into a zoom step.
  private var pinch: CGFloat = 0
  private var content: PreviewText?
  private var images: [String: NSImage] = [:]
  /// Documents left through links, with the first visible character to return to.
  private var history: [(url: URL, character: Int)] = []
  private var loadTask: Task<Void, Never>?
  private var lastEscape: TimeInterval?
  private var columnWidth: CGFloat = 0
  private static let scaleKey = "markdownReaderScale"
  private var scale = CGFloat(UserDefaults.standard.object(forKey: scaleKey) as? Double ?? 1)
  private var typography: MarkdownTypography { .reader(scale: scale) }

  init(url: URL) {
    self.url = url
    super.init(frame: .zero)
    identifier = NSUserInterfaceItemIdentifier("markdown-reader")
    setAccessibilityLabel("Markdown reader")
    fill = Design.raised
    header.bottomLine = true
    title.lineBreakMode = .byTruncatingMiddle
    progress.alignment = .right
    hint.alignment = .right
    message.alignment = .center
    closeButton.identifier = NSUserInterfaceItemIdentifier("close-markdown")
    closeButton.invoke = { [weak self] in self?.close?() }
    backButton.invoke = { [weak self] in self?.goBack() }
    editButton.invoke = { [weak self] in self?.openInEditor() }
    zoomOutButton.invoke = { [weak self] in self?.zoom(-1) }
    zoomResetButton.invoke = { [weak self] in self?.zoom(0) }
    zoomInButton.invoke = { [weak self] in self?.zoom(1) }
    textView.pinched = { [weak self] event in self?.pinch(event) }
    updateZoomControls()
    textView.isEditable = false
    textView.isSelectable = true
    textView.isRichText = true
    textView.importsGraphics = false
    textView.drawsBackground = false
    textView.usesFindBar = true
    textView.isIncrementalSearchingEnabled = true
    textView.isVerticallyResizable = true
    textView.isHorizontallyResizable = false
    textView.autoresizingMask = [.width]
    textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
    textView.textContainer?.widthTracksTextView = true
    // Contiguous layout keeps the scroll height exact: AppKit lays out whatever scrolls into view
    // immediately and finishes the rest during idle time. Non-contiguous estimates drift badly
    // with code blocks and tables, so the end of the document kept moving while scrolling.
    textView.layoutManager?.allowsNonContiguousLayout = false
    textView.linkTextAttributes = [.foregroundColor: Design.accent, .cursor: NSCursor.pointingHand]
    textView.delegate = self
    textView.setAccessibilityLabel("Markdown document")
    scroll.drawsBackground = false
    // The window's transparent titlebar would otherwise add a top inset below the header.
    scroll.automaticallyAdjustsContentInsets = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.horizontalScrollElasticity = .none
    scroll.documentView = textView
    scroll.contentView.postsBoundsChangedNotifications = true
    NotificationCenter.default.addObserver(
      self, selector: #selector(updateProgress), name: NSView.boundsDidChangeNotification,
      object: scroll.contentView)
    for view in [scroll, message, header] { addSubview(view) }
    for view in [backButton, title, progress, zoomOutButton, zoomResetButton, zoomInButton, editButton, hint,
      closeButton] { header.addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }

  func stop() {
    loadTask?.cancel()
    loadTask = nil
  }

  /// Read and render `target` off the main actor, then decode its local images.
  func load(_ target: URL, at position: Position = .top) {
    stop()
    url = target
    title.stringValue = target.lastPathComponent
    title.toolTip = target.path
    // Word documents are shown converted to Markdown; E opens the original in its default app.
    let word = PreviewFormat.forURL(target) == .document
    editButton.title = word ? "Open in app" : "Open in editor"
    editButton.toolTip = word ? "Open in the default app · E" : "Open in your text editor · E"
    content = nil
    images = [:]
    textView.string = ""
    message.stringValue = "Loading…"
    message.isHidden = false
    progress.stringValue = ""
    state = "loading"
    needsLayout = true
    let started = ContinuousClock.now
    loadTask = Task { [weak self] in
      do {
        let content = try await PreviewTextLoader.shared.load(target, budget: .reader)
        try Task.checkCancellation()
        guard let self else { return }
        self.content = content
        self.render()
        self.place(position)
        let elapsed = started.duration(to: .now).components
        self.loadMilliseconds = Double(elapsed.seconds) * 1_000 + Double(elapsed.attoseconds) / 1e15
        self.state = "markdown"
        let decoded = await Self.decodeImages(content, base: target)
        try Task.checkCancellation()
        guard !decoded.isEmpty else { return }
        // Each replaced "Image: alt" range collapses to one attachment character.
        let top = self.topCharacter()
        let shift = content.spans.reduce(0) { total, span in
          guard case .image(let source) = span.style, decoded[source] != nil,
            NSMaxRange(span.range) <= top
          else { return total }
          return total + span.range.length - 1
        }
        self.images = decoded.mapValues {
          NSImage(cgImage: $0, size: NSSize(width: $0.width, height: $0.height))
        }
        self.render()
        self.place(.character(top - shift))
      } catch is CancellationError {
      } catch {
        guard let self else { return }
        self.message.stringValue = error.localizedDescription
        self.state = "failed"
      }
    }
  }

  private func render() {
    guard let content else { return }
    updateColumn()
    textView.textStorage?.setAttributedString(
      MarkdownStyle.attributedString(
        content, typography, base: url, images: images,
        columnWidth: columnWidth > 0 ? columnWidth : .greatestFiniteMagnitude))
    textView.setSelectedRange(NSRange(location: 0, length: 0))
    message.stringValue = content.notice ?? ""
    message.isHidden = content.notice == nil
    needsLayout = true
    layoutSubtreeIfNeeded()
  }

  /// Decode local images referenced by the document. Remote images are never fetched; at most
  /// 48 files are read, each downsampled to 2,400 pixels.
  nonisolated private static func decodeImages(_ content: PreviewText, base: URL) async -> [String: CGImage] {
    var sources: [String] = []
    for span in content.spans {
      if case .image(let source) = span.style, !sources.contains(source) { sources.append(source) }
    }
    var result: [String: CGImage] = [:]
    let options = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: 2_400,
    ] as CFDictionary
    for source in sources.prefix(48) {
      guard !Task.isCancelled else { break }
      guard let url = MarkdownStyle.url(source, base: base), url.isFileURL,
        let file = CGImageSourceCreateWithURL(url as CFURL, nil),
        let image = CGImageSourceCreateThumbnailAtIndex(file, 0, options)
      else { continue }
      result[source] = image
    }
    return result
  }

  override func layout() {
    super.layout()
    let width = bounds.width
    header.frame = NSRect(x: 0, y: 0, width: width, height: 48)
    backButton.isHidden = history.isEmpty
    backButton.frame = NSRect(x: 88, y: 8, width: 32, height: 32)
    let titleX: CGFloat = history.isEmpty ? 88 : 126
    closeButton.frame = NSRect(x: width - 46, y: 8, width: 32, height: 32)
    hint.frame = NSRect(x: width - 222, y: 15, width: 168, height: 20)
    editButton.frame = NSRect(x: width - 362, y: 9, width: 132, height: 30)
    zoomInButton.frame = NSRect(x: width - 398, y: 9, width: 30, height: 30)
    zoomResetButton.frame = NSRect(x: width - 452, y: 9, width: 52, height: 30)
    zoomOutButton.frame = NSRect(x: width - 484, y: 9, width: 30, height: 30)
    progress.frame = NSRect(x: width - 540, y: 15, width: 44, height: 20)
    title.frame = NSRect(x: titleX, y: 15, width: max(0, width - 552 - titleX), height: 20)
    message.frame = NSRect(x: 20, y: content == nil ? bounds.midY - 10 : bounds.height - 30,
      width: max(0, width - 40), height: 20)
    let frame = NSRect(x: 0, y: 48, width: width, height: max(0, bounds.height - 48))
    if scroll.frame != frame {
      // A resized NSTextView scrolls its selection into view, which jumped past the top inset;
      // keep the first visible line instead, also across reflows.
      let top = topCharacter()
      scroll.frame = frame
      updateColumn()
      scroll(toCharacter: top)
    } else {
      updateColumn()
    }
    updateProgress()
  }

  /// Center a column of about 70 characters; narrow windows keep a comfortable margin.
  private func updateColumn() {
    let available = scroll.contentSize.width
    guard available > 0 else { return }
    let measure = (typography.body * 40).rounded()
    let side = max(typography.body * 2, ((available - measure) / 2).rounded())
    let inset = NSSize(width: side, height: (typography.body * 2.5).rounded())
    if textView.textContainerInset != inset { textView.textContainerInset = inset }
    textView.minSize = NSSize(width: 0, height: scroll.contentSize.height)
    textView.setFrameSize(NSSize(width: available, height: max(textView.frame.height, scroll.contentSize.height)))
    // The width lines actually get: line fragment padding sits inside the text container.
    let width = max(0, available - 2 * (side + (textView.textContainer?.lineFragmentPadding ?? 0)))
    guard abs(width - columnWidth) > 0.5 else { return }
    columnWidth = width
    fitImages()
  }

  /// Keep images within the column without enlarging them past their natural size.
  private func fitImages() {
    guard let storage = textView.textStorage, let layout = textView.layoutManager else { return }
    storage.enumerateAttribute(.attachment, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
      guard let attachment = value as? NSTextAttachment, let image = attachment.image else { return }
      let width = min(image.size.width, columnWidth)
      attachment.bounds = NSRect(
        x: 0, y: 0, width: width, height: image.size.height * width / max(1, image.size.width))
      layout.invalidateLayout(forCharacterRange: range, actualCharacterRange: nil)
    }
  }

  @objc private func updateProgress() {
    let visible = scroll.contentView.bounds
    let total = textView.frame.height - visible.height
    progress.stringValue = content != nil && total > 1
      ? "\(Int((min(1, max(0, visible.minY / total)) * 100).rounded()))%" : ""
  }

  private func topCharacter() -> Int {
    guard let layout = textView.layoutManager, let container = textView.textContainer,
      layout.numberOfGlyphs > 0
    else { return 0 }
    let origin = textView.textContainerOrigin
    let point = NSPoint(x: 0, y: max(0, scroll.contentView.bounds.minY - origin.y + 1))
    return layout.characterIndexForGlyph(at: layout.glyphIndex(for: point, in: container))
  }

  private func place(_ position: Position) {
    switch position {
    case .top: scroll(to: 0)
    case .character(let index): scroll(toCharacter: index)
    case .anchor(let fragment): scroll(toAnchor: fragment)
    }
  }

  private func scroll(to y: CGFloat) {
    let clip = scroll.contentView
    let limit = max(0, textView.frame.height - clip.bounds.height)
    clip.scroll(to: NSPoint(x: 0, y: min(max(0, y), limit)))
    scroll.reflectScrolledClipView(clip)
  }

  private func scroll(toCharacter index: Int) {
    guard let layout = textView.layoutManager, let length = textView.textStorage?.length, length > 0 else {
      return
    }
    guard index > 0 else { return scroll(to: 0) }
    let index = min(index, length - 1)
    layout.ensureLayout(forCharacterRange: NSRange(location: 0, length: index + 1))
    let line = layout.lineFragmentRect(forGlyphAt: layout.glyphIndexForCharacter(at: index), effectiveRange: nil)
    scroll(to: line.minY + textView.textContainerOrigin.y)
  }

  private func scroll(toAnchor fragment: String) {
    guard let storage = textView.textStorage else { return }
    let target = fragment.lowercased()
    var found: Int?
    storage.enumerateAttribute(.markdownAnchor, in: NSRange(location: 0, length: storage.length)) { value, range, stop in
      if (value as? String) == target {
        found = range.location
        stop.pointee = true
      }
    }
    if let found { scroll(toCharacter: found) }
  }

  func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
    guard let target = link as? URL else { return true }
    guard target.isFileURL else {
      NSWorkspace.shared.open(target)
      return true
    }
    let fragment = target.fragment(percentEncoded: false)
    let file = URL(fileURLWithPath: target.path).standardizedFileURL
    if file == url.standardizedFileURL {
      if let fragment { scroll(toAnchor: fragment) }
    } else if PreviewFormat.forURL(file) == .markdown {
      history.append((url, topCharacter()))
      load(file, at: fragment.map(Position.anchor) ?? .top)
    } else {
      FileOpening.shared.open(file) { [weak self] outcome in
        if case .failed(let reason) = outcome { self?.alert("Could not open \(file.lastPathComponent)", reason) }
      }
    }
    return true
  }

  private func goBack() {
    guard let previous = history.popLast() else { return }
    load(previous.url, at: .character(previous.character))
  }

  private func openInEditor() {
    let file = url
    FileOpening.shared.open(file) { [weak self] outcome in
      if case .failed(let reason) = outcome { self?.alert("Could not open \(file.lastPathComponent)", reason) }
    }
  }

  private func alert(_ title: String, _ text: String) {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = text
    alert.beginSheetModal(for: window)
  }

  /// Zoom by one 10% step, or reset with 0, keeping the first visible line in place.
  private func zoom(_ step: Int) {
    let next = step == 0 ? 1 : min(2, max(0.7, ((scale * 10).rounded() + CGFloat(step)) / 10))
    guard next != scale, content != nil else { return }
    let top = topCharacter()
    scale = next
    UserDefaults.standard.set(Double(scale), forKey: Self.scaleKey)
    updateZoomControls()
    render()
    scroll(toCharacter: top)
  }

  /// Turn a trackpad pinch into 10% steps; each step reflows the text at the new size.
  private func pinch(_ event: NSEvent) {
    if event.phase == .began { pinch = 0 }
    pinch += event.magnification
    while abs(pinch) >= 0.1 {
      zoom(pinch > 0 ? 1 : -1)
      pinch -= pinch > 0 ? 0.1 : -0.1
    }
  }

  private func updateZoomControls() {
    zoomResetButton.title = "\(Int((scale * 100).rounded()))%"
    zoomOutButton.isEnabled = scale > 0.7
    zoomInButton.isEnabled = scale < 2
  }

  private func find(_ action: NSTextFinder.Action) {
    let item = NSMenuItem()
    item.tag = action.rawValue
    textView.performTextFinderAction(item)
  }

  func cancelEscape() {
    lastEscape = nil
    hint.stringValue = "Esc twice to return"
  }

  func handleKey(_ event: NSEvent) -> Bool {
    let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
    if event.keyCode == 53, modifiers.isEmpty {
      if scroll.isFindBarVisible {
        find(.hideFindInterface)
        window?.makeFirstResponder(textView)
        cancelEscape()
        return true
      }
      guard !event.isARepeat else { return true }
      if let lastEscape, event.timestamp - lastEscape <= 0.7 {
        close?()
      } else {
        lastEscape = event.timestamp
        hint.stringValue = "Press Esc again to return"
      }
      return true
    }
    cancelEscape()
    let key = event.charactersIgnoringModifiers ?? ""
    if modifiers.subtracting(.shift) == .command {
      switch key.lowercased() {
      case "w": close?(); return true
      case "f": find(.showFindInterface); return true
      case "g": find(modifiers.contains(.shift) ? .previousMatch : .nextMatch); return true
      case "[": goBack(); return true
      case "=", "+": zoom(1); return true
      case "-": zoom(-1); return true
      case "0": zoom(0); return true
      default: break
      }
    }
    // The find bar's field receives ordinary typing and editing shortcuts.
    if let editor = window?.firstResponder as? NSTextView, editor.isEditable { return false }
    // Keep explorer shortcuts out of the reader; copy, select all, hide and quit still work.
    if modifiers.contains(.command) { return !["c", "a", "q", "h"].contains(key.lowercased()) }
    guard modifiers.isEmpty || modifiers == .shift else { return true }
    let line = typography.body * 3
    let page = max(line, scroll.contentView.bounds.height - line)
    let y = scroll.contentView.bounds.minY
    switch event.keyCode {
    case 125: scroll(to: y + line)  // Down arrow
    case 126: scroll(to: y - line)  // Up arrow
    case 121: scroll(to: y + page)  // Page Down
    case 116: scroll(to: y - page)  // Page Up
    case 115: scroll(to: 0)  // Home
    case 119: textView.scrollToEndOfDocument(nil)  // End
    case 49: scroll(to: y + (modifiers == .shift ? -page : page))  // Space
    default:
      switch key {
      case "j": scroll(to: y + line)
      case "k": scroll(to: y - line)
      case "d": scroll(to: y + page / 2)
      case "u": scroll(to: y - page / 2)
      case "g": scroll(to: 0)
      case "G": textView.scrollToEndOfDocument(nil)
      case "e": openInEditor()
      case "/": find(.showFindInterface)
      default: break
      }
    }
    return true
  }
}

/// Forwards trackpad pinches, which `NSTextView` otherwise ignores, to the reader's zoom.
@MainActor
final class PinchTextView: NSTextView {
  var pinched: ((NSEvent) -> Void)?
  override func magnify(with event: NSEvent) {
    if let pinched { pinched(event) } else { super.magnify(with: event) }
  }
}

extension Workspace {
  func showMarkdown(_ url: URL, succeeded: (() -> Void)? = nil) {
    guard reader == nil else { return }
    let reader = MarkdownReader(url: url)
    presentReader(reader, focus: reader.textView) { [weak reader] in reader?.stop() }
    var recorded = false
    reader.didChange = { [weak self, weak reader] in
      guard let self, let reader, self.reader === reader else { return }
      if reader.state == "markdown", !recorded {
        recorded = true
        FileOpening.recordRecent(url)
        self.filePalette.reloadRecents()
        succeeded?()
      }
      self.onChange?()
    }
    reader.load(url)
    onChange?()
  }
}
