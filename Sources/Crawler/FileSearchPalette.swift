import AppKit

// Scope funnel is shared by the default workspace and the Columns search overlay.
@MainActor
final class FileSearchPalette: Surface, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
  let search = QuickFileSearch()
  private let input = NSTextField()
  private let searchBorder = Surface()
  private let searchScopeHint = label("This Mac", size: 11, weight: .medium)
  private var inputMode: FileSearchMode = .computer
  private func searchInput() {
    if input.stringValue.hasPrefix("$") {
      inputMode = .folder
      let text = String(input.stringValue.dropFirst()).trimmingCharacters(in: .whitespaces)
      input.stringValue = text
      if let editor = input.currentEditor() as? NSTextView {
        editor.string = text
        editor.setSelectedRange(NSRange(location: text.utf16.count, length: 0))
      }
    }
    if inputMode != search.mode { history = [] }
    search.search(input.stringValue, mode: inputMode)
  }
  private func updateSearchAppearance() {
    let local = search.mode == .folder
    searchBorder.fill = local ? Design.accent.withAlphaComponent(0.07) : Design.background
    searchBorder.layer?.borderColor = (local ? Design.accent : Design.muted).cgColor
    searchScopeHint.textColor = local ? Design.accent : Design.muted
  }
  private let folderLibrary = FileSystemLibrary()
  private let currentFolderPane = FilePane()
  private let childFolderPane = FilePane()
  private var folderSelections: [FolderID: ItemID] = [:]
  var browseFolder: ((FolderID) -> Void)?
  var isBrowsing: Bool { search.root != nil && !isHome && search.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
  private var browsingFolder: FolderID? { search.scope ?? search.root }
  private var browsingItem: FileItem? {
    guard let folder = browsingFolder else { return nil }
    return folderLibrary.item(folderSelections[folder])
  }
  private let scope = label("", size: 11, weight: .medium)
  private let status = label("", size: 10)
  private let hint = label("", size: 10)
  private let recentHeading = label("Recently opened", size: 12, weight: .semibold)
  private var recentFiles: [FileItem] = []
  private var recentTask: Task<Void, Never>?
  private var showingHome = false
  private var showingBrowser = false
  var openRecent: ((FileItem, Bool) -> Void)?
  var isHome: Bool { search.isHome }
  private let empty = label("", size: 12)
  private let topRule = Surface(), bottomRule = Surface(), leftRule = Surface(), rightRule = Surface(), railRule = Surface()
  private let scroll = NSScrollView()
  private let table = SearchResultTable()
  private let scopes = ScopeRail(title: "Narrow the search")
  private let context = ScopeRail(title: "Where it lives")
  let preview = PreviewPane(frame: .zero)
  private var chosenNeighbor: SearchHit?
  private var selectedPath: String?
  private var displayedQuery = ""
  private var displayedScope: FolderID?
  private(set) var displayedHits: [SearchHit] = []
  private var resultsQuery = ""
  private var resultsMode: FileSearchMode = .computer
  private var resultsScope: FolderID?
  private var updatingResults = false
  private var history: [FolderID] = []
  private var navigationTask: Task<Void, Never>?
  private var navigationError: String?
  private var metadataTask: Task<Void, Never>?
  private var siblingTask: Task<Void, Never>?
  private var selectedURL: URL?
  private var parent: FolderID?
  private var siblings: [FileItem] = []
  private var siblingCache: [(FolderID, [FileItem], Bool)] = []
  private var moreSiblings = false
  private var siblingMessage = ""
  private var contextItem: FileItem?
  private var resultUpdateTask: Task<Void, Never>?
  private var windowUpdateObserver: NSObjectProtocol?
  private var showingCurrentResponse = false
  private var pendingScroll: CGFloat?
  private var gAt: TimeInterval = 0
  var embedded = false
  var dismiss: (() -> Void)?
  var activate: ((SearchHit, Bool) -> Void)?
  var showPreview: ((FileItem) -> Void)?
  var didChange: (() -> Void)?
  var selectionChanged: ((FileItem?) -> Void)?
  var selectedHit: SearchHit? {
    !isHome && !isBrowsing && search.responseIsCurrent && resultsMode == search.mode && resultsQuery == search.query && resultsScope == search.activeSearchScope && displayedHits.indices.contains(table.selectedRow)
      ? displayedHits[table.selectedRow] : nil
  }
  var selectedFile: FileItem? {
    if isBrowsing { return browsingItem }
    if isHome { return recentFiles.indices.contains(table.selectedRow) ? recentFiles[table.selectedRow] : nil }
    return contextItem
  }
  override var acceptsFirstResponder: Bool { true }
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    input.isBordered = false
    input.drawsBackground = false
    input.focusRingType = .none
    input.placeholderString = "Find a file…"
    input.setAccessibilityLabel("Find files and folders")
    input.delegate = self
    let column = NSTableColumn(identifier: .init("file"))
    column.resizingMask = .autoresizingMask
    table.addTableColumn(column)
    table.headerView = nil
    table.rowHeight = 58
    table.intercellSpacing = .zero
    table.backgroundColor = .clear
    table.selectionHighlightStyle = .regular
    table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
    table.allowsEmptySelection = false
    table.dataSource = self
    table.delegate = self
    table.target = self
    table.doubleAction = #selector(openSelection)
    // Drags carry the file URL; other apps and the sidebar only ever copy it.
    table.setDraggingSourceOperationMask(.copy, forLocal: true)
    table.setDraggingSourceOperationMask(.copy, forLocal: false)
    table.setAccessibilityLabel("File search results")
    table.hover = { [weak self] row in self?.select(row, scroll: false) }
    scroll.documentView = table
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.hasHorizontalScroller = false
    scroll.horizontalScrollElasticity = .none
    scroll.autohidesScrollers = true
    scroll.scrollerStyle = .overlay
    empty.alignment = .center
    empty.maximumNumberOfLines = 4
    empty.lineBreakMode = .byWordWrapping
    scope.lineBreakMode = .byTruncatingMiddle
    for view in [searchBorder, input, searchScopeHint, scope, scroll, status, hint, empty, topRule, bottomRule, scopes, context, preview, leftRule, rightRule, railRule, recentHeading, currentFolderPane, childFolderPane] { addSubview(view) }
    folderLibrary.didChange = { [weak self] _ in
      guard let self else { return }
      if self.isBrowsing { self.updateFolderPanels() }
      else { self.updateSelectionPreview() }
    }
    for pane in [currentFolderPane, childFolderPane] {
      pane.selectionChanged = { [weak self] id, folder, _ in
        guard let self, let folder else { return }
        self.folderSelections[folder] = id
        if folder != self.browsingFolder { self.browseFolder?(folder) }
        self.updateFolderPanels()
        self.navigateFocus()
      }
      pane.openItem = { [weak self] id in
        guard let self, let file = self.folderLibrary.item(id) else { return }
        self.openBrowsing(file)
      }
      pane.focusFolder = { [weak self] folder in self?.browseFolder?(folder) }
    }
    searchScopeHint.alignment = .right
    input.toolTip = "Type $ to search the current folder. Clear the query to search This Mac again."
    scopes.choose = { [weak self] target in self?.choose(target) }
    context.choose = { [weak self] target in self?.choose(target) }
    preview.document.didChange = { [weak self] in self?.didChange?() }
    search.performance.didMeasure = { [weak self] in self?.didChange?() }
    search.didChange = { [weak self] in
      guard let self else { return }
      if self.isPresented { self.scheduleResultsUpdate() }
      else { self.didChange?() }
    }
  }
  required init?(coder: NSCoder) { nil }
  deinit { recentTask?.cancel(); navigationTask?.cancel(); metadataTask?.cancel(); siblingTask?.cancel(); resultUpdateTask?.cancel() }

  func present(in folder: FolderID, query: String = "", focus: Bool = true) {
    selectedPath = nil
    chosenNeighbor = nil
    navigationTask?.cancel(); navigationError = nil
    history = []
    if search.root != folder { clearContext(); siblingCache = [] }
    displayedHits = []
    table.reloadData()
    inputMode = .computer
    input.stringValue = query
    if embedded { isHidden = false; alphaValue = 1 } else { transitionVisible(true) }
    refreshTheme()
    search.present(in: folder)
    search.setScope(nil)
    searchInput()
    if focus { focusInput() }
  }
  var session: ScopeSession? {
    search.root.map { root in ScopeSession(searchMode: inputMode, computerScope: search.computerScope, volumeRoot: search.volumeRoot, root: root, scope: search.scope, query: input.stringValue,
      selectedPath: isBrowsing ? browsingItem.map { String($0.url.path.dropFirst(root.rawValue == "/" ? 1 : root.rawValue.count + 1)) } : selectedPath ?? selectedHit?.entry.path, chosenNeighbor: chosenNeighbor, history: history, scroll: scroll.contentView.bounds.minY) }
  }
  func restore(_ session: ScopeSession) {
    chosenNeighbor = nil
    navigationTask?.cancel(); navigationError = nil
    if search.root != session.root { present(in: session.root, focus: false) }
    isHidden = false; alphaValue = 1
    search.resume()
    search.setVolumeRoot(session.volumeRoot)
    search.setScope(session.scope)
    inputMode = session.searchMode
    input.stringValue = session.query
    searchInput()
    search.setComputerScope(session.computerScope)
    selectedPath = session.selectedPath
    if isBrowsing, let folder = browsingFolder, let path = session.selectedPath {
      folderSelections[folder] = ItemID(rawValue: session.root.url.appendingPathComponent(path).standardizedFileURL.path)
      updateFolderPanels()
    }
    chosenNeighbor = session.chosenNeighbor
    history = session.history
    pendingScroll = session.scroll
    updateScopes(partial: true)
  }
  func resume() {
    isHidden = false; alphaValue = 1
    search.resume()
    refreshTheme()
  }
  func focusInput() {
    window?.makeFirstResponder(input)
    input.currentEditor()?.selectedRange = NSRange(location: input.stringValue.utf16.count, length: 0)
    updateHint()
  }
  func browse(_ folder: FolderID) {
    navigationTask?.cancel(); navigationError = nil
    let root = search.root ?? .home
    if folder != root && !folder.rawValue.hasPrefix(root.rawValue == "/" ? "/" : root.rawValue + "/") {
      present(in: folder, focus: false)
    } else {
      if !isPresented { present(in: root, focus: false) }
      inputMode = .computer
      input.stringValue = ""
      searchInput()
      narrow(folder)
    }
    // Explicit folder navigation may enter a symlink even though discovery never follows links.
    let indexedRoot = search.root ?? root
    navigationTask = Task { [weak self] in
      let worker = Task.detached(priority: .userInitiated) { () -> Result<Bool, Error> in
        Result {
          let target = folder.url.resolvingSymlinksInPath()
          guard try target.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
            throw NSError(domain: NSCocoaErrorDomain, code: NSFileReadUnknownError,
              userInfo: [NSLocalizedDescriptionKey: "This path is not a folder."])
          }
          let relative = folder == indexedRoot ? "" : String(folder.rawValue.dropFirst(indexedRoot.rawValue == "/" ? 1 : indexedRoot.rawValue.count + 1))
          let expected = indexedRoot.url.resolvingSymlinksInPath().appendingPathComponent(relative).standardizedFileURL
          return target.standardizedFileURL.path != expected.path
        }
      }
      let result = await withTaskCancellationHandler(operation: { await worker.value }, onCancel: { worker.cancel() })
      guard let self, !Task.isCancelled, (self.search.scope ?? self.search.root) == folder else { return }
      switch result {
      case .success(let symlink):
        if symlink && self.search.root != folder { self.present(in: folder, focus: false) }
      case .failure(let error):
        self.navigationError = error.localizedDescription
        self.updateResults()
      }
    }
  }
  func selectPath(_ url: URL, isFolder: Bool) {
    guard let root = search.root else { return }
    let prefix = root.rawValue == "/" ? "/" : root.rawValue + "/"
    guard url.path.hasPrefix(prefix) else { return }
    let relative = String(url.path.dropFirst(prefix.count))
    inputMode = .computer
    input.stringValue = ""
    searchInput()
    selectedPath = relative
    if isBrowsing, let folder = browsingFolder {
      folderSelections[folder] = ItemID(rawValue: url.standardizedFileURL.path)
      updateFolderPanels()
      return
    }
    let entry = SearchEntry(path: relative, isFolder: isFolder)
    chosenNeighbor = SearchHit(entry: entry, score: 0, highlights: [])
    updateResults()
    if table.selectedRow >= 0 { table.scrollRowToVisible(table.selectedRow) }
  }
  func close() {
    guard isPresented, !embedded else { return }
    suspend()
    dismiss?()
  }
  func suspend() {
    resultUpdateTask?.cancel(); resultUpdateTask = nil
    navigationTask?.cancel()
    search.dismiss()
    folderLibrary.watch([])
    clearContext()
    isHidden = true
  }
  func refreshTheme() {
    fill = Design.background
    layer?.cornerRadius = embedded ? 0 : 12
    layer?.borderWidth = embedded ? 0 : 1
    layer?.borderColor = Design.line.cgColor
    searchScopeHint.font = Design.font(11, .medium)
    searchScopeHint.textColor = Design.muted
    searchBorder.fill = Design.background
    searchBorder.wantsLayer = true
    searchBorder.layer?.borderWidth = 1
    searchBorder.layer?.borderColor = Design.muted.cgColor
    searchBorder.layer?.cornerRadius = Design.square ? 0 : 8
    updateSearchAppearance()
    input.font = Design.font(21)
    input.textColor = Design.text
    input.placeholderAttributedString = NSAttributedString(string: "Find a file…", attributes: [.foregroundColor: Design.muted, .font: Design.font(21)])
    for field in [scope, status, hint, empty] { field.textColor = Design.muted }
    recentHeading.textColor = Design.muted
    recentHeading.font = Design.font(12, .semibold)
    scope.textColor = Design.text
    scope.font = Design.font(11, .medium)
    status.font = Design.font(10); hint.font = Design.font(10)
    empty.font = Design.font(12)
    for rule in [topRule, bottomRule, leftRule, rightRule, railRule] { rule.fill = Design.line }
    scopes.refreshTheme(); context.refreshTheme()
    preview.configure(selectedFile)
    currentFolderPane.redrawTheme(); childFolderPane.redrawTheme()
    if isBrowsing { updateFolderPanels() } else { updateSelectionPreview() }
    table.reloadData()
  }
  func reloadRecents() {
    recentTask?.cancel()
    let paths = FileOpening.recentPaths
    recentTask = Task { [weak self] in
      let worker = Task.detached(priority: .utility) {
        var files: [FileItem] = []
        for path in paths {
          guard !Task.isCancelled else { break }
          if let file = try? DirectoryReader.item(at: URL(fileURLWithPath: path)), file.kind.folder == nil {
            files.append(file)
            if files.count == 8 { break }
          }
        }
        return files
      }
      let files = await withTaskCancellationHandler(operation: { await worker.value }, onCancel: { worker.cancel() })
      guard let self, !Task.isCancelled else { return }
      self.recentFiles = files
      if self.isHome {
        self.table.reloadData()
        self.updateContext()
        self.recentHeading.isHidden = false
        self.empty.isHidden = !files.isEmpty
        self.needsLayout = true
      }
    }
  }
  private func scheduleResultsUpdate() {
    let newQuery = resultsQuery != search.query || resultsMode != search.mode || resultsScope != search.activeSearchScope
    // Input changes and the first usable response are immediate. Further
    // discoveries share one short UI update instead of repainting each batch.
    if newQuery || isHome || isBrowsing || (search.responseIsCurrent && (!showingCurrentResponse || (displayedHits.isEmpty && !search.response.hits.isEmpty))) {
      resultUpdateTask?.cancel(); resultUpdateTask = nil
      updateResults()
      return
    }
    guard resultUpdateTask == nil else { return }
    resultUpdateTask = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(40)) } catch { return }
      guard let self else { return }
      self.resultUpdateTask = nil
      if self.isPresented { self.updateResults() }
    }
  }

  private func updateResults() {
    let updateStart = ProcessInfo.processInfo.systemUptime
    defer {
      if search.responseIsCurrent && !isHome && !isBrowsing {
        search.performance.rowsUpdated(since: updateStart)
      }
    }
    showingCurrentResponse = search.responseIsCurrent
    if showingBrowser != isBrowsing { needsLayout = true; showingBrowser = isBrowsing; table.reloadData() }
    currentFolderPane.isHidden = !isBrowsing
    childFolderPane.isHidden = true
    scroll.isHidden = isBrowsing
    preview.isHidden = false
    let driveName = search.volumeRoot?.url.lastPathComponent
    searchScopeHint.stringValue = search.mode == .folder ? "Current folder" : driveName ?? "This Mac"
    searchScopeHint.lineBreakMode = .byTruncatingMiddle
    searchScopeHint.toolTip = search.activeSearchScope?.rawValue
    input.placeholderString = driveName.map { "Search in \($0)…" } ?? "Find a file…"
    input.toolTip = driveName.map { "Search \($0), including subfolders. Type $ to search only the current folder." }
      ?? "Type $ to search the current folder. Clear the query to search This Mac again."
    updateSearchAppearance()
    let homeChanged = showingHome != isHome
    showingHome = isHome
    for view in [scope, scopes, context, bottomRule, leftRule, railRule, status, hint] {
      view.isHidden = isHome
    }
    scopes.isHidden = isHome || isBrowsing
    context.isHidden = isHome || isBrowsing
    leftRule.isHidden = isHome || isBrowsing
    railRule.isHidden = isHome || isBrowsing
    scope.isHidden = isHome
    recentHeading.isHidden = !isHome
    needsLayout = true
    if homeChanged {
      table.setAccessibilityLabel(isHome ? "Recently opened files" : "File search results")
      needsLayout = true; table.reloadData()
    }
    if isBrowsing {
      metadataTask?.cancel(); siblingTask?.cancel()
      selectedURL = nil
      empty.isHidden = true
      displayedHits = []
      status.stringValue = ""
      scope.stringValue = browsingFolder?.rawValue ?? ""
      updateFolderPanels()
      updateHint()
      needsLayout = true
      return
    }
    folderLibrary.watch([])
    if isHome {
      if homeChanged { clearContext(); displayedHits = []; reloadRecents() }
      empty.isHidden = !recentFiles.isEmpty
      empty.stringValue = "Files you open in Crawler appear here."
      didChange?()
      return
    }
    let queryChanged = displayedQuery != search.query || displayedScope != search.activeSearchScope || resultsMode != search.mode
    if queryChanged {
      selectedPath = nil
      displayedQuery = search.query
      displayedScope = search.activeSearchScope
    }
    var hits = search.response.hits
    if let chosenNeighbor, search.query.isEmpty, !hits.contains(where: { $0.entry.path == chosenNeighbor.entry.path }) {
      if hits.count == FFFSearch.resultLimit { hits.removeLast() }
      hits.append(chosenNeighbor)
    }
    if (!search.searching || !hits.isEmpty) && (displayedHits != hits || queryChanged) {
      updatingResults = true
      let previousHits = displayedHits
      let oldCount = previousHits.count
      let oldSelection = table.selectedRow
      let topRow = table.row(at: NSPoint(x: 0, y: scroll.contentView.bounds.minY))
      let anchor = !queryChanged && previousHits.indices.contains(topRow) && topRow > 0
        ? previousHits[topRow].entry.path : nil
      let anchorOffset = topRow >= 0 ? scroll.contentView.bounds.minY - table.rect(ofRow: topRow).minY : 0
      displayedHits = hits
      resultsMode = search.mode
      resultsQuery = search.query
      resultsScope = search.activeSearchScope
      NSAnimationContext.runAnimationGroup { animation in
        animation.duration = 0
        animation.allowsImplicitAnimation = false
        table.beginUpdates()
        if hits.count > oldCount { table.insertRows(at: IndexSet(integersIn: oldCount..<hits.count), withAnimation: []) }
        else if hits.count < oldCount { table.removeRows(at: IndexSet(integersIn: hits.count..<oldCount), withAnimation: []) }
        table.endUpdates()
        if !hits.isEmpty {
          let selected = hits.firstIndex { $0.entry.path == selectedPath } ?? 0
          table.selectRowIndexes(IndexSet(integer: selected), byExtendingSelection: false)
          if search.responseIsCurrent { selectedPath = hits[selected].entry.path }
        } else {
          table.deselectAll(nil)
        }
        table.enumerateAvailableRowViews { row, index in
          guard hits.indices.contains(index), let cell = row.view(atColumn: 0) as? SearchResultCell else { return }
          if previousHits.indices.contains(index), previousHits[index].entry == hits[index].entry,
            previousHits[index].highlights == hits[index].highlights,
            (index == oldSelection) == (index == table.selectedRow) { return }
          cell.configure(hits[index], selected: index == table.selectedRow)
        }
      }
      updatingResults = false
      if let pendingScroll, !hits.isEmpty {
        scroll.contentView.scroll(to: NSPoint(x: 0, y: pendingScroll))
        scroll.reflectScrolledClipView(scroll.contentView)
        self.pendingScroll = nil
      } else if let anchor, let index = hits.firstIndex(where: { $0.entry.path == anchor }) {
        scroll.contentView.scroll(to: NSPoint(x: 0, y: table.rect(ofRow: index).minY + anchorOffset))
        scroll.reflectScrolledClipView(scroll.contentView)
      }
    }
    if search.responseIsCurrent, selectedPath == nil, displayedHits.indices.contains(table.selectedRow) {
      selectedPath = displayedHits[table.selectedRow].entry.path
    }
    status.stringValue = search.status
    status.toolTip = search.status
    let partial = search.resultsArePartial
    empty.isHidden = !displayedHits.isEmpty
    empty.stringValue = search.status
    if search.searching && displayedHits.isEmpty { empty.stringValue = "Searching…" }
    if let navigationError {
      empty.stringValue = navigationError
      empty.isHidden = false
      status.stringValue = navigationError
    }
    updateScopes(partial: partial)
    updateContext()
    updateHint()
    didChange?()
  }
  private func updateFolderPanels() {
    guard isBrowsing, let folder = browsingFolder else { return }
    let files = folderLibrary.children(folder)
    if case .ready = folderLibrary.state(folder), !files.contains(where: { $0.id == folderSelections[folder] }) {
      folderSelections[folder] = files.first?.id
    }
    func rows(_ files: [FileItem]) -> [DisplayRow] {
      files.map { DisplayRow(item: $0, pending: false, dimmed: false, marked: false, locationText: "") }
    }
    currentFolderPane.configure(title: folder.url.lastPathComponent, location: folder,
      rows: rows(files), selected: folderSelections[folder], active: true)
    currentFolderPane.empty.stringValue = folderLibrary.state(folder).message ?? ""
    var watched: Set<FolderID> = [folder]
    let item = browsingItem
    if let item, item.kind.folder != nil {
      let child = FolderID(item.url.path)
      let children = folderLibrary.children(child)
      childFolderPane.configure(title: item.name, location: child, rows: rows(children), selected: nil, active: false)
      childFolderPane.empty.stringValue = folderLibrary.state(child).message ?? ""
      childFolderPane.isHidden = false
      preview.isHidden = true
      preview.configure(nil)
      watched.insert(child)
    } else {
      childFolderPane.isHidden = true
      preview.isHidden = false
      preview.configure(item)
    }
    folderLibrary.watch(watched)
    contextItem = item
    selectionChanged?(item)
    didChange?()
  }
  private func updateSelectionPreview() {
    guard !isBrowsing else { return }
    if let file = contextItem, file.kind.folder != nil {
      let folder = FolderID(file.url.path)
      let children = folderLibrary.children(folder)
      let rows = children.map { DisplayRow(item: $0, pending: false, dimmed: false, marked: false, locationText: "") }
      childFolderPane.configure(title: file.name, location: folder, rows: rows, selected: nil, active: false)
      childFolderPane.empty.stringValue = folderLibrary.state(folder).message ?? ""
      childFolderPane.isHidden = false
      preview.isHidden = true
      preview.configure(nil)
      folderLibrary.watch([folder])
    } else {
      childFolderPane.isHidden = true
      preview.isHidden = false
      preview.configure(contextItem)
      folderLibrary.watch([])
    }
  }
  private func openBrowsing(_ file: FileItem) {
    if file.kind.folder != nil { browseFolder?(FolderID(file.url.path)) }
    else { openRecent?(file, false) }
  }
  private func moveInFolder(_ delta: Int) {
    guard let folder = browsingFolder else { return }
    let files = folderLibrary.children(folder)
    guard !files.isEmpty else { return }
    let index = files.firstIndex { $0.id == folderSelections[folder] } ?? 0
    folderSelections[folder] = files[max(0, min(files.count - 1, index + delta))].id
    updateFolderPanels()
    currentFolderPane.scrollSelectionVisible()
  }
  private func updateScopes(partial: Bool) {
    guard let root = search.isComputerSearch ? (search.volumeRoot ?? search.resultRoot) : search.root else { return }
    let active = search.activeSearchScope ?? root
    let rootName = root.rawValue == "/" ? "This Mac" : root == .home ? "Home" : root.url.lastPathComponent
    let relative = active == root ? "" : String(active.rawValue.dropFirst(root.rawValue == "/" ? 1 : root.rawValue.count + 1))
    scope.stringValue = rootName + (relative.isEmpty ? "  /  All files" : "  /  " + relative.replacingOccurrences(of: "/", with: "  /  "))
    scope.toolTip = active.rawValue
    var rows = [ScopeRail.Row(title: "All files in \(rootName)", detail: "", depth: 0, selected: active == root, target: .scope(root))]
    if !history.isEmpty { rows.append(.init(title: "Previous scope", detail: "←", depth: 0, selected: false, target: .back)) }
    rows += search.response.scopes.sorted { a, b in
      a.key.localizedStandardCompare(b.key) == .orderedAscending
    }.map { path, _ in
      .init(title: (path as NSString).lastPathComponent, detail: "", depth: 0, selected: false, target: .scope(FolderID((search.resultRoot ?? root).url.appendingPathComponent(path).path)))
    }
    if search.response.scopes.isEmpty {
      rows.append(.init(title: partial ? "Discovering folders…" : "No deeper matching folders", detail: "", depth: 0, selected: false, target: nil))
    }
    scopes.configure(rows, subtitle: active == root ? rootName : active.url.lastPathComponent)
  }
  private func narrow(_ folder: FolderID) {
    guard let root = search.isComputerSearch ? (search.volumeRoot ?? search.resultRoot) : search.root else { return }
    let current = search.isComputerSearch ? search.computerScope ?? root : search.scope ?? root
    guard folder != current else { return }
    chosenNeighbor = nil
    navigationTask?.cancel(); navigationError = nil
    history.append(current)
    if history.count > 100 { history.removeFirst() }
    if search.isComputerSearch { search.setComputerScope(folder) } else { search.setScope(folder) }
    navigateFocus()
  }
  private func choose(_ target: ScopeRail.Row.Target) {
    switch target {
    case .scope(let folder): narrow(folder)
    case .back:
      chosenNeighbor = nil
      if let previous = history.popLast() {
        if search.isComputerSearch { search.setComputerScope(previous) } else { search.setScope(previous) }
      }
      navigateFocus()
    case .neighbor(let file):
      guard let root = search.resultRoot else { return }
      let relative = String(file.id.rawValue.dropFirst(root.rawValue == "/" ? 1 : root.rawValue.count + 1))
      if let row = displayedHits.firstIndex(where: { $0.entry.path == relative }) { select(row, scroll: true) }
      else {
        inputMode = .computer
        input.stringValue = ""
        searchInput()
        browseFolder?(file.parent)
        // A nearby entry can fall outside the top 100. Keep this explicit choice visible.
        selectPath(file.url, isFolder: file.kind.folder != nil)
      }
      navigateFocus()
    }
  }
  private func navigateFocus() { window?.makeFirstResponder(self); updateHint() }
  private var editing: Bool { input.currentEditor() != nil }
  private func updateHint() {
    hint.stringValue = editing ? "SEARCH    Esc Navigate    Ctrl D/U Half page    ↵ Open" : "NAVIGATE    j/k Files    h/l Scope    / Search    Space Preview    ↵ Open"
  }
  func controlTextDidChange(_ notification: Notification) {
    let event = NSApp.currentEvent
    search.performance.input(timestamp: event?.type == .keyDown ? event?.timestamp : nil)
    chosenNeighbor = nil
    if input.stringValue.isEmpty { inputMode = .computer }
    searchInput()
  }
  func controlTextDidBeginEditing(_ notification: Notification) { updateHint() }
  func controlTextDidEndEditing(_ notification: Notification) { updateHint() }
  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    if let windowUpdateObserver { NotificationCenter.default.removeObserver(windowUpdateObserver) }
    windowUpdateObserver = nil
    table.didDraw = { [weak self] start in self?.search.performance.drawn(since: start) }
    guard let window else { return }
    windowUpdateObserver = NotificationCenter.default.addObserver(
      forName: NSWindow.didUpdateNotification, object: window, queue: .main
    ) { [weak self] _ in
      MainActor.assumeIsolated { self?.search.performance.windowUpdated() }
    }
  }
  func handleKey(_ event: NSEvent) -> Bool {
    let flags = event.modifierFlags
    let key = event.charactersIgnoringModifiers ?? ""
    if editing && !flags.contains(.command) && !flags.contains(.control) {
      if event.characters == "$" {
        inputMode = .folder
        chosenNeighbor = nil
        searchInput()
        return true
      }
      if event.keyCode == 51 && input.stringValue.isEmpty && inputMode == .folder {
        inputMode = .computer
        searchInput()
        return true
      }
    }
    if isBrowsing && !editing && !flags.contains(.command) && !flags.contains(.option) {
      if key == "h" || event.keyCode == 123 {
        if let folder = browsingFolder, let parent = folderLibrary.parent(folder) {
          folderSelections[parent] = ItemID(rawValue: folder.rawValue)
          browseFolder?(parent)
        }
        return true
      }
      if key == "l" || event.keyCode == 124 {
        if let file = browsingItem, file.kind.folder != nil { openBrowsing(file) }
        return true
      }
      if key == "j" || event.keyCode == 125 { moveInFolder(1); return true }
      if key == "k" || event.keyCode == 126 { moveInFolder(-1); return true }
    }
    if flags.contains(.command) {
      if key.lowercased() == "r" {
        refresh(); return true
      }
      if event.keyCode == 36, isBrowsing, let file = browsingItem { openRecent?(file, true); return true }
      if event.keyCode == 36, isHome, recentFiles.indices.contains(table.selectedRow) {
        openRecent?(recentFiles[table.selectedRow], true); return true
      }
      if event.keyCode == 36, let selectedHit { activate?(selectedHit, true); return true }
      return false
    }
    if flags.contains(.control), !flags.contains(.option), ["d", "u"].contains(key.lowercased()) {
      move((key.lowercased() == "d" ? 1 : -1) * max(1, Int(scroll.contentSize.height / table.rowHeight / 2)))
      return true
    }
    if flags.contains(.option) || flags.contains(.control) { return false }
    switch event.keyCode {
    case 53:
      if editing { navigateFocus() } else if !embedded { close() }
      return true
    case 36, 76: openSelection(); return true
    case 125, 126: move(event.keyCode == 125 ? 1 : -1); return true
    default: break
    }
    guard !editing else { return false }
    if key == "j" { move(1) }
    else if key == "k" { move(-1) }
    else if key == "h" || event.keyCode == 123 { choose(.back) }
    else if key == "l" || event.keyCode == 124 {
      if let hit = selectedHit, let root = search.resultRoot {
        let url = root.url.appendingPathComponent(hit.entry.path)
        let folder = FolderID(hit.entry.isFolder ? url.path : url.deletingLastPathComponent().path)
        browseFolder?(folder)
      }
    } else if key == "/" || key == "i" { focusInput() }
    else if key == "G" { select((isHome ? recentFiles.count : displayedHits.count) - 1, scroll: true) }
    else if key == "g" {
      if event.timestamp - gAt < 0.7 { select(0, scroll: true); gAt = 0 }
      else { gAt = event.timestamp }
    } else if event.keyCode == 49 {
      if isHome, recentFiles.indices.contains(table.selectedRow) { showPreview?(recentFiles[table.selectedRow]) }
      else if let contextItem { showPreview?(contextItem) }
    } else { return false }
    if key != "g" { gAt = 0 }
    return true
  }
  func refresh() {
    if isHome { reloadRecents(); return }
    chosenNeighbor = nil
    navigationError = nil
    siblingCache = []; selectedURL = nil; parent = nil
    if isBrowsing, let folder = browsingFolder {
      folderLibrary.load(folder, refresh: true)
      if let file = browsingItem, file.kind.folder != nil { folderLibrary.load(FolderID(file.url.path), refresh: true) }
    } else { search.refresh(); updateContext() }
  }
  private func move(_ delta: Int) { if isBrowsing { moveInFolder(delta); return }; select(max(0, min((isHome ? recentFiles.count : displayedHits.count) - 1, table.selectedRow + delta)), scroll: true) }
  private func select(_ row: Int, scroll shouldScroll: Bool) {
    guard (isHome ? recentFiles.indices.contains(row) : displayedHits.indices.contains(row)) else { return }
    table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    if !isHome { selectedPath = displayedHits[row].entry.path }
    if shouldScroll { table.scrollRowToVisible(row) }
  }
  @objc func openSelection() {
    if isBrowsing, let file = browsingItem { openBrowsing(file); return }
    if isHome, recentFiles.indices.contains(table.selectedRow) { openRecent?(recentFiles[table.selectedRow], false) }
    else if let selectedHit { activate?(selectedHit, false) }
  }
  func numberOfRows(in tableView: NSTableView) -> Int { isHome ? recentFiles.count : displayedHits.count }
  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
    guard isHome ? recentFiles.indices.contains(row) : displayedHits.indices.contains(row) else { return nil }
    let id = NSUserInterfaceItemIdentifier("search-result")
    let cell = tableView.makeView(withIdentifier: id, owner: nil) as? SearchResultCell ?? SearchResultCell()
    cell.identifier = id
    cell.didDrawTitle = { [weak self] start in
      guard let self, self.search.responseIsCurrent, !self.isHome, !self.isBrowsing else { return }
      self.search.performance.titleDrawn(since: start)
    }
    if isHome { cell.configure(recentFiles[row], selected: row == table.selectedRow) }
    else { cell.configure(displayedHits[row], selected: row == table.selectedRow) }
    return cell
  }
  func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { SearchResultRow() }
  func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> (any NSPasteboardWriting)? {
    if isHome { return recentFiles.indices.contains(row) ? recentFiles[row].url as NSURL : nil }
    guard search.responseIsCurrent, displayedHits.indices.contains(row), let root = search.resultRoot else { return nil }
    return root.url.appendingPathComponent(displayedHits[row].entry.path) as NSURL
  }
  func tableViewSelectionDidChange(_ notification: Notification) {
    guard !updatingResults else { return }
    if isHome {
      table.reloadData(forRowIndexes: IndexSet(integersIn: 0..<recentFiles.count), columnIndexes: IndexSet(integer: 0))
      updateContext()
      return
    }
    selectedPath = selectedHit?.entry.path
    updateVisibleCells(); updateContext(); didChange?()
  }
  private func updateVisibleCells() {
    table.enumerateAvailableRowViews { row, index in
      guard let cell = row.view(atColumn: 0) as? SearchResultCell, displayedHits.indices.contains(index) else { return }
      cell.configure(displayedHits[index], selected: index == table.selectedRow)
    }
  }
  private func clearContext() {
    metadataTask?.cancel(); siblingTask?.cancel()
    selectedURL = nil; parent = nil; contextItem = nil; siblings = []
    childFolderPane.isHidden = true
    preview.isHidden = false
    folderLibrary.watch([])
    preview.configure(nil)
    context.configure([], subtitle: "Select a file to see its location")
    selectionChanged?(nil)
  }
  private func updateContext() {
    if isBrowsing { updateFolderPanels(); return }
    if isHome {
      let file = selectedFile
      guard file?.url != selectedURL || contextItem?.id != file?.id else { return }
      metadataTask?.cancel(); siblingTask?.cancel()
      selectedURL = file?.url
      contextItem = file
      preview.configure(file)
      selectionChanged?(file)
      didChange?()
      return
    }
    // Retain the current preview while a replacement query is working.
    // Old rows remain inert until that query publishes its first response.
    guard search.responseIsCurrent else { return }
    guard let hit = selectedHit, let root = search.resultRoot else { clearContext(); return }
    let url = root.url.appendingPathComponent(hit.entry.path).standardizedFileURL
    guard url != selectedURL else { return }
    selectedURL = url
    childFolderPane.isHidden = true
    preview.isHidden = false
    folderLibrary.watch([])
    contextItem = nil
    selectionChanged?(nil)
    preview.configure(nil)
    preview.titleField.stringValue = url.lastPathComponent
    preview.caption.stringValue = "Loading preview…"
    metadataTask?.cancel()
    metadataTask = Task { [weak self] in
      do { try await Task.sleep(for: .milliseconds(35)) } catch { return }
      let worker = Task.detached(priority: .userInitiated) { () -> Result<FileItem, Error> in
        Result { try DirectoryReader.item(at: url) }
      }
      let result = await withTaskCancellationHandler(operation: { await worker.value }, onCancel: { worker.cancel() })
      guard let self, !Task.isCancelled, self.selectedURL == url else { return }
      switch result {
      case .success(let file):
        self.contextItem = file
        self.updateSelectionPreview()
        self.selectionChanged?(file)
      case .failure(let error):
        if self.chosenNeighbor?.entry.path == self.selectedHit?.entry.path {
          self.chosenNeighbor = nil
          self.updateResults()
          return
        }
        self.preview.titleField.stringValue = url.lastPathComponent
        self.preview.caption.stringValue = error.localizedDescription
        self.preview.caption.toolTip = error.localizedDescription
      }
      self.renderContext(); self.didChange?()
    }
    let folder = FolderID(url.deletingLastPathComponent().path)
    if parent != folder {
      parent = folder
      siblingTask?.cancel()
      siblings = []; moreSiblings = false; siblingMessage = "Loading nearby entries…"
      if let cached = siblingCache.first(where: { $0.0 == folder }) {
        siblings = cached.1; moreSiblings = cached.2; siblingMessage = ""
      } else {
        siblingTask = Task { [weak self] in
          do { try await Task.sleep(for: .milliseconds(35)) } catch { return }
          let worker = Task.detached(priority: .utility) { () -> Result<(rows: [FileItem], hasMore: Bool), Error> in
            Result { try DirectoryReader.nearby(folder) }
          }
          let result = await withTaskCancellationHandler(operation: { await worker.value }, onCancel: { worker.cancel() })
          guard let self, !Task.isCancelled, self.parent == folder else { return }
          switch result {
          case .success(let (rows, hasMore)):
            self.siblings = rows; self.moreSiblings = hasMore; self.siblingMessage = ""
            self.siblingCache.removeAll { $0.0 == folder }
            self.siblingCache.append((folder, rows, hasMore))
            if self.siblingCache.count > 4 { self.siblingCache.removeFirst() }
          case .failure(let error): self.siblingMessage = error.localizedDescription
          }
          self.renderContext()
        }
      }
    }
    renderContext()
  }
  private func renderContext() {
    guard let root = search.isComputerSearch ? (search.volumeRoot ?? search.resultRoot) : search.root, let parent, let selectedURL else { return }
    var ancestors: [FolderID] = []
    var folder = parent
    while folder != root && ancestors.count < 3 {
      ancestors.insert(folder, at: 0)
      let next = FolderID(folder.url.deletingLastPathComponent().path)
      if next == folder { break }; folder = next
    }
    var rows: [ScopeRail.Row] = []
    if folder != parent {
      rows.append(.init(title: folder == root ? (root.rawValue == "/" ? "This Mac" : root == .home ? "Home" : root.url.lastPathComponent) : "… / " + folder.url.lastPathComponent,
        detail: "", depth: 0, selected: false, target: .scope(folder)))
    }
    for (i, ancestor) in ancestors.enumerated() {
      rows.append(.init(title: ancestor.url.lastPathComponent, detail: "", depth: i + 1, selected: false, target: .scope(ancestor)))
    }
    var entries = siblings
    if let contextItem, !entries.contains(where: { $0.id == contextItem.id }) { entries.insert(contextItem, at: 0) }
    rows += entries.map { file in
      .init(title: file.name, detail: file.kind.folder == nil ? "" : "/", depth: min(3, ancestors.count + 1),
        selected: file.id.rawValue == selectedURL.path, target: .neighbor(file))
    }
    if !siblingMessage.isEmpty { rows.append(.init(title: siblingMessage, detail: "", depth: 0, selected: false, target: nil)) }
    context.configure(rows, subtitle: moreSiblings ? "Nearby entries · first 80" : parent.rawValue)
  }
  override func layout() {
    super.layout()
    let searchWidth = min(600, max(0, bounds.width - 64))
    let searchLeft = (bounds.width - searchWidth) / 2
    searchBorder.frame = NSRect(x: searchLeft, y: 14, width: searchWidth, height: 48)
    input.frame = NSRect(x: searchLeft + 16, y: 23, width: searchWidth - 154, height: 32)
    searchScopeHint.frame = NSRect(x: searchLeft + searchWidth - 126, y: 30, width: 110, height: 18)
    if isHome {
      let top: CGFloat = 94
      let listWidth = bounds.width * 0.46
      topRule.frame = NSRect(x: 0, y: top, width: bounds.width, height: 1)
      recentHeading.frame = NSRect(x: 22, y: top + 20, width: listWidth - 44, height: 20)
      scroll.frame = NSRect(x: 6, y: top + (isHome ? 53 : 5), width: listWidth - 12, height: max(0, bounds.height - top - (isHome ? 59 : 52)))
      table.tableColumns.first?.width = scroll.contentSize.width
      rightRule.frame = NSRect(x: listWidth, y: top, width: 1, height: bounds.height - top)
      preview.frame = NSRect(x: listWidth + 1, y: top + 1, width: bounds.width - listWidth - 1, height: bounds.height - top - 1)
      empty.frame = NSRect(x: 22, y: top + 64, width: listWidth - 44, height: 60)
      childFolderPane.frame = preview.frame
      return
    }
    if isBrowsing {
      let top: CGFloat = 94
      let width = bounds.width / 2
      scope.frame = NSRect(x: 24, y: 69, width: bounds.width - 48, height: 19)
      topRule.frame = NSRect(x: 0, y: top, width: bounds.width, height: 1)
      currentFolderPane.frame = NSRect(x: 0, y: top + 1, width: width, height: bounds.height - top - 48)
      childFolderPane.frame = NSRect(x: width + 1, y: top + 1, width: bounds.width - width - 1, height: bounds.height - top - 48)
      preview.frame = childFolderPane.frame
      rightRule.frame = NSRect(x: width, y: top, width: 1, height: bounds.height - top - 47)
      bottomRule.frame = NSRect(x: 0, y: bounds.height - 47, width: bounds.width, height: 1)
      status.frame = NSRect(x: 16, y: bounds.height - 40, width: bounds.width - 32, height: 15)
      hint.frame = NSRect(x: 16, y: bounds.height - 21, width: bounds.width - 32, height: 15)
      return
    }
    let rail = min(250, max(180, bounds.width * 0.23))
    let results = (bounds.width - rail) * 0.43
    let top: CGFloat = 94
    let bottom = max(top, bounds.height - 47)
    let height = bottom - top
    scope.frame = NSRect(x: 24, y: 69, width: bounds.width - 48, height: 19)
    topRule.frame = NSRect(x: 0, y: top, width: bounds.width, height: 1)
    scopes.frame = NSRect(x: 0, y: top + 1, width: rail, height: height * 0.44)
    context.frame = NSRect(x: 0, y: top + height * 0.44 + 1, width: rail, height: height * 0.56 - 1)
    railRule.frame = NSRect(x: 0, y: context.frame.minY, width: rail, height: 1)
    leftRule.frame = NSRect(x: rail, y: top, width: 1, height: height)
    scroll.frame = NSRect(x: rail + 5, y: top + 5, width: results - 10, height: max(0, height - 10))
    table.tableColumns.first?.width = scroll.contentSize.width
    rightRule.frame = NSRect(x: rail + results, y: top, width: 1, height: height)
    preview.frame = NSRect(x: rail + results + 1, y: top + 1, width: bounds.width - rail - results - 1, height: height - 1)
    childFolderPane.frame = preview.frame
    empty.frame = NSRect(x: rail + 24, y: top + 90, width: results - 48, height: 100)
    bottomRule.frame = NSRect(x: 0, y: bottom, width: bounds.width, height: 1)
    status.frame = NSRect(x: 16, y: bottom + 7, width: bounds.width - 32, height: 15)
    hint.frame = NSRect(x: 16, y: bottom + 26, width: bounds.width - 32, height: 15)
  }
}

@MainActor
private final class SearchResultTable: NSTableView {
  var didDraw: ((TimeInterval) -> Void)?
  override func draw(_ dirtyRect: NSRect) {
    let start = ProcessInfo.processInfo.systemUptime
    super.draw(dirtyRect)
    didDraw?(start)
  }
  var hover: ((Int) -> Void)?
  private var area: NSTrackingArea?
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let area { removeTrackingArea(area) }
    let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
    self.area = area; addTrackingArea(area)
  }
  override func mouseMoved(with event: NSEvent) {
    guard event.deltaX != 0 || event.deltaY != 0 else { return }
    let row = row(at: convert(event.locationInWindow, from: nil))
    if row >= 0 && row != selectedRow { hover?(row) }
  }
}
@MainActor
private final class SearchResultRow: NSTableRowView {
  override func drawSelection(in dirtyRect: NSRect) {
    Design.selection.setFill()
    NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 6, yRadius: 6).fill()
  }
}

@MainActor
private final class SearchResultCell: NSTableCellView {
  private let name = SearchResultTitle(labelWithString: "")
  var didDrawTitle: ((TimeInterval) -> Void)? {
    didSet { name.didDraw = didDrawTitle }
  }
  private let path = label("", size: 11)
  private let icon = FileIconView()
  override init(frame: NSRect) {
    super.init(frame: frame)
    name.font = Design.font(14, .medium)
    name.textColor = Design.text
    name.maximumNumberOfLines = 1
    for field in [name, path] {
      field.lineBreakMode = .byTruncatingMiddle
      addSubview(field)
    }
    addSubview(icon)
  }
  convenience init() { self.init(frame: .zero) }
  required init?(coder: NSCoder) { nil }
  func configure(_ hit: SearchHit, selected: Bool) {
    configure(path: hit.entry.path, isFolder: hit.entry.isFolder, highlights: hit.highlights, selected: selected)
  }
  func configure(_ file: FileItem, selected: Bool) {
    let home = FolderID.home.rawValue + "/"
    let path = file.url.path.hasPrefix(home) ? "~/" + file.url.path.dropFirst(home.count) : file.url.path
    configure(path: path, isFolder: false, highlights: [], selected: selected)
  }
  private func configure(path displayPath: String, isFolder: Bool, highlights: [NSRange], selected: Bool) {
    let color = selected ? Design.selectionText : Design.text
    let fullPath = displayPath as NSString
    let filename = fullPath.lastPathComponent
    let filenameStart = fullPath.length - (filename as NSString).length
    let title = NSMutableAttributedString(
      string: filename, attributes: [.font: Design.font(14, .medium), .foregroundColor: color])
    let subtitle = NSMutableAttributedString(
      string: displayPath, attributes: [.font: Design.font(11), .foregroundColor: color])
    for range in highlights {
      subtitle.addAttribute(.font, value: Design.font(11, .bold), range: range)
      if range.location >= filenameStart {
        title.addAttributes(
          [.font: Design.font(14, .bold), .underlineStyle: NSUnderlineStyle.single.rawValue],
          range: NSRange(location: range.location - filenameStart, length: range.length))
      }
    }
    if !name.attributedStringValue.isEqual(to: title) { name.attributedStringValue = title }
    if !path.attributedStringValue.isEqual(to: subtitle) { path.attributedStringValue = subtitle }
    icon.configure(path: displayPath, isFolder: isFolder)
    toolTip = displayPath
    setAccessibilityLabel(displayPath)
  }
  override func layout() {
    super.layout()
    icon.frame = NSRect(x: 12, y: 15, width: 24, height: 24)
    name.frame = NSRect(x: 48, y: 28, width: bounds.width - 62, height: 20)
    path.frame = NSRect(x: 48, y: 8, width: bounds.width - 62, height: 17)
  }
}

@MainActor
private final class SearchResultTitle: NSTextField {
  var didDraw: ((TimeInterval) -> Void)?
  override func draw(_ dirtyRect: NSRect) {
    let start = ProcessInfo.processInfo.systemUptime
    super.draw(dirtyRect)
    didDraw?(start)
  }
}
