import AppKit
import Quartz

@MainActor
final class UtilityView: Surface {
  weak var workspace: Workspace?
  private var editorPicker: EditorPicker?
  private var editorStatus: NSTextField?
  let trash = TrashView(frame: .zero)
  let airDrop = AirDropView(frame: .zero)
  let activity = ActivityMonitorView(frame: .zero)
  let systemSettings = SystemSettingsView(frame: .zero)
  override func layout() {
    super.layout()
    trash.frame = bounds
    airDrop.frame = bounds
    activity.frame = bounds
    systemSettings.frame = bounds
  }
  func updateEditors() {
    editorPicker?.refresh()
    let opening = FileOpening.shared
    editorStatus?.stringValue = opening.scanning ? "Looking for installed text editors…"
      : opening.unavailablePreference ? "Your saved editor is unavailable. Using \(opening.selected?.name ?? "no editor"). Choose another editor above."
      : opening.editors.isEmpty ? "No text editor found. Install an editor and refresh the list."
      : "\(opening.editors.count) installed editors · This choice applies only in Crawler."
  }
  func render() {
    subviews.forEach { $0.removeFromSuperview() }
    editorPicker = nil
    editorStatus = nil
    fill = Design.background
    guard let workspace else { return }
    activity.setActive(workspace.tab.location == .activity)
    systemSettings.setActive(workspace.tab.location == .systemSettings)
    if workspace.tab.location == .trash {
      addSubview(trash)
      trash.frame = bounds
      trash.render()
      return
    }
    if workspace.tab.location == .systemSettings {
      systemSettings.browseStorage = { [weak workspace] in workspace?.navigate(.folder(FolderID(NSHomeDirectory()))) }
      addSubview(systemSettings)
      systemSettings.frame = bounds
      systemSettings.focusIfNeeded()
      return
    }
    if workspace.tab.location == .activity {
      addSubview(activity)
      activity.frame = bounds
      return
    }
    if workspace.tab.location == .airDrop {
      addSubview(airDrop)
      airDrop.frame = bounds
      airDrop.refresh()
      return
    }
    let title = label(workspace.library.title(workspace.tab.location), size: 25, weight: .semibold)
    title.frame = NSRect(x: 36, y: 34, width: 600, height: 36)
    addSubview(title)
    if workspace.tab.location == .preferences {
      let heading = label("Default text editor", size: 14, weight: .semibold)
      heading.frame = NSRect(x: 36, y: 96, width: 400, height: 22)
      addSubview(heading)
      let description = label("Open code, Markdown and plain text with your preferred editor.", size: 12, color: Design.muted)
      description.frame = NSRect(x: 36, y: 126, width: 630, height: 20)
      addSubview(description)
      let picker = EditorPicker()
      picker.frame = NSRect(x: 32, y: 160, width: 300, height: 30)
      addSubview(picker)
      editorPicker = picker
      let refresh = ActionButton("Refresh editors", symbol: "arrow.clockwise") { FileOpening.shared.refresh(force: true) }
      refresh.frame = NSRect(x: 344, y: 160, width: 136, height: 30)
      addSubview(refresh)
      let status = label("", size: 11, color: Design.muted)
      status.frame = NSRect(x: 36, y: 204, width: 640, height: 42)
      status.maximumNumberOfLines = 2
      status.lineBreakMode = .byWordWrapping
      addSubview(status)
      editorStatus = status
      let media = label("PDFs, images and video", size: 14, weight: .semibold)
      media.frame = NSRect(x: 36, y: 280, width: 420, height: 22)
      addSubview(media)
      let defaults = label("PDFs open in Crawler. Images and video use the default applications set in macOS.", size: 12, color: Design.muted)
      defaults.frame = NSRect(x: 36, y: 312, width: 630, height: 20)
      addSubview(defaults)
      updateEditors()
      FileOpening.shared.refresh()
      return
    }
    let message = label("We're building this. Check back in a future update.", size: 13, color: Design.muted)
    message.frame = NSRect(x: 36, y: 86, width: 620, height: 24)
    addSubview(message)
    let status = label("Work in progress", size: 12, weight: .medium)
    status.frame = NSRect(x: 36, y: 126, width: 250, height: 24)
    addSubview(status)
  }
}

@MainActor
extension Workspace {
  func message(_ title: String, _ text: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = text
    alert.addButton(withTitle: "OK")
    if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
  }
  func nameDialog(rename: Bool) { toast("Creating and renaming files are work in progress.") }
  func showPreview(_ file: FileItem) {
    previewWindow?.close()
    previewWindow = nil
    let pdfPreview = [filePalette.preview, previewPane, inspector].first { $0.item?.id == file.id }
    // Hand playback to the Space window without leaving audio in the underlying pane.
    if PreviewFormat.forURL(file.url) == .video {
      filePalette.preview.document.suspendVideo()
      previewPane.document.suspendVideo()
      inspector.document.suspendVideo()
    }
    let frame = NSRect(x: 0, y: 0, width: 640, height: 720)
    let preview: NSView
    if PreviewFormat.forURL(file.url) == .thumbnail {
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
      self?.previewPane.document.resumeVideo()
      self?.inspector.document.resumeVideo()
    }
    panel.isReleasedWhenClosed = false
    previewWindow = panel
    panel.center()
    panel.makeKeyAndOrderFront(nil)
  }

  func showShare() {
    let urls: [URL]
    if filePalette.isPresented {
      if let hit = filePalette.selectedHit, let root = filePalette.search.resultRoot {
        urls = [root.url.appendingPathComponent(hit.entry.path)]
      }
      else { urls = selected.map { [$0.url] } ?? [] }
    } else { urls = selectionIDs.map { URL(fileURLWithPath: $0.rawValue) } }
    navigate(.airDrop)
    utility.airDrop.stage(urls)
  }
  func showQuickSettings() { navigate(.preferences) }
  func showHelp() {
    filePalette.close()
    let show = !shortcuts.isPresented
    if show {
      shortcuts.refreshTheme(activity: tab.location == .activity)
      addSubview(shortcuts, positioned: .above, relativeTo: nil)
      needsLayout = true
    }
    shortcuts.transitionVisible(show)
    onChange?()
  }

}
