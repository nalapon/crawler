import AppKit

/// Page-space values shared by the editing session and the background PDF worker.
struct PDFMarkup: Sendable, Equatable, Identifiable {
  enum Kind: String, Sendable { case highlight, underline, note, stickyNote }
  struct Color: Sendable, Equatable {
    let red: CGFloat
    let green: CGFloat
    let blue: CGFloat
    var native: NSColor { NSColor(srgbRed: red, green: green, blue: blue, alpha: 1) }
    static let yellow = Color(red: 1, green: 0.82, blue: 0.12)
    static let green = Color(red: 0.25, green: 0.78, blue: 0.45)
    static let blue = Color(red: 0.25, green: 0.6, blue: 1)
    static let pink = Color(red: 1, green: 0.4, blue: 0.62)
  }
  struct Quad: Sendable, Equatable {
    let topLeft: CGPoint
    let topRight: CGPoint
    let bottomLeft: CGPoint
    let bottomRight: CGPoint
    init(rect: CGRect) {
      topLeft = CGPoint(x: rect.minX, y: rect.maxY)
      topRight = CGPoint(x: rect.maxX, y: rect.maxY)
      bottomLeft = CGPoint(x: rect.minX, y: rect.minY)
      bottomRight = CGPoint(x: rect.maxX, y: rect.minY)
    }
    init(topLeft: CGPoint, topRight: CGPoint, bottomLeft: CGPoint, bottomRight: CGPoint) {
      self.topLeft = topLeft; self.topRight = topRight
      self.bottomLeft = bottomLeft; self.bottomRight = bottomRight
    }
    var points: [CGPoint] { [topLeft, topRight, bottomLeft, bottomRight] }
    var bounds: CGRect {
      let xs = points.map(\.x), ys = points.map(\.y)
      return CGRect(x: xs.min() ?? 0, y: ys.min() ?? 0,
        width: (xs.max() ?? 0) - (xs.min() ?? 0), height: (ys.max() ?? 0) - (ys.min() ?? 0))
    }
  }
  let id: UUID
  let page: Int
  let kind: Kind
  let quads: [Quad]
  var color: Color
  var note: String
  let editable: Bool
  var bounds: CGRect { quads.reduce(CGRect.null) { $0.union($1.bounds) } }
}

enum PDFAnnotationTool: Equatable {
  case select, highlight, underline, note
  var markupKind: PDFMarkup.Kind? {
    switch self {
    case .select: nil
    case .highlight: .highlight
    case .underline: .underline
    case .note: .note
    }
  }
  var symbol: String {
    switch self {
    case .select: "cursorarrow"
    case .highlight: "highlighter"
    case .underline: "underline"
    case .note: "text.bubble"
    }
  }
  var instruction: String {
    switch self {
    case .select: "Choose a tool, then drag over text"
    case .highlight: "Drag to highlight · Esc to select"
    case .underline: "Drag to underline · Esc to select"
    case .note: "Drag over text to add a note"
    }
  }
}

enum PDFMarkupChange: Sendable, Equatable {
  case put(PDFMarkup)
  case remove(page: Int)
  var page: Int {
    switch self { case .put(let markup): markup.page; case .remove(let page): page }
  }
}

enum PDFAnnotationError: LocalizedError {
  case restricted, changedOnDisk, writeFailed
  var errorDescription: String? {
    switch self {
    case .restricted: "This PDF does not allow annotations. Encrypted PDFs cannot be edited here."
    case .changedOnDisk: "The PDF changed on disk. Save a copy to keep your annotations without replacing those changes."
    case .writeFailed: "The PDF could not be saved. Your annotations are still available here."
    }
  }
}

/// One session per shared PDFSource, so preview, reader and Space see the same marks.
@MainActor
final class PDFAnnotationSession {
  static let changed = Notification.Name("CrawlerPDFAnnotationsChanged")
  private let rasterizer: PDFRasterizer
  private var originals: [Int: [PDFMarkup]] = [:]
  private var changes: [UUID: PDFMarkupChange] = [:]
  private var savedChanges: [UUID: PDFMarkupChange] = [:]
  private(set) var url: URL
  private(set) var allowsEditing = false
  private(set) var saving = false
  private(set) var removing = false
  var isBusy: Bool { saving || removing }
  let undo = UndoManager()
  var isDirty: Bool { changes != savedChanges }

  init(url: URL, rasterizer: PDFRasterizer) {
    self.url = url; self.rasterizer = rasterizer
    undo.levelsOfUndo = 100
    undo.groupsByEvent = false
  }

  func receive(_ content: PDFPageContent, page: Int) {
    guard originals[page] == nil else { return }
    originals[page] = content.markups
    allowsEditing = content.allowsAnnotations
    publish()
  }

  func markups(on page: Int) -> [PDFMarkup] {
    var result = (originals[page] ?? []).filter { changes[$0.id] == nil }
    result += changes.values.compactMap {
      if case .put(let markup) = $0, markup.page == page { return markup }
      return nil
    }
    return result.sorted { $0.id.uuidString < $1.id.uuidString }
  }

  func apply(_ edits: [UUID: PDFMarkupChange], name: String) {
    guard allowsEditing, !isBusy, !edits.isEmpty else { return }
    var next = changes
    for (id, change) in edits { next[id] = change }
    replace(next, name: name)
  }

  private func replace(_ next: [UUID: PDFMarkupChange], name: String) {
    let previous = changes
    guard next != previous else { return }
    let group = !undo.isUndoing && !undo.isRedoing
    if group { undo.beginUndoGrouping() }
    defer { if group { undo.endUndoGrouping() } }
    undo.registerUndo(withTarget: self) { target in target.replace(previous, name: name) }
    undo.setActionName(name)
    changes = next
    publish()
  }

  func discardUnsavedChanges() {
    changes = savedChanges
    undo.removeAllActions()
    publish()
  }

  func save(to destination: URL) async throws {
    guard !isBusy else { return }
    saving = true; publish()
    defer { saving = false; publish() }
    let snapshot = changes
    try await rasterizer.saveAnnotations(snapshot, to: destination)
    savedChanges = snapshot
    url = destination
  }

  /// The explicit action scans annotation metadata on every page, including pages never displayed.
  func removeAll() async throws {
    guard allowsEditing, !isBusy else { return }
    removing = true; publish()
    defer { removing = false; publish() }
    let stored = try await rasterizer.markupIDs()
    try Task.checkCancellation()
    var next = changes
    for (id, page) in stored { next[id] = .remove(page: page) }
    for (id, change) in changes {
      if case .put(let markup) = change { next[id] = .remove(page: markup.page) }
    }
    replace(next, name: "Remove All Annotations")
  }

  private func publish() { NotificationCenter.default.post(name: Self.changed, object: self) }
}
