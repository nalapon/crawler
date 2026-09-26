import AppKit
import ImageIO

private struct SettingsWallpaperImage: @unchecked Sendable {
  let url: URL
  let name: String
  let image: CGImage
}

/// Native, keyboard-focusable previews. Only installed images are offered as wallpapers.
@MainActor
final class SettingsVisualPreview: NSView {
  var chooseWallpaper: ((URL) -> Void)?
  var chooseAppearance: ((Bool) -> Void)?
  var changed: (() -> Void)?
  private(set) var buttons: [SettingsPreviewTile] = []
  private let title = label("", size: 13, weight: .semibold)
  private let note = label("", size: 11, color: Design.muted)
  private let hero = SettingsWallpaperHero()
  private var load: Task<Void, Never>?
  private var worker: Task<[SettingsWallpaperImage], Never>?
  private var section: SystemSettingsSection?
  private var currentURL: URL?
  private var loadedFor: URL?
  private var wallpaperImages: [SettingsWallpaperImage] = []
  private var hasLoaded = false
  private var generation = 0
  override var isFlipped: Bool { true }

  override init(frame: NSRect) {
    super.init(frame: frame)
    hero.imageScaling = .scaleProportionallyUpOrDown
    hero.wantsLayer = true
    hero.layer?.backgroundColor = Design.color(0xf1f2f5).cgColor
    hero.layer?.cornerRadius = 8
    title.maximumNumberOfLines = 1
    note.maximumNumberOfLines = 2
    for view in [title, note, hero] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }

  func update(section: SystemSettingsSection?, currentURL: URL?, dark: Bool) {
    title.textColor = Design.text
    note.textColor = Design.muted
    hero.layer?.backgroundColor = Design.raised.cgColor
    let changedSection = self.section != section
    self.section = section
    self.currentURL = currentURL
    isHidden = section == nil
    guard let section else { stop(); return }
    if section == .appearance {
      stop()
      title.stringValue = "Choose an appearance"
      note.stringValue = "Preview the window style. Selecting a card applies it to macOS."
      hero.isHidden = true
      if changedSection || buttons.count != 2 {
        clearButtons()
        for (name, isDark) in [("Light", false), ("Dark", true)] {
          let button = SettingsPreviewTile(name: name, image: nil, dark: isDark) { [weak self] in self?.chooseAppearance?(isDark) }
          addSubview(button); buttons.append(button)
        }
      }
      for (index, button) in buttons.enumerated() { button.chosen = (index == 1) == dark }
      needsLayout = true
      return
    }
    title.stringValue = "Desktop preview"
    hero.isHidden = false
    if hasLoaded, loadedFor == currentURL || wallpaperImages.contains(where: { $0.url == currentURL }) {
      loadedFor = currentURL
      if changedSection { installWallpapers() }
      else {
        hero.image = wallpaperImages.first(where: { $0.url == currentURL }).map { NSImage(cgImage: $0.image, size: .zero) }
        for (index, button) in buttons.enumerated() { button.chosen = wallpaperImages[index].url == currentURL }
      }
      return
    }
    guard load == nil || loadedFor != currentURL else { return }
    stop()
    loadedFor = currentURL
    hasLoaded = false
    let ticket = generation
    note.stringValue = "Loading installed wallpapers…"
    clearButtons()
    hero.image = nil
    load = Task { [weak self] in
      let worker = Task.detached(priority: .utility) { Self.readImages(currentURL: currentURL) }
      self?.worker = worker
      let images = await worker.value
      guard let self, !Task.isCancelled, self.generation == ticket, self.section == .wallpaper else { return }
      self.load = nil
      self.worker = nil
      self.wallpaperImages = images
      self.hasLoaded = true
      self.installWallpapers()
      self.changed?()
    }
  }
  func stop() {
    generation += 1
    load?.cancel(); load = nil
    worker?.cancel(); worker = nil
  }
  private func clearButtons() { buttons.forEach { $0.removeFromSuperview() }; buttons = [] }
  private func installWallpapers() {
    clearButtons()
    hero.image = wallpaperImages.first(where: { $0.url == currentURL }).map { NSImage(cgImage: $0.image, size: .zero) }
    hero.setAccessibilityLabel("Current desktop wallpaper")
    note.stringValue = wallpaperImages.isEmpty ? "No readable local wallpaper images. Choose an image below." : "\(wallpaperImages.count) local images · Select a thumbnail to apply it."
    for item in wallpaperImages {
      let button = SettingsPreviewTile(name: item.name, image: item.image, dark: nil) { [weak self] in self?.chooseWallpaper?(item.url) }
      button.chosen = item.url == currentURL
      addSubview(button); buttons.append(button)
    }
    needsLayout = true
  }
  func height(for width: CGFloat) -> CGFloat {
    guard !isHidden else { return 0 }
    if section == .appearance { return 216 }
    let columns = max(1, Int(width / 158))
    let count = (buttons.count + columns - 1) / columns
    return 224 + CGFloat(count) * 126 + 16
  }
  override func layout() {
    super.layout()
    title.frame = NSRect(x: 0, y: 0, width: bounds.width, height: 22)
    note.frame = NSRect(x: 0, y: 25, width: bounds.width, height: 32)
    if section == .appearance {
      let width = max(1, min(240, (bounds.width - 12) / 2))
      for (index, button) in buttons.enumerated() { button.frame = NSRect(x: CGFloat(index) * (width + 12), y: 68, width: width, height: 136) }
    } else {
      hero.frame = NSRect(x: 0, y: 66, width: min(420, bounds.width), height: 140)
      let columns = max(1, Int(bounds.width / 158))
      let width = max(1, (bounds.width - CGFloat(columns - 1) * 12) / CGFloat(columns))
      for (index, button) in buttons.enumerated() {
        button.frame = NSRect(x: CGFloat(index % columns) * (width + 12), y: 224 + CGFloat(index / columns) * 126, width: width, height: 114)
      }
    }
  }
  nonisolated private static func readImages(currentURL: URL?) -> [SettingsWallpaperImage] {
    let root = URL(fileURLWithPath: "/System/Library/Desktop Pictures", isDirectory: true)
    var candidates: [URL] = []
    if let currentURL { candidates.append(currentURL) }
    let types = Set(["heic", "jpg", "jpeg", "png", "tiff"])
    if let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles, .skipsPackageDescendants]) {
      var local: [URL] = []
      for case let url as URL in files {
        if Task.isCancelled { return [] }
        guard types.contains(url.pathExtension.lowercased()) else { continue }
        local.append(url)
        if local.count >= 128 { break }
      }
      candidates += local.sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
    }
    var seen = Set<URL>()
    return candidates.compactMap { url in
      guard !Task.isCancelled, seen.insert(url.standardizedFileURL).inserted,
            let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
            let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
              kCGImageSourceCreateThumbnailFromImageAlways: true,
              kCGImageSourceCreateThumbnailWithTransform: true,
              kCGImageSourceThumbnailMaxPixelSize: url == currentURL ? 640 : 320,
              kCGImageSourceShouldCacheImmediately: true,
            ] as CFDictionary) else { return nil }
      return SettingsWallpaperImage(url: url, name: url.deletingPathExtension().lastPathComponent, image: image)
    }
  }
}

@MainActor
final class SettingsPreviewTile: NSButton {
  private let preview: NSImage?
  private let dark: Bool?
  private let invoke: () -> Void
  var chosen = false { didSet { needsDisplay = true; setAccessibilityValue(chosen ? "Selected" : "Not selected") } }
  override var acceptsFirstResponder: Bool { true }
  override var isFlipped: Bool { true }
  init(name: String, image: CGImage?, dark: Bool?, action: @escaping () -> Void) {
    preview = image.map { NSImage(cgImage: $0, size: .zero) }
    self.dark = dark; invoke = action
    super.init(frame: .zero)
    title = name; isBordered = false; focusRingType = .none
    target = self; self.action = #selector(activate)
    setAccessibilityLabel(name)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func activate() { invoke() }
  override func draw(_ dirtyRect: NSRect) {
    let picture = NSRect(x: 3, y: 3, width: max(1, bounds.width - 6), height: max(1, bounds.height - 30))
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(roundedRect: picture, xRadius: 7, yRadius: 7).addClip()
    if let dark {
      (dark ? Design.color(0x242733) : Design.color(0xe8ecf4)).setFill(); picture.fill()
      let window = picture.insetBy(dx: 13, dy: 12)
      (dark ? Design.color(0x17191f) : .white).setFill(); NSBezierPath(roundedRect: window, xRadius: 4, yRadius: 4).fill()
      (dark ? Design.color(0x30333c) : Design.color(0xf1f2f5)).setFill()
      NSRect(x: window.minX, y: window.minY, width: window.width * 0.28, height: window.height).fill()
      NSColor.controlAccentColor.setFill()
      NSBezierPath(roundedRect: NSRect(x: window.minX + 6, y: window.minY + 23, width: max(8, window.width * 0.28 - 12), height: 9), xRadius: 3, yRadius: 3).fill()
      (dark ? Design.color(0x969cae) : Design.color(0x747e90)).setFill()
      for row in 0..<3 { NSRect(x: window.minX + window.width * 0.36, y: window.minY + 15 + CGFloat(row) * 14, width: window.width * (row == 0 ? 0.42 : 0.5), height: row == 0 ? 5 : 3).fill() }
    } else if let preview {
      let scale = max(picture.width / preview.size.width, picture.height / preview.size.height)
      let size = NSSize(width: preview.size.width * scale, height: preview.size.height * scale)
      preview.draw(in: NSRect(x: picture.midX - size.width / 2, y: picture.midY - size.height / 2, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: isEnabled ? 1 : 0.5, respectFlipped: true, hints: nil)
    }
    NSGraphicsContext.restoreGraphicsState()
    (chosen ? NSColor.controlAccentColor : Design.color(0xd6dae2)).setStroke()
    let border = NSBezierPath(roundedRect: picture, xRadius: 7, yRadius: 7)
    border.lineWidth = chosen ? 3 : 1; border.stroke()
    let paragraph = NSMutableParagraphStyle(); paragraph.lineBreakMode = .byTruncatingTail; paragraph.alignment = .center
    (title as NSString).draw(in: NSRect(x: 2, y: bounds.height - 22, width: bounds.width - 4, height: 20), withAttributes: [.font: Design.font(11, chosen ? .semibold : .regular), .foregroundColor: Design.text, .paragraphStyle: paragraph])
    if window?.firstResponder === self {
      NSColor.keyboardFocusIndicatorColor.setStroke()
      let focus = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 9, yRadius: 9)
      focus.lineWidth = 2; focus.stroke()
    }
  }
  override func becomeFirstResponder() -> Bool { needsDisplay = true; return true }
  override func resignFirstResponder() -> Bool { needsDisplay = true; return true }
}

@MainActor
private final class SettingsWallpaperHero: NSImageView {
  override var isFlipped: Bool { true }
  override func draw(_ dirtyRect: NSRect) {
    guard let image, image.size.width > 0, image.size.height > 0 else { return }
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8).addClip()
    let scale = max(bounds.width / image.size.width, bounds.height / image.size.height)
    let size = NSSize(width: image.size.width * scale, height: image.size.height * scale)
    image.draw(in: NSRect(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    NSGraphicsContext.restoreGraphicsState()
  }
}
