import AppKit
import os

/// Byte progress of one background copy, shared between the copyfile callback and the UI.
final class FileCopyProgress: Sendable {
  private struct State {
    var total: Int64 = 0, finished: Int64 = 0, current: Int64 = 0, base: Int64 = 0
    var name = "", cancelled = false
  }
  private let state = OSAllocatedUnfairLock(initialState: State())

  var copied: Int64 { state.withLock { $0.finished + $0.current } }
  var total: Int64 { state.withLock { $0.total } }
  var name: String { state.withLock { $0.name } }
  var isCancelled: Bool { state.withLock { $0.cancelled } }
  func cancel() { state.withLock { $0.cancelled = true } }

  fileprivate func start(total: Int64) { state.withLock { $0.total = total } }
  fileprivate func begin(_ name: String) { state.withLock { $0.name = name; $0.current = 0; $0.base = $0.finished } }
  fileprivate func update(_ bytes: Int64) { state.withLock { $0.current = bytes } }
  fileprivate func finish(_ bytes: Int64) { state.withLock { $0.finished += bytes; $0.current = 0 } }
  /// Settles the item started by `begin`, whether or not copyfile reported its files one by one.
  fileprivate func complete(_ bytes: Int64) { state.withLock { $0.finished = $0.base + bytes; $0.current = 0 } }
}

/// Copies files into a folder with copyfile(3), never replacing an existing name.
/// Runs off the main actor and returns one readable message per failed item.
enum FileCopy {
  nonisolated static func run(_ urls: [URL], to folder: FolderID, place: String, progress: FileCopyProgress) -> [String] {
    if let problem = unwritable(folder, place: place) { return [problem] }
    let sources = urls.map(\.standardizedFileURL)
    progress.start(total: sources.reduce(0) { $0 + size($1) })
    var failures: [String] = []
    for source in sources {
      let name = source.lastPathComponent, target = folder.url.appendingPathComponent(name)
      guard !progress.isCancelled else { break }
      if folder.rawValue == source.path || folder.rawValue.hasPrefix(source.path + "/") {
        failures.append("Can't copy \(name) into itself"); continue
      }
      if FileManager.default.fileExists(atPath: target.path) {
        failures.append("\(name) already exists in \(place)"); continue
      }
      progress.begin(name)
      let state = copyfile_state_alloc()
      defer { copyfile_state_free(state) }
      copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CB), unsafeBitCast(callback, to: UnsafeRawPointer.self))
      copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CTX), Unmanaged.passUnretained(progress).toOpaque())
      let flags = copyfile_flags_t(COPYFILE_ALL | COPYFILE_RECURSIVE | COPYFILE_EXCL)
      if copyfile(source.path, target.path, state, flags) == 0 {
        progress.complete(size(source)); continue
      }
      let code = errno
      // EXCL guarantees a failed or cancelled target was created by this copy, never pre-existing.
      if code != EEXIST { try? FileManager.default.removeItem(at: target) }
      if progress.isCancelled { break }
      failures.append(code == EEXIST ? "\(name) already exists in \(place)"
        : "Couldn't copy \(name): \(String(cString: strerror(code)))")
    }
    return failures
  }

  /// Explains why a destination can't accept files, such as an NTFS drive macOS mounts read-only.
  private nonisolated static func unwritable(_ folder: FolderID, place: String) -> String? {
    let values = try? folder.url.resourceValues(forKeys: [.volumeIsReadOnlyKey, .volumeLocalizedFormatDescriptionKey])
    if values?.volumeIsReadOnly == true {
      let format = values?.volumeLocalizedFormatDescription.map { " (\($0))" } ?? ""
      return "\(place) is read-only\(format). macOS can't write to it."
    }
    return FileManager.default.isWritableFile(atPath: folder.rawValue) ? nil : "No permission to write to \(place)"
  }

  /// Logical bytes of a file or folder tree, without following symlinks.
  private nonisolated static func size(_ url: URL) -> Int64 {
    let keys: [URLResourceKey] = [.fileSizeKey, .isRegularFileKey]
    guard (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true else {
      return Int64((try? url.resourceValues(forKeys: Set(keys)))?.fileSize ?? 0)
    }
    var total: Int64 = 0
    for case let file as URL in FileManager.default.enumerator(at: url, includingPropertiesForKeys: keys) ?? .init() {
      if let values = try? file.resourceValues(forKeys: Set(keys)), values.isRegularFile == true {
        total += Int64(values.fileSize ?? 0)
      }
    }
    return total
  }

  private static let callback: copyfile_callback_t = { what, stage, state, source, _, context in
    guard let context else { return COPYFILE_CONTINUE }
    let progress = Unmanaged<FileCopyProgress>.fromOpaque(context).takeUnretainedValue()
    if progress.isCancelled { return COPYFILE_QUIT }
    if what == COPYFILE_COPY_DATA && stage == COPYFILE_PROGRESS {
      var copied: off_t = 0
      copyfile_state_get(state, UInt32(COPYFILE_STATE_COPIED), &copied)
      progress.update(Int64(copied))
    } else if what == COPYFILE_RECURSE_FILE && stage == COPYFILE_FINISH, let source {
      var info = stat()
      if lstat(source, &info) == 0 { progress.finish(Int64(info.st_size)) }
    }
    return COPYFILE_CONTINUE
  }
}

/// One finished copy for the Transfers list.
struct CopyRecord {
  enum Outcome { case copied, cancelled, failed }
  let title: String
  let detail: String
  let outcome: Outcome
  let date = Date()
}

/// The Copies pane in Transfers: the running copy with its progress, then this session's copies.
@MainActor
final class CopiesView: Surface {
  let cancel = ActionButton("Cancel", action: {})
  private let title = PaneTitle()
  private let rows = Surface()
  private let bar = ProgressBar()

  override init(frame: NSRect) {
    super.init(frame: frame)
    title.title = "Copies"
    for view in [title, rows] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }

  func configure(running: (progress: FileCopyProgress, place: String)?, records: [CopyRecord]) {
    fill = Design.background
    rows.fill = Design.background
    rows.subviews.forEach { $0.removeFromSuperview() }
    title.subtitle = running == nil ? "Drop files on a Folders place to copy them" : "One copy runs at a time"
    title.needsDisplay = true
    var y: CGFloat = 12
    let width = max(0, bounds.width - 32)
    func row(_ heading: String, _ detail: String, color: NSColor) {
      let name = label(heading, size: 12, weight: .semibold, color: color)
      name.frame = NSRect(x: 16, y: y, width: width, height: 18)
      name.lineBreakMode = .byTruncatingMiddle
      let info = label(detail, size: 11, color: Design.muted)
      info.frame = NSRect(x: 16, y: y + 19, width: width, height: 30)
      info.maximumNumberOfLines = 2
      info.lineBreakMode = .byWordWrapping
      rows.addSubview(name)
      rows.addSubview(info)
    }
    if let running {
      let progress = running.progress
      let copied = progress.copied, total = max(progress.total, 1)
      let bytes = { ByteCountFormatter.string(fromByteCount: $0, countStyle: .file) }
      row("Copying \(progress.name) to \(running.place)", "\(bytes(min(copied, total))) of \(bytes(total)) · existing names are skipped, never replaced", color: Design.text)
      bar.value = min(1, Double(copied) / Double(total))
      bar.frame = NSRect(x: 16, y: y + 52, width: max(0, width - 80), height: Design.square ? 12 : 8)
      cancel.frame = NSRect(x: bounds.width - 86, y: y + 44, width: 70, height: 24)
      rows.addSubview(bar)
      rows.addSubview(cancel)
      y += 84
    }
    for record in records {
      let color = record.outcome == .failed ? NSColor.systemRed : Design.text
      row(record.title, record.date.formatted(date: .omitted, time: .shortened) + (record.detail.isEmpty ? "" : " · " + record.detail), color: color)
      y += 58
    }
    if running == nil && records.isEmpty {
      let empty = label("No copies yet. Drag files from Search or Folders onto a place in Folders to copy them there.", size: 12, color: Design.muted)
      empty.maximumNumberOfLines = 3
      empty.lineBreakMode = .byWordWrapping
      empty.frame = NSRect(x: 16, y: y, width: width, height: 48)
      rows.addSubview(empty)
    }
    needsLayout = true
    needsDisplay = true
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill()
  }
  override func layout() {
    super.layout()
    title.frame = NSRect(x: 1, y: 0, width: bounds.width - 1, height: PaneTitle.height)
    rows.frame = NSRect(x: 1, y: PaneTitle.height, width: bounds.width - 1, height: max(0, bounds.height - PaneTitle.height))
  }
}

extension Workspace {
  /// Copies dropped files into a folder in the background. The status bar and Transfers show
  /// byte progress until it finishes; the outcome joins the Transfers list.
  func copy(_ urls: [URL], to folder: FolderID) {
    guard copying == nil else { toast("Wait for the current copy to finish"); return }
    let place = mountedVolumes.volumes.first { $0.folder == folder }?.name ?? folder.url.lastPathComponent
    let progress = FileCopyProgress()
    copying = (progress, place)
    statusBar.cancel.invoke = { progress.cancel() }
    copies.cancel.invoke = { progress.cancel() }
    let timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
      MainActor.assumeIsolated {
        guard let self, progress.total > 0 else { return }
        self.statusBar.showCopy(progress, place: place)
        if self.section == .transfers { self.copies.configure(running: self.copying, records: self.copyRecords) }
      }
    }
    Task { [weak self] in
      let failures = await Task.detached(priority: .userInitiated) {
        FileCopy.run(urls, to: folder, place: place, progress: progress)
      }.value
      timer.invalidate()
      guard let self else { return }
      self.copying = nil
      self.statusBar.showCopy(nil)
      let items = urls.count == 1 ? urls[0].lastPathComponent : "\(urls.count) items"
      let record: CopyRecord
      if let first = failures.first {
        let message = failures.count > 1 ? "\(first) (+\(failures.count - 1) more)" : first
        record = CopyRecord(title: "Could not copy all of \(items) to \(place)", detail: message, outcome: .failed)
        self.toast(message)
      } else if progress.isCancelled {
        record = CopyRecord(title: "Copy of \(items) to \(place) cancelled", detail: "The partial item was removed.", outcome: .cancelled)
        self.toast("Copy to \(place) cancelled")
      } else {
        record = CopyRecord(title: "Copied \(items) to \(place)", detail: ByteCountFormatter.string(fromByteCount: progress.copied, countStyle: .file), outcome: .copied)
        self.toast(urls.count == 1 ? "Copied \(urls[0].lastPathComponent) to \(place)" : "Copied \(urls.count) items to \(place)")
      }
      self.copyRecords.insert(record, at: 0)
      self.render()
    }
  }
}
