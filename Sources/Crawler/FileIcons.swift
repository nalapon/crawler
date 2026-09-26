import AppKit
import UniformTypeIdentifiers

// One lookup per type, shared by browsing, search and preview fallbacks.
@MainActor
private final class FileIcons {
  static let shared = FileIcons()
  private let images = NSCache<NSString, NSImage>()
  private var pending: [String: Task<Data?, Never>] = [:]
  private init() { images.countLimit = 256 }

  static func key(path: String, isFolder: Bool) -> String {
    if isFolder { return Design.classic ? "folder/system7" : "folder" }
    let name = (path as NSString).lastPathComponent.lowercased()
    let ext = (name as NSString).pathExtension
    let material: String?
    if ["go.mod", "go.sum", "go.work", "go.work.sum"].contains(name) {
      material = "go-mod"
    } else if name == "dockerfile" || name.hasPrefix("dockerfile.") {
      material = "docker"
    } else if [".gitignore", ".gitattributes", ".gitmodules", ".gitconfig"].contains(name) {
      material = "git"
    } else {
      switch ext {
      case "ts", "mts", "cts": material = "typescript"
      case "tsx": material = "react_ts"
      case "js", "mjs", "cjs": material = "javascript"
      case "jsx": material = "react"
      case "go": material = "go"
      case "swift": material = "swift"
      case "py", "pyi", "pyw": material = "python"
      case "rs": material = "rust"
      case "md", "markdown", "mdx": material = "markdown"
      case "json", "jsonc", "json5", "jsonl": material = "json"
      case "yml", "yaml": material = "yaml"
      case "xml", "plist": material = "xml"
      case "html", "htm": material = "html"
      case "css": material = "css"
      case "scss", "sass": material = "sass"
      case "less": material = "less"
      case "sh", "bash", "zsh", "fish": material = "console"
      case "c", "h": material = "c"
      case "cpp", "cc", "cxx", "hpp", "hxx": material = "cpp"
      case "cs": material = "csharp"
      case "java": material = "java"
      case "kt", "kts": material = "kotlin"
      case "rb": material = "ruby"
      case "php": material = "php"
      case "lua": material = "lua"
      case "sql": material = "database"
      case "toml": material = "toml"
      case "vue": material = "vue"
      case "svelte": material = "svelte"
      case "astro": material = "astro"
      default: material = nil
      }
    }
    return material.map { "material/" + $0 } ?? "native/" + ext
  }

  private static let classicFolder = NSImage(size: NSSize(width: 24, height: 20), flipped: true) {
    _ in
    NSGraphicsContext.current?.imageInterpolation = .none
    NSGraphicsContext.current?.shouldAntialias = false
    let outline = NSBezierPath()
    outline.move(to: NSPoint(x: 1.5, y: 18.5))
    for point in [
      NSPoint(x: 1.5, y: 3.5), NSPoint(x: 9.5, y: 3.5),
      NSPoint(x: 11.5, y: 5.5), NSPoint(x: 22.5, y: 5.5), NSPoint(x: 22.5, y: 18.5),
    ] {
      outline.line(to: point)
    }
    outline.close()
    NSColor.white.setFill()
    outline.fill()
    NSColor.black.setStroke()
    outline.lineWidth = 1
    outline.stroke()
    NSColor(white: 0.8, alpha: 1).setFill()
    NSRect(x: 3, y: 8, width: 18, height: 9).fill()
    let seam = NSBezierPath()
    seam.move(to: NSPoint(x: 2, y: 7.5))
    seam.line(to: NSPoint(x: 22, y: 7.5))
    seam.lineWidth = 1
    seam.stroke()
    return true
  }

  func cached(_ key: String) -> NSImage? {
    key == "folder/system7" ? Self.classicFolder : images.object(forKey: key as NSString)
  }

  func image(for key: String) async -> NSImage? {
    if let image = cached(key) { return image }
    let task: Task<Data?, Never>
    if let existing = pending[key] {
      task = existing
    } else {
      let resource =
        key.hasPrefix("material/")
        ? Bundle.main.url(
          forResource: String(key.dropFirst(9)), withExtension: "svg", subdirectory: "FileIcons")
        : nil
      task = Task.detached(priority: .userInitiated) {
        if let resource, let data = try? Data(contentsOf: resource) { return data }
        let type =
          key == "folder"
          ? UTType.folder
          : UTType(filenameExtension: String(key.dropFirst(7))) ?? .data
        // Keep only the resolution needed by a 24-point Retina list icon.
        // Serializing every native representation retained tens of MiB per file type.
        return autoreleasepool {
          guard
            let bitmap = NSBitmapImageRep(
              bitmapDataPlanes: nil, pixelsWide: 48, pixelsHigh: 48, bitsPerSample: 8,
              samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
              bytesPerRow: 0, bitsPerPixel: 0),
            let context = NSGraphicsContext(bitmapImageRep: bitmap)
          else { return nil }
          NSGraphicsContext.saveGraphicsState()
          defer { NSGraphicsContext.restoreGraphicsState() }
          NSGraphicsContext.current = context
          context.imageInterpolation = .high
          NSWorkspace.shared.icon(for: type).draw(
            in: NSRect(x: 0, y: 0, width: 48, height: 48), from: .zero,
            operation: .sourceOver, fraction: 1)
          return bitmap.representation(using: .png, properties: [:])
        }
      }
      pending[key] = task
    }
    let data = await task.value
    // Another waiting view may have already decoded this image.
    if let image = cached(key) { return image }
    pending[key] = nil
    guard let data, let image = NSImage(data: data) else { return nil }
    image.isTemplate = false
    images.setObject(image, forKey: key as NSString)
    return image
  }
}

@MainActor
final class FileIconView: NSImageView {
  private var key: String?
  private var load: Task<Void, Never>?
  private static let placeholder = NSImage(systemSymbolName: "doc", accessibilityDescription: nil)

  func configure(path: String, isFolder: Bool) {
    let next = FileIcons.key(path: path, isFolder: isFolder)
    guard next != key else { return }
    key = next
    load?.cancel()
    imageScaling = .scaleProportionallyUpOrDown
    contentTintColor = nil
    setAccessibilityElement(false)
    if let cached = FileIcons.shared.cached(next) {
      image = cached
      return
    }
    image = Self.placeholder
    load = Task { [weak self] in
      let image = await FileIcons.shared.image(for: next)
      guard !Task.isCancelled, let self, self.key == next else { return }
      self.image = image ?? Self.placeholder
    }
  }
}
