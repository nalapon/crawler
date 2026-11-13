import AppKit
import Quartz

@MainActor
extension Workspace {
  func message(_ title: String, _ text: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = text
    alert.addButton(withTitle: "OK")
    if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
  }
  func showPreview(_ file: FileItem) {
    previewWindow?.close()
    previewWindow = nil
    let pdfPreview = filePalette.preview.item?.id == file.id ? filePalette.preview : nil
    // Hand playback to the Space window without leaving audio in the underlying pane.
    if PreviewFormat.forURL(file.url) == .video { filePalette.preview.document.suspendVideo() }
    let frame = NSRect(x: 0, y: 0, width: 640, height: 720)
    let preview: NSView
    if PreviewFormat.forURL(file.url).usesQuickLook {
      guard let fallback = QLPreviewView(frame: frame, style: .normal) else { return }
      fallback.previewItem = file.url as NSURL
      fallback.autostarts = true
      preview = fallback
    } else {
      let document = DocumentPreview(frame: frame)
      document.configure(file, coalesceSelection: false, reusingPDF: pdfPreview?.document, presentation: .space)
      if PreviewFormat.forURL(file.url) == .pdf { pdfPreview?.document.suspendPDF() }
      preview = document
    }
    let panel = FilePreviewPanel(contentRect: frame, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    panel.minSize = NSSize(width: 420, height: 360)
    panel.title = file.name
    panel.contentView = preview
    panel.didClose = { [weak self, weak pdfPreview] in
      pdfPreview?.document.resumePDF()
      self?.filePalette.preview.document.resumeVideo()
    }
    panel.isReleasedWhenClosed = false
    previewWindow = panel
    panel.center()
    panel.makeKeyAndOrderFront(nil)
  }

  /// Stages the selected files in Transfers for AirDrop.
  func showShare() {
    var urls = (section == .search || section == .folders ? filePalette.selectedFiles : []).map(\.url)
    if let hit = filePalette.selectedHit, let root = filePalette.search.resultRoot {
      urls = [root.url.appendingPathComponent(hit.entry.path)]
    }
    show(.transfers)
    airDrop.stage(urls)
  }
  func showHelp() {
    let show = !shortcuts.isPresented
    if show {
      shortcuts.refreshTheme(activity: section == .activity)
      addSubview(shortcuts, positioned: .above, relativeTo: nil)
      needsLayout = true
    }
    shortcuts.transitionVisible(show)
    onChange?()
  }

}
