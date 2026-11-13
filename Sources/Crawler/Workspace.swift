import AppKit
import QuartzCore

/// The main window: the section bar on top and one section below it. Search and Folders
/// share the scope funnel, each with its own saved funnel state; Activity, Transfers and
/// Settings own their views. Readers cover the whole workspace while open.
@MainActor
final class Workspace: Surface {
  let library = FileSystemLibrary()
  let mountedVolumes = MountedVolumes()
  let sectionBar = SectionBar(frame: .zero)
  let placesBar = PlacesBar(frame: .zero)
  let palette = CommandPalette(frame: .zero)
  let filePalette = FileSearchPalette(frame: .zero)
  let shortcuts = ShortcutSheet(frame: .zero)
  let activity = ActivityMonitorView(frame: .zero)
  let airDrop = AirDropView(frame: .zero)
  let settings = SettingsView(frame: .zero)
  let copies = CopiesView(frame: .zero)
  let statusBar = StatusBar(frame: .zero)
  let toastView = Surface(), toastLabel = label("", size: 11)
  var toastTimer: Timer?
  /// The running copy and its destination name; one copy runs at a time.
  var copying: (progress: FileCopyProgress, place: String)?
  /// This session's finished copies, newest first.
  var copyRecords: [CopyRecord] = []
  var previewWindow: NSPanel?
  /// The full-window PDF or Markdown reader covering the workspace, if one is open.
  var reader: (any WorkspaceReader)?
  var pdfReader: PDFReader? { reader as? PDFReader }
  var markdownReader: MarkdownReader? { reader as? MarkdownReader }
  private(set) var section: Section
  /// The folder Folders shows, with its Back and Forward history.
  private(set) var folder: FolderID
  private var history: [FolderID]
  private var historyIndex = 0
  /// Funnel state for Search and Folders while the other section, or a utility, is visible.
  private var sessions: [Section: ScopeSession] = [:]
  /// The section whose state the funnel currently shows, if any.
  private var funnelSection: Section?
  private let searchRoot: FolderID
  private(set) var isEditingPath = false
  private var lastTheme = Design.theme
  private var monitor: Any?
  private var clickMonitor: Any?
  var onChange: (() -> Void)?

  /// The file selected in Search or Folders.
  var selected: FileItem? { section == .search || section == .folders ? filePalette.selectedFile : nil }
  /// The folder shown in Folders; nil elsewhere.
  var currentFolder: FolderID? { section == .folders ? folder : nil }
  var canGoBack: Bool { historyIndex > 0 }
  var canGoForward: Bool { historyIndex + 1 < history.count }

  init(frame: NSRect, section: Section = .search, folder: FolderID = .home, searchRoot: FolderID = .home) {
    self.section = section
    self.folder = folder
    self.searchRoot = searchRoot
    history = [folder]
    super.init(frame: frame)
    for view in [sectionBar, placesBar, filePalette, activity, airDrop, copies, settings, statusBar, toastView] { addSubview(view) }
    statusBar.showTransfers = { [weak self] in self?.show(.transfers) }
    sectionBar.choose = { [weak self] in self?.show($0) }
    sectionBar.showCommands = { [weak self] in self?.showCommandPalette() }
    sectionBar.showHelp = { [weak self] in self?.showHelp() }
    sectionBar.openButton.open = { [weak self] in self?.filePalette.openSelection() }
    placesBar.navigate = { [weak self] in self?.openFolder($0) }
    placesBar.step = { [weak self] in self?.historyStep($0) }
    placesBar.refresh = { [weak self] in self?.refreshFolder() }
    placesBar.pin = { [weak self] in
      guard let self else { return }
      self.library.pin(self.folder)
      self.render()
    }
    placesBar.drop = { [weak self] urls, folder in self?.copy(urls, to: folder) }
    placesBar.eject = { [weak self] volume in
      Task { [weak self] in
        let error = await Task.detached(priority: .userInitiated) { () -> String? in
          do { try NSWorkspace.shared.unmountAndEjectDevice(at: volume.folder.url); return nil }
          catch { return error.localizedDescription }
        }.value
        if let error { self?.toast(error) }
      }
    }
    placesBar.pathEditor.target = self
    placesBar.pathEditor.action = #selector(commitPath)
    mountedVolumes.didChange = { [weak self] previous in
      guard let self else { return }
      if self.filePalette.search.root != nil {
        self.filePalette.search.volumesChanged()
        self.filePalette.refresh()
      }
      if let removed = previous.first(where: { old in
        !self.mountedVolumes.volumes.contains(where: { $0.folder == old.folder }) && old.contains(self.folder)
      }) { self.toast("\(removed.name) was disconnected.") }
      self.render()
    }
    shortcuts.isHidden = true
    shortcuts.dismiss = { [weak self] in self?.hideHelp() }
    addSubview(shortcuts)
    palette.isHidden = true
    palette.dismiss = { [weak self] in
      guard let self else { return }
      self.palette.transitionVisible(false)
      self.focusSection()
      self.onChange?()
    }
    palette.previewTheme = { [weak self] theme in
      guard let self, Design.theme != theme else { return }
      self.changeTheme(theme, persist: false)
    }
    palette.confirmTheme = { [weak self] theme in self?.changeTheme(theme) }
    addSubview(palette)
    filePalette.isHidden = true
    filePalette.embedded = true
    filePalette.didChange = { [weak self] in
      self?.updateStatus()
      self?.onChange?()
    }
    filePalette.showPreview = { [weak self] file in self?.showPreview(file) }
    filePalette.selectionChanged = { [weak self] file in
      self?.sectionBar.openButton.configure(file)
      self?.onChange?()
    }
    filePalette.browseFolder = { [weak self] folder in self?.openFolder(folder) }
    filePalette.openRecent = { [weak self] file, reveal in
      guard let self else { return }
      if reveal { self.openFolder(file.parent, revealing: (file.url, false)) } else { self.openFile(file.url) }
    }
    filePalette.activate = { [weak self] hit, reveal in self?.activateSearchHit(hit, reveal: reveal) }
    activity.didChange = { [weak self] in self?.onChange?() }
    settings.chooseTheme = { [weak self] in self?.changeTheme($0) }
    FileOpening.shared.didChange = { [weak self] in
      guard let self else { return }
      self.sectionBar.openButton.configure(self.selected)
      self.settings.updateEditors()
      self.onChange?()
    }
    FileOpening.shared.refresh()
    toastView.wantsLayer = true
    toastView.layer?.cornerRadius = 7
    toastView.isHidden = true
    toastView.addSubview(toastLabel)
    monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
      guard let self, event.window === self.window, self.window?.attachedSheet == nil else { return event }
      return self.handleKey(event) ? nil : event
    }
    clickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
      guard let self, event.window === self.window else { return event }
      if let reader = self.reader { reader.cancelEscape(); return event }
      if self.palette.isPresented && !self.palette.frame.contains(self.convert(event.locationInWindow, from: nil)) {
        self.palette.close()
      }
      return event
    }
  }
  required init?(coder: NSCoder) { nil }
  override var acceptsFirstResponder: Bool { true }

  func prepare() {
    render()
    layoutSubtreeIfNeeded()
  }

  // MARK: Sections

  /// Show `target`, keeping the funnel state of the section being left.
  func show(_ target: Section) {
    if palette.isPresented { palette.close() }
    isEditingPath = false
    if target != section {
      section = target
      render()
    }
    focusSection()
  }
  /// Gives keyboard focus to the part of the section that typing should reach.
  func focusSection() {
    if section == .search { filePalette.focusInput() } else { window?.makeFirstResponder(self) }
  }
  func render() {
    let themeChanged = lastTheme != Design.theme
    lastTheme = Design.theme
    fill = Design.background
    sectionBar.configure(section: section, selectedFile: selected)
    placesBar.isHidden = section != .folders
    if section == .folders {
      placesBar.configure(folder: folder, canGoBack: canGoBack, canGoForward: canGoForward,
        pinned: library.pinned, volumes: mountedVolumes.volumes, editingPath: isEditingPath)
    }
    showFunnel()
    if themeChanged { filePalette.refreshTheme() }
    activity.isHidden = section != .activity
    activity.setActive(section == .activity)
    airDrop.isHidden = section != .transfers
    copies.isHidden = section != .transfers
    if section == .transfers { airDrop.refresh() }
    settings.isHidden = section != .settings
    updateStatus()
    needsLayout = true
    layoutSubtreeIfNeeded()
    if section == .settings { settings.render() }
    if section == .transfers { copies.configure(running: copying, records: copyRecords) }
    onChange?()
  }
  /// The status bar's left side: the section's status and key hints.
  private func updateStatus() {
    let text: String
    switch section {
    case .search, .folders: text = [filePalette.statusLine, filePalette.hintLine].filter { !$0.isEmpty }.joined(separator: "      ")
    case .activity: text = activity.hints
    case .transfers, .settings: text = ""
    }
    statusBar.configure(text: text)
  }
  /// Points the shared funnel at the current section's state: Search starts at Home, Folders
  /// browses `folder`. Leaving both suspends it so its expensive work stops.
  private func showFunnel() {
    guard section == .search || section == .folders else {
      if funnelSection != nil {
        saveFunnel()
        funnelSection = nil
        filePalette.suspend()
      }
      return
    }
    if funnelSection == section {
      if !filePalette.isPresented { filePalette.resume() }
      return
    }
    saveFunnel()
    funnelSection = section
    if let session = sessions[section] { filePalette.restore(session) }
    else if section == .search { filePalette.present(in: searchRoot, focus: false) }
    else { browse() }
  }
  private func saveFunnel() {
    guard let current = funnelSection, let session = filePalette.session else { return }
    sessions[current] = session
  }
  /// Browses `folder` in the funnel; a connected drive scopes its searches to that drive.
  private func browse() {
    if let volume = mountedVolumes.containing(folder) { filePalette.search.setVolumeRoot(volume.folder) }
    else if filePalette.search.volumeRoot != nil { filePalette.search.setVolumeRoot(nil) }
    filePalette.browse(folder)
  }

  // MARK: Folders

  /// Shows `target` in Folders and records it for Back and Forward unless `record` is false.
  /// `revealing` selects a file or folder inside `target`.
  func openFolder(_ target: FolderID, record: Bool = true, revealing: (url: URL, isFolder: Bool)? = nil) {
    isEditingPath = false
    if record && target != folder {
      history = Array(history.prefix(historyIndex + 1)) + [target]
      historyIndex = history.count - 1
    }
    folder = target
    sessions[.folders] = nil
    if funnelSection == .folders { browse() }
    section = .folders
    render()
    if let revealing { filePalette.selectPath(revealing.url, isFolder: revealing.isFolder) }
    window?.makeFirstResponder(self)
  }
  func historyStep(_ delta: Int) {
    let index = historyIndex + delta
    guard history.indices.contains(index) else { return }
    historyIndex = index
    openFolder(history[index], record: false)
  }
  func up() {
    if let parent = library.parent(folder) { openFolder(parent) }
  }
  func refreshFolder() { filePalette.refresh() }
  func editPath() {
    if section != .folders { show(.folders) }
    isEditingPath = true
    placesBar.pathEditor.stringValue = folder.rawValue
    render()
    window?.makeFirstResponder(placesBar.pathEditor)
    placesBar.pathEditor.selectText(nil)
  }
  @objc func commitPath() {
    guard isEditingPath else { return }
    let input = placesBar.pathEditor.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    let path = (input as NSString).expandingTildeInPath
    guard path.hasPrefix("/") else { toast("Enter an absolute folder path or a path beginning with ~."); return }
    openFolder(FolderID(path))
  }
  private func cancelPathEdit() {
    isEditingPath = false
    render()
    window?.makeFirstResponder(self)
  }
  func choosePinnedFolder() {
    guard let window else { return }
    let panel = NSOpenPanel()
    panel.title = "Choose your pinned folder"
    panel.prompt = "Pin folder"
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = false
    panel.directoryURL = folder.url
    panel.beginSheetModal(for: window) { [weak self] response in
      guard response == .OK, let url = panel.url, let self else { return }
      let folder = FolderID(url.path)
      self.library.pin(folder)
      self.openFolder(folder)
    }
  }

  // MARK: Opening

  func openFile(_ url: URL, succeeded: (() -> Void)? = nil) {
    switch PreviewFormat.forURL(url) {
    case .pdf: showPDF(url, succeeded: succeeded); return
    case .markdown, .document: showMarkdown(url, succeeded: succeeded); return
    case .text, .video, .thumbnail: break
    }
    FileOpening.shared.open(url) { [weak self] outcome in
      switch outcome {
      case .opened(let notice):
        succeeded?()
        self?.filePalette.reloadRecents()
        if let notice { self?.toast(notice) }
      case .failed(let reason): self?.toast(reason)
      }
    }
  }
  /// Shows Search, optionally starting a query in `root`.
  func showFileSearch(query: String = "", root: FolderID? = nil) {
    guard window?.attachedSheet == nil else { return }
    show(.search)
    if !query.isEmpty || root != nil { filePalette.present(in: root ?? searchRoot, query: query) }
  }
  private func activateSearchHit(_ hit: SearchHit, reveal: Bool) {
    guard let root = filePalette.search.resultRoot else { return }
    let url = root.url.appendingPathComponent(hit.entry.path)
    if reveal || hit.entry.isFolder {
      filePalette.search.recordOpen(hit)
      if reveal { openFolder(FolderID(url.deletingLastPathComponent().path), revealing: (url, hit.entry.isFolder)) }
      else { openFolder(FolderID(url.path)) }
    } else {
      openFile(url) { [weak self] in
        guard let self, self.filePalette.search.resultRoot == root else { return }
        self.filePalette.search.recordOpen(hit)
      }
    }
  }

  // MARK: Overlays

  func toast(_ text: String) {
    toastTimer?.invalidate()
    toastLabel.stringValue = text
    toastLabel.textColor = .white
    toastView.fill = Design.color(0x303747)
    toastView.isHidden = false
    addSubview(toastView, positioned: .above, relativeTo: nil)
    needsLayout = true
    toastTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: false) { [weak self] _ in
      MainActor.assumeIsolated { self?.toastView.isHidden = true }
    }
  }
  func changeTheme(_ theme: Theme, persist: Bool = true) {
    if persist { UserDefaults.standard.set(theme.rawValue, forKey: "theme") }
    guard theme != Design.theme else { return }
    wantsLayer = true
    let transition = CATransition()
    transition.type = .fade
    transition.duration = 0.14
    transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
    layer?.add(transition, forKey: "theme-crossfade")
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    Design.theme = theme
    Design.applyAppearance()
    render()
    if shortcuts.isPresented { shortcuts.refreshTheme(activity: section == .activity) }
    if palette.isPresented { palette.refresh() }
    CATransaction.commit()
  }
  func showCommandPalette() {
    configureCommands()
    if palette.isPresented { palette.close(); return }
    shortcuts.isHidden = true
    addSubview(palette, positioned: .above, relativeTo: nil)
    layoutSubtreeIfNeeded()
    palette.present()
    onChange?()
  }
  func hideHelp() {
    shortcuts.transitionVisible(false)
    onChange?()
  }

  override func layout() {
    super.layout()
    let w = bounds.width, h = bounds.height
    let bar: CGFloat = 46
    sectionBar.frame = NSRect(x: 0, y: 0, width: w, height: bar)
    let places: CGFloat = section == .folders ? 44 : 0
    let status = StatusBar.height
    placesBar.frame = NSRect(x: 0, y: bar, width: w, height: places)
    filePalette.frame = NSRect(x: 0, y: bar + places, width: w, height: max(0, h - bar - places - status))
    let utility = NSRect(x: 0, y: bar, width: w, height: max(0, h - bar - status))
    for view in [activity, settings] as [NSView] { view.frame = utility }
    let copiesWidth = min(380, max(300, w * 0.3))
    airDrop.frame = NSRect(x: 0, y: bar, width: w - copiesWidth, height: utility.height)
    copies.frame = NSRect(x: w - copiesWidth, y: bar, width: copiesWidth, height: utility.height)
    statusBar.frame = NSRect(x: 0, y: h - status, width: w, height: status)
    let paletteWidth = min(640, w - 48)
    let paletteTop = min(100, h * 0.12)
    palette.frame = NSRect(x: (w - paletteWidth) / 2, y: paletteTop, width: paletteWidth, height: min(540, h - paletteTop - 24))
    toastView.frame = NSRect(x: max(16, w / 2 - 225), y: h - status - 58, width: 450, height: 43)
    toastLabel.frame = NSRect(x: 14, y: 13, width: 420, height: 19)
    let sheetHeight = min(shortcuts.preferredHeight, h - 76)
    shortcuts.frame = NSRect(x: max(12, w - 362), y: h - sheetHeight - 18, width: 344, height: sheetHeight)
    if let reader {
      for view in subviews where view !== reader { view.isHidden = true }
      reader.frame = bounds
      addSubview(reader, positioned: .above, relativeTo: nil)
    }
  }

  // MARK: Keyboard

  func handleKey(_ event: NSEvent) -> Bool {
    if let reader { return reader.handleKey(event) }
    let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    let command = modifiers.contains(.command)
    let plainCommand = command && modifiers.isDisjoint(with: [.shift, .option, .control])
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    let shortcutKey = event.characters(byApplyingModifiers: [])?.lowercased() ?? key
    if isEditingPath {
      if event.keyCode == 53 { cancelPathEdit(); return true }
      return false
    }
    if (plainCommand && shortcutKey == "k") || (command && modifiers.contains(.shift) && shortcutKey == "p") {
      showCommandPalette()
      return true
    }
    if plainCommand && shortcutKey == "p" { showFileSearch(); return true }
    if palette.isPresented { return palette.handleKey(event) }
    if plainCommand, let target = Section.forKey(shortcutKey) { show(target); return true }
    if shortcuts.isPresented && event.keyCode == 53 { hideHelp(); return true }
    switch section {
    case .search, .folders:
      if filePalette.handleKey(event) { return true }
      if plainCommand {
        switch shortcutKey {
        case "l": editPath(); return true
        case "[": historyStep(-1); return true
        case "]": historyStep(1); return true
        default: break
        }
      }
    case .activity:
      if activity.handleKey(event, vim: true) { return true }
    case .transfers, .settings:
      break
    }
    if plainCommand && shortcutKey == "w" { window?.performClose(nil); return true }
    return false
  }
}
