import AppKit
import UniformTypeIdentifiers

struct TextEditor: Equatable, Sendable {
  let bundleID: String
  let name: String
  let url: URL
}

/// Discovery reads application bundles only, never walks the user's documents or app contents.
enum EditorDiscovery {
  static func discover() -> [TextEditor] {
    let workspace = NSWorkspace.shared
    var candidates = Set<URL>()
    for ext in ["txt", "swift", "md"] {
      candidates.formUnion(
        workspace.urlsForApplications(toOpen: URL(fileURLWithPath: "/Crawler.\(ext)")))
    }
    let roots = [
      URL(fileURLWithPath: "/Applications"), URL(fileURLWithPath: "/System/Applications"),
      FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications"),
    ]
    for root in roots {
      guard
        let reader = FileManager.default.enumerator(
          at: root,
          includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey],
          options: [.skipsHiddenFiles, .skipsPackageDescendants])
      else { continue }
      for case let url as URL in reader {
        if Task.isCancelled { return [] }
        if url.pathExtension.lowercased() == "app" {
          reader.skipDescendants()
          candidates.insert(url)
        } else if (try? url.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) == true {
          reader.skipDescendants()
        }
      }
    }
    // Prefer the normal installation over another registered copy with the same bundle ID.
    let sorted = candidates.sorted {
      $0.path.localizedStandardCompare($1.path) == .orderedAscending
    }
    var editors: [String: TextEditor] = [:]
    for url in sorted {
      guard let editor = application(at: url), editors[editor.bundleID] == nil else { continue }
      editors[editor.bundleID] = editor
    }
    return editors.values.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
  }

  static func application(at url: URL) -> TextEditor? {
    guard let bundle = Bundle(url: url), let info = bundle.infoDictionary,
      let id = bundle.bundleIdentifier, id != Bundle.main.bundleIdentifier,
      info["CFBundlePackageType"] as? String == "APPL",
      info["LSBackgroundOnly"] as? Bool != true,
      let executable = bundle.executableURL,
      FileManager.default.isExecutableFile(atPath: executable.path),
      let types = info["CFBundleDocumentTypes"] as? [[String: Any]],
      types.contains(where: editsText)
    else { return nil }
    let name =
      bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
      ?? bundle.object(forInfoDictionaryKey: "CFBundleName") as? String
      ?? url.deletingPathExtension().lastPathComponent
    return TextEditor(bundleID: id, name: name, url: url.standardizedFileURL)
  }

  private static func editsText(_ declaration: [String: Any]) -> Bool {
    guard declaration["CFBundleTypeRole"] as? String == "Editor" else { return false }
    let extensions = declaration["CFBundleTypeExtensions"] as? [String] ?? []
    let sourceExtensions: Set<String> = [
      "txt", "text", "swift", "js", "ts", "py", "go", "rs", "c", "cpp", "md", "markdown",
    ]
    if extensions.contains(where: { sourceExtensions.contains($0.lowercased()) }) { return true }
    let types = declaration["LSItemContentTypes"] as? [String] ?? []
    if types.contains(where: { UTType($0)?.conforms(to: .sourceCode) == true }) { return true }
    let generalText: Set<String> = [
      "public.text", "public.plain-text", "public.utf8-plain-text", "public.utf16-plain-text",
    ]
    // Numbers declares plain text alongside CSV/TSV. That is an importer, not a general text editor.
    return types.contains(where: generalText.contains)
      && types.allSatisfy {
        generalText.contains($0) || ["public.folder", "public.data", "public.content"].contains($0)
      }
  }
}

@MainActor
final class FileOpening: NSObject {
  static let shared = FileOpening()
  enum Outcome {
    case opened(String?)
    case failed(String)
  }
  private let preferenceKey = "defaultTextEditorBundleID"
  private(set) var editors: [TextEditor] = []
  private(set) var scanning = false
  private var scan: Task<Void, Never>?
  private var lastScan = Date.distantPast
  private var icons: [URL: NSImage] = [:]
  var didChange: (() -> Void)?
  var preferredID: String? { UserDefaults.standard.string(forKey: preferenceKey) }
  var selected: TextEditor? {
    if let preferredID, let editor = editors.first(where: { $0.bundleID == preferredID }) {
      return editor
    }
    return editors.first(where: { $0.bundleID == "com.apple.TextEdit" }) ?? editors.first
  }
  var unavailablePreference: Bool {
    preferredID != nil && !editors.contains { $0.bundleID == preferredID }
  }

  override init() {
    super.init()
    NSWorkspace.shared.notificationCenter.addObserver(
      self, selector: #selector(appActivated),
      name: NSWorkspace.didActivateApplicationNotification, object: nil)
  }
  @objc private func appActivated(_ notification: Notification) {
    guard
      let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
      app.processIdentifier == ProcessInfo.processInfo.processIdentifier
    else { return }
    refresh()
  }
  func refresh(force: Bool = false) {
    guard scan == nil, force || Date().timeIntervalSince(lastScan) > 30 else { return }
    scanning = true
    didChange?()
    scan = Task { [weak self] in
      let found = await Task.detached(priority: .utility) { EditorDiscovery.discover() }.value
      guard let self else { return }
      self.editors = found
      self.icons = self.icons.filter { url, _ in found.contains { $0.url == url } }
      self.lastScan = Date()
      self.scanning = false
      self.scan = nil
      self.didChange?()
    }
  }
  func choose(_ editor: TextEditor) {
    UserDefaults.standard.set(editor.bundleID, forKey: preferenceKey)
    didChange?()
  }
  func icon(for editor: TextEditor) -> NSImage {
    if let icon = icons[editor.url] { return icon }
    let icon = NSWorkspace.shared.icon(forFile: editor.url.path)
    icon.size = NSSize(width: 16, height: 16)
    icons[editor.url] = icon
    return icon
  }
  static var recentPaths: [String] {
    Array((UserDefaults.standard.stringArray(forKey: "recentOpenedFiles") ?? []).prefix(32))
  }
  static func recordRecent(_ url: URL) {
    let path = url.standardizedFileURL.path
    var paths = recentPaths.filter { $0 != path }
    paths.insert(path, at: 0)
    UserDefaults.standard.set(Array(paths.prefix(32)), forKey: "recentOpenedFiles")
  }
  static func usesTextEditor(_ url: URL) -> Bool {
    if PreviewFormat.isGoModuleFile(url) { return true }
    if CodeLanguage.detect(url.pathExtension) != nil { return true }
    if ["csv", "tsv", "rtf", "rtfd"].contains(url.pathExtension.lowercased()) { return false }
    if let type = UTType(filenameExtension: url.pathExtension),
      type.conforms(to: .image) || type.conforms(to: .movie) || type.conforms(to: .audio)
    {
      return false
    }
    return switch PreviewFormat.forURL(url) {
    case .text, .markdown: true
    case .pdf, .video, .document, .thumbnail: false
    }
  }
  func open(_ url: URL, completion: @escaping @MainActor (Outcome) -> Void) {
    Task { [weak self] in
      guard let self else { return }
      if Self.usesTextEditor(url) {
        self.refresh()
        await self.scan?.value
        if let editor = self.selected {
          let available = await Task.detached { EditorDiscovery.application(at: editor.url) != nil }
            .value
          if !available {
            self.refresh(force: true)
            await self.scan?.value
          }
        }
        guard let editor = self.selected else {
          completion(
            .failed(
              "No installed text editor was found. Install an editor, then refresh the list in Crawler Settings."
            ))
          return
        }
        let fallback =
          self.unavailablePreference
          ? "Your saved editor is unavailable. Opened with \(editor.name)." : nil
        do {
          _ = try await NSWorkspace.shared.open(
            [url], withApplicationAt: editor.url, configuration: .init())
          Self.recordRecent(url)
          completion(.opened(fallback))
        } catch {
          completion(
            .failed(
              "Could not open \(url.lastPathComponent) with \(editor.name): \(error.localizedDescription)"
            ))
          self.refresh(force: true)
        }
      } else {
        do {
          _ = try await NSWorkspace.shared.open(url, configuration: .init())
          Self.recordRecent(url)
          completion(.opened(nil))
        } catch {
          completion(
            .failed("Could not open \(url.lastPathComponent): \(error.localizedDescription)"))
        }
      }
    }
  }
}
