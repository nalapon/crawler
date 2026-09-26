import AppKit

extension Workspace {
  /// Capture the visible context before presenting the palette over the workspace.
  func configureCommands() {
    let file = selected
    let folder = currentFolder
    var commands: [CommandPalette.Command] = []
    func add(_ id: String, _ title: String, aliases: String = "", shortcut: String = "",
             symbol: String = "command", action: @escaping (Workspace) -> Void) {
      commands.append(.init(id: id, title: title, aliases: aliases, shortcut: shortcut, symbol: symbol,
        action: { [weak self] in
          guard let self else { return }
          self.palette.finish { action(self) }
        }))
    }
    add("downloads", "Go to Downloads", aliases: "navigate", symbol: "arrow.down.circle") { $0.navigate(.folder(.downloads)) }
    add("desktop", "Go to Desktop", aliases: "navigate", symbol: "desktopcomputer") { $0.navigate(.folder(.desktop)) }
    if let pinned = library.pinned {
      add("pinned", "Go to pinned folder: \(pinned.url.lastPathComponent)", aliases: "favorite", symbol: "pin") { $0.navigate(.folder(pinned)) }
    }
    commands.append(.init(id: "drives", title: "Go to connected drive…", aliases: "volume disk locations", symbol: "externaldrive", action: { [weak self] in
      guard let self else { return }
      self.palette.showChoices("Connected drives", commands: self.mountedVolumes.volumes.map { volume in
        .init(id: volume.folder.rawValue, title: volume.name, symbol: "externaldrive", action: { [weak self] in
          self?.palette.finish { self?.sidebar.navigate?(.folder(volume.folder)) }
        })
      })
    }))
    add("path", "Enter a folder path…", aliases: "go location", shortcut: "⌘L", symbol: "folder") { $0.editPath() }
    if tab.historyIndex > 0 { add("back", "Go back", symbol: "arrow.left") { $0.historyStep(-1) } }
    if tab.historyIndex + 1 < tab.history.count { add("forward", "Go forward", symbol: "arrow.right") { $0.historyStep(1) } }
    if let folder, folder.rawValue != "/" {
      add("parent", "Go to parent folder", aliases: "up", symbol: "arrow.up") { $0.up() }
    }
    if let file {
      add("open", "Open \(file.name)", shortcut: "↵", symbol: "arrow.up.forward.square") {
        if let folder = file.kind.folder { $0.navigate(.folder(folder)) } else { $0.openFile(file.url) }
      }
      if file.kind.folder == nil {
        commands.append(.init(id: "openWith", title: "Open \(file.name) with…", aliases: "open with editor", symbol: "doc", action: { [weak self] in
          self?.showEditorCommands(for: file.url, recordRecent: true)
        }))
        add("preview", "Quick Look \(file.name)", aliases: "preview", shortcut: "Space", symbol: "eye") { $0.showPreview(file) }
      }
      add("copyPath", "Copy path to \(file.name)", aliases: "absolute clipboard", symbol: "doc.on.doc") { $0.copyCommandPath(file.url.path) }
      if let folder {
        let base = folder.url.standardizedFileURL.pathComponents
        let target = file.url.standardizedFileURL.pathComponents
        let shared = zip(base, target).prefix { $0 == $1 }.count
        let parts = Array(repeating: "..", count: base.count - shared) + target.dropFirst(shared)
        let relative = parts.isEmpty ? "." : parts.joined(separator: "/")
        add("relativePath", "Copy relative path to \(file.name)", aliases: "clipboard", symbol: "doc.on.doc") { $0.copyCommandPath(relative) }
      }
      add("reveal", "Reveal \(file.name) in Finder", aliases: "show file", symbol: "folder") { _ in
        NSWorkspace.shared.activateFileViewerSelecting([file.url])
      }
    }
    add("newTab", "New tab", shortcut: "⌘T", symbol: "plus") { $0.newTab() }
    add("closeTab", "Close tab", shortcut: "⌘W", symbol: "xmark") {
      if $0.tabs.count > 1 { $0.closeTab($0.activeTab) } else { $0.window?.close() }
    }
    if tabs.count > 1 {
      add("nextTab", "Next tab", shortcut: "⌃Tab", symbol: "arrow.right") { $0.cycleTab(1) }
      add("previousTab", "Previous tab", shortcut: "⌃⇧Tab", symbol: "arrow.left") { $0.cycleTab(-1) }
    }
    add("sidebar", "Toggle sidebar", aliases: "hide show", shortcut: "⌘B", symbol: "sidebar.left") { $0.toggleSidebar() }
    if mode != .columns { add("columns", "Switch to Columns", aliases: "view", symbol: "rectangle.split.3x1") { $0.mode = .columns; $0.render() } }
    if mode != .list { add("funnel", "Switch to Scope funnel", aliases: "view search", symbol: "line.3.horizontal.decrease") { $0.mode = .list; $0.render() } }
    if let folder {
      add("refresh", "Refresh folder", aliases: "reload", shortcut: "⌘R", symbol: "arrow.clockwise") { $0.refreshFolder() }
      if library.pinned != folder {
        add("pin", "Pin \(folder.url.lastPathComponent)", aliases: "favorite current folder", symbol: "pin") { $0.library.pin(folder); $0.render() }
      }
      add("terminal", "Open current folder in Terminal", aliases: "shell console", symbol: "terminal") { workspace in
        guard let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else {
          workspace.toast("Terminal is unavailable."); return
        }
        workspace.openCommandURL(folder.url, application: terminal)
      }
      commands.append(.init(id: "folderEditor", title: "Open current folder in editor…", aliases: "code project", symbol: "chevron.left.forwardslash.chevron.right", action: { [weak self] in
        self?.showEditorCommands(for: folder.url, recordRecent: false)
      }))
    }
    add("choosePin", "Change pinned folder…", aliases: "favorite", symbol: "pin") { $0.choosePinnedFolder() }
    commands.append(.init(id: "theme", title: "Change theme…", aliases: "skin appearance color dark light", symbol: "paintpalette", action: { [weak self] in self?.palette.showThemes() }))
    for (id, title, location, symbol) in [
      ("airdrop", "AirDrop", Location.airDrop, "airplay.audio"),
      ("activity", "Activity Monitor", .activity, "waveform.path.ecg"),
      ("system", "System Settings", .systemSettings, "gearshape"),
      ("preferences", "Crawler Settings", .preferences, "slider.horizontal.3"),
      ("trash", "Trash", .trash, "trash")
    ] {
      add(id, title, aliases: "utilities settings", symbol: symbol) { $0.navigate(location) }
    }
    add("help", "Keyboard shortcuts", aliases: "help keys", shortcut: "⌘⇧/", symbol: "questionmark.circle") { $0.showHelp() }
    palette.commands = commands
  }

  private func copyCommandPath(_ path: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(path, forType: .string)
    toast("Path copied.")
  }

  private func showEditorCommands(for url: URL, recordRecent: Bool) {
    let supported = NSWorkspace.shared.urlsForApplications(toOpen: url)
    let editors = FileOpening.shared.editors
    let applications = recordRecent ? supported : supported.filter { app in
      editors.contains { $0.url.standardizedFileURL == app.standardizedFileURL }
    }
    let choices: [CommandPalette.Command] = applications.map { application in
      let name = editors.first { $0.url.standardizedFileURL == application.standardizedFileURL }?.name
        ?? application.deletingPathExtension().lastPathComponent
      return .init(id: application.absoluteString, title: name, symbol: "app", action: { [weak self] in
        self?.palette.finish { self?.openCommandURL(url, application: application, recordRecent: recordRecent) }
      })
    }.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
    palette.showChoices(recordRecent ? "Open with" : "Choose editor", commands: choices)
  }

  private func openCommandURL(_ url: URL, application: URL, recordRecent: Bool = false) {
    Task { [weak self] in
      do {
        _ = try await NSWorkspace.shared.open([url], withApplicationAt: application, configuration: .init())
        if recordRecent { FileOpening.recordRecent(url); self?.filePalette.reloadRecents() }
      } catch { self?.toast("Could not open \(url.lastPathComponent): \(error.localizedDescription)") }
    }
  }
}
