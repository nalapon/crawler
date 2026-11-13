import Foundation

struct FolderID: Hashable, Sendable, RawRepresentable {
  let rawValue: String
  init(rawValue: String) { self.rawValue = rawValue }
  init(_ path: String) { rawValue = URL(fileURLWithPath: path).standardizedFileURL.path }
  static let home = Self(FileManager.default.homeDirectoryForCurrentUser.path)
  static let downloads = Self(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads").path)
  static let desktop = Self(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Desktop").path)
  var url: URL { URL(fileURLWithPath: rawValue, isDirectory: true) }
}
struct ItemID: Hashable, Sendable { let rawValue: String }
enum DocumentKind: String, CaseIterable, Sendable {
  case pdf = "PDF document", spreadsheet = "Spreadsheet", markdown = "Markdown"
  case design = "Design file", image = "Image", diskImage = "Disk image"
  case archive = "Archive", presentation = "Presentation", text = "File"
  static func forExtension(_ ext: String) -> Self {
    switch ext.lowercased() {
    case "pdf": .pdf
    case "csv", "xlsx", "xls", "numbers": .spreadsheet
    case "md", "markdown": .markdown
    case "fig", "sketch": .design
    case "png", "jpg", "jpeg", "gif", "heic", "svg", "webp", "tiff": .image
    case "dmg", "iso": .diskImage
    case "zip", "gz", "tar", "7z": .archive
    case "key", "ppt", "pptx": .presentation
    default: .text
    }
  }
}
enum FileKind: Equatable, Sendable {
  case folder(FolderID)
  case document(DocumentKind)
  var label: String {
    switch self { case .folder: "Folder"; case .document(let kind): kind.rawValue }
  }
  var folder: FolderID? {
    if case .folder(let id) = self { return id }; return nil
  }
}
struct FileItem: Identifiable, Equatable, Sendable {
  let id: ItemID
  let name: String
  let parent: FolderID
  let kind: FileKind
  var bytes: Int64?
  let modified: Date?
  let created: Date?
  var url: URL { URL(fileURLWithPath: id.rawValue) }
  var megabytes: Double { Double(bytes ?? 0) / 1_048_576 }
  var date: String { modified?.formatted(date: .abbreviated, time: .shortened) ?? "—" }
  var sizeLabel: String {
    guard let bytes else { return "—" }
    return ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
  }
}
enum Theme: String, CaseIterable {
  case cupertino = "Cupertino", dark = "Dark", catppuccin = "Catppuccin Mocha"
  case github = "GitHub", vercel = "Vercel", system7 = "System 7"
  case terminal = "Terminal", windowsRetro = "Windows Retro", windows = "Windows"
  case linux = "Linux", linuxRetro = "Linux Retro"

  var detail: String {
    switch self {
    case .cupertino: "macOS light"
    case .dark: "Graphite"
    case .catppuccin: "Pastel night"
    case .github: "Paper & blue"
    case .vercel: "Black & white"
    case .system7: "Retro Macintosh"
    case .terminal: "Green phosphor"
    case .windowsRetro: "Windows 95"
    case .windows: "Soft blue"
    case .linux: "Ubuntu aubergine"
    case .linuxRetro: "Classic Unix"
    }
  }
}
enum SortField: String { case name, date, created, kind, size }
enum FileSearchMode: Int { case computer, folder }

/// The funnel state the workspace keeps for Search or Folders while the other is visible.
struct ScopeSession {
  var searchMode: FileSearchMode = .computer
  var computerScope: FolderID? = nil
  var volumeRoot: FolderID? = nil
  var listsRoot = false
  let root: FolderID
  let scope: FolderID?
  let query: String
  let selectedPath: String?
  let chosenNeighbor: SearchHit?
  let history: [FolderID]
  let scroll: CGFloat
}
