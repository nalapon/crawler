import AppKit

@MainActor
final class ActivityMonitorView: Surface, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
  private(set) var snapshot = ActivitySnapshot()
  private var sampling: Task<Void, Never>?
  private var generation = 0
  private var active = false
  private var appliedTheme = Design.theme
  private var updatedCells = 0
  private(set) var paused = false
  private(set) var sortMetric: ActivityMetric = .memory
  private(set) var mapMetric: ActivityMetric = .memory
  private var ascending = false
  private var sortByName = false
  private var syncingSelection = false
  private var groups: [ActivityGroup] = []
  private var rootNodes: [ActivityMapNode] = []
  private var rows: [ActivityListRow] = []
  private var expandedGroups: Set<String> = []
  private var selectedListID: ActivityMapNode.ID?
  private var matchingProcesses: [ActivityProcess] = []
  private var searchIdentities: Set<ActivitySearchIdentity> = []
  private var processSearchTexts: [ActivitySearchIdentity: FuzzySearchText] = [:]
  private var matchedProcessIDs: Set<ActivityProcessID> = []
  private var appliedFilter: String?
  private var filterEvaluations = 0
  private var selectedProcess: ActivityProcessID?
  private var selectedNode: ActivityMapNode?
  private var scope: [ActivityMapNode.ID] = []
  private var mapFocused = false
  private var pendingG = false
  var didChange: (() -> Void)?
  private let summary = label("Reading processes…", size: 12)
  private let listTitle = PaneTitle()
  private let mapTitle = PaneTitle()
  private let mapHint = label("Area = physical footprint · Double-click to explore", size: 11)
  private let filter = NSSearchField()
  private let table = ActivityProcessTable()
  private let scroll = NSScrollView()
  private let listPanel = Surface()
  private let mapPanel = Surface()
  let map = ActivityTreemap(frame: .zero)
  private let detailTitle = label("Select a process or a box", size: 13, weight: .semibold)
  private let detailValues = label("", size: 12)
  private let detailPath = label("", size: 11)
  private let boundary = label("Process breakdown. Libraries inside a process require a runtime profiler.", size: 11)
  /// Key hints for the window's status bar.
  let hints = "j/k Move    Tab Panel    Enter/l Expand/explore    h Collapse/back    / Filter    Space Pause    ⌘I Inspect    ⌘⌫ Quit process    1/2/3 Map    m/c/d Sort"
  private lazy var pause = ActionButton("Pause", symbol: "pause", help: "Freeze the current sample · Space") { [weak self] in self?.togglePause() }
  private lazy var back = ActionButton("All applications", symbol: "chevron.left", help: "Go up one level · h") { [weak self] in self?.goBack() }
  private let metrics = NSSegmentedControl(labels: ActivityMetric.allCases.map(\.rawValue), trackingMode: .selectOne, target: nil, action: nil)

  override init(frame: NSRect) {
    super.init(frame: frame)
    listTitle.title = "Applications & processes"
    mapTitle.title = "Memory by application"
    listTitle.active = true
    for view in [summary, filter, pause, listPanel, mapPanel] { addSubview(view) }
    for view in [listTitle, scroll] { listPanel.addSubview(view) }
    for view in [mapTitle, metrics, back, mapHint, map, detailTitle, detailValues, detailPath, boundary] { mapPanel.addSubview(view) }
    filter.placeholderString = "Filter processes · /"
    filter.delegate = self
    filter.sendsSearchStringImmediately = true
    filter.setAccessibilityLabel("Filter processes by name or PID")
    metrics.selectedSegment = 0
    metrics.target = self; metrics.action = #selector(metricChanged)
    metrics.setAccessibilityLabel("Resource represented by rectangle area")
    table.rowHeight = 42
    table.intercellSpacing = NSSize(width: 0, height: 0)
    table.usesAlternatingRowBackgroundColors = false
    table.backgroundColor = Design.background
    table.style = .plain
    table.allowsEmptySelection = true
    table.allowsMultipleSelection = false
    table.columnAutoresizingStyle = .noColumnAutoresizing
    table.dataSource = self; table.delegate = self
    table.focus = { [weak self] in self?.mapFocused = false; self?.updateFocus() }
    table.contextMenu = { [weak self] row in self?.processMenu(row: row) }
    table.target = self; table.doubleAction = #selector(enterSelection)
    table.setAccessibilityLabel("Running processes. Sort by clicking a column header.")
    for (id, title, width) in [("name", "Process", 170.0), ("cpu", "CPU", 66.0), ("memory", "Memory", 78.0), ("disk", "Disk/s", 78.0)] {
      let column = NSTableColumn(identifier: .init(id))
      column.title = title; column.width = width; column.minWidth = id == "name" ? 110 : 54
      column.resizingMask = []
      column.sortDescriptorPrototype = NSSortDescriptor(key: id, ascending: id == "name")
      table.addTableColumn(column)
    }
    scroll.documentView = table; scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true; scroll.drawsBackground = false
    scroll.horizontalScrollElasticity = .none; scroll.verticalScrollElasticity = .none
    map.choose = { [weak self] node, enter in
      guard let self else { return }
      self.mapFocused = true; self.select(node)
      if enter { self.explore(node) }
    }
    refreshTheme()
  }
  required init?(coder: NSCoder) { nil }

  func setActive(_ active: Bool) {
    if active, appliedTheme != Design.theme { refreshTheme() }
    guard self.active != active else { return }
    self.active = active
    if active {
      if sampling == nil && !paused { startSampling() }
    } else {
      generation += 1; sampling?.cancel(); sampling = nil
      snapshot.counters = [:]
    }
  }
  private func startSampling() {
    generation += 1
    let token = generation
    sampling = Task { [weak self] in
      while !Task.isCancelled {
        guard let self, self.generation == token else { return }
        let previous = self.snapshot.counters
        let work = Task.detached(priority: .utility) { ActivitySampler.read(previous: previous) }
        let result = await withTaskCancellationHandler { await work.value } onCancel: { work.cancel() }
        guard !Task.isCancelled, self.generation == token else { return }
        self.snapshot = result
        self.groups = ActivityGroup.collect(result.processes)
        self.refresh()
        try? await Task.sleep(for: .seconds(1))
      }
    }
  }
  func togglePause() {
    paused.toggle()
    generation += 1; sampling?.cancel(); sampling = nil
    snapshot.counters = [:]
    if !paused, active { startSampling() }
    refresh()
  }
  func refreshTheme() {
    appliedTheme = Design.theme
    table.backgroundColor = Design.background
    fill = Design.background; listPanel.fill = Design.background; mapPanel.fill = Design.background
    detailTitle.textColor = Design.text
    for title in [listTitle, mapTitle] { title.needsDisplay = true }
    detailTitle.font = Design.font(13, .semibold)
    for field in [summary, mapHint, detailPath, boundary] { field.textColor = Design.muted; field.font = Design.font(field === summary ? 12 : 11) }
    detailValues.textColor = Design.text; detailValues.font = Design.font(12)
    filter.font = Design.font(12); metrics.font = Design.font(11)
    for button in [pause, back] { button.font = Design.font(11); button.contentTintColor = Design.text }
    for column in table.tableColumns { column.headerCell.font = Design.font(11, .medium); column.headerCell.textColor = Design.text }
    table.reloadData(); syncTableSelection(); map.rebuild(force: true); needsDisplay = true; needsLayout = true
  }
  private func refresh(animateDisclosure: Bool = false) {
    let previousRows = rows
    let query = filter.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    updateProcessMatches(query)
    let filteredGroups = groups.compactMap { group -> ActivityGroup? in
      let processes = group.processes.filter { matchedProcessIDs.contains($0.id) }
      return processes.isEmpty ? nil : ActivityGroup(key: group.key, name: group.name, processes: processes)
    }
    matchingProcesses = filteredGroups.flatMap(\.processes)
    expandedGroups.formIntersection(Set(groups.map(\.key)))
    func ordered(_ entries: [ActivityListRow]) -> [ActivityListRow] { entries.sorted { lhs, rhs in
      let nameOrder = lhs.name.localizedStandardCompare(rhs.name)
      let tieBreak: Bool
      switch (lhs.item, rhs.item) {
      case (.process(let l, _), .process(let r, _)): tieBreak = l.id.pid < r.id.pid
      default: tieBreak = lhs.groupKey < rhs.groupKey
      }
      if sortByName {
        if nameOrder == .orderedSame { return tieBreak }
        return ascending ? nameOrder == .orderedAscending : nameOrder == .orderedDescending
      }
      let l = lhs.values[sortMetric], r = rhs.values[sortMetric]
      if l == r { return nameOrder == .orderedSame ? tieBreak : nameOrder == .orderedAscending }
      guard let l else { return false }; guard let r else { return true }
      return ascending ? l < r : l > r
    } }
    rows = ordered(filteredGroups.map { ActivityListRow(.group($0, expanded: expandedGroups.contains($0.key))) }).flatMap { row in
      guard case .group(let group, let expanded) = row.item, group.processes.count > 1, expanded else { return [row] }
      return [row] + ordered(group.processes.map { ActivityListRow(.process($0, group: group.key)) })
    }
    rootNodes = buildRoots()
    if selectedListID == nil && selectedProcess == nil && selectedNode == nil, let first = rows.first { selectListRow(first) }
    if let selected = rows.first(where: { $0.id == selectedListID }), case .group = selected.item {
      selectListRow(selected)
    }
    updateTable(previous: previousRows, animateDisclosure: animateDisclosure)
    syncTableSelection()
    let pauseTitle = paused ? "Resume" : "Pause"
    if pause.title != pauseTitle { pause.title = pauseTitle; pause.image = Design.symbol(paused ? "play" : "pause") }
    setText(summary, snapshot.failure ?? (snapshot.processes.isEmpty ? "Reading processes…" :
      "\(snapshot.processes.count) processes · \(paused ? "Paused" : "Live · 1 s") · \(snapshot.unavailable) readings unavailable"))
    summary.toolTip = "Sampled at \(snapshot.sampledAt.formatted(date: .omitted, time: .standard)) · \(String(format: "%.1f", snapshot.milliseconds)) ms to sample. CPU uses 100% per core. Disk includes reads and writes."
    updateMap(); updateDetails(); didChange?()
  }
  private func updateProcessMatches(_ query: String) {
    let identities = Set(groups.flatMap { group in
      group.processes.map { ActivitySearchIdentity(id: $0.id, name: $0.name, application: group.name) }
    })
    guard appliedFilter != query || searchIdentities != identities else { return }
    appliedFilter = query; searchIdentities = identities
    processSearchTexts = processSearchTexts.filter { identities.contains($0.key) }
    if query.isEmpty {
      matchedProcessIDs = Set(identities.map(\.id)); return
    }
    filterEvaluations += 1
    guard let pattern = FuzzyQuery(query) else { matchedProcessIDs = []; return }
    var matches: Set<ActivityProcessID> = []
    for identity in identities {
      let text: FuzzySearchText
      if let cached = processSearchTexts[identity] { text = cached }
      else {
        text = FuzzySearchText("\(identity.application) \(identity.name) \(identity.id.pid)")
        processSearchTexts[identity] = text
      }
      if pattern.matches(text) { matches.insert(identity.id) }
    }
    matchedProcessIDs = matches
  }
  private func setText(_ field: NSTextField, _ text: String) {
    if field.stringValue != text { field.stringValue = text }
  }
  private func updateTable(previous: [ActivityListRow], animateDisclosure: Bool) {
    syncingSelection = true
    defer { syncingSelection = false }
    if animateDisclosure {
      var prefix = 0
      while prefix < min(previous.count, rows.count), previous[prefix].id == rows[prefix].id { prefix += 1 }
      var suffix = 0
      while suffix < min(previous.count, rows.count) - prefix,
        previous[previous.count - suffix - 1].id == rows[rows.count - suffix - 1].id { suffix += 1 }
      let removed = IndexSet(integersIn: prefix..<(previous.count - suffix))
      let inserted = IndexSet(integersIn: prefix..<(rows.count - suffix))
      NSAnimationContext.runAnimationGroup { context in
        context.duration = inserted.isEmpty ? 0.20 : 0.24
        context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        table.beginUpdates()
        if !removed.isEmpty { table.removeRows(at: removed, withAnimation: .slideUp) }
        if !inserted.isEmpty { table.insertRows(at: inserted, withAnimation: .slideDown) }
        table.endUpdates()
      }
    } else if previous.count != rows.count { table.noteNumberOfRowsChanged() }
    let previousByID = animateDisclosure ? Dictionary(uniqueKeysWithValues: previous.map { ($0.id, $0) }) : [:]
    let visible = table.rows(in: scroll.contentView.documentVisibleRect)
    guard visible.location != NSNotFound else { return }
    let range = visible.location..<min(rows.count, NSMaxRange(visible))
    for (column, identifier) in table.tableColumns.enumerated() {
      var changed = IndexSet()
      for index in range {
        let new = rows[index]
        let old: ActivityListRow
        if animateDisclosure {
          guard let retained = previousByID[new.id] else { continue }
          old = retained
        } else {
          guard previous.indices.contains(index), previous[index].id == new.id else { changed.insert(index); continue }
          old = previous[index]
        }
        let columnID = identifier.identifier.rawValue
        let different = old.text(columnID) != new.text(columnID) || (columnID == "name" && old.subtitle != new.subtitle)
        if different { changed.insert(index) }
      }
      if !changed.isEmpty {
        updatedCells += changed.count
        table.reloadData(forRowIndexes: changed, columnIndexes: IndexSet(integer: column))
      }
    }
  }
  private func buildRoots() -> [ActivityMapNode] {
    let matching = Set(matchingProcesses.map(\.id))
    let nodes = groups.compactMap { group -> ActivityMapNode? in
      let processes = group.processes.filter { matching.contains($0.id) }
      guard !processes.isEmpty else { return nil }
      let children = ActivityMapNode.forest(processes, metric: mapMetric)
      return ActivityMapNode(id: .group(group.key), title: group.name,
        value: processes.reduce(0) { $0 + (mapMetric.value($1) ?? 0) }, children: children, process: nil)
    }.sorted { $0.value == $1.value ? $0.title < $1.title : $0.value > $1.value }
    guard nodes.count > 14 else { return nodes }
    let rest = Array(nodes.dropFirst(14))
    return Array(nodes.prefix(14)) + [ActivityMapNode(id: .other, title: "Other processes", value: rest.reduce(0) { $0 + $1.value }, children: rest, process: nil)]
  }
  private func find(_ id: ActivityMapNode.ID, in nodes: [ActivityMapNode]) -> ActivityMapNode? {
    if case .remainder(let parent) = id {
      guard let source = find(parent, in: rootNodes) else { return nil }
      return ActivityMapNode.limited(source.children, parent: parent).first { $0.id == id }
    }
    for node in nodes {
      if node.id == id { return node }
      if let found = find(id, in: node.children) { return found }
    }
    return nil
  }
  private func updateMap() {
    var nodes = rootNodes
    var valid: [ActivityMapNode.ID] = []
    var title = "\(mapMetric.rawValue) by application"
    for id in scope {
      guard let node = find(id, in: nodes), !node.children.isEmpty else { break }
      valid.append(id); nodes = node.children; title = node.title
    }
    scope = valid
    if let parent = scope.last { nodes = ActivityMapNode.limited(nodes, parent: parent) }
    map.update(nodes: nodes, metric: mapMetric, showChildren: !scope.isEmpty)
    if mapTitle.title != title { mapTitle.title = title }
    let backTitle = scope.isEmpty ? "All applications" : "Back · \(scope.count) level\(scope.count == 1 ? "" : "s")"
    if back.title != backTitle { back.title = backTitle }
    if back.isEnabled != !scope.isEmpty { back.isEnabled = !scope.isEmpty }
    setText(mapHint, mapMetric == .memory ? "Area = physical footprint · Includes compressed memory" :
      mapMetric == .cpu ? "Area = measured CPU time · 100% = one core" : "Area = disk reads + writes per second")
    if let selectedNode { map.selected = selectedNode.id }
    else if scope.isEmpty, let id = selectedProcess,
      let group = groups.first(where: { $0.processes.contains { $0.id == id } }) {
      map.selected = rootNodes.contains(where: { $0.id == .group(group.key) }) ? .group(group.key) : .other
    } else { map.selected = selectedProcess.map { .process($0) } }
  }
  private func syncTableSelection() {
    let previous = syncingSelection
    syncingSelection = true
    defer { syncingSelection = previous }
    if let index = rows.firstIndex(where: { row in
      if row.id == selectedListID { return true }
      if case .group(let group, _) = row.item, group.processes.count == 1 {
        return group.processes.first?.id == selectedProcess
      }
      return false
    }) {
      if table.selectedRow != index { table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
    } else if table.selectedRow != -1 { table.deselectAll(nil) }
  }
  private func selectListRow(_ row: ActivityListRow) {
    selectedListID = row.id
    switch row.item {
    case .group(let group, _):
      selectedProcess = group.processes.count == 1 ? group.processes.first?.id : nil
      selectedNode = selectedProcess == nil ? find(.group(group.key), in: rootNodes) : nil
    case .process(let process, _):
      selectedProcess = process.id; selectedNode = nil
    }
  }
  private func collapseSelection() {
    guard let row = rows.first(where: { $0.id == selectedListID }) else { return }
    guard expandedGroups.contains(row.groupKey) else { return }
    expandedGroups.remove(row.groupKey)
    selectedListID = .group(row.groupKey); selectedProcess = nil
    selectedNode = find(.group(row.groupKey), in: rootNodes)
    refresh(animateDisclosure: true)
    if let index = rows.firstIndex(where: { $0.id == selectedListID }) { table.scrollRowToVisible(index) }
  }
  private func select(_ node: ActivityMapNode) {
    selectedNode = node
    selectedProcess = node.process
    selectedListID = node.process.map { .process($0) } ?? node.id
    syncTableSelection()
    map.selected = node.id
    updateDetails(); updateFocus(); didChange?()
  }
  private func updateDetails() {
    let process = selectedProcess.flatMap { id in snapshot.processes.first { $0.id == id } }
    if let selectedNode, selectedNode.process == nil {
      if case .group = selectedNode.id, let row = rows.first(where: { $0.id == selectedNode.id }) {
        setText(detailTitle, row.name)
        var values = ["\(row.text("memory")) memory", "\(row.text("cpu")) CPU"]
        if row.values[.disk] != 0 { values.append("\(row.text("disk")) disk") }
        setText(detailValues, values.joined(separator: "   "))
        setText(detailPath, "\(row.subtitle) · Enter in the list to expand; double-click the box to explore.")
        return
      }
      if let node = find(selectedNode.id, in: rootNodes) {
        setText(detailTitle, node.title)
        setText(detailValues, "\(mapMetric.formatted(node.value)) · \(String(format: "%.1f", node.value / max(1, rootNodes.reduce(0) { $0 + $1.value }) * 100))% of measured \(mapMetric.rawValue.lowercased())")
        setText(detailPath, "Enter or double-click to see its processes.")
        return
      }
    }
    guard let process else {
      setText(detailTitle, selectedProcess == nil ? "No processes match" : "This process has exited")
      setText(detailValues, ""); setText(detailPath, ""); return
    }
    if process.id.started == 0 {
      setText(detailTitle, "PID \(process.id.pid) · Restricted process")
      setText(detailValues, "macOS did not permit reading this process.")
      setText(detailPath, "Name, parent and resource usage are unavailable.")
      return
    }
    setText(detailTitle, "\(process.name) · PID \(process.id.pid) · Parent \(process.parent)")
    var values = ["\(ActivityMetric.memory.formatted(process.memory.map { Double($0) })) memory", "\(ActivityMetric.cpu.formatted(process.cpu)) CPU"]
    if process.readRate != 0 { values.append("\(ActivityMetric.disk.formatted(process.readRate)) read") }
    if process.writeRate != 0 { values.append("\(ActivityMetric.disk.formatted(process.writeRate)) write") }
    setText(detailValues, values.joined(separator: "   "))
    setText(detailPath, process.executable.isEmpty ? "Executable path unavailable" : process.executable)
    if detailPath.toolTip != detailPath.stringValue { detailPath.toolTip = detailPath.stringValue }
  }
  private func updateFocus() {
    map.keyboardFocus = mapFocused
    listTitle.active = !mapFocused
    mapTitle.active = mapFocused
  }
  private func explore(_ node: ActivityMapNode) {
    guard !node.children.isEmpty else { return }
    scope.append(node.id); updateMap()
    if let first = map.visibleNodes.first { select(first) }
    updateDetails(); updateFocus(); didChange?()
  }
  private func goBack() {
    guard !scope.isEmpty else { mapFocused = false; updateFocus(); return }
    scope.removeLast(); selectedNode = nil; updateMap(); updateDetails(); didChange?()
  }
  @objc private func metricChanged() { setMapMetric(ActivityMetric.allCases[metrics.selectedSegment]) }
  private func setMapMetric(_ metric: ActivityMetric) { mapMetric = metric; metrics.selectedSegment = ActivityMetric.allCases.firstIndex(of: metric) ?? 0; rootNodes = buildRoots(); updateMap(); updateDetails(); didChange?() }
  private func setSort(_ metric: ActivityMetric) {
    if sortMetric == metric && !sortByName { ascending.toggle() } else { sortMetric = metric; ascending = false }
    sortByName = false
    refresh()
  }
  func controlTextDidChange(_ notification: Notification) { scope = []; selectedNode = nil; selectedProcess = nil; selectedListID = nil; refresh() }
  func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
    guard rows.indices.contains(row), let column = tableColumn else { return nil }
    let entry = rows[row]
    let cell = (table.makeView(withIdentifier: column.identifier, owner: self) as? ActivityProcessCell) ?? ActivityProcessCell()
    cell.identifier = column.identifier
    cell.configure(entry, column: column.identifier.rawValue)
    cell.toggleDisclosure = { [weak self] in
      guard let self, let row = self.rows.first(where: { $0.id == entry.id }) else { return }
      self.mapFocused = false
      self.selectListRow(row); self.syncTableSelection()
      self.window?.makeFirstResponder(self)
      self.enterSelection()
    }
    return cell
  }
  func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { ActivityProcessRow() }
  func tableViewSelectionDidChange(_ notification: Notification) {
    guard !syncingSelection, rows.indices.contains(table.selectedRow) else { return }
    selectListRow(rows[table.selectedRow]); updateMap()
    updateDetails(); didChange?()
  }
  func tableView(_ tableView: NSTableView, sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]) {
    guard let descriptor = table.sortDescriptors.first else { return }
    if let metric = ActivityMetric.allCases.first(where: { $0.rawValue.lowercased() == descriptor.key }) {
      sortMetric = metric; sortByName = false; ascending = descriptor.ascending; refresh()
    } else if descriptor.key == "name" {
      sortByName = true; ascending = descriptor.ascending; refresh()
    }
  }
  private var actionableProcess: ActivityProcess? {
    if let selectedProcess { return snapshot.processes.first { $0.id == selectedProcess } }
    if let row = rows.first(where: { $0.id == selectedListID }),
      case .group(let group, _) = row.item, group.processes.count == 1 { return group.processes.first }
    return nil
  }
  private func processMenu(row: Int) -> NSMenu? {
    guard rows.indices.contains(row) else { return nil }
    mapFocused = false
    selectListRow(rows[row]); syncTableSelection(); updateMap(); updateDetails(); updateFocus()
    window?.makeFirstResponder(self)
    let menu = NSMenu()
    menu.autoenablesItems = false
    let inspect = NSMenuItem(title: "Inspect…", action: #selector(inspectSelection), keyEquivalent: "i")
    inspect.target = self; menu.addItem(inspect)
    if case .group(let group, _) = rows[row].item, group.processes.count > 1 {
      let expand = NSMenuItem(title: expandedGroups.contains(group.key) ? "Collapse Processes" : "Show Processes", action: #selector(enterSelection), keyEquivalent: "")
      expand.target = self; menu.addItem(expand)
    }
    menu.addItem(.separator())
    let quit = NSMenuItem(title: "Quit Process…", action: #selector(quitSelection), keyEquivalent: "\u{8}")
    quit.target = self; quit.isEnabled = actionableProcess.map { $0.id.pid > 1 && $0.id.started > 0 } ?? false
    if actionableProcess == nil { quit.toolTip = "Expand the group and select an individual process." }
    menu.addItem(quit)
    return menu
  }
  @objc private func inspectSelection() {
    guard let window else { return }
    let title: String
    let details: String
    if let process = actionableProcess {
      title = "\(process.name) · PID \(process.id.pid)"
      let started = process.id.started == 0 ? "Unknown" : Date(timeIntervalSince1970: Double(process.id.started) / 1_000_000).formatted()
      details = "PID: \(process.id.pid)\nParent PID: \(process.id.started == 0 ? "Unknown" : String(process.parent))\nStarted: \(started)\n\nMemory (physical footprint): \(ActivityMetric.memory.formatted(process.memory.map { Double($0) }))\nCPU: \(ActivityMetric.cpu.formatted(process.cpu))\nDisk reads: \(ActivityMetric.disk.formatted(process.readRate))\nDisk writes: \(ActivityMetric.disk.formatted(process.writeRate))\n\nExecutable:\n\(process.executable.isEmpty ? "Unavailable" : process.executable)"
    } else if let row = rows.first(where: { $0.id == selectedListID }), case .group(let group, _) = row.item {
      title = group.name
      details = group.processes.map { process in
        "\(process.name) · PID \(process.id.pid)\nMemory: \(ActivityMetric.memory.formatted(process.memory.map { Double($0) }))   CPU: \(ActivityMetric.cpu.formatted(process.cpu))\n\(process.executable.isEmpty ? "Executable unavailable" : process.executable)"
      }.joined(separator: "\n\n")
    } else { NSSound.beep(); return }
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = "Sampled at \(snapshot.sampledAt.formatted(date: .omitted, time: .standard)). Includes compressed memory."
    alert.addButton(withTitle: "Close")
    let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 520, height: 280))
    scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
    let text = NSTextView(frame: scroll.bounds)
    text.isEditable = false; text.isSelectable = true
    text.font = Design.font(12); text.textColor = Design.text; text.backgroundColor = Design.background
    text.textContainerInset = NSSize(width: 8, height: 8)
    text.isVerticallyResizable = true; text.isHorizontallyResizable = false
    text.autoresizingMask = [.width]; text.textContainer?.widthTracksTextView = true
    text.string = details; text.setAccessibilityLabel("Process inspection")
    scroll.documentView = text; alert.accessoryView = scroll
    alert.beginSheetModal(for: window)
  }
  @objc private func quitSelection() {
    guard let window else { return }
    guard let process = actionableProcess else {
      let alert = NSAlert(); alert.messageText = "Select an individual process"
      alert.informativeText = "Expand the application group and select the process you want to quit."
      alert.beginSheetModal(for: window); return
    }
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = "Quit \(process.name) (PID \(process.id.pid))?"
    alert.informativeText = "Quit requests that the process exit. Force Quit stops it immediately and may lose unsaved work."
    alert.addButton(withTitle: "Quit")
    alert.addButton(withTitle: "Cancel").keyEquivalent = "\u{1b}"
    alert.addButton(withTitle: "Force Quit").keyEquivalent = ""
    alert.beginSheetModal(for: window) { [weak self] response in
      let mode: ActivityProcessTermination
      switch response {
      case .alertFirstButtonReturn: mode = .quit
      case .alertThirdButtonReturn: mode = .forceQuit
      default: return
      }
      self?.terminate(process, mode: mode)
    }
  }
  private func terminate(_ process: ActivityProcess, mode: ActivityProcessTermination) {
    generation += 1; sampling?.cancel(); sampling = nil
    let token = generation, counters = snapshot.counters
    Task { [weak self] in
      do {
        let result = try await Task.detached(priority: .userInitiated) {
          try ActivityProcessControl.terminate(process.id, mode: mode)
          return ActivitySampler.read(previous: counters)
        }.value
        guard let self, self.generation == token else { return }
        self.snapshot = result; self.groups = ActivityGroup.collect(result.processes); self.refresh()
        if self.active && !self.paused { self.startSampling() }
      } catch {
        guard let self else { return }
        if let window = self.window {
          let alert = NSAlert(); alert.messageText = "Could not quit \(process.name)"
          alert.informativeText = error.localizedDescription; alert.beginSheetModal(for: window) { _ in }
        }
        if self.generation == token, self.active && !self.paused { self.startSampling() }
      }
    }
  }
  @objc private func enterSelection() {
    guard let row = rows.first(where: { $0.id == selectedListID }) else { return }
    if case .group(let group, _) = row.item, group.processes.count > 1 {
      if expandedGroups.contains(group.key) { expandedGroups.remove(group.key) }
      else { expandedGroups.insert(group.key) }
      refresh(animateDisclosure: true); return
    }
    guard let id = selectedProcess, let group = groups.first(where: { $0.processes.contains { $0.id == id } }),
      let node = find(.group(group.key), in: rootNodes) else { return }
    mapFocused = true; scope = rootNodes.contains(where: { $0.id == node.id }) ? [] : [.other]; explore(node)
    if let process = find(.process(id), in: map.visibleNodes) { select(process) }
  }
  private func move(_ delta: Int, edge: Bool = false) {
    if mapFocused { if let node = map.move(delta, edge: edge) { select(node) }; return }
    guard !rows.isEmpty else { return }
    let old = rows.firstIndex { $0.id == selectedListID } ?? 0
    let next = edge ? (delta < 0 ? 0 : rows.count - 1) : max(0, min(rows.count - 1, old + delta))
    selectListRow(rows[next]); syncTableSelection()
    table.scrollRowToVisible(next)
    updateMap(); updateDetails(); didChange?()
  }
  func handleKey(_ event: NSEvent, vim: Bool) -> Bool {
    let flags = event.modifierFlags
    let key = event.charactersIgnoringModifiers ?? ""
    let editing = window?.firstResponder is NSTextView
    if flags.contains(.command) {
      if !editing, key == "i" { inspectSelection(); return true }
      if !editing, event.keyCode == 51 { quitSelection(); return true }
      if key == "r" { if !paused { generation += 1; sampling?.cancel(); sampling = nil; startSampling() }; return true }
      return false
    }
    if event.keyCode == 53 {
      if editing { window?.makeFirstResponder(self) }
      else if !scope.isEmpty { goBack() }
      else if !filter.stringValue.isEmpty { filter.stringValue = ""; refresh() }
      else { mapFocused = false; updateFocus() }
      return true
    }
    if editing { return false }
    if flags.contains(.control) {
      if vim && ["d", "u"].contains(key) { move((key == "d" ? 1 : -1) * max(1, Int(scroll.contentView.bounds.height / table.rowHeight / 2))); return true }
      return false
    }
    if event.keyCode == 48 {
      mapFocused.toggle(); window?.makeFirstResponder(self)
      if mapFocused {
        let node = map.navigable.first { $0.node.id == map.selected }?.node ?? map.visibleNodes.first
        if let node { select(node) }
      }
      updateFocus(); return true
    }
    if event.keyCode == 49 { togglePause(); return true }
    if vim && ["/", "i"].contains(key) { window?.makeFirstResponder(filter); return true }
    if let number = Int(key), (1...3).contains(number) { setMapMetric(ActivityMetric.allCases[number - 1]); return true }
    if vim && ["m", "c", "d"].contains(key) { setSort(key == "m" ? .memory : key == "c" ? .cpu : .disk); return true }
    if vim && key == "g" {
      if pendingG { move(-1, edge: true); pendingG = false } else { pendingG = true }
      return true
    }
    pendingG = false
    if vim && key == "G" { move(1, edge: true); return true }
    if event.keyCode == 125 || (vim && key == "j") { move(1); return true }
    if event.keyCode == 126 || (vim && key == "k") { move(-1); return true }
    if event.keyCode == 123 || (vim && key == "h") {
      if mapFocused { goBack() } else { collapseSelection() }
      return true
    }
    if event.keyCode == 36 || event.keyCode == 124 || (vim && key == "l") {
      if mapFocused { if let node = selectedNode { explore(node) } }
      else if event.keyCode != 36, let row = rows.first(where: { $0.id == selectedListID }),
        case .group(let group, let expanded) = row.item, group.processes.count > 1, expanded { move(1) }
      else { enterSelection() }
      return true
    }
    return false
  }
  override var acceptsFirstResponder: Bool { true }
  override func layout() {
    super.layout()
    let w = bounds.width, h = bounds.height
    // A toolbar row, then two flush panes whose title strips show which one has the keyboard.
    summary.frame = NSRect(x: 16, y: 13, width: max(0, w - 420), height: 18)
    filter.frame = NSRect(x: w - 380, y: 8, width: 260, height: 28)
    pause.frame = NSRect(x: w - 108, y: 7, width: 92, height: 30)
    let panelY: CGFloat = 44, panelH = max(100, h - panelY)
    let panelWidth = floor(w / 2)
    listPanel.frame = NSRect(x: 0, y: panelY, width: panelWidth, height: panelH)
    mapPanel.frame = NSRect(x: panelWidth + 1, y: panelY, width: w - panelWidth - 1, height: panelH)
    listTitle.frame = NSRect(x: 0, y: 0, width: panelWidth, height: PaneTitle.height)
    scroll.frame = NSRect(x: 1, y: PaneTitle.height + 2, width: panelWidth - 2, height: panelH - PaneTitle.height - 2)
    let available = scroll.contentSize.width
    let numeric = min(84, max(64, (available - 140) / 3))
    for column in table.tableColumns { column.width = column.identifier.rawValue == "name" ? max(110, available - numeric * 3) : numeric }
    let mw = mapPanel.bounds.width
    mapTitle.frame = NSRect(x: 0, y: 0, width: mw, height: PaneTitle.height)
    metrics.frame = NSRect(x: mw - 190, y: 3, width: 182, height: 24)
    back.frame = NSRect(x: 3, y: PaneTitle.height + 6, width: mw - 12, height: 25); back.alignment = .left
    mapHint.frame = NSRect(x: 12, y: PaneTitle.height + 36, width: mw - 24, height: 18)
    map.frame = NSRect(x: 8, y: PaneTitle.height + 60, width: mw - 16, height: max(70, panelH - PaneTitle.height - 184))
    let detailY = panelH - 116
    detailTitle.frame = NSRect(x: 12, y: detailY, width: mw - 24, height: 22)
    detailValues.frame = NSRect(x: 12, y: detailY + 27, width: mw - 24, height: 34)
    detailValues.maximumNumberOfLines = 2; detailValues.lineBreakMode = .byWordWrapping
    detailPath.frame = NSRect(x: 12, y: detailY + 63, width: mw - 24, height: 18)
    boundary.frame = NSRect(x: 12, y: detailY + 86, width: mw - 24, height: 28)
    boundary.maximumNumberOfLines = 2; boundary.lineBreakMode = .byWordWrapping
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    (Design.classic ? NSColor.black : Design.line).setFill()
    NSRect(x: 0, y: 43, width: bounds.width, height: 1).fill()
    NSRect(x: listPanel.frame.maxX, y: 44, width: 1, height: bounds.height - 44).fill()
  }
  var diagnostics: [String: Any] {
    ["process_count": snapshot.processes.count, "unavailable": snapshot.unavailable, "paused": paused,
     "sample_ms": snapshot.milliseconds, "sample_epoch": snapshot.sampledAt.timeIntervalSince1970,
     "map_metric": mapMetric.rawValue, "sort_metric": sortByName ? "Name" : sortMetric.rawValue, "scope_depth": scope.count,
     "map_focused": mapFocused, "selected_pid": selectedProcess?.pid ?? -1, "filter": filter.stringValue,
     "list_row_count": rows.count, "group_count": groups.count, "expanded_group_count": expandedGroups.count,
     "filter_evaluations": filterEvaluations, "filter_match_count": matchedProcessIDs.count,
     "list_panel_width": listPanel.bounds.width, "map_panel_width": mapPanel.bounds.width,
     "map_tile_count": map.tiles.count, "map_redraws": map.redrawCount, "map_layouts": map.layoutCount, "updated_cells": updatedCells,
     "processes": snapshot.processes.map { p -> [String: Any] in
       ["pid": p.id.pid, "parent": p.parent, "name": p.name, "memory": p.memory.map { Double($0) } ?? -1,
        "cpu": p.cpu ?? -1, "read_per_second": p.readRate ?? -1, "write_per_second": p.writeRate ?? -1]
     }]
  }
}

private struct ActivitySearchIdentity: Hashable {
  let id: ActivityProcessID
  let name: String
  let application: String
}

/// Group totals are computed once per sample; recycled cells compare only their displayed text.
private struct ActivityListRow {
  enum Item {
    case group(ActivityGroup, expanded: Bool)
    case process(ActivityProcess, group: String)
  }
  let item: Item
  let values: [ActivityMetric: Double]
  let partial: Set<ActivityMetric>
  init(_ item: Item) {
    self.item = item
    let processes: [ActivityProcess]
    switch item {
    case .group(let group, _): processes = group.processes
    case .process(let process, _): processes = [process]
    }
    var values: [ActivityMetric: Double] = [:]
    var partial: Set<ActivityMetric> = []
    for metric in ActivityMetric.allCases {
      var count = 0
      for process in processes {
        if let value = metric.value(process) { values[metric, default: 0] += value; count += 1 }
      }
      if count > 0 && count < processes.count { partial.insert(metric) }
    }
    self.values = values; self.partial = partial
  }
  var id: ActivityMapNode.ID {
    switch item {
    case .group(let group, _): .group(group.key)
    case .process(let process, _): .process(process.id)
    }
  }
  var groupKey: String {
    switch item {
    case .group(let group, _): group.key
    case .process(_, let group): group
    }
  }
  var name: String {
    switch item {
    case .group(let group, _): group.name
    case .process(let process, _): process.name
    }
  }
  var subtitle: String {
    switch item {
    case .group(let group, _): group.processes.count == 1
      ? "PID \(group.processes[0].id.pid)"
      : "\(group.processes.count) processes\(partial.isEmpty ? "" : " · * partial readings")"
    case .process(let process, _): "PID \(process.id.pid)"
    }
  }
  var isChild: Bool { if case .process = item { return true }; return false }
  func text(_ column: String) -> String {
    if column == "name" {
      if case .group(let group, let expanded) = item, group.processes.count > 1 { return "\(expanded ? "▾" : "▸") \(name)" }
      return name
    }
    let metric: ActivityMetric = column == "cpu" ? .cpu : column == "memory" ? .memory : .disk
    if metric == .disk, values[metric] == 0 { return partial.contains(metric) ? "*" : "" }
    return metric.formatted(values[metric]) + (partial.contains(metric) ? " *" : "")
  }
}

@MainActor
private final class ActivityProcessCell: NSTableCellView {
  private let title = label("", size: 12)
  private let subtitle = label("", size: 10)
  private let disclosure = NSButton()
  var toggleDisclosure: (() -> Void)?
  private var nameColumn = false
  private var indented = false
  override init(frame: NSRect) {
    super.init(frame: frame)
    addSubview(title); addSubview(subtitle); addSubview(disclosure)
    disclosure.isBordered = false
    disclosure.setButtonType(.momentaryChange)
    disclosure.target = self; disclosure.action = #selector(toggleGroup)
    disclosure.font = Design.font(12)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func toggleGroup() { toggleDisclosure?() }
  func configure(_ row: ActivityListRow, column: String) {
    nameColumn = column == "name"
    indented = nameColumn && row.isChild
    disclosure.isHidden = true
    if nameColumn, case .group(let group, let expanded) = row.item, group.processes.count > 1 {
      disclosure.isHidden = false
      disclosure.title = expanded ? "▾" : "▸"
      disclosure.setAccessibilityLabel("\(expanded ? "Collapse" : "Expand") \(row.name)")
    }
    title.stringValue = nameColumn ? row.name : row.text(column)
    subtitle.stringValue = row.subtitle
    subtitle.isHidden = !nameColumn
    title.alignment = nameColumn ? .left : .right
    title.font = Design.font(nameColumn ? 12 : 11, nameColumn ? .medium : .regular)
    subtitle.font = Design.font(10)
    updateColors(); needsLayout = true
    setAccessibilityElement(disclosure.isHidden); setAccessibilityRole(.staticText)
    setAccessibilityLabel(row.text(column) + (nameColumn ? ", \(row.subtitle)" : ""))
    switch row.item {
    case .group(let group, _):
      if group.processes.count == 1, let process = group.processes.first {
        toolTip = nameColumn ? "\(row.name) · PID \(process.id.pid)\n\(process.executable)" : title.stringValue
      } else {
        toolTip = nameColumn ? "\(row.name) · \(row.subtitle)\nClick the arrow or press Enter to expand or collapse. * marks a total with missing readings." : title.stringValue
      }
    case .process(let process, _): toolTip = nameColumn ? "\(process.name) · PID \(process.id.pid)\n\(process.executable)" : title.stringValue
    }
    if column == "disk", row.values[.disk] == 0 { toolTip = row.partial.contains(.disk) ? "No measured disk activity; some readings are unavailable." : "No disk reads or writes in this sample." }
  }
  override var backgroundStyle: NSView.BackgroundStyle { didSet { updateColors() } }
  private func updateColors() {
    title.textColor = backgroundStyle == .emphasized ? Design.selectionText : Design.text
    subtitle.textColor = backgroundStyle == .emphasized ? Design.selectionText : Design.muted
    disclosure.contentTintColor = title.textColor
  }
  override func layout() {
    super.layout()
    let inset: CGFloat = indented || !disclosure.isHidden ? 24 : 8
    disclosure.frame = NSRect(x: 2, y: 16, width: 20, height: 20)
    title.frame = NSRect(x: inset, y: nameColumn ? 17 : 12, width: bounds.width - inset - 8, height: 18)
    subtitle.frame = NSRect(x: inset, y: 2, width: bounds.width - inset - 8, height: 13)
  }
}

@MainActor
private final class ActivityProcessRow: NSTableRowView {
  override func drawSelection(in dirtyRect: NSRect) {
    Design.selection.setFill(); bounds.fill()
  }
  override var interiorBackgroundStyle: NSView.BackgroundStyle { isSelected ? .emphasized : .normal }
}

@MainActor
private final class ActivityProcessTable: NSTableView {
  var focus: (() -> Void)?
  var contextMenu: ((Int) -> NSMenu?)?
  override func menu(for event: NSEvent) -> NSMenu? {
    contextMenu?(row(at: convert(event.locationInWindow, from: nil)))
  }
  override func mouseDown(with event: NSEvent) { focus?(); super.mouseDown(with: event) }
}
