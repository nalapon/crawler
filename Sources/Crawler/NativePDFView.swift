import AppKit
import QuartzCore
import OSLog

/// Scrolling composites retained page images. Parsing and rasterization never run in draw/layout/scroll callbacks.
@MainActor
final class NativePDFView: NSView, NSPopoverDelegate {
  let source: PDFSource
  private let presentation: PDFPresentation
  private let scroll = NSScrollView()
  private let canvas = PDFCanvas()
  private var pageFrames: [CGRect] = []
  private var pageViews: [Int: PDFPageSurface] = [:]
  private var wanted: [PDFRasterKey] = []
  private var installed: [PDFRasterKey: CGImage] = [:]
  private var seeds: [Int: CGImage]
  private var renderTask: Task<Void, Never>?
  private var textTask: Task<Void, Never>?
  private var selectionTask: Task<Void, Never>?
  private var strokeTasks: [UUID: Task<Void, Never>] = [:]
  private var observer: NSObjectProtocol?
  private var annotationObserver: NSObjectProtocol?
  private var activeMarkup: (page: Int, id: UUID)?
  private var notePopover: NSPopover?
  private var hoverPopover: NSPopover?
  private var hoverTask: Task<Void, Never>?
  private var hoverMarkup: PDFMarkup?
  private var hoverArea: NSTrackingArea?
  private var selectionStart: PDFTextPoint?
  private var selectionEnd: PDFTextPoint?
  private var selection: PDFTextSelection?
  private var selectionGeneration = 0
  private var dragged = false
  private var draftID = UUID()
  private(set) var annotationTool = PDFAnnotationTool.select
  private(set) var annotationCursor = NSCursor.iBeam
  private var suspended = false
  private var viewportSize = CGSize.zero
  private var zoom: CGFloat = 1
  private var pageScale: CGFloat = 1
  private var ready = false
  private var sharpReady = false
  private(set) var currentPage = 1
  private(set) var scrollUpdateMilliseconds: Double = 0
  private(set) var cacheBytes = 0
  private(set) var rasterCount = 0
  private(set) var cacheHits = 0
  private(set) var parseMilliseconds: Double = 0
  private(set) var maximumRasterMilliseconds: Double = 0
  var firstPageReady: (() -> Void)?
  var firstSharpPageReady: (() -> Void)?
  var pageChanged: ((Int, Int) -> Void)?
  var failed: ((String) -> Void)?
  var renderingFinished: (() -> Void)?
  var annotationStateChanged: (() -> Void)?
  var annotations: PDFAnnotationSession { source.annotations }
  var selectedMarkup: PDFMarkup? {
    guard let activeMarkup else { return nil }
    return annotations.markups(on: activeMarkup.page).first { $0.id == activeMarkup.id }
  }
  var canAnnotateSelection: Bool {
    annotations.allowsEditing && !annotations.isBusy && !(selection?.highlights.isEmpty ?? true)
  }
  var hasPendingStroke: Bool { !strokeTasks.isEmpty }
  var annotationDiagnostics: [String: Any] {
    ["tool": String(describing: annotationTool), "pending_strokes": strokeTasks.count,
      "draft_marks": pageViews.values.reduce(0) { $0 + $1.displayedMarkups.filter { $0.id == draftID }.count },
      "tool_cursor_active": NSCursor.current === annotationCursor,
      "dirty": annotations.isDirty]
  }

  init(source: PDFSource, reusing preview: NativePDFView? = nil, presentation: PDFPresentation) {
    self.source = source
    self.presentation = presentation
    seeds = preview?.pageViews.compactMapValues(\.image) ?? [:]
    if let opening = source.openingImage { seeds[0] = opening }
    super.init(frame: .zero)
    wantsLayer = true
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.hasHorizontalScroller = true
    scroll.autohidesScrollers = true
    scroll.scrollerStyle = .overlay
    scroll.horizontalScrollElasticity = .none
    scroll.verticalScrollElasticity = .none
    canvas.owner = self
    scroll.documentView = canvas
    scroll.contentView.postsBoundsChangedNotifications = true
    addSubview(scroll)
    observer = NotificationCenter.default.addObserver(
      forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
    ) { [weak self] _ in MainActor.assumeIsolated { self?.updateViewport() } }
    annotationObserver = NotificationCenter.default.addObserver(
      forName: PDFAnnotationSession.changed, object: source.annotations, queue: .main
    ) { [weak self] _ in MainActor.assumeIsolated {
      self?.dismissHover(); self?.updateHighlights(); self?.annotationStateChanged?()
    } }
    setAccessibilityLabel("PDF document")
  }
  required init?(coder: NSCoder) { nil }
  deinit {
    renderTask?.cancel(); textTask?.cancel(); selectionTask?.cancel(); hoverTask?.cancel()
    strokeTasks.values.forEach { $0.cancel() }
  }
  override var isFlipped: Bool { true }
  override var acceptsFirstResponder: Bool { true }
  var pageCount: Int { source.pages.count }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    window?.acceptsMouseMovedEvents = true
  }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let hoverArea { removeTrackingArea(hoverArea) }
    let area = NSTrackingArea(rect: .zero,
      options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
    addTrackingArea(area); hoverArea = area
  }
  override func mouseMoved(with event: NSEvent) {
    annotationCursor.set(); scheduleHover(at: textPoint(event))
  }
  override func mouseEntered(with event: NSEvent) { annotationCursor.set() }
  override func mouseExited(with event: NSEvent) { dismissHover(); NSCursor.arrow.set() }

  func stop() {
    if ready {
      PDFPerformance.log.info("PDF \(self.presentation.rawValue, privacy: .public) closed: max scroll update \(self.scrollUpdateMilliseconds, privacy: .public) ms, cache \(self.cacheBytes, privacy: .public) bytes, rasters \(self.rasterCount, privacy: .public)")
    }
    suspend()
    if let observer { NotificationCenter.default.removeObserver(observer) }
    observer = nil
    if let annotationObserver { NotificationCenter.default.removeObserver(annotationObserver) }
    annotationObserver = nil
    notePopover?.close(); notePopover = nil
    annotationStateChanged = nil
    firstPageReady = nil; firstSharpPageReady = nil; pageChanged = nil; failed = nil; renderingFinished = nil
    installed = [:]; seeds = [:]
    pageViews.values.forEach { $0.removeFromSuperview() }; pageViews = [:]
  }
  func suspend() {
    suspended = true
    renderTask?.cancel(); renderTask = nil
    textTask?.cancel(); textTask = nil
    selectionTask?.cancel(); selectionTask = nil
    selectionGeneration += 1
    strokeTasks.values.forEach { $0.cancel() }; strokeTasks = [:]
    selectionStart = nil; selectionEnd = nil; selection = nil
    notePopover?.close()
    dismissHover()
  }
  func resume() { suspended = false; arrangePages() }

  override func layout() {
    super.layout()
    scroll.frame = bounds
    scroll.tile()
    let size = scroll.contentView.bounds.size
    if size != viewportSize {
      viewportSize = size
      arrangePages()
    }
  }
  override func viewDidChangeBackingProperties() {
    super.viewDidChangeBackingProperties()
    updateViewport()
  }

  private func page(at point: CGPoint) -> Int? {
    var low = 0, high = pageFrames.count
    while low < high {
      let mid = (low + high) / 2
      if pageFrames[mid].maxY < point.y { low = mid + 1 } else { high = mid }
    }
    guard pageFrames.indices.contains(low), pageFrames[low].contains(point) else { return nil }
    return low
  }

  private func arrangePages() {
    guard viewportSize.width > 0, viewportSize.height > 0 else { return }
    let oldVisible = scroll.contentView.bounds
    let oldPage = page(at: CGPoint(x: oldVisible.midX, y: oldVisible.midY))
    let anchor = oldPage.map { index in
      CGPoint(x: (oldVisible.midX - pageFrames[index].minX) / pageFrames[index].width,
        y: (oldVisible.midY - pageFrames[index].minY) / pageFrames[index].height)
    }
    let maxWidth = source.pages.map { ($0 ?? source.firstPage).size.width }.max() ?? 612
    pageScale = max(0.05, (viewportSize.width - 32) / maxWidth) * zoom
    let width = max(viewportSize.width, maxWidth * pageScale + 32)
    var y: CGFloat = 16
    pageFrames = source.pages.indices.map { index in
      let geometry = source.layoutGeometry(index)
      let size = CGSize(width: geometry.size.width * pageScale, height: geometry.size.height * pageScale)
      let frame = CGRect(x: (width - size.width) / 2, y: y, width: size.width, height: size.height)
      y += size.height + 16
      return frame
    }
    canvas.setFrameSize(CGSize(width: width, height: max(viewportSize.height, y)))
    if let oldPage, let anchor {
      let frame = pageFrames[oldPage]
      move(to: CGPoint(x: frame.minX + anchor.x * frame.width - viewportSize.width / 2,
        y: frame.minY + anchor.y * frame.height - viewportSize.height / 2))
    }
    installed = [:]
    for view in pageViews.values { view.clearDetail() }
    updateViewport()
  }

  private func fullKey(page: Int, width: Int) -> PDFRasterKey {
    let geometry = source.layoutGeometry(page)
    return PDFRasterKey(page: page, fullWidth: width, x: 0, y: 0, width: width,
      height: max(1, Int(ceil(CGFloat(width) * geometry.size.height / geometry.size.width))))
  }

  private func detailKey(page: Int, visible: CGRect) -> PDFRasterKey {
    let frame = pageFrames[page]
    let backing = window?.backingScaleFactor ?? 2
    let width = max(64, Int(ceil(frame.width * backing / 64)) * 64)
    let full = fullKey(page: page, width: width)
    if full.width <= PDFRasterizer.maximumRasterPixels / full.height { return full }
    // Zoomed pages use one viewport-sized crop, snapped to pixel blocks, rather than enormous full-page images.
    let factor = CGFloat(width) / frame.width
    let area = frame.intersection(visible.insetBy(dx: -128, dy: -256))
    let x = max(0, Int(floor((area.minX - frame.minX) * factor / 512)) * 512)
    let y = max(0, Int(floor((area.minY - frame.minY) * factor / 512)) * 512)
    let w = min(full.width - x, min(4096, max(512, Int(ceil((area.maxX - frame.minX) * factor)) - x)))
    let h = min(full.height - y, min(4096, max(512, Int(ceil((area.maxY - frame.minY) * factor)) - y)))
    return PDFRasterKey(page: page, fullWidth: width, x: x, y: y, width: w, height: h)
  }

  private func updateViewport() {
    guard !suspended, !pageFrames.isEmpty else { return }
    let started = ContinuousClock.now
    let visible = scroll.contentView.bounds
    var low = 0, high = pageFrames.count
    while low < high {
      let mid = (low + high) / 2
      if pageFrames[mid].maxY < visible.minY { low = mid + 1 } else { high = mid }
    }
    let first = min(low, pageFrames.count - 1)
    var last = first
    while last + 1 < pageFrames.count, pageFrames[last + 1].minY <= visible.maxY { last += 1 }
    let nearby = max(0, first - 1)...min(pageFrames.count - 1, last + 1)
    CATransaction.begin(); CATransaction.setDisableActions(true)
    for index in Array(pageViews.keys) where !nearby.contains(index) {
      pageViews.removeValue(forKey: index)?.removeFromSuperview()
    }
    for index in nearby {
      let view = pageViews[index] ?? PDFPageSurface(index: index)
      if view.superview == nil { canvas.addSubview(view); pageViews[index] = view }
      view.frame = pageFrames[index]
      if view.image == nil, let seed = seeds.removeValue(forKey: index) {
        view.setImage(seed)
        markReady(index)
      }
    }
    CATransaction.commit()
    seeds = seeds.filter { nearby.contains($0.key) }
    let centerPage = page(at: CGPoint(x: visible.midX, y: visible.midY)) ?? first
    if currentPage != centerPage + 1 {
      currentPage = centerPage + 1; pageChanged?(currentPage, pageCount)
    }
    var requests: [PDFRasterKey] = []
    let visiblePages = Array(first...last).sorted { abs($0 - centerPage) < abs($1 - centerPage) }
    for index in visiblePages {
      let poster = fullKey(page: index, width: 768)
      if pageViews[index]?.image == nil { requests.append(poster) }
      let detail = detailKey(page: index, visible: visible)
      if (pageViews[index]?.image?.width ?? 0) < detail.fullWidth { requests.append(detail) }
    }
    for index in nearby where index < first || index > last {
      requests.append(fullKey(page: index, width: 768))
    }
    wanted = requests
    installed = installed.filter { requests.contains($0.key) }
    updateHighlights()
    dismissHover()
    if let location = window?.mouseLocationOutsideOfEventStream { scheduleHover(at: textPoint(location: location)) }
    startRendering()
    let elapsed = started.duration(to: .now).components
    scrollUpdateMilliseconds = max(scrollUpdateMilliseconds,
      Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15)
  }

  private func startRendering() {
    guard renderTask == nil else { return }
    let rasterizer = source.rasterizer
    renderTask = Task { [weak self] in
      // Read the newest viewport after every render. Momentum does not repeatedly cancel an in-flight page.
      while !Task.isCancelled {
        guard let key = self?.wanted.first(where: { self?.installed[$0] == nil }) else { break }
        do {
          if self?.source.pages[key.page] == nil {
            let geometry = try await rasterizer.pageGeometry(key.page)
            try Task.checkCancellation()
            guard let self, !self.suspended else { break }
            let previousSize = self.source.layoutGeometry(key.page).size
            self.source.setGeometry(geometry, page: key.page)
            if previousSize != geometry.size { self.arrangePages() }
            continue
          }
          let image = try await rasterizer.render(key)
          try Task.checkCancellation()
          guard let self, !self.suspended else { break }
          if self.wanted.contains(key), let view = self.pageViews[key.page] {
            self.installed[key] = image
            CATransaction.begin(); CATransaction.setDisableActions(true)
            if key.x == 0, key.y == 0, key.width == key.fullWidth,
              key.height == self.fullKey(page: key.page, width: key.fullWidth).height {
              view.setImage(image)
              if key.page == 0 { self.source.rememberOpeningImage(image) }
            } else {
              view.setDetail(image, key: key, pageWidth: self.pageFrames[key.page].width)
            }
            CATransaction.commit()
            self.markReady(key.page)
            if !self.sharpReady, key.page == 0,
              key == self.detailKey(page: 0, visible: self.scroll.contentView.bounds) {
              self.sharpReady = true; self.firstSharpPageReady?()
            }
          }
        } catch is CancellationError { break }
        catch {
          self?.failed?(error.localizedDescription)
          self?.wanted.removeAll { $0 == key }
        }
      }
      guard let self, !Task.isCancelled else { return }
      let stats = await rasterizer.statistics()
      self.cacheBytes = stats.bytes; self.rasterCount = stats.renders; self.cacheHits = stats.hits
      self.parseMilliseconds = stats.parseMilliseconds
      self.maximumRasterMilliseconds = stats.maximumRasterMilliseconds
      self.renderTask = nil
      self.renderingFinished?()
      if self.wanted.contains(where: { self.installed[$0] == nil }) { self.startRendering() }
      else { self.loadAccessibleText() }
    }
  }

  private func markReady(_ index: Int) {
    guard index == 0 else { return }
    if !ready { ready = true; firstPageReady?(); pageChanged?(currentPage, pageCount) }
    if !sharpReady, (pageViews[0]?.image?.width ?? 0) >= detailKey(page: 0, visible: scroll.contentView.bounds).fullWidth {
      sharpReady = true; firstSharpPageReady?()
    }
  }

  private func move(to point: CGPoint) {
    let size = scroll.contentView.bounds.size
    scroll.contentView.scroll(to: CGPoint(
      x: max(0, min(point.x, canvas.bounds.width - size.width)),
      y: max(0, min(point.y, canvas.bounds.height - size.height))))
    scroll.reflectScrolledClipView(scroll.contentView)
  }
  func zoomIn() { setZoom(zoom * 1.25) }
  func zoomOut() { setZoom(zoom / 1.25) }
  func fit() { setZoom(1) }
  private func setZoom(_ value: CGFloat) {
    zoom = min(8, max(0.1, value)); arrangePages()
  }
  func magnify(_ event: NSEvent) { setZoom(zoom * (1 + event.magnification)) }

  override func keyDown(with event: NSEvent) {
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    if event.modifierFlags.contains(.command) {
      switch key {
      case "c": copy(nil)
      case "a": selectAll(nil)
      case "+", "=": zoomIn()
      case "-": zoomOut()
      case "0": fit()
      default: super.keyDown(with: event)
      }
      return
    }
    var point = scroll.contentView.bounds.origin
    switch event.keyCode {
    case 125: point.y += 48
    case 126: point.y -= 48
    case 123: point.x -= 48
    case 124: point.x += 48
    case 121: point.y += viewportSize.height * 0.85
    case 116: point.y -= viewportSize.height * 0.85
    case 115: point.y = 0
    case 119: point.y = canvas.bounds.height
    case 49: point.y += viewportSize.height * (event.modifierFlags.contains(.shift) ? -0.85 : 0.85)
    default: super.keyDown(with: event); return
    }
    move(to: point)
  }

  private func textPoint(_ event: NSEvent) -> PDFTextPoint? {
    textPoint(location: event.locationInWindow)
  }
  private func textPoint(location: CGPoint) -> PDFTextPoint? {
    let point = canvas.convert(location, from: nil)
    guard let index = page(at: point) else { return nil }
    guard let geometry = source.pages[index] else { return nil }
    let frame = pageFrames[index]
    let local = CGPoint(x: (point.x - frame.minX) / pageScale,
      y: geometry.size.height - (point.y - frame.minY) / pageScale)
    return PDFTextPoint(page: index, point: local.applying(geometry.transform.inverted()))
  }
  func beginSelection(_ event: NSEvent) {
    guard annotationTool == .select || (annotations.allowsEditing && !annotations.isBusy) else { return }
    guard annotationTool != .note || !hasPendingStroke else { return }
    notePopover?.close()
    dismissHover()
    window?.makeFirstResponder(self)
    selectionStart = textPoint(event); dragged = false
    selectionEnd = selectionStart
    annotationCursor.set()
    draftID = UUID()
    activeMarkup = nil
    selectionTask?.cancel(); selectionGeneration += 1
    selection = nil; updateHighlights()
    annotationStateChanged?()
  }
  func extendSelection(_ event: NSEvent) {
    dismissHover()
    guard let start = selectionStart, let end = textPoint(event) else { return }
    annotationCursor.set(); selectionEnd = end
    dragged = true
    select(from: start, to: end)
    canvas.autoscroll(with: event)
  }
  func endSelection(_ event: NSEvent) {
    guard let start = selectionStart else { return }
    if dragged, let end = textPoint(event) ?? selectionEnd {
      if let kind = annotationTool.markupKind {
        finishStroke(from: start, to: end, kind: kind)
      } else { select(from: start, to: end) }
    }
    else if let point = textPoint(event) {
      if let markup = markup(at: point) {
        activeMarkup = (point.page, markup.id)
        updateHighlights(); annotationStateChanged?()
        if presentation == .reader, !markup.note.isEmpty { editNote(nil) }
      } else if annotationTool == .select { followLink(point) }
    }
    selectionStart = nil; selectionEnd = nil; dragged = false
  }
  /// Final strokes outlive the next mouse-down; rapid consecutive drags cannot cancel a finished mark.
  private func finishStroke(from start: PDFTextPoint, to end: PDFTextPoint, kind: PDFMarkup.Kind) {
    selectionTask?.cancel(); selectionGeneration += 1
    let generation = selectionGeneration, id = UUID(), color = annotationColor, rasterizer = source.rasterizer
    strokeTasks[id] = Task { [weak self] in
      defer { self?.strokeTasks.removeValue(forKey: id); self?.annotationStateChanged?() }
      guard let result = try? await rasterizer.selection(from: start, to: end),
        !Task.isCancelled, let self else { return }
      guard !result.highlights.isEmpty else {
        if self.selectionGeneration == generation { self.selection = nil; self.updateHighlights() }
        return
      }
      if kind == .note {
        guard self.selectionGeneration == generation else { return }
        self.selection = result; self.updateHighlights(); self.editNote(nil)
      } else {
        self.addMarkup(kind: kind, using: result, color: color)
        if self.selectionGeneration == generation { self.selection = nil; self.updateHighlights() }
      }
    }
    annotationStateChanged?()
  }
  func finishPendingAnnotations() async {
    for task in Array(strokeTasks.values) { await task.value }
  }
  private func select(from start: PDFTextPoint?, to end: PDFTextPoint?) {
    selectionTask?.cancel(); selectionGeneration += 1
    let token = selectionGeneration, rasterizer = source.rasterizer
    selectionTask = Task { [weak self] in
      guard let result = try? await rasterizer.selection(from: start, to: end),
        !Task.isCancelled, let self, self.selectionGeneration == token else { return }
      self.selection = result; self.updateHighlights()
      self.annotationStateChanged?()
    }
  }
  override func selectAll(_ sender: Any?) {
    chooseTool(.select); select(from: nil, to: nil)
  }
  @objc func copy(_ sender: Any?) {
    guard let text = selection?.text, !text.isEmpty else { return }
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
  }
  private func updateHighlights() {
    let draft = annotationTool.markupKind.map { kind in
      makeMarkups(selection, kind: kind, color: annotationColor, note: "", draft: true)
    } ?? []
    for (index, view) in pageViews {
      guard let geometry = source.pages[index] else { continue }
      let rects: [CGRect] = selection?.highlights.filter { $0.page == index }.map { highlight in
        let rect = highlight.rect.applying(geometry.transform)
        return CGRect(x: rect.minX * pageScale, y: (geometry.size.height - rect.maxY) * pageScale,
          width: rect.width * pageScale, height: rect.height * pageScale)
      } ?? []
      view.setHighlights(annotationTool == .select ? rects : [])
      view.setMarkups(annotations.markups(on: index) + draft.filter { $0.page == index }, geometry: geometry, scale: pageScale,
        selected: activeMarkup?.id)
    }
  }
  private func loadAccessibleText() {
    textTask?.cancel()
    let pages = pageViews.filter { !$0.value.hasText }.map(\.key).sorted()
    let rasterizer = source.rasterizer
    textTask = Task { [weak self] in
      for index in pages {
        guard !Task.isCancelled else { return }
        guard let content = try? await rasterizer.pageContent(index) else { continue }
        guard !Task.isCancelled, let self else { return }
        self.pageViews[index]?.setContent(content)
        self.annotations.receive(content, page: index)
      }
    }
  }
  private func followLink(_ point: PDFTextPoint) {
    guard let link = pageViews[point.page]?.content?.links.first(where: { $0.rect.contains(point.point) }) else { return }
    switch link.target {
    case .url(let url):
      if ["https", "http", "mailto"].contains(url.scheme?.lowercased() ?? "") { NSWorkspace.shared.open(url) }
    case .page(let index):
      if pageFrames.indices.contains(index) { move(to: CGPoint(x: 0, y: pageFrames[index].minY)) }
    }
  }

  private func markup(at point: PDFTextPoint) -> PDFMarkup? {
    annotations.markups(on: point.page).reversed().first { markup in
      return markup.quads.contains { quad in
        let path = CGMutablePath()
        path.move(to: quad.topLeft); path.addLine(to: quad.topRight)
        path.addLine(to: quad.bottomRight); path.addLine(to: quad.bottomLeft); path.closeSubpath()
        if path.contains(point.point) { return true }
        let underline = CGMutablePath()
        underline.move(to: quad.bottomLeft); underline.addLine(to: quad.bottomRight)
        return underline.copy(strokingWithWidth: 8 / pageScale, lineCap: .round, lineJoin: .round, miterLimit: 1).contains(point.point)
      }
    }
  }

  @objc func highlight(_ sender: Any?) { chooseTool(.highlight) }
  @objc func underline(_ sender: Any?) { chooseTool(.underline) }
  var annotationColor = PDFMarkup.Color.yellow

  func chooseTool(_ tool: PDFAnnotationTool) {
    guard tool == .select || (presentation == .reader && annotations.allowsEditing && !annotations.isBusy) else { return }
    notePopover?.close(); dismissHover()
    selectionTask?.cancel(); selectionGeneration += 1
    selectionStart = nil; selectionEnd = nil; selection = nil; activeMarkup = nil
    annotationTool = tool
    updateAnnotationCursor(); updateHighlights(); annotationStateChanged?()
    window?.makeFirstResponder(self)
  }

  private func updateAnnotationCursor() {
    if annotationTool == .select { annotationCursor = .iBeam }
    else {
      let symbol = NSImage(systemSymbolName: annotationTool.symbol, accessibilityDescription: nil)?
        .withSymbolConfiguration(.init(paletteColors: [.black]))
      let color = annotationColor.native
      let image = NSImage(size: NSSize(width: 32, height: 32), flipped: true) { _ in
        NSColor.white.setFill()
        NSBezierPath(roundedRect: NSRect(x: 7, y: 1, width: 24, height: 24), xRadius: 4, yRadius: 4).fill()
        symbol?.draw(in: NSRect(x: 9, y: 3, width: 20, height: 20))
        NSColor.white.setStroke()
        let tip = NSBezierPath()
        tip.move(to: NSPoint(x: 0, y: 27)); tip.line(to: NSPoint(x: 8, y: 27))
        tip.move(to: NSPoint(x: 4, y: 23)); tip.line(to: NSPoint(x: 4, y: 31))
        tip.lineWidth = 3; tip.stroke()
        color.setStroke(); tip.lineWidth = 1.5; tip.stroke()
        return true
      }
      annotationCursor = NSCursor(image: image, hotSpot: NSPoint(x: 4, y: 27))
    }
    window?.invalidateCursorRects(for: canvas)
    if let location = window?.mouseLocationOutsideOfEventStream,
      canvas.visibleRect.contains(canvas.convert(location, from: nil)) { annotationCursor.set() }
  }

  private func makeMarkups(_ chosen: PDFTextSelection?, kind: PDFMarkup.Kind,
    color: PDFMarkup.Color, note: String, draft: Bool = false) -> [PDFMarkup] {
    guard let chosen else { return [] }
    let pages = Dictionary(grouping: chosen.highlights, by: \.page)
    return pages.keys.sorted().compactMap { page in
      let quads = (pages[page] ?? []).filter { !$0.rect.isEmpty && !$0.rect.isInfinite }.map { PDFMarkup.Quad(rect: $0.rect) }
      guard !quads.isEmpty else { return nil }
      return PDFMarkup(id: draft ? draftID : UUID(), page: page, kind: kind, quads: quads,
        color: color, note: note, editable: true)
    }
  }

  private func addMarkup(kind: PDFMarkup.Kind, note: String = "", using captured: PDFTextSelection?, color: PDFMarkup.Color? = nil) {
    guard annotations.allowsEditing, !annotations.isBusy else { return }
    let values = makeMarkups(captured, kind: kind, color: color ?? annotationColor, note: note)
    let edits = Dictionary(uniqueKeysWithValues: values.map { ($0.id, PDFMarkupChange.put($0)) })
    annotations.apply(edits, name: note.isEmpty ? (kind == .highlight ? "Highlight" : "Underline") : "Add Note")
    updateHighlights(); annotationStateChanged?()
  }

  @objc func editNote(_ sender: Any?) {
    guard presentation == .reader, annotations.allowsEditing, !annotations.isBusy else { return }
    let existing = selectedMarkup
    guard existing?.editable != false else { return }
    let captured = selection
    guard let page = existing?.page ?? captured?.highlights.first?.page,
      let rect = existing?.quads.first?.bounds ?? captured?.highlights.first?.rect,
      let surface = pageViews[page], let geometry = source.pages[page] else { return }
    notePopover?.close()
    dismissHover()
    let popover = NSPopover()
    popover.behavior = .transient
    popover.delegate = self
    let editor = PDFNoteEditor(note: existing?.note ?? "", isExisting: existing != nil)
    editor.commit = { [weak self, weak popover] text in
      guard let self else { return }
      if var value = existing {
        value.note = text
        self.annotations.apply([value.id: .put(value)], name: "Edit Note")
      } else if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        self.addMarkup(kind: .note, note: text, using: captured)
      }
      popover?.close(); self.window?.makeFirstResponder(self)
    }
    editor.cancel = { [weak self, weak popover] in popover?.close(); self?.window?.makeFirstResponder(self) }
    editor.remove = { [weak self, weak popover] in self?.deleteMarkup(nil); popover?.close() }
    popover.contentViewController = editor
    notePopover = popover
    let transformed = rect.applying(geometry.transform)
    let anchor = CGRect(x: transformed.minX * pageScale,
      y: (geometry.size.height - transformed.maxY) * pageScale,
      width: transformed.width * pageScale, height: transformed.height * pageScale)
    popover.show(relativeTo: anchor, of: surface, preferredEdge: .maxY)
    editor.focus()
  }

  @objc func deleteMarkup(_ sender: Any?) {
    guard let value = selectedMarkup, value.editable else { return }
    annotations.apply([value.id: .remove(page: value.page)], name: "Delete Annotation")
    activeMarkup = nil; updateHighlights(); annotationStateChanged?()
  }

  func chooseColor(_ color: PDFMarkup.Color) {
    annotationColor = color
    if annotationTool == .select, var value = selectedMarkup, value.editable {
      value.color = color
      annotations.apply([value.id: .put(value)], name: "Change Annotation Color")
    }
    updateAnnotationCursor(); updateHighlights()
    annotationStateChanged?()
  }

  func popoverDidClose(_ notification: Notification) {
    selection = nil; updateHighlights(); annotationStateChanged?()
  }

  func dismissNote() -> Bool {
    guard notePopover?.isShown == true else { return false }
    notePopover?.close(); window?.makeFirstResponder(self); return true
  }

  private func dismissHover() {
    hoverTask?.cancel(); hoverTask = nil; hoverMarkup = nil
    hoverPopover?.close(); hoverPopover = nil
  }

  private func scheduleHover(at point: PDFTextPoint?) {
    guard !suspended, NSEvent.pressedMouseButtons == 0, notePopover?.isShown != true,
      let point, let value = markup(at: point), !value.note.isEmpty else {
      dismissHover(); return
    }
    guard value != hoverMarkup else { return }
    dismissHover(); hoverMarkup = value
    hoverTask = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
      guard let self, !self.suspended, self.hoverMarkup == value,
        let surface = self.pageViews[value.page], let geometry = self.source.pages[value.page] else { return }
      let rect = value.quads.first(where: { $0.bounds.contains(point.point) })?.bounds ?? value.bounds
      let box = rect.applying(geometry.transform)
      let anchor = CGRect(x: box.minX * self.pageScale, y: (geometry.size.height - box.maxY) * self.pageScale,
        width: box.width * self.pageScale, height: box.height * self.pageScale)
      let popover = NSPopover()
      popover.animates = false; popover.behavior = .semitransient
      popover.contentViewController = PDFNoteReader(note: value.note)
      self.hoverPopover = popover
      popover.show(relativeTo: anchor, of: surface, preferredEdge: .maxY)
      self.hoverTask = nil
    }
  }

  func annotationMenu(for event: NSEvent) -> NSMenu {
    if let point = textPoint(event), let value = markup(at: point) {
      activeMarkup = (point.page, value.id); updateHighlights(); annotationStateChanged?()
    }
    let menu = NSMenu()
    menu.autoenablesItems = false
    func item(_ title: String, _ action: Selector, enabled: Bool = true) {
      let entry = NSMenuItem(title: title, action: action, keyEquivalent: "")
      entry.target = self; entry.isEnabled = enabled; menu.addItem(entry)
    }
    item("Copy", #selector(copy(_:)), enabled: !(selection?.text.isEmpty ?? true))
    item("Select All", #selector(selectAll(_:)))
    if presentation == .reader {
      menu.addItem(.separator())
      let writable = annotations.allowsEditing && !annotations.isBusy
      item("Highlight tool", #selector(highlight(_:)), enabled: writable)
      item("Underline tool", #selector(underline(_:)), enabled: writable)
      let editable = selectedMarkup?.editable == true && annotations.allowsEditing && !annotations.isBusy
      item(selectedMarkup == nil ? "Add Note…" : "Edit Note…", #selector(editNote(_:)), enabled: editable || canAnnotateSelection)
      item("Delete Annotation", #selector(deleteMarkup(_:)), enabled: editable)
    }
    return menu
  }
}

@MainActor
private final class PDFCanvas: NSView {
  weak var owner: NativePDFView?
  override var isFlipped: Bool { true }
  override var wantsUpdateLayer: Bool { true }
  override init(frame: NSRect) {
    super.init(frame: frame); wantsLayer = true
    setAccessibilityRole(.group); setAccessibilityLabel("PDF pages")
  }
  required init?(coder: NSCoder) { nil }
  override func updateLayer() { layer?.backgroundColor = Design.raised.cgColor }
  override func resetCursorRects() {
    addCursorRect(visibleRect, cursor: owner?.annotationCursor ?? .iBeam)
  }
  override func mouseDown(with event: NSEvent) { owner?.beginSelection(event) }
  override func mouseDragged(with event: NSEvent) { owner?.extendSelection(event) }
  override func mouseUp(with event: NSEvent) { owner?.endSelection(event) }
  override func magnify(with event: NSEvent) { owner?.magnify(event) }
  override func menu(for event: NSEvent) -> NSMenu? {
    owner?.annotationMenu(for: event)
  }
}

@MainActor
private final class PDFPageSurface: NSView {
  private let bitmap = CALayer()
  private let detail = CALayer()
  private let highlights = CAShapeLayer()
  private var markupLayers: [CALayer] = []
  private(set) var displayedMarkups: [PDFMarkup] = []
  private var markupScale: CGFloat = 0
  private var markupTransform = CGAffineTransform.identity
  private var selectedMarkup: UUID?
  private(set) var image: CGImage?
  private(set) var content: PDFPageContent?
  var hasText: Bool { content != nil }
  override var isFlipped: Bool { true }
  override var wantsUpdateLayer: Bool { true }
  init(index: Int) {
    super.init(frame: .zero)
    wantsLayer = true
    layer?.backgroundColor = NSColor.white.cgColor
    bitmap.contentsGravity = .resize
    layer?.masksToBounds = true
    layer?.addSublayer(bitmap); layer?.addSublayer(detail); layer?.addSublayer(highlights)
    highlights.fillColor = Design.accent.withAlphaComponent(0.25).cgColor
    setAccessibilityRole(.group); setAccessibilityLabel("Page \(index + 1)")
  }
  required init?(coder: NSCoder) { nil }
  override func updateLayer() {}
  override func layout() {
    super.layout()
    CATransaction.begin(); CATransaction.setDisableActions(true)
    bitmap.frame = bounds
    CATransaction.commit()
  }
  func setImage(_ image: CGImage) {
    self.image = image; bitmap.frame = bounds; bitmap.contents = image
    clearDetail()
  }
  func clearDetail() { detail.contents = nil }
  func setDetail(_ image: CGImage, key: PDFRasterKey, pageWidth: CGFloat) {
    let scale = pageWidth / CGFloat(key.fullWidth)
    detail.frame = CGRect(x: CGFloat(key.x) * scale, y: CGFloat(key.y) * scale,
      width: CGFloat(key.width) * scale, height: CGFloat(key.height) * scale)
    detail.contents = image; detail.contentsGravity = .resize
  }
  func setHighlights(_ rects: [CGRect]) {
    let path = CGMutablePath()
    for rect in rects { path.addRect(rect) }
    CATransaction.begin(); CATransaction.setDisableActions(true)
    highlights.frame = bounds; highlights.path = path
    CATransaction.commit()
  }
  func setContent(_ content: PDFPageContent) {
    self.content = content
    let text = NSAccessibilityElement()
    text.setAccessibilityParent(self)
    text.setAccessibilityRole(.staticText); text.setAccessibilityValue(content.text)
    setAccessibilityChildren([text])
  }

  func setMarkups(_ values: [PDFMarkup], geometry: PDFPageGeometry, scale: CGFloat, selected: UUID?) {
    guard displayedMarkups != values || markupScale != scale || markupTransform != geometry.transform || selectedMarkup != selected else { return }
    displayedMarkups = values; markupScale = scale; markupTransform = geometry.transform; selectedMarkup = selected
    markupLayers.forEach { $0.removeFromSuperlayer() }; markupLayers = []
    func convert(_ point: CGPoint) -> CGPoint {
      let p = point.applying(geometry.transform)
      return CGPoint(x: p.x * scale, y: (geometry.size.height - p.y) * scale)
    }
    CATransaction.begin(); CATransaction.setDisableActions(true)
    for value in values {
      let path = CGMutablePath()
      let underline = value.kind == .underline || value.kind == .note
      for quad in value.quads {
        if underline {
          path.move(to: convert(quad.bottomLeft)); path.addLine(to: convert(quad.bottomRight))
        } else {
          path.move(to: convert(quad.topLeft)); path.addLine(to: convert(quad.topRight))
          path.addLine(to: convert(quad.bottomRight)); path.addLine(to: convert(quad.bottomLeft)); path.closeSubpath()
        }
      }
      let mark = CAShapeLayer()
      mark.frame = bounds; mark.path = path
      mark.fillColor = underline ? nil : value.color.native.withAlphaComponent(0.3).cgColor
      mark.strokeColor = underline || value.id == selected ? value.color.native.cgColor : nil
      mark.lineWidth = underline ? max(1.5, scale) : 1
      if value.kind == .note {
        mark.lineCap = .round
        mark.lineDashPattern = [0, NSNumber(value: Double(max(4, scale * 3)))]
      }
      layer?.insertSublayer(mark, below: highlights); markupLayers.append(mark)
    }
    CATransaction.commit()
  }

}
