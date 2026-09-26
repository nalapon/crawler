import AppKit
import UniformTypeIdentifiers
import IOBluetoothUI

/// A persistent settings workspace. Search and category selection never wait for a hardware read.
@MainActor
final class SystemSettingsView: Surface, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
  var browseStorage: (() -> Void)?
  private let search = NSSearchField()
  private let categories = NSTableView()
  private let categoryScroll = NSScrollView()
  private let detailScroll = NSScrollView()
  private let content = Surface()
  private let visualPreview = SettingsVisualPreview(frame: .zero)
  private let heading = label("System Settings", size: 22, weight: .semibold)
  private let subtitle = label("", size: 12)
  private let status = label("", size: 11)
  private let reader = SystemSettingsReader()
  private let locationAccess = SettingsLocationAccess()
  private var pairing: IOBluetoothPairingController?
  private var sections = SystemSettingsSection.allCases
  private var section = SystemSettingsSection.sound
  private var rows: [SettingsRow] = []
  private var rowViews: [NSView] = []
  private var controls: [NSControl] = []
  private var task: Task<Void, Never>?
  private var operation: Task<Void, Never>?
  private var generation = 0
  private var active = false
  private var timer: Timer?
  private var audioObservation: SettingsAudioObservation?
  private var audioObservationTask: Task<Void, Never>?
  private var audioRefreshNeeded = false
  private var history: [SystemSettingsSection] = [.sound]
  private var historyIndex = 0
  private var pending = false
  private var theme: Theme?

  override init(frame: NSRect) {
    super.init(frame: frame)
    search.placeholderAttributedString = NSAttributedString(string: "Find a setting", attributes: [.foregroundColor: Design.muted])
    search.sendsSearchStringImmediately = true
    search.delegate = self
    search.setAccessibilityLabel("Find a system setting")
    let column = NSTableColumn(identifier: .init("section"))
    categories.addTableColumn(column)
    categories.headerView = nil
    categories.rowHeight = 36
    categories.intercellSpacing = .zero
    categories.backgroundColor = .clear
    categories.selectionHighlightStyle = .regular
    categories.dataSource = self
    categories.delegate = self
    categories.setAccessibilityLabel("System settings sections")
    categoryScroll.documentView = categories
    categoryScroll.drawsBackground = false
    categoryScroll.hasVerticalScroller = true
    categoryScroll.autohidesScrollers = true
    detailScroll.documentView = content
    detailScroll.drawsBackground = false
    detailScroll.hasVerticalScroller = true
    detailScroll.autohidesScrollers = true
    detailScroll.horizontalScrollElasticity = .none
    detailScroll.hasHorizontalScroller = false
    status.maximumNumberOfLines = 2
    status.lineBreakMode = .byWordWrapping
    for view in [search, categoryScroll, heading, subtitle, status, detailScroll] { addSubview(view) }
    categories.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    content.addSubview(visualPreview)
    visualPreview.isHidden = true
    visualPreview.changed = { [weak self] in self?.rebuildRows() }
    visualPreview.chooseWallpaper = { [weak self] url in self?.applyWallpaper(url) }
    visualPreview.chooseAppearance = { [weak self] dark in
      self?.perform { reader in
        try await reader.automation.enable()
        try await reader.automation.set(.darkMode, boolean: dark)
      }
    }
  }
  required init?(coder: NSCoder) { nil }

  func setActive(_ value: Bool) {
    if theme != Design.theme {
      theme = Design.theme
      fill = Design.background
      content.fill = Design.background
      heading.textColor = Design.text
      heading.font = Design.font(22, .semibold)
      subtitle.textColor = Design.muted
      categories.reloadData()
      if let index = sections.firstIndex(of: section) { categories.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
      rebuildRows()
    }
    guard active != value else { return }
    active = value
    if value {
      observeAudio()
      refresh()
      timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
        Task { @MainActor [weak self] in
          guard let self, self.active, !self.pending, self.task == nil,
                !(self.window?.firstResponder is NSSlider), self.search.stringValue.isEmpty else { return }
          self.refresh(quiet: true)
        }
      }
    } else {
      timer?.invalidate(); timer = nil
      audioObservationTask?.cancel(); audioObservationTask = nil
      audioObservation = nil
      audioRefreshNeeded = false
      visualPreview.stop()
      generation += 1
      task?.cancel(); task = nil
    }
  }

  private func observeAudio() {
    audioObservationTask?.cancel()
    audioObservationTask = Task { [weak self] in
      let observation = await Task.detached(priority: .userInitiated) { [weak self] in
        try? SettingsAudioObservation { [weak self] topology in
          Task { @MainActor [weak self] in
            guard let self, self.active else { return }
            if topology { self.observeAudio() }
            guard self.section == .sound || self.section == .airpods, !self.pending else { return }
            if self.task != nil { self.audioRefreshNeeded = true }
            else { self.refresh(quiet: true) }
          }
        }
      }.value
      guard !Task.isCancelled, let self, self.active else { return }
      self.audioObservation = observation
      self.audioObservationTask = nil
      // Cover a change that occurred while listener registration was in flight.
      if self.section == .sound || self.section == .airpods {
        if self.task != nil { self.audioRefreshNeeded = true }
        else { self.refresh(quiet: true) }
      }
    }
  }

  override func layout() {
    super.layout()
    let rail: CGFloat = min(190, max(145, bounds.width * 0.23))
    search.frame = NSRect(x: 18, y: 20, width: max(120, bounds.width - 36), height: 30)
    categoryScroll.frame = NSRect(x: 10, y: 70, width: rail, height: max(0, bounds.height - 86))
    categories.tableColumns.first?.width = rail - 4
    let x = rail + 32, width = max(160, bounds.width - x - 24)
    heading.frame = NSRect(x: x, y: 70, width: width, height: 30)
    subtitle.frame = NSRect(x: x, y: 106, width: width, height: 20)
    status.frame = NSRect(x: x, y: 134, width: width, height: 34)
    detailScroll.frame = NSRect(x: x, y: 180, width: width, height: max(0, bounds.height - 196))
    let rowWidth = detailScroll.contentSize.width
    var y = visualPreview.height(for: rowWidth)
    visualPreview.frame = NSRect(x: 0, y: 0, width: rowWidth, height: y)
    for view in rowViews {
      let height: CGFloat = rowWidth < 480 ? 108 : 76
      view.frame = NSRect(x: 0, y: y, width: rowWidth, height: height)
      if let row = view as? SettingsRowView { row.layoutRow() }
      y += height
    }
    content.frame = NSRect(x: 0, y: 0, width: rowWidth, height: max(y, detailScroll.contentSize.height))
  }

  func numberOfRows(in tableView: NSTableView) -> Int { sections.count }
  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
    let item = sections[row]
    let cell = SettingsCategoryCell()
    let title = label(item.rawValue, size: 12, weight: item == section ? .medium : .regular)
    title.frame = NSRect(x: 31, y: 9, width: max(80, tableView.bounds.width - 36), height: 18)
    title.autoresizingMask = [.width]
    let icon = NSImageView(frame: NSRect(x: 8, y: 10, width: 16, height: 16))
    icon.image = Design.symbol(item.symbol)
    icon.contentTintColor = Design.text
    cell.addSubview(icon); cell.addSubview(title)
    cell.textField = title
    return cell
  }
  func tableViewSelectionDidChange(_ notification: Notification) {
    guard sections.indices.contains(categories.selectedRow) else { return }
    select(sections[categories.selectedRow])
  }
  func controlTextDidChange(_ obj: Notification) {
    sections = SystemSettingsSection.allCases.filter { $0.matches(search.stringValue) }
    categories.reloadData()
    if let first = sections.first {
      categories.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
      select(first)
    } else {
      generation += 1; task?.cancel(); task = nil
      heading.stringValue = "No matching settings"
      subtitle.stringValue = "Try volume, keyboard, Wi-Fi or wallpaper."
      status.stringValue = ""
      rows = []; rebuildRows()
    }
  }
  private func select(_ value: SystemSettingsSection, record: Bool = true) {
    guard value != section || rows.isEmpty else { return }
    section = value
    if record {
      history = Array(history.prefix(historyIndex + 1))
      if history.last != value { history.append(value) }
      historyIndex = history.count - 1
    }
    rows = []; rebuildRows()
    detailScroll.contentView.scroll(to: .zero)
    refresh()
  }
  private func refresh(quiet: Bool = false) {
    guard active, sections.contains(section) else { return }
    generation += 1
    let ticket = generation, requested = section
    task?.cancel()
    heading.stringValue = section.rawValue
    subtitle.stringValue = "System Settings  /  " + section.rawValue
    if !quiet { status.stringValue = "Reading macOS settings…" }
    task = Task { [weak self, reader] in
      do {
        let loaded = try await reader.rows(for: requested)
        guard let self, !Task.isCancelled, self.active, self.generation == ticket else { return }
        self.task = nil
        self.updateVisualPreview()
        if self.rows != loaded {
          self.rows = loaded
          self.rebuildRows()
        }
        if !quiet { self.status.stringValue = "Changes apply to macOS. ⌘R refreshes the current state." }
        if self.audioRefreshNeeded {
          self.audioRefreshNeeded = false
          self.refresh(quiet: true)
        }
      } catch {
        guard let self, !Task.isCancelled, self.generation == ticket else { return }
        self.task = nil
        self.status.stringValue = error.localizedDescription + " Press ⌘R to retry."
      }
    }
  }
  private func updateVisualPreview() {
    let visible = active && sections.contains(section) && (section == .wallpaper || section == .appearance)
    let current = window?.screen.flatMap { NSWorkspace.shared.desktopImageURL(for: $0) }
    let dark = rows.compactMap { row -> Bool? in
      if case .scriptToggle(.darkMode, let enabled) = row.control { return enabled }
      return nil
    }.first ?? (UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark")
    visualPreview.update(section: visible ? section : nil, currentURL: current, dark: dark)
  }

  private func rebuildRows() {
    let focusedIndex = controls.firstIndex { $0 === window?.firstResponder }
    rowViews.forEach { $0.removeFromSuperview() }
    rowViews = []
    updateVisualPreview()
    controls = visualPreview.isHidden ? [] : visualPreview.buttons
    for row in rows {
      let view = SettingsRowView(title: row.title, detail: row.detail)
      let control: NSControl?
      switch row.control {
      case .value: control = nil
      case .link(let pane):
        control = SettingsButton("Open in macOS ↗") { [weak self] in self?.open(pane) }
      case .wallpaper:
        control = SettingsButton("Choose image…") { [weak self] in self?.chooseWallpaper() }
      case .scanWifi:
        control = SettingsButton("Scan networks") { [weak self] in
          self?.status.stringValue = "Waiting for Location permission to scan Wi-Fi…"
          self?.locationAccess.request { [weak self] result in
            guard let self, self.active, self.section == .wifi else { return }
            switch result {
            case .success: self.perform { reader in try await reader.wireless.scan() }
            case .failure(let error): self.status.stringValue = error.localizedDescription
            }
          }
        }
      case .wifiNetwork(let network, let connected):
        let button = SettingsButton(connected ? "Connected" : "Connect…") { [weak self] in self?.join(network) }
        button.isEnabled = !connected
        control = button
      case .disconnectWifi:
        control = SettingsButton("Disconnect") { [weak self] in self?.perform { reader in try await reader.wireless.disconnect() } }
      case .bluetoothConnection(let address, let connected):
        control = SettingsButton(connected ? "Disconnect" : "Connect") { [weak self] in
          self?.perform { reader in try await reader.wireless.connectBluetooth(address: address, connected: !connected) }
        }
      case .pairBluetooth:
        control = SettingsButton("Pair device…") { [weak self] in self?.pairDevice() }
      case .halListeningMode(let device, let modes, let current):
        let labels = modes.map { SettingsPrivate.halModes[$0, default: "Unknown"] }
        let currentLabel = SettingsPrivate.halModes[current, default: "Unknown"]
        let picker = SettingsChoice(options: modes.contains(current) ? labels : [currentLabel] + labels, selected: currentLabel)
        if !modes.contains(current) { picker.item(at: 0)?.isEnabled = false }
        picker.changed = { [weak self] selected in
          guard let index = labels.firstIndex(of: selected) else { return }
          let mode = modes[index]
          self?.perform { reader in try await reader.privateControls.setHALMode(mode, device: device) }
        }
        control = picker
      case .headTracking(let id, let enabled):
        control = SettingsButton(enabled ? "Turn off" : "Turn on") { [weak self] in
          self?.perform { reader in try await reader.privateControls.setHeadTracking(!enabled, device: id) }
        }
      case .preference(let preference, let current):
        let values: [String]
        let labels: [String]
        switch preference {
        case .language:
          values = Array(Set(Locale.LanguageCode.isoLanguageCodes.map(\.identifier) + [current])).sorted()
          labels = values.map { (Locale.current.localizedString(forLanguageCode: $0) ?? $0) + " · " + $0 }
        case .region:
          values = Array(Set(Locale.availableIdentifiers + [current])).sorted()
          labels = values.map { (Locale.current.localizedString(forIdentifier: $0) ?? $0) + " · " + $0 }
        case .accent:
          values = ["-2", "-1", "0", "1", "2", "3", "4", "5", "6"]
          labels = ["Multicolor", "Graphite", "Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Pink"]
        case .repeatInterval, .repeatDelay:
          values = Array(Set((preference == .repeatInterval ? [1, 2, 3, 4, 6, 12] : [10, 15, 25, 30, 45, 60]) + [Int(current) ?? 2])).sorted().map(String.init)
          labels = values.map { "\(Int((Double($0) ?? 0) * 1000 / 60)) ms" }
        }
        let selected = values.firstIndex(of: current).map { labels[$0] } ?? ""
        let picker = SettingsChoice(options: labels, selected: selected)
        picker.changed = { [weak self] selected in
          guard let index = labels.firstIndex(of: selected) else { return }
          let value = values[index]
          self?.perform { reader in try await reader.preferences.set(preference, value: value) }
        }
        control = picker
      case .shortcut(let id, let enabled, let keyCode, let modifiers):
        control = SettingsButton("Edit…") { [weak self] in self?.editShortcut(id, title: row.title, enabled: enabled, keyCode: keyCode, modifiers: modifiers) }
      case .downloadUpdates, .installUpdates:
        let install = row.control == .installUpdates
        control = SettingsButton(install ? "Install…" : "Download…") { [weak self] in
          guard let self else { return }
          self.confirm(title: install ? "Install all available updates?" : "Download all available updates?", detail: install ? "Save your work first. This requests administrator authorization and may take a long time. Crawler will not request an automatic restart." : "This may use several gigabytes of disk space and network data.") {
            self.perform { reader in try await reader.automation.applyUpdates(install: install); try await reader.general.checkUpdates() }
          }
        }
      case .bluetoothPower(let enabled):
        control = SettingsButton(enabled ? "Turn off…" : "Turn on") { [weak self] in
          guard let self else { return }
          if enabled {
            self.confirm(title: "Turn off Bluetooth?", detail: "Bluetooth keyboards, mice and headphones will disconnect.") {
              self.perform { reader in try await reader.privateControls.setPower(false) }
            }
          } else { self.perform { reader in try await reader.privateControls.setPower(true) } }
        }
      case .listeningMode(let id, let modes, let current):
        let labels = modes.map { SettingsPrivate.listeningModes[$0] ?? $0 }
        let picker = SettingsChoice(options: labels, selected: current.flatMap { SettingsPrivate.listeningModes[$0] } ?? "")
        picker.changed = { [weak self] selected in
          guard let index = labels.firstIndex(of: selected) else { return }
          let mode = modes[index]
          self?.perform { reader in try await reader.privateControls.setMode(mode, device: id) }
        }
        control = picker
      case .conversationDetection(let id, let enabled):
        control = SettingsButton(enabled ? "Turn off" : "Turn on") { [weak self] in
          self?.perform { reader in try await reader.privateControls.setConversation(!enabled, device: id) }
        }
      case .timeZone(let current):
        let picker = SettingsChoice(options: TimeZone.knownTimeZoneIdentifiers, selected: current)
        picker.changed = { [weak self] selected in self?.perform { reader in try await reader.automation.setTimeZone(selected) } }
        control = picker
      case .manualDate:
        control = SettingsButton("Set manually…") { [weak self] in self?.chooseDate() }
      case .browseStorage:
        control = SettingsButton("Browse files") { [weak self] in self?.browseStorage?() }
      case .wallpaperLayout:
        control = SettingsButton("Configure…") { [weak self] in self?.configureWallpaper() }
      case .networkTime:
        control = SettingsButton("Configure…") { [weak self] in self?.configureNetworkTime() }
      case .computerName(let name):
        control = SettingsButton("Rename…") { [weak self] in self?.renameComputer(name) }
      case .updateDetails(let report):
        control = SettingsButton("View results…") { [weak self] in self?.showUpdateReport(report) }
      case .checkUpdates:
        control = SettingsButton("Check now") { [weak self] in self?.perform { reader in try await reader.general.checkUpdates() } }
      case .keyboardTiming(let timing, let current):
        let values = Array(Set(timing.choices + [current])).sorted()
        let labels = values.map { "\($0) ms" }
        let picker = SettingsChoice(options: labels, selected: "\(current) ms")
        picker.changed = { [weak self] selected in
          guard let index = labels.firstIndex(of: selected) else { return }
          let milliseconds = values[index]
          self?.perform { _ in try await Task.detached { try SettingsKeyboard.set(timing, milliseconds: milliseconds) }.value }
        }
        control = picker
      case .inputSource(let id, let selected):
        let button = SettingsButton(selected ? "Selected" : "Use input source") { [weak self] in
          self?.perform { reader in try await reader.selectInputSource(id) }
        }
        button.isEnabled = !selected
        control = button
      case .powerMode(let source, let enabled):
        control = SettingsButton(enabled ? "Turn off…" : "Turn on…") { [weak self] in
          self?.perform { reader in try await reader.automation.setPower(source, lowPower: !enabled) }
        }
      case .displaySleep(let source, let minutes):
        let values = Array(Set([0, 1, 2, 5, 10, 15, 30, 60, minutes])).sorted()
        let labels = values.map { $0 == 0 ? "Never" : "\($0) min" }
        let picker = SettingsChoice(options: labels, selected: minutes == 0 ? "Never" : "\(minutes) min")
        picker.changed = { [weak self] selected in
          guard let index = labels.firstIndex(of: selected) else { return }
          let minutes = values[index]
          self?.perform { reader in try await reader.automation.setDisplaySleep(source, minutes: minutes) }
        }
        control = picker
      case .enableAutomation:
        control = SettingsButton("Enable controls…") { [weak self] in
          self?.perform { reader in try await reader.automation.enable() }
        }
      case .scriptToggle(let property, let value):
        control = SettingsButton(value ? "Turn off" : "Turn on") { [weak self] in
          self?.perform { reader in try await reader.automation.set(property, boolean: !value) }
        }
      case .scriptScalar(let property, let value):
        control = makeSlider(value, title: row.title) { [weak self] value in
          self?.perform { reader in try await reader.automation.set(property, scalar: value) }
        }
      case .scriptChoice(let property, let selected):
        let picker = SettingsChoice(options: property.options, selected: selected)
        picker.changed = { [weak self] choice in
          self?.perform { reader in try await reader.automation.set(property, choice: choice) }
        }
        control = picker
      case .audioInputVolume(let id, let value):
        control = makeSlider(Double(value), title: row.title) { [weak self] value in
          self?.perform { reader in try await reader.setAudioInputVolume(id, value: Float32(value)) }
        }
      case .audioBalance(let id, let value):
        control = makeSlider(Double(value), title: row.title) { [weak self] value in
          self?.perform { reader in try await reader.setAudioBalance(id, value: Float32(value)) }
        }
      case .effectsDevice(let id, let selected):
        let button = SettingsButton(selected ? "Selected" : "Use for effects") { [weak self] in
          self?.perform { reader in try await reader.setEffectsDevice(id) }
        }
        button.isEnabled = !selected
        control = button
      case .wifiPower(let on):
        control = SettingsButton(on ? "Turn off" : "Turn on") { [weak self] in
          self?.perform { reader in try await reader.setWifi(!on) }
        }
      case .audioDevice(let id, let input, let selected):
        let button = SettingsButton(selected ? "Selected" : "Use device") { [weak self] in
          self?.perform { reader in try await reader.setAudioDevice(id, input: input) }
        }
        button.isEnabled = !selected
        control = button
      case .mute(let id, let muted):
        control = SettingsButton(muted ? "Unmute" : "Mute") { [weak self] in
          self?.perform { reader in try await reader.setMute(id, value: !muted) }
        }
      case .volume(let id, let volume):
        control = makeSlider(Double(volume), title: row.title) { [weak self] value in
          self?.perform { reader in try await reader.setVolume(id, value: Float32(value)) }
        }
      }
      if let control {
        if !(control is NSSlider) { control.setAccessibilityLabel(row.title + ", " + ((control as? NSButton)?.title ?? "")) }
        view.control = control
        view.addSubview(control)
        controls.append(control)
      }
      content.addSubview(view); rowViews.append(view)
    }
    if pending { controls.forEach { $0.isEnabled = false } }
    search.nextKeyView = categories
    categories.nextKeyView = controls.first ?? search
    for (index, control) in controls.enumerated() { control.nextKeyView = index + 1 < controls.count ? controls[index + 1] : search }
    needsLayout = true
    layoutSubtreeIfNeeded()
    if let focusedIndex, controls.indices.contains(focusedIndex), controls[focusedIndex].isEnabled {
      window?.makeFirstResponder(controls[focusedIndex])
    }
  }
  private func makeSlider(_ value: Double, title: String, changed: @escaping (Double) -> Void) -> NSSlider {
    let slider = SettingsSlider(frame: .zero)
    slider.minValue = 0; slider.maxValue = 1; slider.doubleValue = value
    slider.isContinuous = false
    slider.changed = changed
    slider.setAccessibilityLabel(title)
    return slider
  }
  private func showUpdateReport(_ report: String) {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = "Software Update"
    alert.addButton(withTitle: "Done")
    let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 460, height: 260))
    scroll.hasVerticalScroller = true
    let text = NSTextView(frame: scroll.bounds)
    text.string = report; text.isEditable = false
    text.autoresizingMask = [.width]; text.textContainer?.widthTracksTextView = true
    scroll.documentView = text; alert.accessoryView = scroll
    alert.beginSheetModal(for: window)
  }
  private func chooseDate() {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = "Set date and time"
    alert.informativeText = "This turns off automatic time synchronization. macOS will request administrator authorization."
    alert.addButton(withTitle: "Apply"); alert.addButton(withTitle: "Cancel")
    let picker = NSDatePicker(frame: NSRect(x: 0, y: 0, width: 320, height: 32))
    picker.datePickerElements = [.yearMonthDay, .hourMinuteSecond]
    picker.dateValue = Date(); alert.accessoryView = picker
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn else { return }
      let date = picker.dateValue
      self?.perform { reader in try await reader.automation.setDate(date) }
    }
  }
  private func configureWallpaper() {
    guard let window, let screen = window.screen, let url = NSWorkspace.shared.desktopImageURL(for: screen) else { return }
    let alert = NSAlert()
    alert.messageText = "Picture layout"
    alert.informativeText = "Applies to the display containing Crawler."
    alert.addButton(withTitle: "Apply"); alert.addButton(withTitle: "Cancel")
    let picker = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 320, height: 28))
    picker.addItems(withTitles: ["Fill screen", "Fit to screen", "Stretch", "Center"])
    let original = NSWorkspace.shared.desktopImageOptions(for: screen) ?? [:]
    let scaling = (original[.imageScaling] as? NSNumber)?.uintValue
    let clipping = original[.allowClipping] as? Bool ?? false
    picker.selectItem(at: scaling == NSImageScaling.scaleAxesIndependently.rawValue ? 2 : scaling == NSImageScaling.scaleNone.rawValue ? 3 : clipping ? 0 : 1)
    alert.accessoryView = picker
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn else { return }
      var options = original
      options[.imageScaling] = picker.indexOfSelectedItem == 2 ? NSImageScaling.scaleAxesIndependently.rawValue : picker.indexOfSelectedItem == 3 ? NSImageScaling.scaleNone.rawValue : NSImageScaling.scaleProportionallyUpOrDown.rawValue
      options[.allowClipping] = picker.indexOfSelectedItem == 0
      do { try NSWorkspace.shared.setDesktopImageURL(url, for: screen, options: options); self?.refresh() }
      catch { self?.status.stringValue = error.localizedDescription }
    }
  }
  private func editShortcut(_ id: String, title: String, enabled: Bool, keyCode: Int, modifiers: Int) {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = "Click the key field and press a combination. Saving updates the macOS shortcut definition. Some apps may require a new login."
    alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Cancel")
    let box = NSView(frame: NSRect(x: 0, y: 0, width: 340, height: 72))
    let toggle = NSButton(checkboxWithTitle: "Enabled", target: nil, action: nil)
    toggle.frame = NSRect(x: 0, y: 44, width: 340, height: 24); toggle.state = enabled ? .on : .off
    let recorder = SettingsShortcutRecorder(frame: NSRect(x: 0, y: 0, width: 340, height: 32))
    recorder.keyCode = keyCode; recorder.modifiers = modifiers; recorder.updateLabel()
    box.addSubview(toggle); box.addSubview(recorder); alert.accessoryView = box
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn else { return }
      let enabled = toggle.state == .on, code = recorder.keyCode, flags = recorder.modifiers
      self?.perform { reader in try await reader.preferences.setShortcut(id, enabled: enabled, keyCode: code, modifiers: flags) }
    }
    alert.window.makeFirstResponder(recorder)
  }
  private func confirm(title: String, detail: String, action: @escaping () -> Void) {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = title; alert.informativeText = detail
    alert.addButton(withTitle: "Continue"); alert.addButton(withTitle: "Cancel")
    alert.beginSheetModal(for: window) { response in if response == .alertFirstButtonReturn { action() } }
  }
  private func configureNetworkTime() {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = "Automatic date and time"
    alert.informativeText = "macOS will request administrator authorization."
    alert.addButton(withTitle: "Enable"); alert.addButton(withTitle: "Disable"); alert.addButton(withTitle: "Cancel")
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn || response == .alertSecondButtonReturn else { return }
      let enabled = response == .alertFirstButtonReturn
      self?.perform { reader in try await reader.automation.setNetworkTime(enabled) }
    }
  }
  private func renameComputer(_ current: String) {
    guard let window else { return }
    let alert = NSAlert()
    alert.messageText = "Computer name"
    alert.informativeText = "Changes the name this Mac advertises. Administrator authorization is required."
    alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Cancel")
    let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 28))
    field.stringValue = current; field.setAccessibilityLabel("Computer name")
    alert.accessoryView = field
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn else { return }
      let name = field.stringValue
      self?.perform { reader in try await reader.automation.setComputerName(name) }
    }
    alert.window.makeFirstResponder(field)
  }
  private func join(_ network: SettingsNetwork) {
    guard let window, !pending else { return }
    let alert = NSAlert()
    alert.messageText = "Connect to \(network.name)"
    alert.informativeText = network.security == .open ? "This network is open and does not encrypt Wi-Fi traffic." : "Credentials are used for this connection and are not saved by Crawler."
    alert.addButton(withTitle: "Connect")
    alert.addButton(withTitle: "Cancel")
    let credentials = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: network.security == .enterprise ? 80 : 32))
    let password = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 28))
    password.placeholderString = "Network password"
    password.setAccessibilityLabel("Network password")
    let username = NSTextField(frame: NSRect(x: 0, y: 44, width: 320, height: 28))
    username.placeholderString = "Username"
    username.setAccessibilityLabel("Network username")
    if network.security != .open {
      credentials.addSubview(password)
      if network.security == .enterprise { credentials.addSubview(username) }
      alert.accessoryView = credentials
    }
    alert.beginSheetModal(for: window) { [weak self] response in
      guard response == .alertFirstButtonReturn else { password.stringValue = ""; username.stringValue = ""; return }
      let secret = network.security == .open ? nil : password.stringValue
      let account = network.security == .enterprise ? username.stringValue : nil
      password.stringValue = ""; username.stringValue = ""
      self?.perform { reader in try await reader.wireless.connect(network.id, username: account, password: secret) }
    }
    if network.security != .open { alert.window.makeFirstResponder(network.security == .enterprise ? username : password) }
  }
  private func pairDevice() {
    guard pairing == nil else { return }
    let controller = IOBluetoothPairingController()
    pairing = controller
    _ = controller.runModal()
    pairing = nil
    refresh()
  }

  private func perform(_ action: @escaping @Sendable (SystemSettingsReader) async throws -> Void) {
    guard !pending else { return }
    let origin = section
    pending = true
    generation += 1
    task?.cancel(); task = nil
    controls.forEach { $0.isEnabled = false }
    status.stringValue = "Applying change…"
    operation = Task { [weak self, reader] in
      var failure: String?
      do { try await action(reader) } catch { failure = error.localizedDescription }
      guard let self else { return }
      self.pending = false
      self.operation = nil
      self.rebuildRows()
      if let failure {
        self.status.stringValue = origin.rawValue + ": " + failure
        self.refresh(quiet: true)
      } else { self.refresh() }
    }
  }
  private func open(_ pane: String) {
    guard let url = URL(string: "x-apple.systempreferences:" + pane), NSWorkspace.shared.open(url) else {
      status.stringValue = "Could not open this macOS settings page. Open System Settings from the Apple menu."
      return
    }
  }
  private func applyWallpaper(_ url: URL) {
    guard !pending, let screen = window?.screen else { return }
    do {
      let options = NSWorkspace.shared.desktopImageOptions(for: screen) ?? [:]
      try NSWorkspace.shared.setDesktopImageURL(url, for: screen, options: options)
      updateVisualPreview()
      status.stringValue = "Desktop picture updated."
    } catch { status.stringValue = error.localizedDescription }
  }
  private func chooseWallpaper() {
    guard let window, window.screen != nil else { return }
    let panel = NSOpenPanel()
    panel.allowedContentTypes = [.image]
    panel.canChooseDirectories = false
    panel.allowsMultipleSelection = false
    panel.prompt = "Set wallpaper"
    panel.message = "Choose a desktop picture for the display containing this window."
    panel.beginSheetModal(for: window) { [weak self] response in
      guard response == .OK, let url = panel.url, let self else { return }
      self.applyWallpaper(url)
    }
  }
  func focusIfNeeded() {
    if window?.firstResponder is Workspace || window?.firstResponder === window { window?.makeFirstResponder(categories) }
  }

  func handleKey(_ event: NSEvent, vim: Bool) -> Bool {
    let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
    if event.modifierFlags.contains(.command) {
      switch key {
      case "f": window?.makeFirstResponder(search); return true
      case "r": refresh(); return true
      case "[", "]":
        let next = historyIndex + (key == "[" ? -1 : 1)
        guard history.indices.contains(next) else { return true }
        historyIndex = next
        search.stringValue = ""; sections = SystemSettingsSection.allCases
        let destination = history[next]
        categories.reloadData()
        select(destination, record: false)
        if let index = sections.firstIndex(of: destination) { categories.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
        return true
      default: return false
      }
    }
    if event.keyCode == 48 && !event.modifierFlags.contains(.control) && !event.modifierFlags.contains(.option) {
      let chain: [NSView] = [search, categories] + controls.filter(\.isEnabled)
      let responder = window?.firstResponder
      let index = chain.firstIndex { $0 === responder || ($0 === search && search.currentEditor() === responder) }
      let delta = event.modifierFlags.contains(.shift) ? -1 : 1
      let next = ((index ?? (delta > 0 ? -1 : 0)) + delta + chain.count) % chain.count
      window?.makeFirstResponder(chain[next])
      chain[next].scrollToVisible(chain[next].bounds)
      return true
    }
    if event.keyCode == 53 {
      search.stringValue = ""; sections = SystemSettingsSection.allCases
      categories.reloadData()
      if let index = sections.firstIndex(of: section) { categories.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
      window?.makeFirstResponder(categories)
      refresh()
      return true
    }
    let editing = window?.firstResponder is NSTextView
    if editing {
      if event.keyCode == 125 || event.keyCode == 36 {
        window?.makeFirstResponder(categories); return true
      }
      return false
    }
    if window?.firstResponder === categories {
      if event.keyCode == 36 || event.keyCode == 124 {
        if let control = controls.first(where: \.isEnabled) { window?.makeFirstResponder(control) }
        return true
      }
      if vim && (key == "j" || key == "k") && !sections.isEmpty {
        let next = min(sections.count - 1, max(0, categories.selectedRow + (key == "j" ? 1 : -1)))
        categories.selectRowIndexes(IndexSet(integer: next), byExtendingSelection: false)
        categories.scrollRowToVisible(next)
        return true
      }
    }
    return false
  }
}

@MainActor
private final class SettingsRowView: Surface {
  let title: NSTextField
  let detail: NSTextField
  var control: NSControl?
  init(title: String, detail: String) {
    self.title = label(title, size: 13, weight: .medium)
    self.detail = label(detail, size: 11, color: Design.muted)
    super.init(frame: .zero)
    self.detail.maximumNumberOfLines = 2
    self.detail.lineBreakMode = .byWordWrapping
    bottomLine = true
    addSubview(self.title); addSubview(self.detail)
  }
  required init?(coder: NSCoder) { nil }
  func layoutRow() {
    let stacked = bounds.width < 480
    let textWidth = max(100, bounds.width - (control == nil || stacked ? 16 : 188))
    title.frame = NSRect(x: 0, y: 12, width: textWidth, height: 20)
    detail.frame = NSRect(x: 0, y: 35, width: textWidth, height: 32)
    control?.frame = NSRect(x: stacked ? 0 : bounds.width - 168, y: stacked ? 72 : 21, width: 164, height: 28)
  }
}

@MainActor
private final class SettingsButton: NSButton {
  let invoke: () -> Void
  override var acceptsFirstResponder: Bool { true }
  init(_ title: String, action: @escaping () -> Void) {
    invoke = action
    super.init(frame: .zero)
    self.title = title
    bezelStyle = .rounded
    font = Design.font(11)
    target = self
    self.action = #selector(activate)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func activate() { invoke() }
}

@MainActor
private final class SettingsSlider: NSSlider {
  var changed: ((Double) -> Void)?
  override var acceptsFirstResponder: Bool { true }
  override init(frame: NSRect) {
    super.init(frame: frame)
    target = self; action = #selector(updateValue)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func updateValue() { changed?(doubleValue) }
}

@MainActor
private final class SettingsCategoryCell: NSTableCellView {
  override var backgroundStyle: NSView.BackgroundStyle {
    didSet {
      textField?.textColor = backgroundStyle == .emphasized ? .alternateSelectedControlTextColor : Design.text
    }
  }
}

@MainActor
private final class SettingsChoice: NSPopUpButton {
  var changed: ((String) -> Void)?
  private let options: [String]
  override var acceptsFirstResponder: Bool { true }
  init(options: [String], selected: String) {
    self.options = options
    super.init(frame: .zero, pullsDown: false)
    addItems(withTitles: options.map(\.capitalized))
    if let index = options.firstIndex(of: selected) { selectItem(at: index) }
    target = self; action = #selector(updateChoice)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func updateChoice() {
    guard options.indices.contains(indexOfSelectedItem) else { return }
    changed?(options[indexOfSelectedItem])
  }
}

@MainActor
private final class SettingsShortcutRecorder: NSButton {
  var keyCode = 0
  var modifiers = 0
  override init(frame: NSRect) {
    super.init(frame: frame)
    target = self; action = #selector(beginRecording)
  }
  required init?(coder: NSCoder) { nil }
  @objc private func beginRecording() { window?.makeFirstResponder(self) }
  override var acceptsFirstResponder: Bool { true }
  override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self) }
  override func keyDown(with event: NSEvent) {
    if event.keyCode == 48 { super.keyDown(with: event); return }
    keyCode = Int(event.keyCode)
    modifiers = Int(event.modifierFlags.intersection([.command, .option, .control, .shift]).rawValue)
    updateLabel()
  }
  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    guard window?.firstResponder === self else { return super.performKeyEquivalent(with: event) }
    keyDown(with: event); return true
  }
  func updateLabel() { title = SettingsKeyboard.shortcutLabel(keyCode: keyCode, modifiers: modifiers); setAccessibilityLabel("Shortcut combination, " + title) }
}
