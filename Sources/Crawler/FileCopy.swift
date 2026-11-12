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

/// Bottom card with the current copy's name, byte progress and a Cancel button.
@MainActor
final class CopyProgressPanel: Surface {
  private let title = label("", size: 11)
  private let detail = label("", size: 10)
  private let bar = NSProgressIndicator()
  let cancel = ActionButton("Cancel", action: {})

  override init(frame: NSRect) {
    super.init(frame: frame)
    fill = Design.color(0x303747)
    wantsLayer = true
    layer?.cornerRadius = 7
    title.textColor = .white
    title.lineBreakMode = .byTruncatingMiddle
    detail.textColor = NSColor.white.withAlphaComponent(0.7)
    bar.isIndeterminate = false
    bar.minValue = 0
    bar.maxValue = 1
    bar.controlSize = .small
    cancel.contentTintColor = .white
    for view in [title, detail, bar, cancel] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }

  func show(_ progress: FileCopyProgress, place: String) {
    let copied = progress.copied, total = max(progress.total, 1)
    title.stringValue = "Copying \(progress.name) to \(place)"
    detail.stringValue = "\(Self.bytes(min(copied, total))) of \(Self.bytes(total))"
    bar.doubleValue = min(1, Double(copied) / Double(total))
  }

  private static func bytes(_ value: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: value, countStyle: .file)
  }

  override func layout() {
    super.layout()
    title.frame = NSRect(x: 14, y: 9, width: bounds.width - 104, height: 17)
    detail.frame = NSRect(x: 14, y: 26, width: bounds.width - 104, height: 15)
    bar.frame = NSRect(x: 14, y: 42, width: bounds.width - 104, height: 8)
    cancel.frame = NSRect(x: bounds.width - 82, y: 14, width: 70, height: 28)
  }
  override var isFlipped: Bool { true }
}

extension Workspace {
  /// Copies dropped files into a folder in the background, showing byte progress until done.
  func copy(_ urls: [URL], to folder: FolderID) {
    guard copying == nil else { toast("Wait for the current copy to finish"); return }
    let place = mountedVolumes.volumes.first { $0.folder == folder }?.name ?? folder.url.lastPathComponent
    let progress = FileCopyProgress()
    copying = progress
    copyPanel.cancel.invoke = { progress.cancel() }
    let timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
      MainActor.assumeIsolated {
        guard let self, progress.total > 0 else { return }
        self.copyPanel.show(progress, place: place)
        if self.copyPanel.isHidden { self.copyPanel.isHidden = false; self.needsLayout = true }
      }
    }
    Task { [weak self] in
      let failures = await Task.detached(priority: .userInitiated) {
        FileCopy.run(urls, to: folder, place: place, progress: progress)
      }.value
      timer.invalidate()
      guard let self else { return }
      self.copying = nil
      self.copyPanel.isHidden = true
      self.needsLayout = true
      if let first = failures.first { self.toast(failures.count > 1 ? "\(first) (+\(failures.count - 1) more)" : first) }
      else if progress.isCancelled { self.toast("Copy to \(place) cancelled") }
      else { self.toast(urls.count == 1 ? "Copied \(urls[0].lastPathComponent) to \(place)" : "Copied \(urls.count) items to \(place)") }
    }
  }
}
