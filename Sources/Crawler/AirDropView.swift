import AppKit
import QuartzCore
import AirDropBridge

/// Nearby AirDrop recipients discovered over peer-to-peer Wi-Fi.
@MainActor
final class AirDropView: Surface, NSSharingServiceDelegate {
  private let discovery = CrawlerAirDrop()
  private var files: [URL] = []
  private var selectedDevice: String?
  private var transferring = false
  private var delivered = false
  private var deviceTiles: [String: AirDropDeviceTile] = [:]
  private var transferMessage: String?
  private var transferError = false
  private var picker: AirDropFilePicker?
  private var identificationTasks: [String: Task<Void, Never>] = [:]
  private var identificationStarted = Set<String>()
  private var readyDevices = Set<String>()
  private var transferTask: Task<Void, Never>?
  private var systemService: NSSharingService?
  private let heading = label("AirDrop", size: 25, weight: .semibold)
  private let detail = label("Drop files onto a device, or click it to choose what to send.", size: 13)
  private let beacon = AirDropBeacon(frame: .zero)
  private let status = label("Looking for nearby devices…", size: 18, weight: .semibold)
  private let hint = label("", size: 12)
  private let devicesScroll = NSScrollView()
  private let devicesHost = Surface()
  private let filesLabel = label("", size: 12)
  private let progress = AirDropProgress(frame: .zero)
  private lazy var choose = ActionButton("Choose files…", symbol: "plus") { [weak self] in self?.chooseFiles() }
  private lazy var send = ActionButton("Send", symbol: "arrow.up") { [weak self] in self?.sendFiles() }
  private lazy var cancel = ActionButton("Cancel transfer", symbol: "xmark") { [weak self] in
    guard let self else { return }
    self.transferTask?.cancel(); self.transferTask = nil
    self.transferMessage = "Cancelling…"
    self.refresh()
  }
  private lazy var clear = ActionButton("Clear files", symbol: "xmark") { [weak self] in
    guard let self, !self.transferring else { return }
    self.files = []; self.transferMessage = nil; self.delivered = false; self.progress.reset(); self.refresh()
  }
  private lazy var retry = ActionButton("Refresh devices", symbol: "arrow.clockwise") { [weak self] in
    guard let self, !self.transferring else { return }
    self.stopDiscovery(); self.discovery.start(); self.refresh()
  }
  private let help = label("Keep the receiving device unlocked, with Wi-Fi and Bluetooth on. For testing, use AirDrop's Everyone for 10 Minutes setting.", size: 12)

  override init(frame: NSRect) {
    super.init(frame: frame)
    registerForDraggedTypes([.fileURL])
    for view in [heading, detail, beacon, status, hint, devicesScroll, filesLabel, progress, choose, send, clear, cancel, retry, help] { addSubview(view) }
    for field in [status, hint, filesLabel, help] {
      field.alignment = .center
      field.maximumNumberOfLines = 0
      field.lineBreakMode = .byWordWrapping
    }
    devicesScroll.documentView = devicesHost
    devicesScroll.hasVerticalScroller = true
    devicesScroll.autohidesScrollers = true
    devicesScroll.drawsBackground = false
    devicesScroll.horizontalScrollElasticity = .none
    progress.isHidden = true
    discovery.devicesChanged = { [weak self] in MainActor.assumeIsolated { self?.refresh() } }
    NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(motionChanged),
      name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
    refresh()
  }
  required init?(coder: NSCoder) { nil }

  func stage(_ urls: [URL]) {
    guard !transferring else { return }
    var seen = Set(files)
    files.append(contentsOf: urls.filter(\.isFileURL).map(\.standardizedFileURL).filter { seen.insert($0).inserted })
    transferMessage = nil; transferError = false; delivered = false
    progress.reset()
    refresh()
    beacon.pulse(repeating: false)
  }
  func refresh() {
    heading.font = Design.font(25, .semibold); heading.textColor = Design.text
    status.font = Design.font(18, .semibold); status.textColor = transferError ? .systemRed : Design.text
    for field in [detail, hint, filesLabel, help] { field.font = Design.font(12); field.textColor = Design.muted }
    let devices = discovery.devices
    identify(devices)
    if !devices.contains(where: { $0.identifier == selectedDevice }) && !transferring { selectedDevice = nil }
    let previousStatus = status.stringValue
    status.stringValue = transferMessage ?? discovery.failure ?? (devices.isEmpty ? "Looking for nearby devices…" : "Choose a device")
    if previousStatus != status.stringValue && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
      status.wantsLayer = true
      let transition = CATransition()
      transition.type = .fade; transition.duration = 0.16
      status.layer?.add(transition, forKey: "status-change")
    }
    hint.stringValue = discovery.failure != nil ? "Network discovery is unavailable. You can still use the macOS AirDrop window."
      : devices.isEmpty ? "Your iPad, iPhone or Mac will appear here when discovered." : "Click a device to select files. You can also drop files directly onto it."
    filesLabel.stringValue = files.isEmpty ? "No files selected" : files.count == 1 ? files[0].lastPathComponent : "\(files.count) items ready to send"
    filesLabel.toolTip = files.map(\.path).joined(separator: "\n")
    choose.isEnabled = !transferring
    clear.isEnabled = !transferring && !files.isEmpty
    send.title = discovery.failure == nil ? "Send" : "Use macOS AirDrop…"
    send.isEnabled = !transferring && !files.isEmpty && (selectedDevice.map { readyDevices.contains($0) } == true || discovery.failure != nil)
    send.setSelected(send.isEnabled, primary: true)
    cancel.isHidden = !transferring
    cancel.isEnabled = systemService == nil
    retry.isEnabled = !transferring
    let present = Set(devices.map(\.identifier))
    for id in Array(deviceTiles.keys) where !present.contains(id) {
      deviceTiles.removeValue(forKey: id)?.removeFromSuperview()
    }
    for device in devices {
      let tile: AirDropDeviceTile
      if let existing = deviceTiles[device.identifier] { tile = existing }
      else {
        tile = AirDropDeviceTile(device: device, selected: false)
        deviceTiles[device.identifier] = tile
        devicesHost.addSubview(tile)
        tile.reveal()
      }
      tile.update(device: device, selected: selectedDevice == device.identifier)
      tile.isEnabled = !transferring && readyDevices.contains(device.identifier)
      tile.choose = { [weak self] in
        guard let self, !self.transferring else { return }
        self.selectedDevice = device.identifier
        self.transferMessage = nil; self.delivered = false; self.progress.reset()
        self.refresh()
        if self.files.isEmpty { self.chooseFiles(sendAfterChoosing: true) }
      }
      tile.receiveFiles = { [weak self] urls in
        guard let self, !self.transferring else { return false }
        self.selectedDevice = device.identifier
        self.files = []
        self.stage(urls)
        self.sendFiles()
        return true
      }
    }
    beacon.completed = delivered
    progress.refresh()
    beacon.refresh(); updateMotion(); needsLayout = true
  }
  override func layout() {
    super.layout()
    let width = min(620, bounds.width - 72), x = (bounds.width - width) / 2
    heading.frame = NSRect(x: 36, y: 28, width: bounds.width - 72, height: 36)
    detail.frame = NSRect(x: 36, y: 72, width: bounds.width - 72, height: 22)
    beacon.frame = NSRect(x: (bounds.width - 130) / 2, y: 102, width: 130, height: 130)
    status.frame = NSRect(x: x, y: 230, width: width, height: 40)
    hint.frame = NSRect(x: x, y: 270, width: width, height: 36)
    let tileHeight = max(124, min(220, bounds.height - 482))
    devicesScroll.frame = NSRect(x: x, y: 316, width: width, height: tileHeight)
    let columns = max(1, Int(width / 180))
    let tiles = devicesHost.subviews
    devicesHost.frame = NSRect(x: 0, y: 0, width: devicesScroll.contentSize.width, height: max(tileHeight, CGFloat((tiles.count + columns - 1) / columns) * 128))
    let cellWidth = devicesHost.bounds.width / CGFloat(columns)
    for (index, tile) in tiles.enumerated() {
      let rowStart = (index / columns) * columns
      let rowCount = min(columns, tiles.count - rowStart)
      let inset = (devicesHost.bounds.width - CGFloat(rowCount) * cellWidth) / 2
      tile.frame = NSRect(x: inset + CGFloat(index % columns) * cellWidth + 4, y: CGFloat(index / columns) * 128 + 4, width: cellWidth - 8, height: 120)
    }
    let y = devicesScroll.frame.maxY + 8
    filesLabel.frame = NSRect(x: x, y: y, width: width, height: 26)
    progress.frame = NSRect(x: x + 70, y: y + 30, width: width - 140, height: 14)
    choose.frame = NSRect(x: bounds.width / 2 - 250, y: y + 44, width: 142, height: 32)
    send.frame = NSRect(x: bounds.width / 2 - 96, y: y + 44, width: 192, height: 32)
    clear.frame = NSRect(x: bounds.width / 2 + 104, y: y + 44, width: 132, height: 32)
    cancel.frame = NSRect(x: bounds.width / 2 - 90, y: y + 44, width: 180, height: 32)
    choose.isHidden = transferring; send.isHidden = transferring; clear.isHidden = transferring
    help.frame = NSRect(x: x, y: y + 90, width: width, height: 40)
    retry.frame = NSRect(x: bounds.width - 188, y: 30, width: 152, height: 30)
  }
  private func chooseFiles(sendAfterChoosing: Bool = false) {
    guard !transferring, let window else { return }
    let picker = AirDropFilePicker()
    self.picker = picker
    picker.choose = { [weak self] urls in
      self?.stage(urls)
      if sendAfterChoosing { self?.sendFiles() }
      self?.picker = nil
    }
    picker.present(on: window)
  }
  private func sendFiles() {
    guard !files.isEmpty, !transferring else { return }
    if discovery.failure != nil {
      guard let service = NSSharingService(named: .sendViaAirDrop), service.canPerform(withItems: files) else {
        transferMessage = "AirDrop is unavailable for these files."; transferError = true; refresh(); return
      }
      service.delegate = self
      systemService = service
      transferring = true; transferError = false
      transferMessage = "Follow the transfer in the macOS AirDrop window."
      refresh()
      service.perform(withItems: files)
      return
    }
    guard let device = discovery.devices.first(where: { $0.identifier == selectedDevice }) else { return }
    let endpoint = AirDropEndpoint(host: device.host, port: device.port)
    let files = files
    delivered = false; progress.reset()
    transferError = false; transferring = true; transferMessage = "Preparing files…"
    refresh()
    transferTask = Task { [weak self] in
      var prepared: AirDropPreparedFiles?
      var connection: AirDropConnection?
      var stage = "Preparing files"
      do {
        let preparation = Task.detached(priority: .userInitiated) { try AirDropPreparedFiles.prepare(files) }
        prepared = try await withTaskCancellationHandler { try await preparation.value } onCancel: { preparation.cancel() }
        guard let prepared else { return }
        try Task.checkCancellation()
        let client = try await AirDropSender.shared.connection(to: endpoint)
        connection = client
        try await withTaskCancellationHandler {
          stage = "Connecting"
          self?.transferMessage = "Connecting…"; self?.refresh()
          try await client.connect()
          stage = "Requesting permission"
          self?.transferMessage = "Waiting for the receiving device…"; self?.refresh()
          _ = try await client.post("/Ask", body: prepared.ask)
          stage = "Sending files"
          self?.transferMessage = "Sending…"; self?.progress.isHidden = false; self?.refresh()
          try await client.upload(prepared) { [weak self] fraction in
            await MainActor.run {
              self?.progress.update(fraction)
              if fraction >= 1 { self?.transferMessage = "Waiting for delivery confirmation…"; self?.refresh() }
            }
          }
        } onCancel: { client.close() }
        self?.delivered = true
        self?.transferMessage = "Sent to \(device.name)"
      } catch {
        self?.transferMessage = Task.isCancelled ? "Transfer cancelled" : "\(stage): \(error.localizedDescription)"
        self?.transferError = !Task.isCancelled
      }
      connection?.close()
      if let prepared { let directory = prepared.directory; _ = await Task.detached { try? FileManager.default.removeItem(at: directory) }.value }
      self?.transferring = false; self?.progress.isHidden = self?.delivered != true; self?.transferTask = nil; self?.refresh()
    }
  }
  func sharingService(_ sharingService: NSSharingService, didShareItems items: [Any]) {
    // This callback also fires when the system recipient picker is cancelled.
    systemService = nil; transferring = false
    transferMessage = "AirDrop window closed. Check the receiving device to confirm delivery."
    refresh()
  }
  func sharingService(_ sharingService: NSSharingService, didFailToShareItems items: [Any], error: Error) {
    systemService = nil; transferring = false; transferError = true
    transferMessage = error.localizedDescription
    refresh()
  }
  func sharingService(_ sharingService: NSSharingService, sourceWindowForShareItems items: [Any],
    sharingContentScope: UnsafeMutablePointer<NSSharingService.SharingContentScope>) -> NSWindow? { window }
  private func droppedURLs(_ sender: NSDraggingInfo) -> [URL] {
    (sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
  }
  override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { !transferring && !droppedURLs(sender).isEmpty ? .copy : [] }
  override func prepareForDragOperation(_ sender: NSDraggingInfo) -> Bool { !transferring && !droppedURLs(sender).isEmpty }
  override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    let urls = droppedURLs(sender)
    guard !transferring, !urls.isEmpty else { return false }
    stage(urls); return true
  }
  override func viewDidHide() { super.viewDidHide(); beacon.stop(); stopDiscovery() }
  override func viewDidUnhide() { super.viewDidUnhide(); startDiscovery() }
  override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); startDiscovery() }
  private func startDiscovery() {
    if window != nil && !isHiddenOrHasHiddenAncestor { discovery.start(); updateMotion() }
    else { stopDiscovery(); beacon.stop() }
  }
  private func stopDiscovery() {
    discovery.stop()
    identificationTasks.values.forEach { $0.cancel() }
    identificationTasks = [:]; identificationStarted = []; readyDevices = []
  }
  private func identify(_ devices: [CrawlerAirDropDevice]) {
    let present = Set(devices.map(\.identifier))
    identificationStarted.formIntersection(present)
    readyDevices.formIntersection(present)
    for (id, task) in identificationTasks where !present.contains(id) { task.cancel(); identificationTasks[id] = nil }
    for device in devices where identificationStarted.insert(device.identifier).inserted {
      let id = device.identifier
      let endpoint = AirDropEndpoint(host: device.host, port: device.port)
      identificationTasks[id] = Task { [weak self] in
        do {
          let connection = try await AirDropSender.shared.connection(to: endpoint)
          defer { connection.close() }
          let body = try PropertyListSerialization.data(fromPropertyList: [:], format: .binary, options: 0)
          let response = try await withTaskCancellationHandler {
            try await connection.connect()
            return try await connection.post("/Discover", body: body)
          } onCancel: { connection.close() }
          try Task.checkCancellation()
          let recipient = try PropertyListDecoder().decode(AirDropDiscoveryResponse.self, from: response)
          guard !recipient.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw AirDropNetworkError.message("The device didn't provide an AirDrop name.")
          }
          self?.readyDevices.insert(id)
          self?.discovery.identifyDevice(id, name: recipient.name)
        } catch {
          if !Task.isCancelled {
            self?.discovery.identificationFailed(id, reason: "Couldn't identify. Refresh to retry.")
          }
        }
        if !Task.isCancelled { self?.identificationTasks[id] = nil }
      }
    }
  }
  @objc private func motionChanged() { updateMotion() }
  private func updateMotion() {
    if window != nil && !isHiddenOrHasHiddenAncestor && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion && (transferring || discovery.devices.isEmpty && discovery.failure == nil) { beacon.pulse(repeating: true) }
    else { beacon.stop() }
  }
}

@MainActor
private final class AirDropDeviceTile: NSButton {
  var choose: (() -> Void)?
  var receiveFiles: (([URL]) -> Bool)?
  private var dropHighlighted = false { didSet { needsDisplay = true } }
  private var device: CrawlerAirDropDevice
  private var selectedDevice: Bool
  init(device: CrawlerAirDropDevice, selected: Bool) {
    self.device = device; selectedDevice = selected
    super.init(frame: .zero)
    title = ""; isBordered = false; target = self; action = #selector(activate)
    registerForDraggedTypes([.fileURL])
    setAccessibilityLabel("\(device.name), \(device.detail). Choose files to send.")
    toolTip = "Drop files onto \(device.name) to send"
  }
  required init?(coder: NSCoder) { nil }
  override var isFlipped: Bool { true }
  func update(device: CrawlerAirDropDevice, selected: Bool) {
    self.device = device
    if selectedDevice != selected && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
      wantsLayer = true
      let fade = CATransition(); fade.type = .fade; fade.duration = 0.14
      layer?.add(fade, forKey: "selection")
    }
    selectedDevice = selected
    setAccessibilityLabel("\(device.name), \(device.detail). Choose files to send.")
    setAccessibilityValue(selected ? "selected" : "")
    toolTip = "Drop files onto \(device.name) to send"
    needsDisplay = true
  }
  func reveal() {
    guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { return }
    wantsLayer = true
    let fade = CABasicAnimation(keyPath: "opacity")
    fade.fromValue = 0; fade.toValue = 1; fade.duration = 0.24
    layer?.add(fade, forKey: "discovered")
  }
  @objc private func activate() { choose?() }
  override func draw(_ dirtyRect: NSRect) {
    if selectedDevice || dropHighlighted || isHighlighted {
      Design.selection.setFill()
      NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: Design.classic ? 0 : 12, yRadius: Design.classic ? 0 : 12).fill()
    }
    let color = selectedDevice || dropHighlighted ? Design.selectionText : Design.text
    let symbol = device.name.lowercased().contains("ipad") ? "ipad" : device.name.lowercased().contains("iphone") ? "iphone" : "airplayaudio"
    let image = Design.symbol(symbol, size: 36)
    image?.draw(in: NSRect(x: (bounds.width - 44) / 2, y: 15, width: 44, height: 44))
    drawText(device.name, in: NSRect(x: 8, y: 70, width: bounds.width - 16, height: 20), size: 13, color: color, weight: .medium, align: .center)
    drawText(device.detail, in: NSRect(x: 8, y: 95, width: bounds.width - 16, height: 18), size: 11, color: color, align: .center)
  }
  private func urls(_ sender: NSDraggingInfo) -> [URL] {
    (sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
  }
  override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
    guard isEnabled, !urls(sender).isEmpty else { return [] }
    dropHighlighted = true; return .copy
  }
  override func draggingExited(_ sender: NSDraggingInfo?) { dropHighlighted = false }
  override func draggingEnded(_ sender: NSDraggingInfo) { dropHighlighted = false }
  override func prepareForDragOperation(_ sender: NSDraggingInfo) -> Bool { isEnabled && !urls(sender).isEmpty }
  override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    dropHighlighted = false
    return isEnabled && (receiveFiles?(urls(sender)) ?? false)
  }
}

/// Expanding radio waves indicate activity, never a made-up transfer percentage.
@MainActor
private final class AirDropBeacon: NSView {
  var completed = false
  var highlighted = false { didSet { refresh() } }
  private let icon = NSImageView()
  private var iconSymbol = ""
  private let waves = (0..<3).map { _ in CAShapeLayer() }
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    for wave in waves { layer?.addSublayer(wave) }
    addSubview(icon)
    setAccessibilityElement(false)
    refresh()
  }
  required init?(coder: NSCoder) { nil }
  func refresh() {
    let symbol = completed ? "checkmark.circle.fill" : "airplayaudio"
    if iconSymbol != symbol {
      iconSymbol = symbol
      icon.image = Design.symbol(symbol, size: 36)
      if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
        icon.wantsLayer = true
        let fade = CATransition(); fade.type = .fade; fade.duration = 0.22
        icon.layer?.add(fade, forKey: "outcome")
      }
    }
    icon.contentTintColor = Design.accent
    for wave in waves {
      wave.fillColor = NSColor.clear.cgColor
      wave.strokeColor = Design.accent.withAlphaComponent(highlighted ? 0.55 : 0.2).cgColor
      wave.lineWidth = highlighted ? 2 : 1
    }
    needsLayout = true
  }
  override func layout() {
    super.layout()
    icon.frame = bounds.insetBy(dx: bounds.width / 3, dy: bounds.height / 3)
    for (i, wave) in waves.enumerated() {
      wave.frame = bounds
      wave.path = CGPath(ellipseIn: bounds.insetBy(dx: CGFloat(12 + i * 20), dy: CGFloat(12 + i * 20)), transform: nil)
    }
  }
  func stop() { waves.forEach { $0.removeAllAnimations() } }
  func pulse(repeating: Bool) {
    guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion, !isHiddenOrHasHiddenAncestor else { return }
    if repeating && waves.first?.animation(forKey: "radioWave")?.repeatCount == .infinity { return }
    for (i, wave) in waves.enumerated() {
      let scale = CABasicAnimation(keyPath: "transform.scale")
      scale.fromValue = 0.72
      scale.toValue = 1.12
      let fade = CAKeyframeAnimation(keyPath: "opacity")
      fade.values = [0, 1, 0]
      fade.keyTimes = [0, 0.25, 1]
      let group = CAAnimationGroup()
      group.animations = [scale, fade]
      group.duration = 2.4
      group.beginTime = CACurrentMediaTime() + Double(i) * 0.25
      group.repeatCount = repeating ? .infinity : 1
      group.timingFunction = CAMediaTimingFunction(name: .easeOut)
      wave.add(group, forKey: "radioWave")
    }
  }
}

/// The bar follows submitted archive bytes. A full bar still waits for the receiver's acknowledgement.
@MainActor
private final class AirDropProgress: NSView {
  private let track = CAShapeLayer()
  private let fill = CAShapeLayer()
  private let percentage = label("0%", size: 11, weight: .medium)
  private var fraction = 0.0
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    layer?.addSublayer(track); layer?.addSublayer(fill)
    addSubview(percentage)
    for shape in [track, fill] { shape.lineWidth = 5; shape.lineCap = .round; shape.fillColor = nil }
    fill.strokeEnd = 0
    setAccessibilityElement(true)
    setAccessibilityRole(.progressIndicator)
    setAccessibilityLabel("File upload")
    refresh()
  }
  required init?(coder: NSCoder) { nil }
  override var isFlipped: Bool { true }
  func refresh() {
    track.strokeColor = Design.line.cgColor
    fill.strokeColor = Design.accent.cgColor
    percentage.font = Design.font(11, .medium); percentage.textColor = Design.text
  }
  override func layout() {
    super.layout()
    let path = CGMutablePath()
    path.move(to: CGPoint(x: 3, y: bounds.midY))
    path.addLine(to: CGPoint(x: max(3, bounds.width - 48), y: bounds.midY))
    CATransaction.begin(); CATransaction.setDisableActions(true)
    track.frame = bounds; fill.frame = bounds
    track.path = path; fill.path = path
    CATransaction.commit()
    percentage.frame = NSRect(x: bounds.width - 40, y: 0, width: 40, height: bounds.height)
    percentage.alignment = .right
  }
  func reset() {
    fill.removeAllAnimations(); fraction = 0
    CATransaction.begin(); CATransaction.setDisableActions(true); fill.strokeEnd = 0; CATransaction.commit()
    percentage.stringValue = "0%"; setAccessibilityValue(0)
    isHidden = true
  }
  func update(_ value: Double) {
    fraction = max(fraction, min(1, max(0, value)))
    let start = fill.presentation()?.strokeEnd ?? fill.strokeEnd
    CATransaction.begin(); CATransaction.setDisableActions(true); fill.strokeEnd = fraction; CATransaction.commit()
    if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
      let animation = CABasicAnimation(keyPath: "strokeEnd")
      animation.fromValue = start; animation.toValue = fraction; animation.duration = 0.12
      animation.timingFunction = CAMediaTimingFunction(name: .easeOut)
      fill.add(animation, forKey: "bytes")
    }
    percentage.stringValue = "\(Int(fraction * 100))%"
    setAccessibilityValue(fraction)
  }
}
