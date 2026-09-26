import AppKit
import QuartzCore

let processEntry = ProcessInfo.processInfo.systemUptime

@MainActor
final class CrawlerApplication: NSApplication {
  override func sendEvent(_ event: NSEvent) {
    guard event.type == .keyDown else { super.sendEvent(event); return }
    let flags = event.modifierFlags
    let character = event.charactersIgnoringModifiers ?? ""
    let plain = event.characters(byApplyingModifiers: []) ?? ""
    if flags.contains(.command),
      character == "?" || (flags.contains(.shift) && plain == "/"),
      let workspace = (delegate as? AppDelegate)?.workspace,
      keyWindow === (delegate as? AppDelegate)?.window
    {
      workspace.showHelp()
      return
    }
    super.sendEvent(event)
  }
}

@MainActor
final class AppMenuItem: NSMenuItem {
  let invoke: () -> Void
  init(
    _ title: String, key: String = "", modifiers: NSEvent.ModifierFlags = .command,
    invoke: @escaping () -> Void
  ) {
    self.invoke = invoke
    super.init(title: title, action: #selector(run), keyEquivalent: key)
    target = self
    keyEquivalentModifierMask = modifiers
  }
  required init(coder: NSCoder) { fatalError("Programmatic menu") }
  @objc func run() { invoke() }
}
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
  var window: NSWindow?
  var workspace: Workspace?
  func applicationDidFinishLaunching(_ notification: Notification) {
    NSApp.setActivationPolicy(.regular)
    Design.applyAppearance()
    let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1512, height: 982)
    let size = NSSize(width: min(1440, screen.width - 70), height: min(870, screen.height - 80))
    let window = NSWindow(
      contentRect: NSRect(origin: .zero, size: size),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered, defer: false)
    window.title = "Crawler"
    window.titleVisibility = .hidden
    window.titlebarAppearsTransparent = true
    window.isMovableByWindowBackground = false
    window.minSize = NSSize(width: 1050, height: 680)
    window.isReleasedWhenClosed = false
    window.delegate = self
    window.backgroundColor = Design.background
    let initialFolder = argument("--folder").map { FolderID(($0 as NSString).expandingTildeInPath) } ?? .home
    let root = Workspace(frame: NSRect(origin: .zero, size: size), initialFolder: initialFolder, searchRoot: argument("--search-root").map { FolderID($0) } ?? .home, initialLocation: CommandLine.arguments.contains("--settings") ? .systemSettings : CommandLine.arguments.contains("--activity") ? .activity : nil)
    self.workspace = root
    self.window = window
    window.contentView = root
    root.autoresizingMask = [.width, .height]
    makeMenu(root)
    root.prepare()
    window.center()
    for (index, type) in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton]
      .enumerated()
    {
      if let b = window.standardWindowButton(type) {
        b.setFrameOrigin(
          NSPoint(x: 15 + CGFloat(index) * 20, y: (b.superview?.bounds.height ?? 46) - 29))
      }
    }
    window.makeKeyAndOrderFront(nil)
    window.makeFirstResponder(root)
    NSApp.activate(ignoringOtherApps: true)
    window.displayIfNeeded()
    CATransaction.flush()
    let displaySubmitted = ProcessInfo.processInfo.systemUptime - processEntry
    DispatchQueue.main.async { [weak self] in
      guard let self, let window = self.window else { return }
      window.displayIfNeeded()
      CATransaction.flush()
      self.reportLaunch(displaySubmitted: displaySubmitted)
      if let query = self.argument("--find") {
        self.workspace?.showFileSearch(query: query,
          root: self.argument("--search-root").map { FolderID($0) } ?? .home)
      }
      if let path = self.argument("--snapshot") {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.saveSnapshot(path) }
      }
    }
    if let delay = argument("--quit-after").flatMap(Double.init) {
      DispatchQueue.main.asyncAfter(deadline: .now() + delay) { NSApp.terminate(nil) }
    }
  }
  func argument(_ name: String) -> String? {
    guard let i = CommandLine.arguments.firstIndex(of: name),
      CommandLine.arguments.indices.contains(i + 1)
    else { return nil }
    return CommandLine.arguments[i + 1]
  }
  func reportLaunch(displaySubmitted: TimeInterval) {
    let values: [String: Any] = [
      "pid": ProcessInfo.processInfo.processIdentifier,
      "entry_to_display_submit_ms": displaySubmitted * 1000,
      "entry_to_next_runloop_ms": (ProcessInfo.processInfo.systemUptime - processEntry) * 1000,
      "ready_epoch_seconds": Date().timeIntervalSince1970,
      "window_visible": window?.isVisible ?? false,
      "window_frame": window.map { NSStringFromRect($0.frame) } ?? "",
      "screen_visible": NSScreen.main.map { NSStringFromRect($0.visibleFrame) } ?? "",
      "configuration": "release build recommended",
      "note":
        "App-side readiness; excludes process creation and is not a compositor first-frame measurement.",
    ]
    if let path = argument("--launch-report"),
      let data = try? JSONSerialization.data(
        withJSONObject: values, options: [.prettyPrinted, .sortedKeys])
    {
      try? data.write(to: URL(fileURLWithPath: path), options: .atomic)
    }
    if let path = argument("--state-report") {
      workspace?.onChange = { [weak self] in self?.reportState(path) }
      reportState(path)
    }
  }
  func reportState(_ path: String) {
    guard let root = workspace else { return }
    let state: [String: Any] = [
      "activity_monitor": root.utility.activity.diagnostics,
      "location": root.library.title(root.tab.location),
      "path": root.currentFolder.map { root.library.path($0) } ?? "", "view": root.mode.rawValue,
      "query": root.tab.query, "scope": root.filter.scope.rawValue,
      "search_gathering": root.fileSearch.gathering,
      "selection": root.selected?.name ?? "", "sidebar_collapsed": root.sidebarCollapsed,
      "command_held": root.commandHeld, "pending_count": root.moving?.ids.count ?? 0,
      "marked_count": root.marked.count,
      "shortcuts_visible": root.shortcuts.isPresented,
      "palette_visible": root.palette.isPresented,
      "file_search_engine": "fff",
      "file_search_engine_revision": FFFSearch.revision,
      "file_search_visible": root.filePalette.isPresented,
      "file_search_searching": root.filePalette.search.searching,
      "file_search_query": root.filePalette.search.query,
      "file_search_status": root.filePalette.search.status,
      "file_search_indexed": root.filePalette.search.indexedCount,
      "file_search_matches": root.filePalette.search.response.total,
      "file_search_first_result_ms": root.filePalette.search.firstResultMilliseconds,
      "file_search_scope": root.filePalette.search.activeSearchScope?.rawValue ?? "",
      "file_search_scope_counts": root.filePalette.search.response.scopes,
      "file_search_preview_state": root.filePalette.preview.document.state,
      "video_seconds": root.filePalette.preview.document.videoSeconds ?? -1,
      "preview_load_ms": root.filePalette.preview.document.loadMilliseconds ?? -1,
      "pdf_reader_visible": root.pdfReader != nil,
      "pdf_reader_path": root.pdfReader?.document.pdf?.annotations.url.path ?? root.pdfReader?.file.id.rawValue ?? "",
      "pdf_reader_state": root.pdfReader?.document.state ?? "closed",
      "pdf_reader_load_ms": root.pdfReader?.document.loadMilliseconds ?? -1,
      "pdf_reader_reused_preview": root.pdfReader?.document.reusedPDF ?? false,
      "pdf_reader_frame": root.pdfReader.map { NSStringFromRect($0.frame) } ?? "",
      "pdf_reader_rendering": root.pdfReader?.document.pdfDiagnostics ?? [:],
      "pdf_reader_annotations": root.pdfReader?.document.pdf?.annotationDiagnostics ?? [:],
      "file_search_preview_path": root.filePalette.selectedFile?.id.rawValue ?? "",
      "file_search_root": root.filePalette.search.resultRoot?.rawValue ?? "",
      "file_search_mode": root.filePalette.search.mode == .computer ? "computer" : "folder",
      "report_uptime": ProcessInfo.processInfo.systemUptime,
      "file_search_ms": root.filePalette.search.response.milliseconds,
      "file_search_performance": root.filePalette.search.performance.diagnostics,
      "file_search_query_cache": ["entries": root.filePalette.search.queryCacheCount,
        "hits": root.filePalette.search.queryCacheHits, "misses": root.filePalette.search.queryCacheMisses],
      "file_search_workers": QuickFileSearch.workerLimit,
      "file_search_index_ms": root.filePalette.search.indexMilliseconds,
      "file_search_displayed_count": root.filePalette.displayedHits.count,
      "file_search_results": root.filePalette.search.response.hits.map(\.entry.path),
      "file_search_displayed_results": root.filePalette.displayedHits.map(\.entry.path),
      "file_search_selected": root.filePalette.selectedHit?.entry.path ?? "",
      "active_tab": root.activeTab,
      "tabs": root.tabs.map { root.library.title($0.location) },
      "ui_font": Design.font(13).fontName,
      "pane_widths": root.paneViews.map { $0.frame.width },
      "preview_width": root.previewPane.frame.width,
      "items": root.items().map(\.name), "theme": Design.theme.rawValue,
      "active_pane": root.paneViews.firstIndex { $0.active && !$0.isHidden } ?? -1,
      "preview_visible": !root.previewPane.isHidden,
    ]
    if let data = try? JSONSerialization.data(
      withJSONObject: state, options: [.prettyPrinted, .sortedKeys])
    {
      try? data.write(to: URL(fileURLWithPath: path), options: .atomic)
    }
    if let log = argument("--event-log"),
      let data = try? JSONSerialization.data(withJSONObject: state, options: .sortedKeys),
      let handle = try? FileHandle(forWritingTo: URL(fileURLWithPath: log))
    {
      defer { try? handle.close() }
      _ = try? handle.seekToEnd()
      try? handle.write(contentsOf: data)
      try? handle.write(contentsOf: Data([10]))
    }
  }
  func saveSnapshot(_ path: String) {
    guard let view = window?.contentView,
      let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
    else { return }
    view.cacheDisplay(in: view.bounds, to: rep)
    if let data = rep.representation(using: .png, properties: [:]) {
      try? data.write(to: URL(fileURLWithPath: path))
    }
  }
  func windowDidResignKey(_ notification: Notification) { workspace?.releaseCommand() }
  func applicationDidResignActive(_ notification: Notification) {
    workspace?.releaseCommand()
  }
  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
  func windowShouldClose(_ sender: NSWindow) -> Bool {
    guard let reader = workspace?.pdfReader,
      reader.document.pdf?.annotations.isDirty == true || reader.document.pdf?.annotations.isBusy == true else { return true }
    reader.confirmClose { sender.performClose(nil) }
    return false
  }
  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    guard let reader = workspace?.pdfReader,
      reader.document.pdf?.annotations.isDirty == true || reader.document.pdf?.annotations.isBusy == true else { return .terminateNow }
    reader.confirmClose(completed: { sender.reply(toApplicationShouldTerminate: true) },
      cancelled: { sender.reply(toApplicationShouldTerminate: false) })
    return .terminateLater
  }
  func makeMenu(_ root: Workspace) {
    let main = NSMenu()
    func menu(_ title: String) -> NSMenu {
      let item = NSMenuItem()
      item.title = title
      let menu = NSMenu(title: title)
      item.submenu = menu
      main.addItem(item)
      return menu
    }
    let app = menu("Crawler")
    app.addItem(
      AppMenuItem("About Crawler") { [weak root] in
        root?.message(
          "Crawler",
          "Native file browsing and AirDrop sharing in Swift and AppKit. Utilities and file changes are work in progress."
        )
      })
    app.addItem(.separator())
    app.addItem(AppMenuItem("Settings…", key: ",") { [weak root] in root?.navigate(.preferences) })
    app.addItem(.separator())
    app.addItem(AppMenuItem("Hide Crawler", key: "h") { NSApp.hide(nil) })
    app.addItem(AppMenuItem("Quit Crawler", key: "q") { NSApp.terminate(nil) })
    let file = menu("File")
    file.addItem(AppMenuItem("New tab", key: "t") { [weak root] in root?.newTab() })
    file.addItem(AppMenuItem("Find files…", key: "p") { [weak root] in root?.showFileSearch() })
    file.addItem(AppMenuItem("Save PDF", key: "s") { [weak root] in root?.pdfReader?.save() })
    file.addItem(AppMenuItem("Save PDF as…", key: "s", modifiers: [.command, .shift]) { [weak root] in root?.pdfReader?.save(asCopy: true) })
    file.addItem(
      AppMenuItem("New folder", key: "N") { [weak root] in root?.nameDialog(rename: false) })
    file.addItem(AppMenuItem("Move selected…") { [weak root] in root?.startMove() })
    file.addItem(AppMenuItem("Rename…") { [weak root] in root?.nameDialog(rename: true) })
    file.addItem(AppMenuItem("Share…") { [weak root] in root?.showShare() })
    file.addItem(AppMenuItem("Move to Trash") { [weak root] in root?.trashSelection() })
    let edit = menu("Edit")
    edit.addItem(AppMenuItem("Undo", key: "z") { [weak root] in
      if let text = NSApp.keyWindow?.firstResponder as? NSTextView, text.isEditable { text.undoManager?.undo() }
      else if let reader = root?.pdfReader { reader.undoAnnotation() }
      else { root?.undoLast() }
    })
    edit.addItem(AppMenuItem("Redo", key: "z", modifiers: [.command, .shift]) { [weak root] in
      if let text = NSApp.keyWindow?.firstResponder as? NSTextView, text.isEditable { text.undoManager?.redo() }
      else { root?.pdfReader?.redoAnnotation() }
    })
    edit.addItem(NSMenuItem(title: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x"))
    edit.addItem(NSMenuItem(title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c"))
    edit.addItem(
      NSMenuItem(title: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v"))
    edit.addItem(
      NSMenuItem(title: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a"))
    let view = menu("View")
    view.addItem(
      AppMenuItem("Columns") { [weak root] in
        root?.mode = .columns
        root?.render()
      })
    view.addItem(
      AppMenuItem("Scope funnel") { [weak root] in
        root?.mode = .list
        root?.render()
      })
    view.addItem(AppMenuItem("Toggle sidebar", key: "b") { [weak root] in root?.toggleSidebar() })
    view.addItem(.separator())
    for theme in Theme.allCases {
      view.addItem(
        AppMenuItem(theme.rawValue) { [weak root] in
          root?.changeTheme(theme)
        })
    }
    let help = menu("Help")
    help.addItem(
      AppMenuItem("Keyboard shortcuts", key: "/", modifiers: [.command, .shift]) { [weak root] in
        root?.showHelp()
      })
    NSApp.mainMenu = main
  }
}
if let index = CommandLine.arguments.firstIndex(of: "--benchmark-search-reload"),
  CommandLine.arguments.indices.contains(index + 1) {
  let query = CommandLine.arguments[index + 1]
  let cacheURL = CommandLine.arguments.firstIndex(of: "--benchmark-cache").flatMap { index in
    CommandLine.arguments.indices.contains(index + 1) ? URL(fileURLWithPath: CommandLine.arguments[index + 1]) : nil
  }
  Task { @MainActor in
    do {
      let search = QuickFileSearch()
      let measurement = try await search.measureCacheReload(query: query, cacheURL: cacheURL)
      withExtendedLifetime(search) {}
      let encoder = JSONEncoder()
      encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
      let data = try encoder.encode(measurement)
      print(String(decoding: data, as: UTF8.self))
      exit(measurement.identicalResults ? 0 : 1)
    } catch {
      fputs("Search reload benchmark failed: \(error)\n", stderr)
      exit(1)
    }
  }
  dispatchMain()
}
let app = CrawlerApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
