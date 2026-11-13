import AppKit
import QuickLookThumbnailing

@MainActor
final class DocumentArtwork: Surface {
  private let fileIcon = FileIconView()
  override init(frame: NSRect) {
    super.init(frame: frame)
    addSubview(fileIcon)
  }
  required init?(coder: NSCoder) { nil }
  override func layout() {
    super.layout()
    fileIcon.frame = NSRect(x: bounds.midX - 36, y: 65, width: 72, height: 68)
  }
  private var thumbnail: NSImage?
  private var request: QLThumbnailGenerator.Request?
  var item: FileItem? {
    didSet {
      guard item != oldValue else { return }
      if let request { QLThumbnailGenerator.shared.cancel(request) }
      thumbnail = nil
      fileIcon.isHidden = item == nil
      if let item { fileIcon.configure(path: item.url.path, isFolder: item.kind.folder != nil) }
      needsDisplay = true
      guard let item, item.kind.folder == nil else { return }
      let request = QLThumbnailGenerator.Request(fileAt: item.url, size: NSSize(width: 700, height: 900), scale: 1, representationTypes: .thumbnail)
      self.request = request
      QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { [weak self] representation, _ in
        let image = representation?.cgImage
        Task { @MainActor [weak self] in
          guard let self, self.item == item else { return }
          self.thumbnail = image.map { NSImage(cgImage: $0, size: .zero) }
          self.fileIcon.isHidden = self.thumbnail != nil
          self.needsDisplay = true
        }
      }
    }
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    guard let item else { return }
    if let thumbnail {
      let available = bounds.insetBy(dx: 24, dy: 24)
      let scale = min(available.width / thumbnail.size.width, available.height / thumbnail.size.height, 1)
      let size = NSSize(width: thumbnail.size.width * scale, height: thumbnail.size.height * scale)
      thumbnail.draw(in: NSRect(x: (bounds.width - size.width) / 2, y: 24, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    } else {
      drawText(item.name, in: NSRect(x: 20, y: 155, width: bounds.width - 40, height: 24), size: 12, color: Design.text, align: .center)
      drawText(item.kind.label + " · " + item.sizeLabel, in: NSRect(x: 20, y: 190, width: bounds.width - 40, height: 22), size: 11, color: Design.muted, align: .center)
    }
  }
}
@MainActor
final class PreviewPane: Surface {
  let header = PaneTitle()
  let document = DocumentPreview()
  var scroll: FileScrollView { document.textScroll }
  let details = Surface()
  var inspector = false
  var item: FileItem?
  var share: (() -> Void)?, open: (() -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    addSubview(header)
    addSubview(document)
    addSubview(details)
  }
  required init?(coder: NSCoder) { nil }
  func configure(_ file: FileItem?, inspector: Bool = false, location: String = "") {
    item = file
    self.inspector = inspector
    document.configure(file)
    fill = Design.raised
    details.fill = Design.raised
    header.title = file?.name ?? "No file selected"
    header.subtitle = file.map { inspector ? "Details" : $0.kind.label } ?? ""
    header.needsDisplay = true
    details.subviews.forEach { $0.removeFromSuperview() }
    if inspector, let file {
      let name = label(file.name, size: 12, weight: .semibold)
      name.frame = NSRect(x: 20, y: 12, width: max(100, bounds.width - 40), height: 22)
      details.addSubview(name)
      let meta = label("\(file.kind.label) · \(file.sizeLabel)", size: 10, color: Design.muted)
      meta.frame = NSRect(x: 20, y: 36, width: 250, height: 20)
      details.addSubview(meta)
      for (i, pair) in [
        ("Location", location), ("Modified", file.date), ("Created", file.created?.formatted(date: .abbreviated, time: .shortened) ?? "—"),
        ("Size", file.sizeLabel),
      ].enumerated() {
        let key = label(pair.0, size: 10, color: Design.muted)
        let value = label(pair.1, size: 10)
        key.frame = NSRect(x: 20, y: 75 + CGFloat(i) * 25, width: 60, height: 18)
        value.frame = NSRect(x: 90, y: 75 + CGFloat(i) * 25, width: 200, height: 18)
        details.addSubview(key)
        details.addSubview(value)
      }
      let share = ActionButton(
        "Share", symbol: "square.and.arrow.up", action: { [weak self] in self?.share?() })
      share.frame = NSRect(x: 20, y: 186, width: 75, height: 28)
      details.addSubview(share)
      let open = ActionButton("Open", action: { [weak self] in self?.open?() })
      open.setSelected(true, primary: true)
      open.frame = NSRect(x: 104, y: 186, width: 70, height: 28)
      details.addSubview(open)
    }
    details.isHidden = !inspector
    needsLayout = true
    needsDisplay = true
  }
  override func layout() {
    super.layout()
    header.frame = NSRect(x: 0, y: 0, width: bounds.width, height: PaneTitle.height)
    let detailsH: CGFloat = inspector ? 230 : 0
    let artH = max(100, bounds.height - PaneTitle.height - detailsH)
    document.frame = NSRect(x: 0, y: PaneTitle.height, width: bounds.width, height: artH)
    details.frame = NSRect(x: 0, y: PaneTitle.height + artH, width: bounds.width, height: detailsH)
  }
}
