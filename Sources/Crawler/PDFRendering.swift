import AppKit
import CoreGraphics
import PDFKit
import OSLog

enum PDFPerformance {
  static let log = Logger(subsystem: "studio.crawler.native-prototype", category: "PDF Performance")
}

enum PDFPresentation: String { case pane, reader, space }

struct PDFPageGeometry: Sendable {
  let size: CGSize
  let transform: CGAffineTransform
}

struct PDFDocumentInfo: Sendable {
  let pageCount: Int
  let firstPage: PDFPageGeometry
}

struct PDFRasterKey: Hashable, Sendable {
  let page: Int
  let fullWidth: Int
  let x: Int
  let y: Int
  let width: Int
  let height: Int
}

struct PDFTextPoint: Sendable {
  let page: Int
  let point: CGPoint
}

struct PDFTextSelection: Sendable {
  struct Highlight: Sendable {
    let page: Int
    let rect: CGRect
  }
  let text: String
  let highlights: [Highlight]
}

struct PDFPageContent: Sendable {
  struct Link: Sendable {
    enum Target: Sendable { case url(URL), page(Int) }
    let rect: CGRect
    let target: Target
  }
  let text: String
  let links: [Link]
  let markups: [PDFMarkup]
  let allowsAnnotations: Bool
}

enum PDFReadError: LocalizedError {
  case unreadable, locked, invalidPage, allocation
  var errorDescription: String? {
    switch self {
    case .unreadable: "Unable to read this PDF. It may be damaged or inaccessible."
    case .locked: "This PDF is password protected. Open it in your PDF application to unlock it."
    case .invalidPage: "This PDF contains an invalid page."
    case .allocation: "Unable to prepare this PDF page within the memory limit."
    }
  }
}

/// Owns all PDF parsing and drawing on its actor. Only immutable bitmaps and geometry cross to AppKit.
actor PDFRasterizer {
  static let cacheLimit = 128 * 1024 * 1024
  static let maximumRasterPixels = 16 * 1024 * 1024
  private let url: URL
  private let colorSpace: CGColorSpace?
  private var document: CGPDFDocument?
  private var textDocument: PDFDocument?
  private var annotationIDs: [Int: [Int: UUID]] = [:]
  private var savedFileVersion: PDFFileVersion?
  private var savedURL: URL?
  private var geometry: [Int: PDFPageGeometry] = [:]
  private var cache: [PDFRasterKey: CGImage] = [:]
  private var order: [PDFRasterKey] = []
  private var cacheBytes = 0
  private var renderCount = 0
  private var hitCount = 0
  private var parseMilliseconds: Double = 0
  private var maximumRasterMilliseconds: Double = 0

  init(url: URL, colorSpace: CGColorSpace? = nil) {
    self.url = url
    self.colorSpace = colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB)
  }

  func load() throws -> PDFDocumentInfo {
    if let document, let first = geometry[0] {
      return PDFDocumentInfo(pageCount: document.numberOfPages, firstPage: first)
    }
    try Task.checkCancellation()
    let started = ContinuousClock.now
    guard let pdf = CGPDFDocument(url as CFURL) else { throw PDFReadError.unreadable }
    guard pdf.isUnlocked else { throw PDFReadError.locked }
    guard pdf.numberOfPages > 0 else { throw PDFReadError.invalidPage }
    document = pdf
    savedFileVersion = try? PDFFileVersion(url)
    savedURL = url
    let info = PDFDocumentInfo(pageCount: pdf.numberOfPages, firstPage: try pageGeometry(0))
    let elapsed = started.duration(to: .now).components
    parseMilliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
    return info
  }

  func pageGeometry(_ index: Int) throws -> PDFPageGeometry {
    try Task.checkCancellation()
    if let known = geometry[index] { return known }
    guard index >= 0, let page = document?.page(at: index + 1) else { throw PDFReadError.invalidPage }
    let box = page.getBoxRect(.cropBox).intersection(page.getBoxRect(.mediaBox))
    guard box.width.isFinite, box.height.isFinite, box.width > 0, box.height > 0 else {
      throw PDFReadError.invalidPage
    }
    let rotation = ((page.rotationAngle % 360) + 360) % 360
    let size = rotation == 90 || rotation == 270
      ? CGSize(width: box.height, height: box.width) : box.size
    let result = PDFPageGeometry(size: size, transform: page.getDrawingTransform(
      .cropBox, rect: CGRect(origin: .zero, size: size), rotate: 0, preserveAspectRatio: true))
    geometry[index] = result
    return result
  }

  func render(_ key: PDFRasterKey) throws -> CGImage {
    try Task.checkCancellation()
    if let image = cache[key] {
      hitCount += 1
      order.removeAll { $0 == key }; order.append(key)
      return image
    }
    let started = ContinuousClock.now
    guard key.width > 0, key.height > 0,
      key.width <= Self.maximumRasterPixels / key.height,
      let page = document?.page(at: key.page + 1)
    else { throw PDFReadError.invalidPage }
    guard let colorSpace, let context = CGContext(
      data: nil, width: key.width, height: key.height, bitsPerComponent: 8, bytesPerRow: 0,
      space: colorSpace,
      bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue)
    else { throw PDFReadError.allocation }
    context.setFillColor(CGColor(gray: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: key.width, height: key.height))
    let pageGeometry = try pageGeometry(key.page)
    let size = pageGeometry.size
    let fullHeight = CGFloat(key.fullWidth) * size.height / size.width
    context.translateBy(x: -CGFloat(key.x), y: -(fullHeight - CGFloat(key.y + key.height)))
    // The drawing transform normalizes crop/rotation; apply raster scale explicitly (it does not upscale).
    let scale = CGFloat(key.fullWidth) / size.width
    context.scaleBy(x: scale, y: scale)
    context.concatenate(pageGeometry.transform)
    context.interpolationQuality = .high
    context.drawPDFPage(page)
    try Task.checkCancellation()
    guard let image = context.makeImage() else { throw PDFReadError.allocation }
    renderCount += 1
    let bytes = image.bytesPerRow * image.height
    while cacheBytes + bytes > Self.cacheLimit, let oldest = order.first {
      order.removeFirst()
      if let previous = cache.removeValue(forKey: oldest) {
        cacheBytes -= previous.bytesPerRow * previous.height
      }
    }
    cache[key] = image; order.append(key); cacheBytes += bytes
    let elapsed = started.duration(to: .now).components
    maximumRasterMilliseconds = max(maximumRasterMilliseconds,
      Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15)
    return image
  }

  /// PDFKit supplies text geometry only. It never draws a page or creates a PDFView.
  func selection(from start: PDFTextPoint?, to end: PDFTextPoint?) throws -> PDFTextSelection {
    try Task.checkCancellation()
    if textDocument == nil { textDocument = PDFDocument(url: url) }
    guard let pdf = textDocument, pdf.allowsCopying else {
      return PDFTextSelection(text: "", highlights: [])
    }
    let selection: PDFSelection?
    if let start, let end, let first = pdf.page(at: start.page), let last = pdf.page(at: end.page) {
      selection = pdf.selection(from: first, at: start.point, to: last, at: end.point)
    } else { selection = pdf.selectionForEntireDocument }
    var highlights: [PDFTextSelection.Highlight] = []
    for line in selection?.selectionsByLine() ?? [] {
      try Task.checkCancellation()
      for page in line.pages {
        highlights.append(.init(page: pdf.index(for: page), rect: line.bounds(for: page)))
      }
    }
    return PDFTextSelection(text: selection?.string ?? "", highlights: highlights)
  }

  func statistics() -> (bytes: Int, renders: Int, hits: Int, parseMilliseconds: Double, maximumRasterMilliseconds: Double) {
    (cacheBytes, renderCount, hitCount, parseMilliseconds, maximumRasterMilliseconds)
  }

  func pageContent(_ index: Int) throws -> PDFPageContent {
    try Task.checkCancellation()
    if textDocument == nil { textDocument = PDFDocument(url: url) }
    guard let pdf = textDocument, let page = pdf.page(at: index) else {
      throw PDFReadError.invalidPage
    }
    let links = page.annotations.compactMap { annotation -> PDFPageContent.Link? in
      if let action = annotation.action as? PDFActionURL, let url = action.url {
        return .init(rect: annotation.bounds, target: .url(url))
      }
      if let action = annotation.action as? PDFActionGoTo, let target = action.destination.page {
        return .init(rect: annotation.bounds, target: .page(pdf.index(for: target)))
      }
      return nil
    }
    return PDFPageContent(text: page.string ?? "", links: links,
      markups: annotationValues(page, index: index),
      allowsAnnotations: pdf.allowsCommenting && !pdf.isEncrypted)
  }

  func close() {
    cache.removeAll(); order.removeAll(); cacheBytes = 0
    textDocument = nil; document = nil; geometry = [:]
  }
}

private struct PDFFileVersion: Equatable {
  let modified: Date
  let size: UInt64
  let inode: UInt64
  init(_ url: URL) throws {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    guard let modified = attributes[.modificationDate] as? Date,
      let size = attributes[.size] as? NSNumber,
      let inode = attributes[.systemFileNumber] as? NSNumber else { throw PDFAnnotationError.writeFailed }
    self.modified = modified; self.size = size.uint64Value; self.inode = inode.uint64Value
  }
}

extension PDFRasterizer {
  /// Collect only annotation identities, without text extraction or page rasterization.
  func markupIDs() throws -> [UUID: Int] {
    if textDocument == nil { textDocument = PDFDocument(url: url) }
    guard let pdf = textDocument, pdf.allowsCommenting, !pdf.isEncrypted else {
      throw PDFAnnotationError.restricted
    }
    var result: [UUID: Int] = [:]
    for index in 0..<pdf.pageCount {
      try Task.checkCancellation()
      guard let page = pdf.page(at: index) else { throw PDFReadError.invalidPage }
      for (position, annotation) in page.annotations.enumerated() {
        switch annotation.type {
        case "Highlight", "Underline", "Text": break
        default: continue
        }
        let id = annotationIDs[index]?[position] ?? UUID()
        annotationIDs[index, default: [:]][position] = id
        result[id] = index
      }
    }
    return result
  }

  private func annotationValues(_ page: PDFPage, index: Int) -> [PDFMarkup] {
    page.annotations.enumerated().compactMap { position, annotation in
      let kind: PDFMarkup.Kind
      switch annotation.type {
      case "Highlight": kind = (annotation.contents ?? "").isEmpty ? .highlight : .note
      case "Underline":
        let textNote = (annotation.value(forAnnotationKey: PDFAnnotationKey(rawValue: "/CrawlerTextNote")) as? NSNumber)?.boolValue == true
        kind = textNote || !(annotation.contents ?? "").isEmpty ? .note : .underline
      case "Text": kind = .stickyNote
      default: return nil
      }
      guard annotation.shouldDisplay else { return nil }
      let id = annotationIDs[index]?[position] ?? UUID()
      annotationIDs[index, default: [:]][position] = id
      let origin = annotation.bounds.origin
      let points = annotation.quadrilateralPoints?.map {
        CGPoint(x: $0.pointValue.x + origin.x, y: $0.pointValue.y + origin.y)
      } ?? []
      var quads: [PDFMarkup.Quad] = []
      if points.count >= 4, points.count.isMultiple(of: 4) {
        for start in stride(from: 0, to: points.count, by: 4) {
          quads.append(.init(topLeft: points[start], topRight: points[start + 1],
            bottomLeft: points[start + 2], bottomRight: points[start + 3]))
        }
      } else { quads = [.init(rect: annotation.bounds)] }
      guard quads.allSatisfy({ !$0.bounds.isNull && !$0.bounds.isInfinite && !$0.bounds.isEmpty }) else { return nil }
      let color = annotation.color.usingColorSpace(.sRGB) ?? .systemYellow
      let flags = (annotation.value(forAnnotationKey: PDFAnnotationKey(rawValue: "/F")) as? NSNumber)?.intValue ?? 0
      return PDFMarkup(id: id, page: index, kind: kind, quads: quads,
        color: .init(red: color.redComponent, green: color.greenComponent, blue: color.blueComponent),
        note: annotation.contents ?? "", editable: flags & 64 == 0)
    }
  }

  /// Clone only on an explicit save. The baseline stays immutable for undo and failed saves.
  func saveAnnotations(_ changes: [UUID: PDFMarkupChange], to destination: URL) throws {
    if textDocument == nil { textDocument = PDFDocument(url: url) }
    guard let baseline = textDocument, baseline.allowsCommenting, !baseline.isEncrypted else {
      throw PDFAnnotationError.restricted
    }
    guard let data = baseline.dataRepresentation(), let output = PDFDocument(data: data) else {
      throw PDFAnnotationError.writeFailed
    }
    var existing: [UUID: PDFAnnotation] = [:]
    for (pageIndex, ids) in annotationIDs {
      guard let page = output.page(at: pageIndex) else { continue }
      let annotations = page.annotations
      for (position, id) in ids where annotations.indices.contains(position) { existing[id] = annotations[position] }
    }
    for (id, change) in changes {
      guard let page = output.page(at: change.page) else { throw PDFReadError.invalidPage }
      switch change {
      case .remove:
        if let previous = existing[id] { page.removeAnnotation(previous) }
      case .put(let value):
        if let previous = existing[id] {
          previous.contents = value.note
          previous.color = value.color.native
          previous.modificationDate = Date()
        } else {
          let subtype: PDFAnnotationSubtype
          switch value.kind {
          case .highlight: subtype = .highlight
          case .underline, .note: subtype = .underline
          case .stickyNote: subtype = .text
          }
          let annotation = PDFAnnotation(bounds: value.bounds, forType: subtype, withProperties: nil)
          annotation.color = value.color.native
          annotation.contents = value.note
          annotation.modificationDate = Date()
          annotation.setValue(id.uuidString, forAnnotationKey: PDFAnnotationKey(rawValue: "/NM"))
          if value.kind == .note {
            annotation.setValue(true, forAnnotationKey: PDFAnnotationKey(rawValue: "/CrawlerTextNote"))
            let border = PDFBorder()
            border.style = .dashed; border.lineWidth = 1; border.dashPattern = [1, 2]
            annotation.border = border
          }
          if value.kind != .stickyNote {
            annotation.quadrilateralPoints = value.quads.flatMap(\.points).map {
              NSValue(point: CGPoint(x: $0.x - value.bounds.minX, y: $0.y - value.bounds.minY))
            }
          }
          page.addAnnotation(annotation)
        }
      }
    }
    let target = destination.standardizedFileURL.resolvingSymlinksInPath()
    let replacingCurrent = target == savedURL?.standardizedFileURL.resolvingSymlinksInPath()
    let temporary = target.deletingLastPathComponent().appendingPathComponent(".crawler-\(UUID().uuidString).pdf")
    defer { try? FileManager.default.removeItem(at: temporary) }
    guard output.write(to: temporary) else { throw PDFAnnotationError.writeFailed }
    var coordinationError: NSError?
    var writeError: Error?
    NSFileCoordinator().coordinate(writingItemAt: target, options: .forReplacing, error: &coordinationError) { coordinated in
      do {
        if replacingCurrent {
          guard let expected = savedFileVersion, try PDFFileVersion(coordinated) == expected else {
            throw PDFAnnotationError.changedOnDisk
          }
        }
        if FileManager.default.fileExists(atPath: coordinated.path) {
          _ = try FileManager.default.replaceItemAt(coordinated, withItemAt: temporary)
        } else { try FileManager.default.moveItem(at: temporary, to: coordinated) }
      } catch { writeError = error }
    }
    if let error = coordinationError ?? writeError { throw error }
    savedFileVersion = try PDFFileVersion(target)
    savedURL = target
  }
}

/// Starts once, on demand, and shares preparation even if Enter arrives before the preview is ready.
@MainActor
final class PDFPreparation {
  private let url: URL
  private let rasterizer: PDFRasterizer
  private var task: Task<PDFSource, Error>?
  init(url: URL, colorSpace: CGColorSpace?) {
    self.url = url
    rasterizer = PDFRasterizer(url: url, colorSpace: colorSpace)
  }
  deinit { task?.cancel() }
  func load() async throws -> PDFSource {
    if let task { return try await task.value }
    let rasterizer = rasterizer, url = url
    let task = Task {
      let info = try await rasterizer.load()
      try Task.checkCancellation()
      return PDFSource(url: url, rasterizer: rasterizer, info: info)
    }
    self.task = task
    return try await task.value
  }
}

/// Shares a selected document between its preview and reader, and releases PDF objects off the main actor.
@MainActor
final class PDFSource {
  let rasterizer: PDFRasterizer
  let annotations: PDFAnnotationSession
  let firstPage: PDFPageGeometry
  private(set) var pages: [PDFPageGeometry?]
  private(set) var openingImage: CGImage?
  init(url: URL, rasterizer: PDFRasterizer, info: PDFDocumentInfo) {
    self.rasterizer = rasterizer; firstPage = info.firstPage
    annotations = PDFAnnotationSession(url: url, rasterizer: rasterizer)
    pages = Array(repeating: nil, count: info.pageCount)
    pages[0] = info.firstPage
  }
  /// Unread pages use an estimated layout size until they approach the viewport; their drawing always uses real geometry.
  func layoutGeometry(_ index: Int) -> PDFPageGeometry { pages[index] ?? firstPage }
  func setGeometry(_ geometry: PDFPageGeometry, page: Int) { pages[page] = geometry }
  /// Keep one opening bitmap while the document is selected, so re-entering never waits behind a preview draw.
  func rememberOpeningImage(_ image: CGImage) {
    if image.width > (openingImage?.width ?? 0) { openingImage = image }
  }
  deinit {
    let rasterizer = rasterizer
    Task { await rasterizer.close() }
  }
}
