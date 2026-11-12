import AppKit

struct DisplayRow: Equatable {
  var item: FileItem
  let pending: Bool
  let dimmed: Bool
  var marked: Bool
  let locationText: String
  static func == (a: Self, b: Self) -> Bool {
    a.item == b.item
      && a.pending == b.pending && a.dimmed == b.dimmed && a.marked == b.marked
      && a.locationText == b.locationText
  }
}
@MainActor
final class FileCell: NSTableCellView {
  private let fileIcon = FileIconView()
  var row: DisplayRow? {
    didSet {
      needsDisplay = true
      if let row {
        fileIcon.configure(path: row.item.url.path, isFolder: row.item.kind.folder != nil)
        fileIcon.alphaValue = row.pending || row.dimmed ? 0.38 : 1
      }
      fileIcon.isHidden = row == nil || field != "file"
      setAccessibilityLabel(row.map { $0.item.name + ($0.pending ? ", pending move" : "") })
    }
  }
  var detailed = false
  var field = "file" { didSet { fileIcon.isHidden = row == nil || field != "file" } }
  override var isFlipped: Bool { true }
  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    addSubview(fileIcon)
    setAccessibilityElement(true)
    setAccessibilityRole(.cell)
  }
  required init?(coder: NSCoder) { nil }
  override func layout() {
    super.layout()
    fileIcon.frame = NSRect(x: 10, y: (bounds.height - 24) / 2, width: 22, height: 24)
  }
  override func draw(_ dirtyRect: NSRect) {
    guard let row else { return }
    var ancestor = superview
    while ancestor != nil && !(ancestor is NSTableRowView) { ancestor = ancestor?.superview }
    let selected = (ancestor as? NSTableRowView)?.isSelected == true
    if field != "file" {
      let value: String
      switch field {
      case "date": value = row.locationText
      case "created": value = row.item.created?.formatted(date: .abbreviated, time: .shortened) ?? "—"
      case "kind": value = row.item.kind.label
      default: value = row.item.sizeLabel
      }
      drawText(value, in: bounds.insetBy(dx: 8, dy: (bounds.height - 16) / 2), size: 10, color: selected ? Design.selectionText : Design.muted)
      return
    }
    let opacity: CGFloat = row.pending || row.dimmed ? 0.38 : 1
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current?.cgContext.setAlpha(opacity)
    let h = bounds.height
    let nameWidth = detailed ? bounds.width - 50 : bounds.width - 116
    let text = selected ? Design.selectionText : Design.text
    drawText(
      row.item.name, in: NSRect(x: 42, y: (h - 14) / 2, width: max(20, nameWidth), height: 17),
      size: 11, color: text)
    let suffix =
      row.pending
      ? "Pending" : row.marked ? "✓" : row.item.kind.folder != nil && row.item.bytes == nil ? "›" : row.item.sizeLabel
    if !detailed { drawText(
      suffix, in: NSRect(x: bounds.width - 70, y: (h - 13) / 2, width: 61, height: 17),
      size: row.item.kind.folder != nil && row.item.bytes == nil ? 17 : 9, color: selected ? Design.selectionText : Design.muted, align: .right) }
    NSGraphicsContext.restoreGraphicsState()
  }
}
@MainActor
final class FileRowView: NSTableRowView {
  var activePane = false
  var marked = false
  private var hoverArea: NSTrackingArea?
  private var hovered: Bool {
    guard let window, window.isKeyWindow else { return false }
    return visibleRect.contains(convert(window.mouseLocationOutsideOfEventStream, from: nil))
  }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let hoverArea { removeTrackingArea(hoverArea) }
    let area = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
      owner: self)
    addTrackingArea(area)
    hoverArea = area
    updateHover()
  }
  func updateHover() { needsDisplay = true }
  override func mouseEntered(with event: NSEvent) {
    updateHover()
  }
  override func mouseExited(with event: NSEvent) {
    updateHover()
  }
  override func drawBackground(in dirtyRect: NSRect) {
    super.drawBackground(in: dirtyRect)
    if marked && !isSelected {
      Design.selection.setFill()
      NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 2), xRadius: 4, yRadius: 4).fill()
    } else if hovered && !isSelected {
      Design.accent.withAlphaComponent(Design.classic ? 0.07 : 0.055).setFill()
      NSBezierPath(
        roundedRect: bounds.insetBy(dx: 1, dy: 2), xRadius: Design.square ? 0 : 4,
        yRadius: Design.square ? 0 : 4
      ).fill()
    }
  }
  override func drawSelection(in dirtyRect: NSRect) {
    let color =
      Design.selection
    color.setFill()
    NSBezierPath(
      roundedRect: bounds.insetBy(dx: 1, dy: 2), xRadius: Design.square ? 0 : 4,
      yRadius: Design.square ? 0 : 4
    ).fill()
  }
}
@MainActor
final class FileTable: NSTableView {
  var blankClick: (() -> Void)?
  override func mouseDown(with event: NSEvent) {
    if row(at: convert(event.locationInWindow, from: nil)) < 0 { blankClick?() }
    super.mouseDown(with: event)
  }
}
// Keep the scrollbar independent of AppKit's automatic flashes during keyboard scrolling.
@MainActor
final class FileScrollView: NSScrollView {
  private let indicator = NSScroller(frame: NSRect(x: 0, y: 0, width: 14, height: 100))
  private var hideIndicator: Timer?
  var pointerScrolled: (() -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    indicator.scrollerStyle = .legacy
    indicator.knobStyle = .dark
    indicator.isEnabled = true
    indicator.target = self
    indicator.action = #selector(dragIndicator)
    indicator.isHidden = true
    addSubview(indicator)
    contentView.postsBoundsChangedNotifications = true
    NotificationCenter.default.addObserver(self, selector: #selector(updateIndicator),
      name: NSView.boundsDidChangeNotification, object: contentView)
  }
  required init?(coder: NSCoder) { nil }
  override func layout() {
    super.layout()
    indicator.frame = NSRect(x: bounds.width - 14, y: 0, width: 14, height: bounds.height)
    updateIndicator()
  }
  // Scroll offsets describe visible rows, excluding AppKit's overlaid table header.
  var visibleDocumentRect: NSRect {
    let bounds = contentView.bounds
    let insets = contentView.contentInsets
    return NSRect(x: bounds.minX, y: bounds.minY + insets.top, width: bounds.width,
      height: max(0, bounds.height - insets.top - insets.bottom))
  }
  var scrollOffset: CGFloat { visibleDocumentRect.minY }
  func scrollToOffset(_ offset: CGFloat) {
    let maximum = max(0, (documentView?.bounds.height ?? 0) - visibleDocumentRect.height)
    contentView.scroll(to: NSPoint(x: 0,
      y: min(maximum, max(0, offset)) - contentView.contentInsets.top))
    reflectScrolledClipView(contentView)
  }
  @objc private func updateIndicator() {
    let height = documentView?.bounds.height ?? 0
    let visible = visibleDocumentRect
    indicator.knobProportion = height > 0 ? min(1, visible.height / height) : 1
    indicator.doubleValue = height > visible.height ? visible.minY / (height - visible.height) : 0
    if height <= visible.height { indicator.isHidden = true }
  }
  private func showIndicator() {
    indicator.frame = NSRect(x: bounds.width - 14, y: 0, width: 14, height: bounds.height)
    addSubview(indicator, positioned: .above, relativeTo: nil)
    updateIndicator()
    indicator.isHidden = (documentView?.bounds.height ?? 0) <= visibleDocumentRect.height
    hideIndicator?.invalidate()
    let timer = Timer(timeInterval: 1.5, repeats: false) { [weak self] _ in
      MainActor.assumeIsolated { self?.indicator.isHidden = true }
    }
    hideIndicator = timer
    RunLoop.main.add(timer, forMode: .common)
  }
  override func scrollWheel(with event: NSEvent) {
    super.scrollWheel(with: event)
    if event.momentumPhase.isEmpty { pointerScrolled?() }
    showIndicator()
  }
  @objc private func dragIndicator() {
    let visible = visibleDocumentRect
    let maximum = max(0, (documentView?.bounds.height ?? 0) - visible.height)
    let y: CGFloat
    switch indicator.hitPart {
    case .decrementPage: y = visible.minY - visible.height
    case .incrementPage: y = visible.minY + visible.height
    case .decrementLine: y = visible.minY - 44
    case .incrementLine: y = visible.minY + 44
    default: y = indicator.doubleValue * maximum
    }
    scrollToOffset(y)
    showIndicator()
  }
  func useKeyboard() {
    hideIndicator?.invalidate()
    indicator.isHidden = true
  }
}
@MainActor
final class FilePane: Surface, NSTableViewDataSource, NSTableViewDelegate {
  let table = FileTable()
  let scroll = FileScrollView()
  let header = Surface()
  let heading: ActionButton
  let border = BorderOverlay()
  private let topEdge = ScrollEdge(), bottomEdge = ScrollEdge()
  let empty = label("Empty folder", size: 12, color: Design.muted)
  var rows: [DisplayRow] = []
  var location: FolderID?
  var active = false
  var detailed = false
  var selectionChanged: ((ItemID, FolderID?, NSEvent.ModifierFlags) -> Void)?
  var openItem: ((ItemID) -> Void)?
  var focusFolder: ((FolderID) -> Void)?
  var sortChanged: ((SortField) -> Void)?
  var sortButtons: [ActionButton] = []
  private var updating = false
  private var fittingColumns = false
  private var configured = false
  private var context: String = ""
  init() {
    heading = ActionButton("", action: {})
    super.init(frame: .zero)
    header.bottomLine = true
    addSubview(header)
    header.addSubview(heading)
    for (title, field) in [
      ("Name", SortField.name), ("Date modified", .date), ("Kind", .kind), ("Size", .size),
    ] {
      let button = ActionButton(title, action: { [weak self] in self?.sortChanged?(field) })
      button.alignment = .left
      header.addSubview(button)
      sortButtons.append(button)
    }
    scroll.hasVerticalScroller = false
    scroll.hasHorizontalScroller = false
    scroll.horizontalScrollElasticity = .none
    scroll.autohidesScrollers = true
    scroll.scrollerStyle = .overlay
    scroll.contentView.postsBoundsChangedNotifications = true
    NotificationCenter.default.addObserver(
      self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification,
      object: scroll.contentView)
    scroll.drawsBackground = false
    table.headerView = nil
    table.rowHeight = 44
    table.intercellSpacing = NSSize(width: 0, height: 0)
    table.backgroundColor = Design.background
    table.gridStyleMask = []
    table.selectionHighlightStyle = .regular
    table.allowsEmptySelection = true
    table.allowsMultipleSelection = false
    table.focusRingType = .none
    let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("file"))
    col.minWidth = 0
    col.resizingMask = .autoresizingMask
    table.addTableColumn(col)
    table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
    table.dataSource = self
    table.delegate = self
    table.target = self
    table.doubleAction = #selector(doubleClick)
    // Drags carry the file URL; other apps and the sidebar only ever copy it.
    table.setDraggingSourceOperationMask(.copy, forLocal: true)
    table.setDraggingSourceOperationMask(.copy, forLocal: false)
    scroll.documentView = table
    addSubview(scroll)
    empty.maximumNumberOfLines = 4
    empty.lineBreakMode = .byWordWrapping
    addSubview(empty)
    topEdge.isHidden = true
    bottomEdge.isHidden = true
    bottomEdge.bottom = true
    addSubview(topEdge)
    addSubview(bottomEdge)
    addSubview(border)
    heading.invoke = { [weak self] in
      guard let self, let location = self.location else { return }
      self.focusFolder?(location)
    }
    table.blankClick = { [weak self] in
      guard let self, let location = self.location else { return }
      self.focusFolder?(location)
    }
  }
  required init?(coder: NSCoder) { nil }
  func configure(
    title: String, location: FolderID?, rows: [DisplayRow], selected: ItemID?, active: Bool,
    detailed: Bool = false, context: String = ""
  ) {
    updating = true
    defer { updating = false }
    let resetScroll = self.location != location || self.context != context || self.detailed != detailed
    self.location = location
    self.active = active
    let changedMode = self.detailed != detailed
    self.detailed = detailed
    if changedMode || !configured {
      for column in table.tableColumns { table.removeTableColumn(column) }
      let columns: [(String, String, CGFloat)] = detailed
        ? [("file", "Name", 260), ("date", "Date modified", 145), ("created", "Date created", 145), ("kind", "Kind", 100), ("size", "Size", 75)]
        : [("file", "Name", 260)]
      for (id, title, width) in columns {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
        column.title = title
        column.minWidth = detailed ? (id == "file" ? 100 : 55) : 0
        column.width = width
        column.resizingMask = [.userResizingMask, .autoresizingMask]
        table.addTableColumn(column)
      }
      table.headerView = detailed ? NSTableHeaderView() : nil
      table.columnAutoresizingStyle = detailed ? .uniformColumnAutoresizingStyle : .lastColumnOnlyAutoresizingStyle
      table.allowsColumnReordering = false
      table.rowHeight = detailed ? 32 : 44
    }
    self.context = context
    heading.isHidden = detailed
    sortButtons.forEach { $0.isHidden = true }
    header.isHidden = detailed
    if detailed { sortButtons[1].title = context == "search" ? "Location" : "Date modified" }
    heading.title = title
    heading.font = Design.font(12, .medium)
    heading.contentTintColor = Design.text
    header.fill = Design.background
    fill = Design.background
    table.backgroundColor = Design.background
    border.active = active
    empty.font = Design.font(12)
    empty.textColor = Design.muted
    empty.isHidden = !rows.isEmpty
    if !configured || changedMode || self.rows != rows {
      self.rows = rows
      table.reloadData()
      configured = true
    }
    if let selected, let index = rows.firstIndex(where: { $0.item.id == selected && !$0.pending }) {
      table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
    } else {
      table.deselectAll(nil)
    }
    table.enumerateAvailableRowViews { view, index in
      (view as? FileRowView)?.activePane = active
      (view as? FileRowView)?.marked = rows.indices.contains(index) && rows[index].marked
      view.needsDisplay = true
      for column in table.tableColumns.indices { (view.view(atColumn: column) as? NSView)?.needsDisplay = true }
    }
    if resetScroll {
      scroll.tile()
      scroll.scrollToOffset(0)
    }
    needsLayout = true
    needsDisplay = true
  }
  func updateSize(_ item: FileItem) {
    guard let index = rows.firstIndex(where: { $0.item.id == item.id }) else { return }
    rows[index].item = item
    for column in table.tableColumns.indices {
      if let cell = table.view(atColumn: column, row: index, makeIfNecessary: false) as? FileCell {
        cell.row = rows[index]
      }
    }
  }
  func updateSelection(_ selected: ItemID?, marked: Set<ItemID>) {
    updating = true
    defer { updating = false }
    let oldSelection = table.selectedRow
    let newSelection = selected.flatMap { id in rows.firstIndex { $0.item.id == id && !$0.pending } } ?? -1
    var changed = Set([oldSelection, newSelection])
    for index in rows.indices where rows[index].marked != marked.contains(rows[index].item.id) {
      rows[index].marked = marked.contains(rows[index].item.id)
      changed.insert(index)
    }
    if oldSelection != newSelection {
      if newSelection >= 0 { table.selectRowIndexes(IndexSet(integer: newSelection), byExtendingSelection: false) }
      else { table.deselectAll(nil) }
    }
    for index in changed where rows.indices.contains(index) {
      if let view = table.rowView(atRow: index, makeIfNecessary: false) as? FileRowView {
        view.marked = rows[index].marked
        view.needsDisplay = true
      }
      if let cell = table.view(atColumn: 0, row: index, makeIfNecessary: false) as? FileCell {
        cell.row = rows[index]
      }
    }
  }
  override func layout() {
    super.layout()
    header.frame = NSRect(x: 0, y: 0, width: bounds.width, height: 42)
    heading.frame = NSRect(x: 15, y: 8, width: bounds.width - 30, height: 26)
    heading.alignment = .left
    let top: CGFloat = detailed ? 0 : 48
    scroll.frame = NSRect(x: 8, y: top, width: bounds.width - 16, height: max(0, bounds.height - top))
    let contentWidth = max(0, scroll.contentSize.width)
    if !detailed { table.tableColumns.first?.width = contentWidth }
    else if abs(table.frame.width - contentWidth) > 1 { table.sizeToFit() }
    table.setFrameSize(NSSize(width: contentWidth, height: table.frame.height))
    empty.frame = NSRect(x: 20, y: 104, width: bounds.width - 40, height: 100)
    border.frame = NSRect(x: 0, y: detailed ? 0 : 42, width: bounds.width, height: max(0, bounds.height - (detailed ? 0 : 42)))
    topEdge.frame = NSRect(x: 8, y: 48, width: bounds.width - 16, height: 9)
    bottomEdge.frame = NSRect(x: 8, y: bounds.height - 9, width: bounds.width - 16, height: 9)
    scrolled()
  }
  func tableViewColumnDidResize(_ notification: Notification) {
    guard detailed, !fittingColumns else { return }
    fittingColumns = true
    defer { fittingColumns = false }
    let width = scroll.contentSize.width
    let total = table.tableColumns.reduce(CGFloat.zero) { $0 + $1.width }
    if total > width + 1 {
      table.sizeToFit()
      table.setFrameSize(NSSize(width: width, height: table.frame.height))
    }
  }
  func tableView(_ tableView: NSTableView, didClick tableColumn: NSTableColumn) {
    let field = tableColumn.identifier.rawValue
    if field == "file" { sortChanged?(.name) }
    else if let sort = SortField(rawValue: field) { sortChanged?(sort) }
  }
  func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView?
  {
    let id = NSUserInterfaceItemIdentifier("file-cell-" + (tableColumn?.identifier.rawValue ?? "file"))
    let cell =
      (tableView.makeView(withIdentifier: id, owner: nil) as? FileCell) ?? FileCell(frame: .zero)
    cell.identifier = id
    cell.row = rows[row]
    cell.detailed = detailed
    cell.field = tableColumn?.identifier.rawValue ?? "file"
    return cell
  }
  func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
    let v = FileRowView()
    v.activePane = active
    v.marked = rows[row].marked
    return v
  }
  func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { !rows[row].pending }
  func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> (any NSPasteboardWriting)? {
    rows.indices.contains(row) && !rows[row].pending ? rows[row].item.url as NSURL : nil
  }
  func tableViewSelectionDidChange(_ notification: Notification) {
    guard !updating, rows.indices.contains(table.selectedRow) else { return }
    selectionChanged?(
      rows[table.selectedRow].item.id, location, NSApp.currentEvent?.modifierFlags ?? [])
  }
  @objc private func doubleClick() {
    guard rows.indices.contains(table.clickedRow), !rows[table.clickedRow].pending else { return }
    openItem?(rows[table.clickedRow].item.id)
  }
  var hoveredItem: ItemID? {
    guard let window, !isHidden else { return nil }
    let point = table.convert(window.mouseLocationOutsideOfEventStream, from: nil)
    guard table.visibleRect.contains(point) else { return nil }
    let row = table.row(at: point)
    guard rows.indices.contains(row), !rows[row].pending else { return nil }
    return rows[row].item.id
  }
  func scrollSelectionVisible() {
    scroll.useKeyboard()
    guard table.selectedRow >= 0 else { return }
    let row = table.rect(ofRow: table.selectedRow)
    let visible = scroll.visibleDocumentRect
    guard row.minY < visible.minY || row.maxY > visible.maxY else { return }
    let y = row.minY < visible.minY ? row.minY : row.maxY - visible.height
    scroll.scrollToOffset(y)
  }
  func restoreScroll(_ offset: CGFloat) {
    scroll.useKeyboard()
    layoutSubtreeIfNeeded()
    scroll.tile()
    scroll.scrollToOffset(offset)
    scrolled()
  }
  @objc private func scrolled() {
    table.enumerateAvailableRowViews { view, _ in
      (view as? FileRowView)?.updateHover()
    }
    let visible = scroll.visibleDocumentRect
    topEdge.isHidden = visible.minY <= 1
    bottomEdge.isHidden = visible.maxY >= table.bounds.height - 1
  }
  func redrawTheme() { configured = false }
}

@MainActor
private final class ScrollEdge: NSView {
  var bottom = false
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func draw(_ dirtyRect: NSRect) {
    NSGradient(starting: NSColor.black.withAlphaComponent(0.10), ending: .clear)?.draw(
      in: bounds, angle: bottom ? 90 : 270)
  }
}
