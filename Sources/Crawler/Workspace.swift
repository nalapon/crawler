import AppKit
import QuartzCore

@MainActor
final class Workspace: Surface, NSTextFieldDelegate {
  let library = FileSystemLibrary()
  let mountedVolumes = MountedVolumes()
  let fileSearch = FileSearch()
  var tabs: [WorkspaceTab]
  var activeTab = 0
  var tab: WorkspaceTab {
    get { tabs[activeTab] }
    set { tabs[activeTab] = newValue }
  }
  var mode: ViewMode = .list
  private var funnelFolder: FolderID?
  private var funnelTab: UUID?
  private var initialSearchRoot: FolderID = .home
  var marked: Set<ItemID> = []
  private var visualAnchor: ItemID?
  private var suppressSidebarPeek = false
  private var sidebarAnimationID = 0
  private var sidebarAnimating = false
  let tabStrip = TabStrip(frame: .zero)
  let palette = CommandPalette(frame: .zero)
  let filePalette = FileSearchPalette(frame: .zero)
  let shortcuts = ShortcutSheet(frame: .zero)
  var moving: MoveDraft?
  var clipboard: (ids: [ItemID], cut: Bool)?
  var filter = SearchFilter(), filterOpen = false
  var sort: SortField = .name, ascending = true
  var sidebarCollapsed = false, commandHeld = false
  var detailsVisible = true, motion = !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
  var vim = true
  var saved: [SavedSearch] = []
  let titlebar = Surface(), navigation = Surface(), commands = Surface(), page = Surface(),
    guidance = Surface(), paneHost = Surface()
  let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: 208, height: 700))
  let paneViews = [FilePane(), FilePane(), FilePane()]
  private weak var pointerNavigationPane: FilePane?
  let previewPane = PreviewPane(frame: .zero), inspector = PreviewPane(frame: .zero)
  let utility = UtilityView(frame: .zero)
  let editorOpen = EditorOpenButton(frame: .zero)
  let pathEditor = NSTextField()
  let toastView = Surface(), toastLabel = label("", size: 11)
  var toastTimer: Timer?
  let copyPanel = CopyProgressPanel()
  var copying: FileCopyProgress?
  var isEditingPath = false
  var popover: NSPopover?
  var previewWindow: NSPanel?
  /// The full-window PDF or Markdown reader covering the workspace, if one is open.
  var reader: (any WorkspaceReader)?
  var pdfReader: PDFReader? { reader as? PDFReader }
  var markdownReader: MarkdownReader? { reader as? MarkdownReader }
  private var lastTheme: Theme = .cupertino
  private var lastSidebarState = ""
  private var rowCache: [FileItem] = []
  private var monitor: Any?
  private var flagsMonitor: Any?
  private var paletteClickMonitor: Any?
  private var selectionStart: ItemID?
  private var pendingSearchReveal: (folder: FolderID, item: ItemID)?
  var onChange: (() -> Void)?
  init(frame: NSRect, initialFolder: FolderID = .home, searchRoot: FolderID = .home, initialLocation: Location? = nil) {
    tabs = [WorkspaceTab(initialLocation ?? .folder(initialFolder))]
    initialSearchRoot = searchRoot
    super.init(frame: frame)
    library.didChange = { [weak self] folder in
      guard let self else { return }
      if folder == self.currentFolder {
        if let reveal = self.pendingSearchReveal, reveal.folder == folder {
          self.tab.selected = reveal.item
          self.pendingSearchReveal = nil
        }
        self.render()
      } else if let pane = self.paneViews.first(where: { !$0.isHidden && $0.location == folder }) {
        pane.configure(title: self.library.title(.folder(folder)), location: folder,
          rows: self.displayRows(self.library.children(folder), in: folder, includePending: false),
          selected: self.currentFolder.flatMap { self.library.owner($0)?.id }, active: false)
        self.updateVisibleFolders()
      }
    }
    library.didUpdateSize = { [weak self] item in
      guard let self else { return }
      if let index = self.rowCache.firstIndex(where: { $0.id == item.id }) { self.rowCache[index] = item }
      self.paneViews.filter { !$0.isHidden }.forEach { $0.updateSize(item) }
      if self.tab.selected == item.id { self.updateSelectionDetails() }
      self.onChange?()
    }
    fileSearch.didChange = { [weak self] in
      guard let self, self.searching, self.filter.scope == .all else { return }
      self.render()
    }
    for view in [
      titlebar, navigation, commands, page, guidance, paneHost, sidebar, utility, copyPanel, toastView,
    ] { addSubview(view) }
    copyPanel.isHidden = true
    titlebar.bottomLine = true
    navigation.bottomLine = false
    commands.bottomLine = false
    for pane in paneViews {
      paneHost.addSubview(pane)
      pane.scroll.pointerScrolled = { [weak self, weak pane] in
        self?.pointerNavigationPane = pane
      }
      pane.selectionChanged = { [weak self] id, folder, flags in
        self?.select(id, in: folder, flags: flags)
      }
      pane.openItem = { [weak self] id in self?.open(id) }
      pane.sortChanged = { [weak self] field in
        guard let self else { return }
        if self.sort == field {
          self.ascending.toggle()
        } else {
          self.sort = field
          self.ascending = true
        }
        self.render()
      }
      pane.focusFolder = { [weak self] folder in self?.navigate(.folder(folder)) }
    }
    paneHost.addSubview(previewPane)
    paneHost.addSubview(inspector)
    inspector.open = { [weak self] in self?.openSelected() }
    inspector.share = { [weak self] in self?.showShare() }
    sidebar.choosePin = { [weak self] in self?.choosePinnedFolder() }
    sidebar.navigate = { [weak self] destination in
      guard let self else { return }
      if self.mountedVolumes.containing(destination.folder) != nil { self.mode = .list; self.funnelFolder = nil }
      self.navigate(destination)
    }
    sidebar.drop = { [weak self] urls, folder in self?.copy(urls, to: folder) }
    sidebar.eject = { [weak self] volume in
      Task { [weak self] in
        let error = await Task.detached(priority: .userInitiated) { () -> String? in
          do { try NSWorkspace.shared.unmountAndEjectDevice(at: volume.folder.url); return nil }
          catch { return error.localizedDescription }
        }.value
        if let error { self?.toast(error) }
      }
    }
    mountedVolumes.didChange = { [weak self] previous in
      guard let self else { return }
      if self.filePalette.search.root != nil {
        self.filePalette.search.volumesChanged()
        self.filePalette.refresh()
      }
      if let removed = previous.first(where: { old in
        !self.mountedVolumes.volumes.contains(where: { $0.folder == old.folder })
          && (self.currentFolder.map(old.contains) ?? false)
      }) { self.toast("\(removed.name) was disconnected.") }
      self.refreshSidebar(force: true)
      self.render()
    }
    sidebar.openSaved = { [weak self] index in
      guard let self, self.saved.indices.contains(index) else { return }
      let s = self.saved[index]
      self.navigate(s.location)
      self.tab.query = s.query
      self.filter = s.filter
      self.filterOpen = true
      self.render()
    }
    pathEditor.font = Design.font(11)
    pathEditor.delegate = self
    pathEditor.target = self
    pathEditor.action = #selector(commitPath)
    pathEditor.setAccessibilityLabel("Folder path")
    navigation.addSubview(pathEditor)
    pathEditor.isHidden = true
    shortcuts.isHidden = true
    shortcuts.dismiss = { [weak self] in
      self?.shortcuts.transitionVisible(false)
      self?.onChange?()
    }
    addSubview(shortcuts)
    palette.isHidden = true
    palette.dismiss = { [weak self] in
      guard let self else { return }
      self.palette.transitionVisible(false)
      self.window?.makeFirstResponder(self)
      self.onChange?()
    }
    palette.previewTheme = { [weak self] theme in
      guard let self, Design.theme != theme else { return }
      self.changeTheme(theme, persist: false)
    }
    palette.confirmTheme = { [weak self] theme in self?.changeTheme(theme) }
    addSubview(palette)
    filePalette.isHidden = true
    filePalette.dismiss = { [weak self] in
      guard let self else { return }
      self.filePalette.transitionVisible(false)
      self.window?.makeFirstResponder(self)
      self.onChange?()
    }
    filePalette.didChange = { [weak self] in self?.onChange?() }
    filePalette.showPreview = { [weak self] file in self?.showPreview(file) }
    filePalette.selectionChanged = { [weak self] file in
      guard let self, self.filePalette.embedded else { return }
      self.editorOpen.configure(file)
      self.onChange?()
    }
    filePalette.browseFolder = { [weak self] folder in self?.navigate(.folder(folder)) }
    filePalette.openRecent = { [weak self] file, reveal in
      guard let self else { return }
      if reveal { self.navigate(.folder(file.parent)); self.filePalette.selectPath(file.url, isFolder: false) }
      else {
        if !PreviewFormat.forURL(file.url).opensInReader { self.filePalette.close() }
        self.openFile(file.url)
      }
    }
    filePalette.activate = { [weak self] hit, reveal in self?.activateSearchHit(hit, reveal: reveal) }
    addSubview(filePalette)
    tabStrip.select = { [weak self] in self?.selectTab($0) }
    tabStrip.close = { [weak self] in self?.closeTab($0) }
    utility.workspace = self
    utility.activity.didChange = { [weak self] in self?.onChange?() }
    editorOpen.open = { [weak self] in self?.openSelected() }
    FileOpening.shared.didChange = { [weak self] in
      guard let self else { return }
      self.editorOpen.configure(self.selected)
      self.utility.updateEditors()
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
    paletteClickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
      guard let self, event.window === self.window else { return event }
      if let reader = self.reader { reader.cancelEscape(); return event }
      let point = self.convert(event.locationInWindow, from: nil)
      if self.palette.isPresented && !self.palette.frame.contains(point) { self.palette.close() }
      if self.filePalette.isPresented && !self.filePalette.embedded && !self.filePalette.frame.contains(point) { self.filePalette.close() }
      return event
    }
    flagsMonitor = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { [weak self] event in
      guard let self else { return event }
      let wasHeld = self.commandHeld
      self.commandHeld = event.modifierFlags.contains(.command)
      if !self.commandHeld { self.suppressSidebarPeek = false }
      else if !wasHeld && self.sidebarCollapsed {
        // Let a Command shortcut arrive before revealing the temporary sidebar.
        self.suppressSidebarPeek = true
        let generation = self.sidebarAnimationID
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
          guard let self, self.commandHeld, self.sidebarCollapsed,
            self.sidebarAnimationID == generation else { return }
          self.suppressSidebarPeek = false
          self.refreshSidebar(force: true)
          self.needsLayout = true
        }
      }
      self.refreshSidebar(force: true)
      self.needsLayout = true
      self.onChange?()
      return event
    }
  }
  required init?(coder: NSCoder) { nil }
  override var acceptsFirstResponder: Bool { true }
  func prepare() {
    render()
    needsLayout = true
    layoutSubtreeIfNeeded()
  }
  func releaseCommand() {
    commandHeld = false
    suppressSidebarPeek = false
    refreshSidebar(force: true)
    needsLayout = true
    onChange?()
  }
  var selected: FileItem? {
    if filePalette.embedded && filePalette.isPresented { return filePalette.selectedFile }
    return rowCache.first { $0.id == tab.selected } ?? library.item(tab.selected) }
  var currentFolder: FolderID? { tab.location.folder }
  var searching: Bool { !tab.query.isEmpty || filterOpen }
  var selectionIDs: [ItemID] { marked.isEmpty ? tab.selected.map { [$0] } ?? [] : Array(marked) }
  func items() -> [FileItem] {
    var rows: [FileItem]
    if searching {
      rows = filter.scope == .all ? fileSearch.rows : currentFolder.map { library.children($0) } ?? []
      let words = tab.query.lowercased().split(whereSeparator: { $0.isWhitespace })
      rows = rows.filter { item in words.allSatisfy { item.name.lowercased().contains($0) } }
      if filter.kind != "Any kind" { rows = rows.filter { $0.kind.label == filter.kind } }
      if filter.large { rows = rows.filter { $0.megabytes >= 10 } }
      if filter.thisWeek {
        let start = Calendar.current.dateInterval(of: .weekOfYear, for: Date())?.start ?? Date()
        rows = rows.filter { ($0.modified ?? .distantPast) >= start }
      }
    } else {
      switch tab.location {
      case .folder(let id): rows = library.children(id)
      case .locations, .trash, .airDrop, .activity, .systemSettings, .preferences: return []
      }
    }
    if sort == .name && ascending && currentFolder != nil && !searching { return rows }
    return rows.sorted { a, b in
      let lhs = ascending ? a : b
      let rhs = ascending ? b : a
      let less: Bool
      switch sort {
      case .name: less = DirectoryReader.nameOrder(lhs, rhs)
      case .date:
        less = lhs.modified == rhs.modified ? DirectoryReader.nameOrder(lhs, rhs) : (lhs.modified ?? .distantPast) < (rhs.modified ?? .distantPast)
      case .created:
        less = lhs.created == rhs.created ? DirectoryReader.nameOrder(lhs, rhs) : (lhs.created ?? .distantPast) < (rhs.created ?? .distantPast)
      case .kind:
        less =
          lhs.kind.label == rhs.kind.label
          ? DirectoryReader.nameOrder(lhs, rhs) : lhs.kind.label < rhs.kind.label
      case .size:
        less =
          lhs.megabytes == rhs.megabytes
          ? DirectoryReader.nameOrder(lhs, rhs) : lhs.megabytes < rhs.megabytes
      }
      return less
    }
  }
  func navigate(_ destination: Location, record: Bool = true) {
    pendingSearchReveal = nil
    rememberTabPosition()
    filePalette.close()
    pointerNavigationPane = nil
    if moving != nil && (destination.isUtility || destination == .trash)
    {
      toast("Finish or cancel the pending move first.")
      return
    }
    visualAnchor = nil
    let oldFolder = currentFolder
    if let oldFolder, !searching {
      tab.folderSelection[oldFolder] = tab.selected
      if let pane = paneViews.first(where: { !$0.isHidden && ($0.active || mode == .list) }) {
        tab.folderScroll[oldFolder] = pane.scroll.scrollOffset
      }
    }
    if moving == nil { marked.removeAll() }
    tab.location = destination
    tab.query = ""
    filterOpen = false
    tab.selected = destination.folder.flatMap { tab.folderSelection[$0] }
    isEditingPath = false
    if record {
      tab.history = Array(tab.history.prefix(tab.historyIndex + 1))
      tab.history.append(destination)
      tab.historyIndex = tab.history.count - 1
    }
    if let oldFolder, library.parent(oldFolder) == destination.folder {
      tab.selected = library.owner(oldFolder)?.id
    }
    render()
    if let folder = currentFolder,
      let pane = paneViews.first(where: { !$0.isHidden && ($0.active || mode == .list) })
    {
      pane.restoreScroll(tab.folderScroll[folder] ?? 0)
      if tab.folderScroll[folder] == nil { pane.scrollSelectionVisible() }
      if motion, oldFolder != folder {
        pane.wantsLayer = true
        let transition = CABasicAnimation(keyPath: "transform.translation.x")
        transition.fromValue = library.parent(oldFolder ?? folder) == folder ? -14 : 14
        transition.toValue = 0
        transition.duration = 0.14
        transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
        pane.layer?.add(transition, forKey: "navigation")
      }
    }
    window?.makeFirstResponder(self)
    if moving != nil, motion {
      animatePending(
        direction: destination.folder == oldFolder.flatMap { library.parent($0) } ? -1 : 1)
    }
  }
  func select(_ id: ItemID, in folder: FolderID?, flags: NSEvent.ModifierFlags = []) {
    pointerNavigationPane = nil
    if let folder, folder != currentFolder { navigate(.folder(folder)) }
    guard let f = rowCache.first(where: { $0.id == id }) ?? library.item(id) else { return }
    if moving == nil {
      if flags.contains(.command) {
        if marked.contains(id) { marked.remove(id) } else { marked.insert(id) }
      } else if flags.contains(.shift), let start = selectionStart,
        let a = rowCache.firstIndex(where: { $0.id == start }),
        let b = rowCache.firstIndex(where: { $0.id == id })
      {
        marked.formUnion(rowCache[min(a, b)...max(a, b)].map(\.id))
      } else {
        marked.removeAll()
        selectionStart = id
      }
    }
    tab.selected = f.id
    extendVisualSelection()
    refreshSelection()
  }
  func open(_ id: ItemID) {
    guard let f = rowCache.first(where: { $0.id == id }) ?? library.item(id) else { return }
    if let folder = f.kind.folder { navigate(.folder(folder)); return }
    openFile(f.url)
  }
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
  func openSelected() {
    if filePalette.embedded, (filePalette.isHome || filePalette.isBrowsing) { filePalette.openSelection() }
    else if filePalette.embedded, let hit = filePalette.selectedHit { activateSearchHit(hit, reveal: false) }
    else if let id = tab.selected { open(id) }
  }
  func up() {
    if let folder = currentFolder, let parent = library.parent(folder) { navigate(.folder(parent)) }
  }
  func startMove() { toast("Moving files is work in progress.") }
  func moveProblem() -> String? { "Moving files is work in progress." }
  func cancelMove() {
    moving = nil
    marked.removeAll()
    render()
    window?.makeFirstResponder(self)
  }
  func commitMove() { toast("Moving files is work in progress.") }
  private func animatePending(direction: Int) {
    guard let pane = paneViews.first(where: { $0.active && !$0.isHidden }) else { return }
    for (index, row) in pane.rows.enumerated()
    where row.pending || (moving == nil && row.item.id == tab.selected) {
      guard let cell = pane.table.view(atColumn: 0, row: index, makeIfNecessary: false) else {
        continue
      }
      cell.wantsLayer = true
      let animation = CABasicAnimation(keyPath: "transform.translation.x")
      animation.fromValue = direction == 0 ? -10 : direction * 18
      animation.toValue = 0
      animation.duration = 0.18
      animation.timingFunction = CAMediaTimingFunction(name: .easeOut)
      cell.layer?.add(animation, forKey: "move")
    }
  }
  func displayRows(_ files: [FileItem], in folder: FolderID?, includePending: Bool) -> [DisplayRow]
  {
    var rows = files.map { f in
      DisplayRow(
        item: f, pending: false, dimmed: moving?.ids.contains(f.id) == true,
        marked: marked.contains(f.id),
        locationText: searching
          ? library.title(.folder(f.parent))
          : f.date)
    }
    if includePending, let moving, let folder {
      rows += moving.ids.compactMap { library.item($0) }.filter { $0.parent != folder }.map {
        DisplayRow(
          item: $0, pending: true, dimmed: true, marked: false, locationText: "Pending move")
      }
    }
    return rows
  }
  func render() {
    let themeChanged = lastTheme != Design.theme
    if themeChanged {
      paneViews.forEach { $0.redrawTheme() }
      lastTheme = Design.theme
    }
    fill = Design.background
    titlebar.fill = Design.raised
    navigation.fill = Design.background
    commands.fill = Design.background
    page.fill = Design.background
    guidance.fill = Design.background
    paneHost.fill = Design.background
    fileSearch.search(searching && filter.scope == .all ? tab.query.trimmingCharacters(in: .whitespacesAndNewlines) : "")
    rowCache = items()
    if !rowCache.contains(where: { $0.id == tab.selected }) {
      tab.selected =
        (mode == .columns
        ? rowCache.first(where: { $0.kind.folder != nil }) ?? rowCache.first : rowCache.first)?.id
    }
    buildTitlebar()
    buildNavigation()
    buildCommands()
    buildPage()
    titlebar.applyMaterial(.titlebar)
    navigation.subviews.compactMap { $0 as? NSVisualEffectView }.forEach { $0.removeFromSuperview() }
    commands.subviews.compactMap { $0 as? NSVisualEffectView }.forEach { $0.removeFromSuperview() }
    refreshSidebar(force: themeChanged)
    utility.trash.setActive(tab.location == .trash)
    utility.activity.setActive(tab.location == .activity)
    utility.systemSettings.setActive(tab.location == .systemSettings)
    utility.isHidden = !tab.location.isUtility
    paneHost.isHidden = tab.location.isUtility
    page.isHidden = tab.location.isUtility
    guidance.isHidden = tab.location.isUtility
    let funnel = mode == .list && !tab.location.isUtility
    if funnel {
      let volume = mountedVolumes.containing(currentFolder)
      // Keep an open drive's scope when it disconnects, so it cannot become a Mac-wide search.
      if let volume { filePalette.search.setVolumeRoot(volume.folder) }
      else if let root = filePalette.search.volumeRoot,
        currentFolder.map({ $0 == root || $0.rawValue.hasPrefix(root.rawValue + "/") }) != true {
        filePalette.search.setVolumeRoot(nil)
      }
      let first = funnelTab == nil
      filePalette.embedded = true
      if first {
        filePalette.present(in: initialSearchRoot, focus: false)
        if initialSearchRoot == .home, let folder = currentFolder, folder != .home {
          filePalette.browse(folder)
        }
      } else if funnelTab != tab.id, let session = tab.scopeSession {
        filePalette.restore(session)
      } else {
        if !filePalette.isPresented { filePalette.resume() }
        if funnelFolder != currentFolder || funnelTab != tab.id, let folder = currentFolder {
          filePalette.browse(folder)
        }
      }
      funnelFolder = currentFolder; funnelTab = tab.id
      if themeChanged { filePalette.refreshTheme() }
      paneViews.forEach { $0.isHidden = true }
      previewPane.isHidden = true; inspector.isHidden = true
      previewPane.configure(nil); inspector.configure(nil)
      library.watch([]); library.calculateSizes(in: nil)
    } else {
      if filePalette.embedded { filePalette.suspend(); filePalette.embedded = false }
      if tab.location.isUtility {
        previewPane.configure(nil); inspector.configure(nil)
        utility.render(); library.watch([])
      } else { renderPanes() }
    }
    needsLayout = true
    layoutSubtreeIfNeeded()
    onChange?()
  }
  // Selection changes affect rows and previews, not tabs, paths, filters or toolbar structure.
  private func extendVisualSelection() {
    guard let anchor = visualAnchor,
      let start = rowCache.firstIndex(where: { $0.id == anchor }),
      let end = rowCache.firstIndex(where: { $0.id == tab.selected })
    else { return }
    marked = Set(rowCache[min(start, end)...max(start, end)].map(\.id))
  }
  private func refreshSelection() {
    guard !tab.location.isUtility else { return }
    paneViews.first { !$0.isHidden && $0.active }?.updateSelection(tab.selected, marked: marked)
    let visiblePanes = paneViews.map(\.isHidden)
    let hadPreview = !previewPane.isHidden
    updateSelectionDetails()
    updateVisibleFolders()
    if hadPreview != !previewPane.isHidden || visiblePanes != paneViews.map(\.isHidden) {
      needsLayout = true
      layoutSubtreeIfNeeded()
    }
    onChange?()
  }
  private func updateVisibleFolders() {
    if previewPane.isHidden { previewPane.configure(nil) }
    if inspector.isHidden { inspector.configure(nil) }
    guard !filePalette.embedded else { return }
    let folders = Set(paneViews.filter { !$0.isHidden }.compactMap(\.location))
    library.watch(folders)
    library.calculateSizes(in: currentFolder)
    for pane in paneViews where !pane.isHidden {
      if searching {
        pane.empty.stringValue = filter.scope == .all
          ? (tab.query.isEmpty ? "Type a filename to search this Mac." : fileSearch.gathering ? "Searching Spotlight…" : "No indexed files match. Try This folder for unindexed files.")
          : currentFolder.flatMap { library.state($0).message } ?? "No matching files"
      } else if let folder = pane.location {
        pane.empty.stringValue = library.state(folder).message ?? "Empty folder"
      }
      pane.empty.toolTip = pane.empty.stringValue
    }
  }
  private func renderPanes() {
    defer { updateVisibleFolders() }
    paneViews.forEach { $0.isHidden = true }
    previewPane.isHidden = true
    inspector.isHidden = true
    if mode == .list || searching || currentFolder == nil {
      let pane = paneViews[0]
      pane.isHidden = false
      pane.configure(
        title: searching
          ? "Name                                      Location                         Kind           Size"
          : "Name                                      Date modified                  Kind           Size",
        location: currentFolder,
        rows: displayRows(rowCache, in: currentFolder, includePending: true),
        selected: tab.selected, active: true, detailed: true,
        context: searching ? "search" : "list")
      if detailsVisible {
        inspector.isHidden = false
        inspector.configure(
          selected, inspector: true,
          location: selected.map { library.title(.folder($0.parent)) } ?? "")
      }
      return
    }
    guard let folder = currentFolder else { return }
    let parent = library.parent(folder)
    let activeIndex = parent == nil ? 0 : 1
    if let parent {
      let pane = paneViews[0]
      pane.isHidden = false
      pane.configure(
        title: library.title(.folder(parent)), location: parent,
        rows: displayRows(library.children(parent), in: parent, includePending: false),
        selected: library.owner(folder)?.id, active: false)
    }
    let current = paneViews[activeIndex]
    current.isHidden = false
    current.configure(
      title: library.title(tab.location), location: folder,
      rows: displayRows(rowCache, in: folder, includePending: true), selected: tab.selected,
      active: true)
    updateSelectionDetails()
  }
  private func updateSelectionDetails() {
    editorOpen.configure(selected)
    if mode == .list || searching || currentFolder == nil {
      if detailsVisible {
        inspector.configure(selected, inspector: true,
          location: selected.map { library.title(.folder($0.parent)) } ?? "")
      }
      return
    }
    guard let folder = currentFolder else { return }
    let activeIndex = library.parent(folder) == nil ? 0 : 1
    let child = selected?.kind.folder
    let pane = paneViews[activeIndex + 1]
    pane.isHidden = child == nil
    previewPane.isHidden = child != nil && activeIndex != 0
    if let child {
      pane.configure(
        title: library.title(.folder(child)), location: child,
        rows: displayRows(library.children(child), in: child, includePending: false), selected: nil,
        active: false)
      if activeIndex == 0 {
        previewPane.isHidden = false
        previewPane.configure(nil)
      }
    } else {
      previewPane.isHidden = false
      previewPane.configure(selected)
    }
  }
  private func refreshSidebar(force: Bool = false) {
    let visible = !sidebarCollapsed || (commandHeld && !suppressSidebarPeek)
    sidebar.isHidden = !visible && !sidebarAnimating
    let key = "\(tab.location)|\(commandHeld)|\(saved.count)|\(Design.theme)|\(String(describing: library.pinned))"
    if key != lastSidebarState || force {
      sidebar.configure(location: tab.location, commandHeld: commandHeld, saved: saved, pinned: library.pinned, volumes: mountedVolumes.volumes)
      lastSidebarState = key
    }
    if sidebarCollapsed && commandHeld && !suppressSidebarPeek {
      sidebar.wantsLayer = true
      sidebar.layer?.shadowColor = NSColor.black.cgColor
      sidebar.layer?.shadowOpacity = 0.12
      sidebar.layer?.shadowRadius = 18
      sidebar.layer?.shadowOffset = NSSize(width: 8, height: 0)
      addSubview(sidebar, positioned: .above, relativeTo: utility)
    } else {
      sidebar.layer?.shadowOpacity = 0
    }
    if let toggle = titlebar.subviews.first(where: { $0.identifier?.rawValue == "sidebar-toggle" })
      as? ActionButton
    {
      toggle.setAccessibilityValue(visible ? "expanded" : "collapsed")
    }
  }
  func toggleSidebar() {
    let views: [NSView] = [sidebar, navigation, commands, page, guidance, paneHost, utility] + (filePalette.embedded ? [filePalette] : [])
    views.forEach { $0.wantsLayer = true }
    layoutSubtreeIfNeeded()
    let origins = views.map { $0.layer?.presentation()?.position ?? $0.layer?.position ?? .zero }
    sidebarAnimationID += 1
    let generation = sidebarAnimationID
    // Sidebar movement is explicitly requested, independently of the system motion preference.
    sidebarAnimating = true
    if commandHeld { suppressSidebarPeek = true }
    sidebarCollapsed.toggle()
    let duration = sidebarCollapsed ? 0.22 : 0.28
    refreshSidebar(force: true)
    needsLayout = true
    layoutSubtreeIfNeeded()
    if sidebarAnimating {
      for (view, origin) in zip(views, origins) {
        let animation = CABasicAnimation(keyPath: "position")
        animation.fromValue = NSValue(point: origin)
        animation.toValue = NSValue(point: view.layer?.position ?? .zero)
        animation.duration = duration
        animation.timingFunction = CAMediaTimingFunction(controlPoints: 0.16, 1, 0.3, 1)
        view.layer?.add(animation, forKey: "sidebar-slide")
      }
      DispatchQueue.main.asyncAfter(deadline: .now() + duration) { [weak self] in
        guard let self, self.sidebarAnimationID == generation else { return }
        self.sidebarAnimating = false
        self.refreshSidebar()
      }
    }
    onChange?()
  }
  override func layout() {
    super.layout()
    let w = bounds.width
    let h = bounds.height
    let paletteWidth = min(640, w - 48)
    palette.frame = NSRect(x: (w - paletteWidth) / 2, y: min(100, h * 0.12), width: paletteWidth, height: min(540, h - min(100, h * 0.12) - 24))
    let searchWidth = min(1200, w - 32)
    filePalette.frame = NSRect(x: (w - searchWidth) / 2, y: 55,
      width: searchWidth, height: h - 71)
    let side: CGFloat = sidebarCollapsed ? 0 : 208
    let mainW = w - side
    titlebar.frame = NSRect(x: 0, y: 0, width: w, height: 46)
    sidebar.frame = NSRect(
      x: sidebarCollapsed && !(commandHeld && !suppressSidebarPeek) ? -208 : 0, y: 46, width: 208,
      height: h - 46)
    navigation.frame = NSRect(x: side, y: 46, width: isEditingPath ? mainW : (mode == .list ? 108 : 0), height: 56)
    commands.frame = NSRect(x: side + mainW - 410, y: 50, width: 248, height: 44)
    commands.isHidden = tab.location.isUtility
    page.frame = NSRect(x: side, y: 46, width: mainW, height: 64)
    addSubview(commands, positioned: .above, relativeTo: page)
    navigation.isHidden = mode == .columns && !isEditingPath
    if !navigation.isHidden { addSubview(navigation, positioned: .above, relativeTo: page) }
    let guideH: CGFloat = searching ? 80 : 0
    guidance.frame = NSRect(x: side, y: 110, width: mainW, height: guideH)
    let paneY: CGFloat = 110 + guideH
    paneHost.frame = NSRect(x: side, y: paneY, width: mainW, height: max(0, h - paneY - 2))
    if filePalette.embedded {
      filePalette.frame = NSRect(x: side, y: 110, width: mainW, height: max(0, h - 110))
    }
    utility.frame = NSRect(x: side, y: 102, width: mainW, height: max(0, h - 102))
    if mode == .list || searching || currentFolder == nil {
      let inspectorW: CGFloat = detailsVisible ? min(300, mainW * 0.28) : 0
      paneViews[0].frame = NSRect(
        x: 0, y: 0, width: mainW - inspectorW, height: paneHost.bounds.height)
      inspector.frame = NSRect(
        x: mainW - inspectorW, y: 0, width: inspectorW, height: paneHost.bounds.height)
    } else {
      let width = mainW / 3
      for (i, pane) in paneViews.enumerated() {
        pane.frame = NSRect(
          x: CGFloat(i) * width, y: 0, width: width, height: paneHost.bounds.height)
      }
      previewPane.frame = NSRect(
        x: 2 * width, y: 0, width: mainW - 2 * width, height: paneHost.bounds.height)
    }
    pathEditor.frame = NSRect(x: 111, y: 14, width: max(100, mainW - 140), height: 28)
    // A running copy keeps the bottom slot; toasts stack above it.
    copyPanel.frame = NSRect(x: max(side + 16, w / 2 - 225), y: h - 81, width: 450, height: 58)
    toastView.frame = NSRect(x: max(side + 16, w / 2 - 225), y: h - (copyPanel.isHidden ? 66 : 132), width: 450, height: 43)
    toastLabel.frame = NSRect(x: 14, y: 13, width: 350, height: 19)
    let sheetHeight = min(shortcuts.preferredHeight, h - 76)
    shortcuts.frame = NSRect(
      x: max(12, w - 362), y: h - sheetHeight - 18, width: 344, height: sheetHeight)
    layoutFlexibleControls()
    if let reader {
      for view in subviews where view !== reader { view.isHidden = true }
      reader.frame = bounds
      addSubview(reader, positioned: .above, relativeTo: nil)
    }
  }
  func buildTitlebar() {
    titlebar.subviews.filter { !($0 is NSVisualEffectView) }.forEach { $0.removeFromSuperview() }
    let toggle = ActionButton("", symbol: "sidebar.left", help: "Toggle sidebar · ⌘B") { [weak self] in self?.toggleSidebar() }
    toggle.identifier = NSUserInterfaceItemIdentifier("sidebar-toggle")
    toggle.frame = NSRect(x: 88, y: 10, width: 30, height: 28)
    titlebar.addSubview(toggle)
    tabStrip.configure(titles: tabs.map { library.title($0.location) }, selected: activeTab)
    titlebar.addSubview(tabStrip)
    let plus = ActionButton("", symbol: "plus", help: "New tab · ⌘T") { [weak self] in self?.newTab() }
    plus.identifier = NSUserInterfaceItemIdentifier("new-tab")
    titlebar.addSubview(plus)
    let quick = ActionButton(
      "Quick settings", symbol: "slider.horizontal.3",
      action: { [weak self] in self?.showQuickSettings() })
    quick.identifier = NSUserInterfaceItemIdentifier("quick")
    titlebar.addSubview(quick)
    let help = ActionButton(
      "", symbol: "questionmark.circle", help: "Keyboard shortcuts",
      action: { [weak self] in self?.showHelp() })
    help.identifier = NSUserInterfaceItemIdentifier("help")
    titlebar.addSubview(help)
  }
  func buildNavigation() {
    navigation.subviews.filter { $0 !== pathEditor && !($0 is NSVisualEffectView) }
      .forEach {
        $0.removeFromSuperview()
      }
    for (i, entry) in [
      ("chevron.left", "Back"), ("chevron.right", "Forward"),
    ].enumerated() {
      let b = ActionButton(
        "", symbol: entry.0, help: entry.1,
        action: { [weak self] in
          guard let self else { return }
          self.historyStep(i == 0 ? -1 : 1)
        })
      b.frame = NSRect(x: 16 + CGFloat(i) * 40, y: 10, width: 34, height: 34)
      b.image = Design.symbol(entry.0, size: 17)
      b.wantsLayer = true
      b.layer?.cornerRadius = 7
      b.layer?.backgroundColor = Design.background.withAlphaComponent(0.8).cgColor
      b.isEnabled =
        i == 0
        ? tab.historyIndex > 0
        : i == 1
          ? tab.historyIndex < tab.history.count - 1
          : currentFolder.flatMap { library.parent($0) } != nil
      b.contentTintColor = b.isEnabled ? Design.text : Design.muted.withAlphaComponent(0.45)
      b.isHidden = mode == .columns
      navigation.addSubview(b)
    }
    pathEditor.isHidden = !isEditingPath
  }

  func buildCommands() {
    commands.subviews.filter { !($0 is NSVisualEffectView) }.forEach { $0.removeFromSuperview() }
    editorOpen.frame = NSRect(x: 0, y: 8, width: 98, height: 28)
    editorOpen.configure(selected)
    commands.addSubview(editorOpen)
    let refresh = ActionButton("", symbol: "arrow.clockwise", help: "Refresh folder (⌘R)") { [weak self] in
      self?.refreshFolder()
    }
    refresh.frame = NSRect(x: 100, y: 8, width: 30, height: 28)
    refresh.isEnabled = currentFolder != nil
    commands.addSubview(refresh)
    let isPinned = currentFolder != nil && currentFolder == library.pinned
    let pin = ActionButton(isPinned ? "Pinned folder" : "Pin this folder", symbol: "pin") { [weak self] in
      guard let self, let folder = self.currentFolder else { return }
      self.library.pin(folder)
      self.render()
    }
    pin.isEnabled = currentFolder != nil && !isPinned
    pin.frame = NSRect(x: 134, y: 8, width: 112, height: 28)
    commands.addSubview(pin)
  }
  func refreshFolder() {
    if filePalette.embedded { filePalette.refresh(); return }
    if let folder = currentFolder { library.load(folder, refresh: true); render() }
  }
  func choosePinnedFolder() {
    guard let window else { return }
    let panel = NSOpenPanel()
    panel.title = "Choose your pinned folder"
    panel.prompt = "Pin folder"
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = false
    panel.directoryURL = currentFolder?.url
    panel.beginSheetModal(for: window) { [weak self] response in
      guard response == .OK, let url = panel.url, let self else { return }
      let folder = FolderID(url.path)
      self.library.pin(folder)
      self.navigate(.folder(folder))
    }
  }
  private var statusText: String {
    if searching && filter.scope == .all {
      if fileSearch.gathering { return "Searching Spotlight…" }
      return "\(rowCache.count) indexed results" + (fileSearch.limited ? " · first 5,000 matches, narrow your search" : "")
    }
    if let folder = currentFolder, let message = library.state(folder).message { return message }
    return searching ? "\(rowCache.count) matches in this folder" : "\(rowCache.count) \(rowCache.count == 1 ? "item" : "items")"
  }
  func buildPage() {
    page.subviews.forEach { $0.removeFromSuperview() }
    guidance.subviews.forEach { $0.removeFromSuperview() }
    let path = currentFolder.map { library.ancestors($0).map { library.title(.folder($0)) }.joined(separator: "  ›  ") } ?? library.title(tab.location)
    let title = label(path, size: 12, weight: .medium)
    title.identifier = NSUserInterfaceItemIdentifier("breadcrumb")
    title.lineBreakMode = .byTruncatingHead
    title.toolTip = currentFolder?.rawValue
    page.addSubview(title)
    let count = label(
      mode == .list ? "Scope funnel" : statusText, size: 11,
      color: Design.muted)
    count.frame = NSRect(x: mode == .list ? 112 : 24, y: 32, width: 500, height: 18)
    page.addSubview(count)
    if searching {
      let save = ActionButton(
        "Save search", symbol: "plus",
        action: { [weak self] in
          guard let self else { return }
          self.saved.append(
            SavedSearch(
              name: self.tab.query.isEmpty ? "Filtered files" : self.tab.query,
              query: self.tab.query, filter: self.filter, location: self.tab.location))
          self.render()
          self.toast("Search added to the sidebar.")
        })
      save.identifier = NSUserInterfaceItemIdentifier("save-search")
      page.addSubview(save)
      buildFilters()
    } else {
      let list = ActionButton(
        "", symbol: "line.3.horizontal.decrease", help: "Scope funnel view",
        action: { [weak self] in
          self?.mode = .list
          self?.render()
        })
      list.identifier = NSUserInterfaceItemIdentifier("list-view")
      list.setSelected(mode == .list)
      page.addSubview(list)
      let columns = ActionButton(
        "Columns", symbol: "rectangle.split.3x1", help: "Columns view",
        action: { [weak self] in
          self?.mode = .columns
          self?.render()
        })
      columns.identifier = NSUserInterfaceItemIdentifier("columns-view")
      columns.setSelected(mode == .columns)
      page.addSubview(columns)
    }
  }
  func buildFilters() {
    var x: CGFloat = 24
    var y: CGFloat = 0
    var pickerCount = 0
    func picker(
      _ title: String, values: [String], selected: String, changed: @escaping (String) -> Void
    ) {
      if pickerCount == 2 { x = 24; y = 36 }
      pickerCount += 1
      let l = label(title, size: 10, color: Design.muted)
      l.frame = NSRect(x: x, y: y + 12, width: CGFloat(title.count) * 5.5 + 6, height: 18)
      guidance.addSubview(l)
      x += l.frame.width + 4
      let p = ChoicePopup(values: values, selected: selected, changed: changed)
      p.frame = NSRect(x: x, y: y + 6, width: 125, height: 28)
      guidance.addSubview(p)
      x += 134
    }
    picker(
      "Search in", values: SearchScope.allCases.map(\.rawValue), selected: filter.scope.rawValue
    ) { [weak self] value in
      guard let scope = SearchScope(rawValue: value) else { return }
      self?.filter.scope = scope
      self?.render()
    }
    picker(
      "Kind", values: ["Any kind", "Folder"] + DocumentKind.allCases.map(\.rawValue),
      selected: filter.kind
    ) { [weak self] in
      self?.filter.kind = $0
      self?.render()
    }
    picker(
      "Modified", values: ["Any time", "This week"],
      selected: filter.thisWeek ? "This week" : "Any time"
    ) { [weak self] in
      self?.filter.thisWeek = $0 == "This week"
      self?.render()
    }
    picker(
      "Size", values: ["Any size", "Over 10 MB"], selected: filter.large ? "Over 10 MB" : "Any size"
    ) { [weak self] in
      self?.filter.large = $0 == "Over 10 MB"
      self?.render()
    }
    let reset = ActionButton(
      "Reset",
      action: { [weak self] in
        self?.filter = SearchFilter()
        self?.render()
      })
    reset.frame = NSRect(x: x, y: y + 8, width: 50, height: 24)
    guidance.addSubview(reset)
  }
  private func layoutFlexibleControls() {
    func position(_ parent: NSView, _ id: String, _ frame: NSRect) {
      parent.subviews.first { $0.identifier?.rawValue == id }?.frame = frame
    }
    let availableTabs = max(100, bounds.width - 336)
    tabStrip.frame = NSRect(x: 128, y: 0, width: availableTabs, height: 46)
    position(titlebar, "new-tab", NSRect(x: 132 + min(availableTabs, CGFloat(tabs.count) * 184 + 4), y: 10, width: 28, height: 28))
    position(titlebar, "quick", NSRect(x: bounds.width - 174, y: 11, width: 130, height: 26))
    position(titlebar, "help", NSRect(x: bounds.width - 38, y: 11, width: 25, height: 26))
    let breadcrumbX: CGFloat = mode == .list ? 112 : 24
    position(page, "breadcrumb", NSRect(x: breadcrumbX, y: 10, width: max(40, page.bounds.width - breadcrumbX - 425), height: 20))
    position(page, "list-view", NSRect(x: page.bounds.width - 145, y: 21, width: 29, height: 26))
    position(page, "columns-view", NSRect(x: page.bounds.width - 113, y: 21, width: 91, height: 26))
    position(page, "save-search", NSRect(x: page.bounds.width - 130, y: 35, width: 108, height: 26))

  }
  func editPath() {
    isEditingPath = true
    pathEditor.stringValue = currentFolder.map { library.path($0) } ?? FolderID.downloads.rawValue
    buildNavigation()
    needsLayout = true
    window?.makeFirstResponder(pathEditor)
    pathEditor.selectText(nil)
  }
  @objc func commitPath() {
    let input = pathEditor.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    let path = (input as NSString).expandingTildeInPath
    guard path.hasPrefix("/") else { toast("Enter an absolute folder path or a path beginning with ~."); return }
    navigate(.folder(FolderID(path)))
  }

  func historyStep(_ delta: Int) {
    let index = tab.historyIndex + delta
    guard tab.history.indices.contains(index) else { return }
    tab.historyIndex = index
    navigate(tab.history[index], record: false)
  }
  private func rememberTabPosition() {
    if filePalette.embedded { tab.scopeSession = filePalette.session }
    if let folder = currentFolder, !searching {
      tab.folderSelection[folder] = tab.selected
      if let pane = paneViews.first(where: { !$0.isHidden && ($0.active || mode == .list) }) {
        tab.folderScroll[folder] = pane.scroll.scrollOffset
      }
    }
  }
  func selectTab(_ index: Int) {
    guard tabs.indices.contains(index), index != activeTab else { return }
    filePalette.close()
    rememberTabPosition()
    activeTab = index
    marked.removeAll()
    visualAnchor = nil
    render()
    if let folder = currentFolder, !searching,
      let pane = paneViews.first(where: { !$0.isHidden && ($0.active || mode == .list) }) {
      pane.restoreScroll(tab.folderScroll[folder] ?? 0)
    }
  }
  func cycleTab(_ delta: Int) {
    guard !tabs.isEmpty else { return }
    selectTab((activeTab + delta + tabs.count) % tabs.count)
  }
  func newTab() {
    rememberTabPosition()
    var duplicate = WorkspaceTab(tab.location, selected: tab.selected)
    duplicate.scopeSession = tab.scopeSession
    duplicate.folderSelection = tab.folderSelection
    duplicate.folderScroll = tab.folderScroll
    tabs.append(duplicate)
    selectTab(tabs.count - 1)
  }
  func closeTab(_ index: Int) {
    guard tabs.count > 1, tabs.indices.contains(index) else { return }
    filePalette.close()
    rememberTabPosition()
    tabs.remove(at: index)
    if index < activeTab { activeTab -= 1 }
    activeTab = min(activeTab, tabs.count - 1)
    marked.removeAll()
    visualAnchor = nil
    render()
  }
  func toast(_ text: String, undo: Bool = false) {
    toastTimer?.invalidate()
    toastLabel.stringValue = text
    toastLabel.textColor = .white
    toastView.fill = Design.color(0x303747)
    toastView.isHidden = false
    toastView.subviews.filter { $0 !== toastLabel }.forEach { $0.removeFromSuperview() }
    if undo {
      let b = ActionButton("Undo", action: { [weak self] in self?.undoLast() })
      b.contentTintColor = .white
      b.frame = NSRect(x: 365, y: 8, width: 70, height: 28)
      toastView.addSubview(b)
    }
    addSubview(toastView, positioned: .above, relativeTo: nil)
    toastTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: false) { [weak self] _ in
      MainActor.assumeIsolated { self?.toastView.isHidden = true }
    }
  }
  func undoLast() { toast("File changes are work in progress.") }
  func copyOrCut(cut: Bool) { toast("Copying and moving files are work in progress.") }
  func paste() { toast("Pasting files is work in progress.") }
  func trashSelection() { toast("Trash is work in progress.") }
  func changeTheme(_ theme: Theme, persist: Bool = true) {
    if persist { UserDefaults.standard.set(theme.rawValue, forKey: "theme") }
    guard theme != Design.theme else { return }
    do {
      wantsLayer = true
      let transition = CATransition()
      transition.type = .fade
      transition.duration = 0.14
      transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
      layer?.add(transition, forKey: "theme-crossfade")
    }
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    Design.theme = theme
    Design.applyAppearance()
    render()
    if shortcuts.isPresented { shortcuts.refreshTheme() }
    if palette.isPresented { palette.refresh() }
    if filePalette.isPresented { filePalette.refreshTheme() }
    CATransaction.commit()
  }
  func showCommandPalette() {
    configureCommands()
    filePalette.close()
    if palette.isPresented { palette.close(); return }
    shortcuts.isHidden = true
    addSubview(palette, positioned: .above, relativeTo: nil)
    layoutSubtreeIfNeeded()
    palette.present()
    onChange?()
  }
  func showFileSearch(query: String = "", root: FolderID = .home) {
    guard window?.attachedSheet == nil else { return }
    if palette.isPresented { palette.close() }
    shortcuts.transitionVisible(false)
    isEditingPath = false
    pathEditor.isHidden = true
    if filePalette.isPresented && query.isEmpty && root == .home {
      filePalette.focusInput(); return
    }
    addSubview(filePalette, positioned: .above, relativeTo: nil)
    layoutSubtreeIfNeeded()
    filePalette.present(in: root, query: query)
    onChange?()
  }
  private func activateSearchHit(_ hit: SearchHit, reveal: Bool) {
    guard let root = filePalette.search.resultRoot else { return }
    let url = root.url.appendingPathComponent(hit.entry.path)
    if reveal || hit.entry.isFolder {
      filePalette.search.recordOpen(hit)
      navigate(.folder(FolderID(reveal ? url.deletingLastPathComponent().path : url.path)))
      if reveal && mode == .list {
        filePalette.selectPath(url, isFolder: hit.entry.isFolder)
      } else if reveal {
        tab.selected = ItemID(rawValue: url.standardizedFileURL.path)
        if let folder = currentFolder {
          tab.folderSelection[folder] = tab.selected
          if let selected = tab.selected {
            pendingSearchReveal = (folder, selected)
            library.load(folder, refresh: true)
          }
        }
        render()
      }
    } else {
      if !PreviewFormat.forURL(url).opensInReader { filePalette.close() }
      openFile(url) { [weak self] in
        guard let self, self.filePalette.search.resultRoot == root else { return }
        self.filePalette.search.recordOpen(hit)
      }
    }
  }

  func handleKey(_ event: NSEvent) -> Bool {
    if let reader { return reader.handleKey(event) }
    let modifiers = event.modifierFlags
    let command = modifiers.contains(.command)
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    let shortcutKey = event.characters(byApplyingModifiers: [])?.lowercased() ?? key
    if command && modifiers.contains(.shift) && shortcutKey == "p" {
      showCommandPalette()
      return true
    }
    if command && !modifiers.contains(.shift) && !modifiers.contains(.option) && shortcutKey == "p" {
      showFileSearch(); return true
    }
    if palette.isPresented { return palette.handleKey(event) }
    if filePalette.isPresented && !isEditingPath && !shortcuts.isPresented {
      if filePalette.handleKey(event) { return true }
      if modifiers.contains(.control) && event.keyCode == 48 { cycleTab(modifiers.contains(.shift) ? -1 : 1); return true }
      if !command { return false }
    }
    if tab.location == .trash, !shortcuts.isPresented, utility.trash.handleKey(event, vim: vim) { return true }
    if tab.location == .activity, !shortcuts.isPresented, utility.activity.handleKey(event, vim: vim) { return true }
    if tab.location == .systemSettings, !shortcuts.isPresented, utility.systemSettings.handleKey(event, vim: vim) { return true }
    let editing = window?.firstResponder is NSTextView
    if modifiers.contains(.control) && !command && event.keyCode == 48 {
      cycleTab(modifiers.contains(.shift) ? -1 : 1)
      return true
    }
    if command {
      if modifiers.contains(.option), let number = Int(shortcutKey), (1...9).contains(number) {
        selectTab(number - 1)
        return true
      }
      if key == "t" { newTab(); return true }
      if key == "?" || (shortcutKey == "/" && modifiers.contains(.shift)) {
        showHelp()
        return true
      }
      if key == "b" {
        toggleSidebar()
        return true
      }
      if let target = SidebarView.groups(pinned: library.pinned, volumes: mountedVolumes.volumes).flatMap(\.1).first(where: {
        $0.key == (modifiers.contains(.shift) ? "⇧" : "") + shortcutKey
      }) {
        sidebar.navigate?(target.location)
        return true
      }
      switch key {
      case "r": refreshFolder(); return true
      case "l":
        editPath()
        return true
      case "n":
        if modifiers.contains(.shift) { nameDialog(rename: false) } else { newTab() }
        return true
      case "w":
        if tabs.count > 1 { closeTab(activeTab) } else { window?.close() }
        return true
      case "z":
        if !editing {
          undoLast()
          return true
        }
      case "c":
        if !editing {
          copyOrCut(cut: false)
          return true
        }
      case "x":
        if !editing {
          copyOrCut(cut: true)
          return true
        }
      case "v":
        if !editing {
          paste()
          return true
        }
      case "a":
        if !editing {
          marked = Set(rowCache.map(\.id))
          refreshSelection()
          return true
        }
      default: break
      }
      return false
    }
    if event.keyCode == 53 {
      if shortcuts.isPresented {
        shortcuts.transitionVisible(false)
        onChange?()
        return true
      }
      if visualAnchor != nil {
        visualAnchor = nil
        marked.removeAll()
        refreshSelection()
        return true
      }
      if moving != nil {
        cancelMove()
      } else if isEditingPath {
        isEditingPath = false
        render()
        window?.makeFirstResponder(self)
      } else if searching {
        tab.query = ""
        filterOpen = false
        render()
        window?.makeFirstResponder(self)
      } else {
        popover?.close()
      }
      return true
    }
    if editing || tab.location.isUtility { return false }
    let navigationKey = (123...126).contains(event.keyCode) || (vim && ["h", "j", "k", "l"].contains(key))
    if navigationKey {
      // Hand off from pointer scrolling once; a stationary pointer must not reset later keys.
      let pane = pointerNavigationPane
      pointerNavigationPane = nil
      if let pane, let id = pane.hoveredItem { select(id, in: pane.location) }
      paneViews.forEach { $0.scroll.useKeyboard() }
      previewPane.scroll.useKeyboard()
      inspector.scroll.useKeyboard()
    }
    if key == "v", modifiers.contains(.shift), moving == nil, let id = tab.selected {
      visualAnchor = visualAnchor == nil ? id : nil
      if visualAnchor != nil { marked = [id] }
      refreshSelection()
      return true
    }
    if event.keyCode == 36 {
      if moving != nil { commitMove() } else { openSelected() }
      return true
    }
    if event.keyCode == 49 {
      if let selected { showPreview(selected) }
      return true
    }
    if event.keyCode == 51 {
      trashSelection()
      return true
    }
    if event.keyCode == 123 || (vim && key == "h") {
      up()
      return true
    }
    if event.keyCode == 124 || (vim && key == "l") {
      if selected?.kind.folder != nil { openSelected() }
      return true
    }
    if event.keyCode == 125 || event.keyCode == 126 || (vim && (key == "j" || key == "k")) {
      guard !rowCache.isEmpty else { return true }
      let old = rowCache.firstIndex { $0.id == tab.selected } ?? 0
      let delta = (event.keyCode == 125 || key == "j") ? 1 : -1
      let index = max(0, min(rowCache.count - 1, old + delta))
      tab.selected = rowCache[index].id
      extendVisualSelection()
      refreshSelection()
      paneViews.first { $0.active || (mode == .list && !$0.isHidden) }?.scrollSelectionVisible()
      return true
    }
    if key == "m" {
      if moving == nil { startMove() }
      return true
    }
    if key == "x", moving == nil, let id = tab.selected {
      if marked.contains(id) { marked.remove(id) } else { marked.insert(id) }
      refreshSelection()
      return true
    }
    return false
  }
}
@MainActor
final class ChoicePopup: NSPopUpButton {
  var changed: (String) -> Void
  init(values: [String], selected: String, changed: @escaping (String) -> Void) {
    self.changed = changed
    super.init(frame: .zero, pullsDown: false)
    addItems(withTitles: values)
    selectItem(withTitle: selected)
    font = Design.font(10)
    target = self
    action = #selector(picked)
  }
  required init?(coder: NSCoder) { nil }
  @objc func picked() { if let title = selectedItem?.title { changed(title) } }
}
