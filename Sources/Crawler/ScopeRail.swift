import AppKit

// Both lists recycle native cells, including folders with thousands of child scopes.
@MainActor
final class ScopeRail: Surface, NSTableViewDataSource, NSTableViewDelegate {
  struct Row: Equatable {
    enum Target: Equatable {
      case scope(FolderID), neighbor(FileItem), back
    }
    let title: String
    let detail: String
    let depth: Int
    let selected: Bool
    let target: Target?
  }
  private let heading = PaneTitle()
  private let scroll = NSScrollView()
  private let table = NSTableView()
  private var rows: [Row] = []
  var choose: ((Row.Target) -> Void)?
  init(title: String) {
    super.init(frame: .zero)
    heading.title = title
    let column = NSTableColumn(identifier: .init("scope"))
    table.addTableColumn(column)
    table.headerView = nil
    table.rowHeight = 30
    table.intercellSpacing = .zero
    table.backgroundColor = .clear
    table.selectionHighlightStyle = .none
    table.dataSource = self
    table.delegate = self
    table.target = self
    table.action = #selector(activateRow)
    table.setAccessibilityLabel(title)
    scroll.documentView = table
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.scrollerStyle = .overlay
    scroll.hasHorizontalScroller = false
    scroll.horizontalScrollElasticity = .none
    for view in [heading, scroll] { addSubview(view) }
  }
  required init?(coder: NSCoder) { nil }
  func configure(_ rows: [Row], subtitle: String) {
    heading.subtitle = subtitle
    heading.toolTip = subtitle
    heading.needsDisplay = true
    fill = Design.raised
    if self.rows != rows { self.rows = rows; table.reloadData() }
    needsLayout = true
  }
  func refreshTheme() {
    heading.needsDisplay = true
    fill = Design.raised
    table.reloadData()
  }
  @objc private func activateRow() {
    guard rows.indices.contains(table.clickedRow), let target = rows[table.clickedRow].target else { return }
    choose?(target)
  }
  func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
  func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { rows[row].target != nil }
  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
    let id = NSUserInterfaceItemIdentifier("scope-row")
    let cell = tableView.makeView(withIdentifier: id, owner: nil) as? ScopeCell ?? ScopeCell()
    cell.identifier = id
    cell.configure(rows[row])
    return cell
  }
  override func layout() {
    super.layout()
    heading.frame = NSRect(x: 0, y: 0, width: bounds.width, height: PaneTitle.height)
    scroll.frame = NSRect(x: 7, y: PaneTitle.height + 4, width: bounds.width - 14, height: max(0, bounds.height - PaneTitle.height - 8))
    table.tableColumns.first?.width = scroll.contentSize.width
  }
}

@MainActor
private final class ScopeCell: NSTableCellView {
  private let title = label("", size: 11)
  private let detail = label("", size: 10)
  private let icon = FileIconView()
  private var depth = 0
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    layer?.cornerRadius = 5
    detail.alignment = .right
    addSubview(title)
    addSubview(detail)
    addSubview(icon)
  }
  convenience init() { self.init(frame: .zero) }
  required init?(coder: NSCoder) { nil }
  func configure(_ row: ScopeRail.Row) {
    title.stringValue = row.title
    detail.stringValue = row.detail
    depth = row.depth
    title.font = Design.font(11, row.selected ? .semibold : .regular)
    title.textColor = row.selected ? Design.selectionText : Design.text
    detail.font = Design.font(10)
    detail.textColor = row.selected ? Design.selectionText : Design.muted
    layer?.backgroundColor = (row.selected ? Design.selection : .clear).cgColor
    icon.isHidden = false
    switch row.target {
    case .scope(let folder):
      toolTip = folder.rawValue
      icon.configure(path: folder.rawValue, isFolder: true)
    case .neighbor(let file):
      toolTip = file.id.rawValue
      icon.configure(path: file.id.rawValue, isFolder: file.kind.folder != nil)
    default: toolTip = row.title; icon.isHidden = true
    }
    setAccessibilityLabel(toolTip ?? row.title)
    needsLayout = true
  }
  override func layout() {
    super.layout()
    let left = CGFloat(depth * 10 + 9)
    let width: CGFloat = detail.stringValue.isEmpty ? 0 : min(65, bounds.width * 0.3)
    icon.frame = NSRect(x: left, y: 7, width: 16, height: 16)
    let textLeft = left + (icon.isHidden ? 0 : 23)
    title.frame = NSRect(x: textLeft, y: 6, width: max(0, bounds.width - textLeft - width - 10), height: 18)
    detail.frame = NSRect(x: bounds.width - width - 9, y: 6, width: width, height: 18)
  }
}
