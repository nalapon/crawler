import AppKit
import UniformTypeIdentifiers

/// Covers the workspace without replacing its tabs, search, selection, or scroll views.
@MainActor
final class PDFReader: Surface, WorkspaceReader {
  let file: FileItem
  let document = DocumentPreview()
  var close: (() -> Void)?
  private let header = Surface()
  private let tools = Surface()
  private let annotationStatus = label("Choose a tool, then drag over text", size: 11, color: Design.muted)
  private let selectButton = ActionButton("", symbol: "cursorarrow", help: "Select text and annotations · Esc", action: {})
  private let highlightButton = ActionButton("Highlight", symbol: "highlighter", help: "Highlight tool · Drag over text", action: {})
  private let underlineButton = ActionButton("Underline", symbol: "underline", help: "Underline tool · Drag over text", action: {})
  private let noteButton = ActionButton("Note", symbol: "text.bubble", help: "Note tool · Drag over text", action: {})
  private let deleteButton = ActionButton("", symbol: "trash", help: "Delete selected annotation", action: {})
  private let undoButton = ActionButton("", symbol: "arrow.uturn.backward", help: "Undo · ⌘Z", action: {})
  private let redoButton = ActionButton("", symbol: "arrow.uturn.forward", help: "Redo · ⇧⌘Z", action: {})
  private let removeAllButton = ActionButton("Remove annotations", symbol: "eraser", help: "Remove all highlights, underlines and notes, including saved annotations", action: {})
  private let saveButton = ActionButton("Save", symbol: "square.and.arrow.down", help: "Save PDF · ⌘S", action: {})
  private let saveAsButton = ActionButton("Save as…", help: "Save PDF as… · ⇧⌘S", action: {})
  private var colorButtons: [(button: ActionButton, color: PDFMarkup.Color)] = []
  private var promptingClose = false
  private let title = label("", size: 12, weight: .medium)
  private let hint = label("Esc twice to return", size: 11, color: Design.muted)
  private let closeButton = ActionButton("", symbol: "xmark", help: "Close PDF · Esc twice", action: {})
  private let copyNameButton = ActionButton("", symbol: "doc.on.doc", help: "Copy file name", action: {})
  private var lastEscape: TimeInterval?

  init(file: FileItem) {
    self.file = file
    super.init(frame: .zero)
    identifier = NSUserInterfaceItemIdentifier("pdf-reader")
    setAccessibilityLabel("PDF reader")
    header.bottomLine = true
    title.stringValue = file.name
    title.lineBreakMode = .byTruncatingMiddle
    title.toolTip = file.name
    closeButton.identifier = NSUserInterfaceItemIdentifier("close-pdf")
    closeButton.invoke = { [weak self] in self?.requestClose() }
    copyNameButton.invoke = { [weak self] in self?.copyName() }
    tools.bottomLine = true
    for view in [document, header, tools] { addSubview(view) }
    for view in [title, copyNameButton, hint, closeButton] { header.addSubview(view) }
    for view in [selectButton, highlightButton, underlineButton, noteButton, deleteButton, undoButton, redoButton,
      removeAllButton, annotationStatus, saveAsButton, saveButton] { tools.addSubview(view) }
    selectButton.invoke = { [weak self] in self?.document.pdf?.chooseTool(.select) }
    highlightButton.invoke = { [weak self] in self?.document.pdf?.chooseTool(.highlight) }
    underlineButton.invoke = { [weak self] in self?.document.pdf?.chooseTool(.underline) }
    noteButton.invoke = { [weak self] in self?.document.pdf?.chooseTool(.note) }
    deleteButton.invoke = { [weak self] in self?.document.pdf?.deleteMarkup(nil) }
    undoButton.invoke = { [weak self] in self?.undoAnnotation() }
    redoButton.invoke = { [weak self] in self?.redoAnnotation() }
    removeAllButton.invoke = { [weak self] in self?.removeAllAnnotations() }
    saveButton.invoke = { [weak self] in self?.save() }
    saveAsButton.invoke = { [weak self] in self?.save(asCopy: true) }
    for (name, color) in [("Yellow", PDFMarkup.Color.yellow), ("Green", .green), ("Blue", .blue), ("Pink", .pink)] {
      let button = ActionButton("", symbol: "circle.fill", help: "\(name) annotation color") { [weak self] in
        self?.document.pdf?.chooseColor(color)
      }
      button.contentTintColor = color.native
      colorButtons.append((button, color)); tools.addSubview(button)
    }
    updateAnnotationControls()
  }
  required init?(coder: NSCoder) { nil }
  override var acceptsFirstResponder: Bool { true }

  func cancelEscape() {
    lastEscape = nil
    hint.stringValue = "Esc twice to return"
  }

  func handleKey(_ event: NSEvent) -> Bool {
    let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
    if event.keyCode == 53, modifiers.isEmpty {
      if document.pdf?.dismissNote() == true { cancelEscape(); return true }
      if let pdf = document.pdf, pdf.annotationTool != .select {
        pdf.chooseTool(.select); cancelEscape(); return true
      }
      guard !event.isARepeat else { return true }
      if let lastEscape, event.timestamp - lastEscape <= 0.7 {
        requestClose()
      } else {
        lastEscape = event.timestamp
        hint.stringValue = "Press Esc again to return"
      }
      return true
    }
    cancelEscape()
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    if modifiers == .command, key == "w" { requestClose(); return true }
    if let editor = window?.firstResponder as? NSTextView, editor.isEditable { return false }
    if modifiers == .command, key == "s" { save(); return true }
    if modifiers == [.command, .shift], key == "s" { save(asCopy: true); return true }
    if modifiers == .command, key == "z" { undoAnnotation(); return true }
    if modifiers == [.command, .shift], key == "z" { redoAnnotation(); return true }
    if [51, 117].contains(event.keyCode), modifiers.isEmpty, document.pdf?.selectedMarkup != nil {
      document.pdf?.deleteMarkup(nil); return true
    }
    // Keep explorer shortcuts out of the reader; pass reading and copy keys to the document.
    return modifiers.contains(.command) && !["c", "a", "+", "=", "-", "0", "q", "h"].contains(key)
  }

  override func layout() {
    super.layout()
    header.frame = NSRect(x: 0, y: 0, width: bounds.width, height: 48)
    // The copy button follows the name; a long name truncates before it.
    let titleWidth = min(ceil(title.attributedStringValue.size().width) + 4, max(0, bounds.width - 376))
    title.frame = NSRect(x: 88, y: 15, width: titleWidth, height: 20)
    copyNameButton.frame = NSRect(x: 88 + titleWidth + 4, y: 9, width: 30, height: 30)
    hint.frame = NSRect(x: bounds.width - 236, y: 15, width: 176, height: 20)
    closeButton.frame = NSRect(x: bounds.width - 46, y: 8, width: 32, height: 32)
    tools.frame = NSRect(x: 0, y: 48, width: bounds.width, height: 40)
    var x: CGFloat = 20
    for (button, width) in [(selectButton, CGFloat(32)), (highlightButton, 94), (underlineButton, 94), (noteButton, 72)] {
      button.frame = NSRect(x: x, y: 5, width: width, height: 30); x += width + 4
    }
    x += 10
    for (button, _) in colorButtons {
      button.frame = NSRect(x: x, y: 6, width: 28, height: 28); x += 30
    }
    x += 12
    for button in [deleteButton, undoButton, redoButton] {
      button.frame = NSRect(x: x, y: 5, width: 32, height: 30); x += 36
    }
    x += 8
    removeAllButton.frame = NSRect(x: x, y: 5, width: 144, height: 30); x += 148
    annotationStatus.frame = NSRect(x: x + 12, y: 12, width: max(0, bounds.width - x - 218), height: 18)
    annotationStatus.lineBreakMode = .byTruncatingTail
    saveAsButton.frame = NSRect(x: bounds.width - 192, y: 5, width: 94, height: 30)
    saveButton.frame = NSRect(x: bounds.width - 92, y: 5, width: 76, height: 30)
    document.frame = NSRect(x: 0, y: 88, width: bounds.width, height: max(0, bounds.height - 88))
  }

  func updateAnnotationControls() {
    let pdf = document.pdf, session = pdf?.annotations
    let writable = session?.allowsEditing == true && session?.isBusy == false
    let selected = pdf?.selectedMarkup
    selectButton.isEnabled = pdf != nil
    highlightButton.isEnabled = writable
    underlineButton.isEnabled = writable
    noteButton.isEnabled = writable
    for (button, tool) in [(selectButton, PDFAnnotationTool.select), (highlightButton, .highlight),
      (underlineButton, .underline), (noteButton, .note)] {
      button.setSelected(pdf?.annotationTool == tool)
      button.setAccessibilityValue(pdf?.annotationTool == tool ? "Active" : "Inactive")
    }
    deleteButton.isEnabled = writable && selected?.editable == true
    undoButton.isEnabled = writable && session?.undo.canUndo == true
    redoButton.isEnabled = writable && session?.undo.canRedo == true
    removeAllButton.isEnabled = writable
    saveButton.isEnabled = writable && session?.isDirty == true
    saveAsButton.isEnabled = writable
    for (button, color) in colorButtons {
      button.isEnabled = writable
      button.setSelected((selected?.color ?? pdf?.annotationColor) == color)
      button.contentTintColor = color.native
    }
    annotationStatus.stringValue = session?.saving == true ? "Saving…"
      : session?.removing == true ? "Removing annotations…"
      : session?.allowsEditing == true ? pdf?.annotationTool.instruction ?? "Choose a tool, then drag over text"
      : pdf == nil ? "Loading PDF…" : "Annotations unavailable"
    title.stringValue = (session?.url.lastPathComponent ?? file.name) + (session?.isDirty == true ? " •" : "")
    title.toolTip = session?.url.lastPathComponent ?? file.name
    needsLayout = true
  }

  /// Copies the file name with its extension, then briefly shows a checkmark.
  private func copyName() {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(document.pdf?.annotations.url.lastPathComponent ?? file.name, forType: .string)
    copyNameButton.image = Design.symbol("checkmark")
    copyNameButton.toolTip = "Copied"
    DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
      self?.copyNameButton.image = Design.symbol("doc.on.doc")
      self?.copyNameButton.toolTip = "Copy file name"
    }
  }

  func undoAnnotation() {
    if let pdf = document.pdf, pdf.hasPendingStroke {
      Task { [weak self] in await pdf.finishPendingAnnotations(); self?.undoAnnotation() }
      return
    }
    guard document.pdf?.annotations.isBusy == false else { return }
    document.pdf?.annotations.undo.undo(); updateAnnotationControls()
  }
  func redoAnnotation() {
    if let pdf = document.pdf, pdf.hasPendingStroke {
      Task { [weak self] in await pdf.finishPendingAnnotations(); self?.redoAnnotation() }
      return
    }
    guard document.pdf?.annotations.isBusy == false else { return }
    document.pdf?.annotations.undo.redo(); updateAnnotationControls()
  }

  private func removeAllAnnotations() {
    guard let pdf = document.pdf, pdf.annotations.allowsEditing, !pdf.annotations.isBusy else { return }
    Task { [weak self] in
      await pdf.finishPendingAnnotations()
      pdf.chooseTool(.select)
      do { try await pdf.annotations.removeAll() }
      catch {
        guard let window = self?.window else { return }
        let alert = NSAlert()
        alert.messageText = "Could not remove annotations"
        alert.informativeText = error.localizedDescription
        await alert.beginSheetModal(for: window)
      }
    }
  }

  func save(asCopy: Bool = false, completed: (() -> Void)? = nil, cancelled: (() -> Void)? = nil) {
    if let pdf = document.pdf, pdf.hasPendingStroke {
      Task { [weak self] in
        await pdf.finishPendingAnnotations()
        self?.save(asCopy: asCopy, completed: completed, cancelled: cancelled)
      }
      return
    }
    guard let session = document.pdf?.annotations, session.allowsEditing, !session.isBusy, let window else { return }
    func write(_ url: URL) {
      Task { [weak self] in
        do { try await session.save(to: url); completed?() }
        catch {
          guard let self, let window = self.window else { return }
          let alert = NSAlert()
          alert.messageText = "Could not save PDF"
          alert.informativeText = error.localizedDescription
          alert.addButton(withTitle: "OK")
          if case PDFAnnotationError.changedOnDisk = error { alert.addButton(withTitle: "Save as…") }
          alert.beginSheetModal(for: window) { [weak self] response in
            if response == .alertSecondButtonReturn { self?.save(asCopy: true, completed: completed, cancelled: cancelled) }
            else { cancelled?() }
          }
        }
      }
    }
    if asCopy {
      let panel = NSSavePanel()
      panel.title = "Save PDF as"
      panel.allowedContentTypes = [.pdf]
      panel.nameFieldStringValue = session.url.lastPathComponent
      panel.directoryURL = session.url.deletingLastPathComponent()
      panel.beginSheetModal(for: window) { response in
        if response == .OK, let url = panel.url { write(url) }
        else { cancelled?() }
      }
    } else { write(session.url) }
  }

  func requestClose() { confirmClose { [weak self] in self?.close?() } }

  /// Used by reader close, window close and Quit so no exit silently drops annotations.
  func confirmClose(completed: @escaping () -> Void, cancelled: (() -> Void)? = nil) {
    if let pdf = document.pdf, pdf.hasPendingStroke {
      Task { [weak self] in
        await pdf.finishPendingAnnotations()
        self?.confirmClose(completed: completed, cancelled: cancelled)
      }
      return
    }
    guard let session = document.pdf?.annotations else { completed(); return }
    guard !session.isBusy, !promptingClose else { cancelled?(); return }
    guard session.isDirty else { completed(); return }
    guard let window else { cancelled?(); return }
    promptingClose = true
    let alert = NSAlert()
    alert.messageText = "Save your PDF annotations?"
    alert.informativeText = "Your highlights, underlines and notes have not been saved."
    alert.addButton(withTitle: "Save")
    alert.addButton(withTitle: "Cancel")
    alert.addButton(withTitle: "Discard changes")
    alert.beginSheetModal(for: window) { [weak self] response in
      guard let self else { return }
      self.promptingClose = false
      switch response {
      case .alertFirstButtonReturn: self.save(completed: completed, cancelled: cancelled)
      case .alertThirdButtonReturn: session.discardUnsavedChanges(); completed()
      default: cancelled?()
      }
    }
  }
}

/// A full-window document view that temporarily covers the workspace.
@MainActor
protocol WorkspaceReader: NSView {
  var close: (() -> Void)? { get set }
  func handleKey(_ event: NSEvent) -> Bool
  /// A click elsewhere interrupts a pending double-Escape.
  func cancelEscape()
}

extension Workspace {
  /// Cover the workspace with `reader`. Closing restores view visibility, focus and any
  /// text selection after running `closed`.
  func presentReader(_ reader: some WorkspaceReader, focus: NSResponder, closed: @escaping () -> Void) {
    previewWindow?.close()
    previewWindow = nil
    let visibility = subviews.map { (view: $0, hidden: $0.isHidden) }
    let responder = window?.firstResponder
    let editor = responder as? NSTextView
    let returnResponder = editor?.isFieldEditor == true ? editor?.delegate as? NSResponder : responder
    let selectedRange = editor?.selectedRange()
    self.reader = reader
    reader.close = { [weak self, weak reader, weak returnResponder] in
      guard let self, let reader, self.reader === reader else { return }
      closed()
      reader.removeFromSuperview()
      self.reader = nil
      for (view, hidden) in visibility { view.isHidden = hidden }
      self.window?.makeFirstResponder(returnResponder ?? self)
      if let selectedRange, let editor = self.window?.firstResponder as? NSTextView {
        editor.setSelectedRange(selectedRange)
      }
      self.onChange?()
    }
    addSubview(reader, positioned: .above, relativeTo: nil)
    needsLayout = true
    layoutSubtreeIfNeeded()
    window?.makeFirstResponder(focus)
  }

  func showPDF(_ url: URL, succeeded: (() -> Void)? = nil) {
    guard reader == nil else { return }
    let id = ItemID(rawValue: url.standardizedFileURL.path)
    let preview = [filePalette.preview, previewPane, inspector].first { $0.item?.id == id }
    let file = preview?.item ?? library.item(id) ?? FileItem(
      id: id, name: url.lastPathComponent, parent: FolderID(url.deletingLastPathComponent().path),
      kind: .document(.pdf), bytes: nil, modified: nil, created: nil)
    let reader = PDFReader(file: file)
    presentReader(reader, focus: reader) { [weak reader, weak preview] in
      guard let reader else { return }
      let savedElsewhere = reader.document.pdf?.annotations.url.standardizedFileURL != file.url.standardizedFileURL
      reader.document.stop()
      preview?.document.resumePDF()
      if savedElsewhere {
        preview?.document.stop()
        preview?.document.configure(file, coalesceSelection: false)
      }
    }
    var recorded = false
    reader.document.didChange = { [weak self, weak reader] in
      guard let self, let reader, self.reader === reader else { return }
      reader.updateAnnotationControls()
      if reader.document.state == "pdf", !recorded {
        recorded = true
        FileOpening.recordRecent(url)
        succeeded?()
        reader.document.focusPDF()
      }
      self.onChange?()
    }
    reader.document.configure(file, coalesceSelection: false, reusingPDF: preview?.document, presentation: .reader)
    preview?.document.suspendPDF()
    onChange?()
  }
}
