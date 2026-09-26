import AppKit

struct MountedVolume: Equatable, Sendable {
  let folder: FolderID
  let name: String
  let ejectable: Bool

  func contains(_ folder: FolderID) -> Bool {
    folder == self.folder || folder.rawValue.hasPrefix(self.folder.rawValue + "/")
  }
}

// macOS publishes user-visible volumes, including partitions and mounted disk images.
// Read their metadata off the main actor; never expose the boot/system volumes.
@MainActor
final class MountedVolumes {
  private(set) var volumes: [MountedVolume] = []
  var didChange: (([MountedVolume]) -> Void)?
  private var readTask: Task<Void, Never>?

  init() {
    let center = NSWorkspace.shared.notificationCenter
    for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification,
                 NSWorkspace.didRenameVolumeNotification] {
      center.addObserver(self, selector: #selector(reload), name: name, object: nil)
    }
    reload()
  }

  deinit { readTask?.cancel(); NSWorkspace.shared.notificationCenter.removeObserver(self) }

  @objc private func reload() {
    readTask?.cancel()
    readTask = Task { [weak self] in
      let worker = Task.detached(priority: .utility) {
        let keys: Set<URLResourceKey> = [.volumeNameKey, .volumeIsBrowsableKey,
          .volumeIsEjectableKey, .volumeIsRemovableKey]
        let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: Array(keys),
          options: [.skipHiddenVolumes]) ?? []
        return urls.compactMap { url -> MountedVolume? in
          let folder = FolderID(url.path)
          guard folder.rawValue != "/", !folder.rawValue.hasPrefix("/System/Volumes/"),
            let values = try? url.resourceValues(forKeys: keys), values.volumeIsBrowsable != false
          else { return nil }
          return MountedVolume(folder: folder, name: values.volumeName ?? url.lastPathComponent,
            ejectable: values.volumeIsEjectable == true || values.volumeIsRemovable == true)
        }.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
      }
      let volumes = await worker.value
      guard let self, !Task.isCancelled else { return }
      let previous = self.volumes
      self.volumes = volumes
      self.didChange?(previous)
    }
  }

  func containing(_ folder: FolderID?) -> MountedVolume? {
    guard let folder else { return nil }
    return volumes.filter { $0.contains(folder) }.max { $0.folder.rawValue.count < $1.folder.rawValue.count }
  }
}
