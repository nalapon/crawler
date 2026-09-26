import AppKit

/// Native menus share the same preference in the toolbar and Settings.
@MainActor
final class EditorPicker: NSPopUpButton {
  private let opening = FileOpening.shared
  init() {
    super.init(frame: .zero, pullsDown: false)
    font = Design.font(12)
    target = self
    action = #selector(picked)
    setAccessibilityLabel("Default text editor")
    refresh()
  }
  required init?(coder: NSCoder) { nil }
  func refresh() {
    font = Design.font(12)
    removeAllItems()
    for editor in opening.editors {
      let item = NSMenuItem(title: editor.name, action: nil, keyEquivalent: "")
      item.representedObject = editor.bundleID
      item.image = opening.icon(for: editor)
      item.toolTip = editor.url.path
      menu?.addItem(item)
      if editor.bundleID == opening.selected?.bundleID { select(item) }
    }
    if opening.editors.isEmpty {
      addItem(withTitle: opening.scanning ? "Finding editors…" : "No text editor found")
    }
    isEnabled = !opening.editors.isEmpty
  }
  @objc private func picked() {
    guard let id = selectedItem?.representedObject as? String,
      let editor = opening.editors.first(where: { $0.bundleID == id })
    else { return }
    opening.choose(editor)
  }
}

@MainActor
final class EditorOpenButton: Surface {
  private let opening = FileOpening.shared
  private let openButton = ActionButton("Open", symbol: "doc") {}
  private let dropdown = ActionButton(
    "", symbol: "chevron.down", help: "Choose default text editor"
  ) {}
  var open: (() -> Void)?
  override init(frame: NSRect) {
    super.init(frame: frame)
    addSubview(openButton)
    addSubview(dropdown)
    openButton.invoke = { [weak self] in self?.open?() }
    dropdown.invoke = { [weak self] in self?.showEditors() }
    wantsLayer = true
  }
  required init?(coder: NSCoder) { nil }
  func configure(_ file: FileItem?) {
    fill = Design.raised
    layer?.cornerRadius = Design.square ? 0 : 6
    layer?.borderWidth = 1
    layer?.borderColor = Design.line.cgColor
    for button in [openButton, dropdown] {
      button.font = Design.font(11)
      button.contentTintColor = Design.text
    }
    openButton.isEnabled = file != nil
    openButton.image = Design.symbol(file?.kind.folder != nil ? "folder" : "doc")
    openButton.toolTip = "Open selected item · Return"
    if let file, file.kind.folder == nil, FileOpening.usesTextEditor(file.url),
      let editor = opening.selected
    {
      openButton.image = opening.icon(for: editor)
      openButton.toolTip = "Open with \(editor.name) · Return"
    }
    openButton.setAccessibilityLabel(openButton.toolTip)
  }
  private func showEditors() {
    opening.refresh(force: true)
    let menu = NSMenu()
    menu.autoenablesItems = false
    let heading = NSMenuItem(title: "Default text editor", action: nil, keyEquivalent: "")
    heading.isEnabled = false
    menu.addItem(heading)
    for editor in opening.editors {
      let item = NSMenuItem(title: editor.name, action: #selector(pick(_:)), keyEquivalent: "")
      item.target = self
      item.representedObject = editor.bundleID
      item.image = opening.icon(for: editor)
      item.toolTip = editor.url.path
      item.state = editor.bundleID == opening.selected?.bundleID ? .on : .off
      menu.addItem(item)
    }
    if opening.editors.isEmpty {
      let empty = NSMenuItem(
        title: opening.scanning ? "Finding installed editors…" : "No text editor found",
        action: nil, keyEquivalent: "")
      empty.isEnabled = false
      menu.addItem(empty)
    }
    menu.popUp(positioning: nil, at: NSPoint(x: 0, y: bounds.height + 4), in: self)
  }
  @objc private func pick(_ sender: NSMenuItem) {
    guard let id = sender.representedObject as? String,
      let editor = opening.editors.first(where: { $0.bundleID == id })
    else { return }
    opening.choose(editor)
  }
  override func layout() {
    super.layout()
    openButton.frame = NSRect(x: 2, y: 1, width: bounds.width - 28, height: bounds.height - 2)
    dropdown.frame = NSRect(x: bounds.width - 26, y: 1, width: 25, height: bounds.height - 2)
  }
  override func draw(_ rect: NSRect) {
    super.draw(rect)
    Design.line.setFill()
    NSRect(x: bounds.width - 27, y: 4, width: 1, height: bounds.height - 8).fill()
  }
}
