import AppKit

@MainActor
final class PDFNoteReader: NSViewController {
  private let note: String
  init(note: String) { self.note = note; super.init(nibName: nil, bundle: nil) }
  required init?(coder: NSCoder) { nil }
  override func loadView() {
    let font = Design.font(13)
    let measured = (note as NSString).boundingRect(with: CGSize(width: 288, height: 180),
      options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height
    let height = min(180, max(24, ceil(measured) + 8))
    let root = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: height + 48))
    let hint = label("Click the marked text to edit", size: 10, color: Design.muted)
    hint.frame = NSRect(x: 16, y: 10, width: 288, height: 16)
    let scroll = NSScrollView(frame: NSRect(x: 16, y: 32, width: 288, height: height))
    scroll.hasVerticalScroller = true; scroll.drawsBackground = false
    let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 288, height: height))
    text.isEditable = false; text.isSelectable = false; text.isRichText = false; text.drawsBackground = false
    text.font = font; text.textColor = Design.text; text.string = note
    text.textContainerInset = .zero
    text.isHorizontallyResizable = false; text.isVerticallyResizable = true
    text.autoresizingMask = [.width]; text.textContainer?.widthTracksTextView = true
    text.setAccessibilityLabel("Annotation note")
    scroll.documentView = text
    root.addSubview(scroll); root.addSubview(hint); view = root
  }
}

/// A small native editor anchored to the annotated text, with an explicit Apply action.
@MainActor
final class PDFNoteEditor: NSViewController {
  private let text = NSTextView()
  private let initial: String
  private let isExisting: Bool
  var commit: ((String) -> Void)?
  var cancel: (() -> Void)?
  var remove: (() -> Void)?

  init(note: String, isExisting: Bool) {
    initial = note; self.isExisting = isExisting
    super.init(nibName: nil, bundle: nil)
  }
  required init?(coder: NSCoder) { nil }

  override func loadView() {
    let root = NSView(frame: NSRect(x: 0, y: 0, width: 340, height: 246))
    let title = label(isExisting ? "Edit note" : "Add note", size: 13, weight: .semibold)
    title.frame = NSRect(x: 16, y: 212, width: 300, height: 20)
    root.addSubview(title)
    let scroll = NSScrollView(frame: NSRect(x: 16, y: 54, width: 308, height: 146))
    scroll.hasVerticalScroller = true; scroll.borderType = .bezelBorder
    text.frame = NSRect(x: 0, y: 0, width: 308, height: 146)
    text.isRichText = false; text.isEditable = true; text.isSelectable = true
    text.allowsUndo = true; text.font = Design.font(13)
    text.textColor = Design.text; text.backgroundColor = Design.raised
    text.textContainerInset = NSSize(width: 8, height: 8)
    text.isVerticallyResizable = true; text.isHorizontallyResizable = false
    text.autoresizingMask = [.width]
    text.textContainer?.widthTracksTextView = true
    text.string = initial; text.setAccessibilityLabel("Note text")
    scroll.documentView = text; root.addSubview(scroll)
    let apply = NSButton(title: "Apply", target: self, action: #selector(applyNote))
    apply.bezelStyle = .rounded; apply.frame = NSRect(x: 244, y: 12, width: 84, height: 30)
    apply.keyEquivalent = "\r"; apply.keyEquivalentModifierMask = .command
    let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelNote))
    cancel.bezelStyle = .rounded; cancel.frame = NSRect(x: 156, y: 12, width: 84, height: 30)
    cancel.keyEquivalent = "\u{1b}"; cancel.keyEquivalentModifierMask = []
    root.addSubview(apply); root.addSubview(cancel)
    if isExisting {
      let delete = ActionButton("Delete", symbol: "trash", help: "Delete annotation") { [weak self] in self?.remove?() }
      delete.frame = NSRect(x: 16, y: 12, width: 90, height: 30); root.addSubview(delete)
    }
    view = root
  }
  func focus() { view.window?.makeFirstResponder(text) }
  @objc private func applyNote() { commit?(text.string) }
  @objc private func cancelNote() { cancel?() }
}
