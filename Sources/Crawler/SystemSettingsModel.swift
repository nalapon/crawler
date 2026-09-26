import AppKit
import CoreAudio
import AudioToolbox
import CoreWLAN
import IOBluetooth
import IOKit.ps
import Carbon
import SystemSettingsBridge

// Search uses this local catalogue; it never waits for a device or a network scan.
enum SystemSettingsSection: String, CaseIterable, Sendable {
  case sound = "Sound", keyboard = "Keyboard", general = "General", wifi = "Wi-Fi"
  case bluetooth = "Bluetooth", battery = "Battery", wallpaper = "Wallpaper"
  case appearance = "Appearance", desktop = "Desktop & Dock", airpods = "AirPods"
  var symbol: String {
    switch self {
    case .sound: "speaker.wave.2"
    case .keyboard: "keyboard"
    case .general: "gearshape"
    case .wifi: "wifi"
    case .bluetooth: "antenna.radiowaves.left.and.right"
    case .battery: "battery.100percent"
    case .wallpaper: "photo"
    case .appearance: "paintpalette"
    case .desktop: "dock.rectangle"
    case .airpods: "airpodspro"
    }
  }
  var pane: String {
    switch self {
    case .sound: "com.apple.Sound-Settings.extension"
    case .keyboard: "com.apple.Keyboard-Settings.extension"
    case .general: "com.apple.systempreferences.GeneralSettings"
    case .wifi: "com.apple.wifi-settings-extension"
    case .bluetooth: "com.apple.BluetoothSettings"
    case .battery: "com.apple.Battery-Settings.extension"
    case .wallpaper: "com.apple.Wallpaper-Settings.extension"
    case .appearance: "com.apple.Appearance-Settings.extension"
    case .desktop: "com.apple.Desktop-Settings.extension"
    case .airpods: "com.apple.HeadphoneSettings"
    }
  }
  var keywords: String {
    switch self {
    case .sound: "sonido audio volume volumen output salida input entrada microphone micrófono mute silencio speakers altavoces"
    case .keyboard: "teclado typing escritura repeat repetición shortcuts atajos dictation dictado input sources idioma"
    case .general: "general about información mac storage almacenamiento language idioma region región date fecha time hora updates actualizaciones"
    case .wifi: "wifi wi-fi wireless inalámbrica internet network red connection conexión"
    case .bluetooth: "bluetooth devices dispositivos paired emparejados connect conectar"
    case .battery: "batería battery power energía charging carga low power ahorro"
    case .wallpaper: "fondos pantalla wallpaper desktop image imagen"
    case .appearance: "aspecto appearance light dark claro oscuro accent acento color"
    case .desktop: "escritorio dock docks windows ventanas stage manager mission control"
    case .airpods: "airpods headphones auriculares noise cancellation cancelación ruido spatial espacial"
    }
  }
  func matches(_ query: String) -> Bool {
    let text = (rawValue + " " + keywords).folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
    return query.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
      .split(whereSeparator: \.isWhitespace).allSatisfy { text.contains($0) }
  }
}

struct SettingsRow: Sendable, Equatable {
  let title: String
  let detail: String
  var control: Control = .value
  enum Control: Sendable, Equatable {
    case value
    case link(String)
    case wifiPower(Bool)
    case audioDevice(AudioDeviceID, input: Bool, selected: Bool)
    case volume(AudioDeviceID, Float32)
    case mute(AudioDeviceID, Bool)
    case wallpaper
    case audioInputVolume(AudioDeviceID, Float32)
    case audioBalance(AudioDeviceID, Float32)
    case effectsDevice(AudioDeviceID, selected: Bool)
    case scanWifi
    case wifiNetwork(SettingsNetwork, connected: Bool)
    case disconnectWifi
    case bluetoothConnection(String, connected: Bool)
    case pairBluetooth
    case inputSource(String, selected: Bool)
    case keyboardTiming(SettingsKeyboard.Timing, Int)
    case timeZone(String)
    case networkTime
    case manualDate
    case browseStorage
    case wallpaperLayout
    case computerName(String)
    case checkUpdates
    case updateDetails(String)
    case bluetoothPower(Bool)
    case listeningMode(String, [String], String?)
    case conversationDetection(String, Bool)
    case headTracking(String, Bool)
    case halListeningMode(AudioDeviceID, [UInt32], UInt32)
    case preference(SettingsPreferences.Choice, String)
    case shortcut(String, Bool, Int, Int)
    case downloadUpdates
    case installUpdates
    case powerMode(SettingsPowerSource, Bool)
    case displaySleep(SettingsPowerSource, Int)
    case enableAutomation
    case scriptToggle(SettingsScriptProperty, Bool)
    case scriptScalar(SettingsScriptProperty, Double)
    case scriptChoice(SettingsScriptProperty, String)
  }
}

/// Blocking framework calls run on this actor, never on AppKit's main actor.
actor SystemSettingsReader {
  let wireless = SettingsWireless()
  let automation = SettingsAutomation()
  let general = SettingsGeneral()
  let privateControls = SettingsPrivate()
  let preferences = SettingsPreferences()
  func rows(for section: SystemSettingsSection) async throws -> [SettingsRow] {
    switch section {
    case .sound:
      let audio = try SettingsAudio.snapshot()
      var rows = audio.devices.filter(\.output).map {
        SettingsRow(title: $0.name, detail: "Output device", control: .audioDevice($0.id, input: false, selected: audio.output == $0.id))
      }
      if let volume = audio.volume { rows.append(.init(title: "Output volume", detail: "\(Int(volume * 100))%", control: .volume(audio.output, volume))) }
      else { rows.append(.init(title: "Output volume", detail: "This output does not expose a software volume control.")) }
      if let muted = audio.muted { rows.append(.init(title: "Mute output", detail: muted ? "Muted" : "Sound on", control: .mute(audio.output, muted))) }
      rows += audio.devices.filter(\.input).map {
        SettingsRow(title: $0.name, detail: "Input device", control: .audioDevice($0.id, input: true, selected: audio.input == $0.id))
      }
      if let level = audio.inputVolume {
        rows.append(.init(title: "Input volume", detail: "\(Int(level * 100))%", control: .audioInputVolume(audio.input, level)))
      } else { rows.append(.init(title: "Input volume", detail: "The selected device does not provide adjustable input gain.")) }
      if let balance = audio.balance {
        rows.append(.init(title: "Output balance", detail: "Left ↔ Right", control: .audioBalance(audio.output, balance)))
      }
      rows += audio.devices.filter(\.output).map {
        SettingsRow(title: $0.name, detail: "Sound effects output", control: .effectsDevice($0.id, selected: audio.effectsOutput == $0.id))
      }
      return rows
    case .wifi:
      guard let wifi = CWWiFiClient.shared().interface() else { return [.init(title: "Wi-Fi unavailable", detail: "macOS did not report a Wi-Fi interface.")] }
      var rows: [SettingsRow] = [
        .init(title: "Wi-Fi", detail: wifi.powerOn() ? "On" : "Off", control: .wifiPower(wifi.powerOn())),
        .init(title: "Network", detail: wifi.ssid() ?? "Network name unavailable. Scan to request Location access."),
        .init(title: "Available networks", detail: "Scan nearby networks, then choose one to connect.", control: .scanWifi),
      ]
      if wifi.ssidData() != nil {
        rows.append(.init(title: "Disconnect from network", detail: "Wi-Fi remains on.", control: .disconnectWifi))
      }
      for network in await wireless.results() {
        rows.append(.init(title: network.name, detail: "\(network.security.rawValue) · \(network.signal) dBm", control: .wifiNetwork(network, connected: wifi.ssid() == network.name)))
      }
      return rows
    case .bluetooth, .airpods:
      // AirPods names can be changed. Do not claim that a name filter identifies every AirPods model.
      let devices = (IOBluetoothDevice.pairedDevices() as? [IOBluetoothDevice]) ?? []
      let visible = section == .airpods ? devices.filter { ($0.name ?? "").localizedCaseInsensitiveContains("airpods") } : devices
      var rows = visible.map { device in
        SettingsRow(title: device.name ?? "Bluetooth device", detail: device.isConnected() ? "Connected" : "Not connected",
                    control: device.addressString.map { .bluetoothConnection($0, connected: device.isConnected()) } ?? .value)
      }
      if rows.isEmpty { rows.append(.init(title: section == .airpods ? "No device named AirPods found" : "No paired devices reported", detail: section == .airpods ? "Renamed AirPods may appear under Bluetooth." : "Manage pairing in macOS Bluetooth settings.")) }
      rows.append(.init(title: "Pair a device", detail: "Discover and pair using the native Bluetooth assistant.", control: .pairBluetooth))
      if section == .airpods { rows += try await privateControls.rows() }
      if section == .bluetooth, CRSystemSettingsBridge.canSetBluetoothPower() {
        let on = IOBluetoothHostController.default()?.powerState == kBluetoothHCIPowerStateON
        rows.insert(.init(title: "Bluetooth", detail: on ? "On" : "Off", control: .bluetoothPower(on)), at: 0)
      }
      return rows
    case .battery:
      guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
            let sources = IOPSCopyPowerSourcesList(info)?.takeRetainedValue() as? [CFTypeRef] else {
        throw CocoaError(.fileReadUnknown)
      }
      var rows: [SettingsRow] = []
      for source in sources {
        guard let data = IOPSGetPowerSourceDescription(info, source)?.takeUnretainedValue() as? [String: Any] else { continue }
        let name = data[kIOPSNameKey] as? String ?? "Power source"
        if let current = data[kIOPSCurrentCapacityKey] as? Int, let maximum = data[kIOPSMaxCapacityKey] as? Int, maximum > 0 {
          rows.append(.init(title: name, detail: "\(current * 100 / maximum)%"))
        }
        if let state = data[kIOPSPowerSourceStateKey] as? String { rows.append(.init(title: "Power source", detail: state)) }
        if let charging = data[kIOPSIsChargingKey] as? Bool { rows.append(.init(title: "Charging", detail: charging ? "Yes" : "No")) }
      }
      if rows.isEmpty { rows.append(.init(title: "No battery reported", detail: "This Mac may use external power only.")) }
      rows.append(.init(title: "Low Power Mode", detail: ProcessInfo.processInfo.isLowPowerModeEnabled ? "On" : "Off"))
      for power in try SettingsPower.read() {
        if let enabled = power.lowPower {
          rows.append(.init(title: "Low Power Mode · " + power.source.title, detail: "Administrator authorization required", control: .powerMode(power.source, enabled)))
        }
        if let minutes = power.displaySleep {
          rows.append(.init(title: "Turn display off · " + power.source.title, detail: minutes == 0 ? "Never" : "After \(minutes) minutes", control: .displaySleep(power.source, minutes)))
        }
      }
      return rows
    case .general:
      let process = ProcessInfo.processInfo
      let disk = try? URL(fileURLWithPath: NSHomeDirectory()).resourceValues(forKeys: [.volumeTotalCapacityKey, .volumeAvailableCapacityKey])
      var rows: [SettingsRow] = [
        .init(title: "macOS", detail: process.operatingSystemVersionString),
        .init(title: "Memory", detail: ByteCountFormatter.string(fromByteCount: Int64(process.physicalMemory), countStyle: .memory)),
        .init(title: "Processors", detail: "\(process.processorCount) logical processors"),
      ]
      if let available = disk?.volumeAvailableCapacity, let total = disk?.volumeTotalCapacity {
        rows.append(.init(title: "Storage · home volume", detail: "\(ByteCountFormatter.string(fromByteCount: Int64(available), countStyle: .file)) available of \(ByteCountFormatter.string(fromByteCount: Int64(total), countStyle: .file))"))
      }
      rows.append(.init(title: "Computer name", detail: Host.current().localizedName ?? "Mac", control: .computerName(Host.current().localizedName ?? "Mac")))
      rows.append(.init(title: "Time zone", detail: TimeZone.current.identifier, control: .timeZone(TimeZone.current.identifier)))
      rows.append(.init(title: "Automatic date and time", detail: "Set whether macOS synchronizes its clock with a network time server.", control: .networkTime))
      rows.append(.init(title: "Set date and time", detail: "Set the clock manually and disable automatic time synchronization.", control: .manualDate))
      rows.append(.init(title: "Browse storage", detail: "Inspect files and folder sizes in Crawler.", control: .browseStorage))
      rows += await general.updateRows()
      rows.append(.init(title: "Preferred language", detail: "Applies to apps after relaunch; macOS may require signing out.", control: .preference(.language, await preferences.read(.language) ?? Locale.preferredLanguages.first ?? "en")))
      rows.append(.init(title: "Region and formats", detail: "Date, number and currency formats. Apps may need to restart.", control: .preference(.region, await preferences.read(.region) ?? Locale.current.identifier)))
      rows.append(.init(title: "Download available updates", detail: "Downloads updates without installing or restarting.", control: .downloadUpdates))
      rows.append(.init(title: "Install available updates", detail: "Administrator authorization required. Installation can take a long time; some updates require a restart or volume-owner authorization.", control: .installUpdates))
      return rows
    case .keyboard:
      var rows = try await Self.keyboardRows()
      for (timing, preference): (SettingsKeyboard.Timing, SettingsPreferences.Choice) in [(.interval, .repeatInterval), (.delay, .repeatDelay)] {
        let milliseconds = try SettingsKeyboard.milliseconds(timing)
        let ticks = await preferences.read(preference) ?? String(Int((Double(milliseconds) * 60 / 1000).rounded()))
        rows.append(.init(title: timing.title, detail: "Saved for future logins and applied now", control: .preference(preference, ticks)))
      }
      let names = ["17": "Zoom in", "23": "Smooth zoomed images", "79": "Previous desktop", "80": "Previous desktop slowly", "81": "Next desktop", "82": "Next desktop slowly", "98": "Help menu", "32": "Mission Control", "33": "Application windows", "36": "Show desktop", "60": "Previous input source", "61": "Next input source", "64": "Spotlight", "65": "Finder search"]
      for shortcut in await preferences.shortcuts() {
        let number = Int(shortcut.id) ?? 0
        let title = names[shortcut.id] ?? ((118...133).contains(number) ? "Switch to desktop \(number - 117)" : "Other shortcut \(shortcut.id)")
        let combination = await SettingsKeyboard.shortcutLabel(keyCode: shortcut.keyCode, modifiers: shortcut.modifiers)
        rows.append(.init(title: title,
                          detail: "\(shortcut.enabled ? "Enabled" : "Disabled") · " + combination,
                          control: .shortcut(shortcut.id, shortcut.enabled, shortcut.keyCode, shortcut.modifiers)))
      }
      return rows
    case .wallpaper:
      return [.init(title: "Desktop picture", detail: "Choose an image for this display", control: .wallpaper),
              .init(title: "Picture layout", detail: "Scale and crop the current desktop image", control: .wallpaperLayout)]
    case .appearance, .desktop:
      var rows = try await automation.rows(for: section)
      if section == .appearance {
        rows.append(.init(title: "Accent color", detail: "Applications may need to restart", control: .preference(.accent, await preferences.read(.accent) ?? "-2")))
      }
      return rows
    }
  }
  // Carbon input-source APIs require the main queue, including property reads.
  @MainActor private static func keyboardRows() throws -> [SettingsRow] {
      let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
      let currentID = inputString(current, kTISPropertyInputSourceID)
      guard let category = kTISCategoryKeyboardInputSource else { throw SettingsFailure(message: "Keyboard input sources are unavailable.") }
      let filter = [kTISPropertyInputSourceIsSelectCapable: true, kTISPropertyInputSourceIsEnabled: true, kTISPropertyInputSourceCategory: category] as CFDictionary
      let sources = TISCreateInputSourceList(filter, false).takeRetainedValue() as? [TISInputSource] ?? []
      return sources.compactMap { source in
        guard let id = inputString(source, kTISPropertyInputSourceID) else { return nil }
        return SettingsRow(title: inputString(source, kTISPropertyLocalizedName) ?? id, detail: "Keyboard input source", control: .inputSource(id, selected: id == currentID))
      }
  }
  @MainActor private static func inputString(_ source: TISInputSource, _ key: CFString) -> String? {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return nil }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
  }
  @MainActor func selectInputSource(_ id: String) throws {
    let filter = [kTISPropertyInputSourceID: id] as CFDictionary
    let sources = TISCreateInputSourceList(filter, false).takeRetainedValue() as? [TISInputSource] ?? []
    guard let source = sources.first else { throw SettingsFailure(message: "This keyboard input source is no longer available.") }
    let result = TISSelectInputSource(source)
    guard result == noErr else { throw SettingsFailure(message: "macOS rejected the input source change (\(result)).") }
  }
  func setAudioInputVolume(_ id: AudioDeviceID, value: Float32) throws {
    try SettingsAudio.write(id, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, value: min(1, max(0, value)), scope: kAudioDevicePropertyScopeInput)
  }
  func setAudioBalance(_ id: AudioDeviceID, value: Float32) throws {
    try SettingsAudio.write(id, kAudioHardwareServiceDeviceProperty_VirtualMainBalance, value: min(1, max(0, value)), scope: kAudioDevicePropertyScopeOutput)
  }
  func setEffectsDevice(_ id: AudioDeviceID) throws {
    try SettingsAudio.write(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultSystemOutputDevice, value: id)
  }
  private func link(_ title: String, _ detail: String, _ pane: String) -> SettingsRow {
    .init(title: title, detail: detail, control: .link(pane))
  }
  func setWifi(_ powered: Bool) throws {
    guard let wifi = CWWiFiClient.shared().interface() else { throw CocoaError(.featureUnsupported) }
    try wifi.setPower(powered)
    guard wifi.powerOn() == powered else { throw CocoaError(.validationMissingMandatoryProperty) }
  }
  func setAudioDevice(_ id: AudioDeviceID, input: Bool) throws {
    try SettingsAudio.write(AudioObjectID(kAudioObjectSystemObject), input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice, value: id)
  }
  func setVolume(_ id: AudioDeviceID, value: Float32) throws {
    try SettingsAudio.write(id, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, value: min(1, max(0, value)), scope: kAudioDevicePropertyScopeOutput)
  }
  func setMute(_ id: AudioDeviceID, value: Bool) throws {
    try SettingsAudio.write(id, kAudioDevicePropertyMute, value: UInt32(value ? 1 : 0), scope: kAudioDevicePropertyScopeOutput)
  }
}
