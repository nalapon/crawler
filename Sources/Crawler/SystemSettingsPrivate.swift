import Foundation
import IOBluetooth
import CoreAudio
import SystemSettingsBridge

/// Compatibility boundary for user-authorized private macOS controls.
actor SettingsPrivate {
  struct Endpoint: Sendable {
    let id: String
    let name: String
    let modes: [String]
    let current: String?
    let conversation: Bool?
    let headTracking: Bool?
    let writable: Bool
  }
  static let listeningModes = [
    "AVOutputDeviceBluetoothListeningModeNormal": "Off",
    "AVOutputDeviceBluetoothListeningModeAudioTransparency": "Transparency",
    "AVOutputDeviceBluetoothListeningModeAutomatic": "Adaptive",
    "AVOutputDeviceBluetoothListeningModeActiveNoiseCancellation": "Noise cancellation",
  ]
  func endpoints() throws -> [Endpoint] {
    try CRSystemSettingsBridge.audioEndpoints().compactMap { row in
      guard let id = row["id"] as? String, let modes = row["modes"] as? [String] else { return nil }
      return Endpoint(id: id, name: row["name"] as? String ?? "Headphones", modes: modes.filter { Self.listeningModes[$0] != nil },
                      current: row["current"] as? String, conversation: row["conversation"] as? Bool, headTracking: row["headTracking"] as? Bool, writable: row["canSetMode"] as? Bool ?? false)
    }
  }
  func rows() throws -> [SettingsRow] {
    let devices = try endpoints()
    var rows: [SettingsRow] = []
    for device in devices {
      if device.writable, !device.modes.isEmpty {
        rows.append(.init(title: device.name + " · Noise control", detail: "Changes the connected headphones", control: .listeningMode(device.id, device.modes, device.current)))
      }
      if let enabled = device.headTracking {
        rows.append(.init(title: device.name + " · Head tracking", detail: enabled ? "Allowed for spatial audio" : "Off", control: .headTracking(device.id, enabled)))
      }
      if let enabled = device.conversation {
        rows.append(.init(title: device.name + " · Conversation awareness", detail: enabled ? "On" : "Off", control: .conversationDetection(device.id, enabled)))
      }
    }
    for device in try SettingsAudio.snapshot().devices where device.output {
      let selector: UInt32 = 0x6c73746d // lstm: Bluetooth listening mode
      let supportSelector: UInt32 = 0x6c736d73 // lsms: supported mode bitmask
      guard SettingsAudio.editable(device.id, selector, scope: kAudioObjectPropertyScopeGlobal),
            let current = try? SettingsAudio.read(device.id, selector, initial: UInt32(0)),
            let support = try? SettingsAudio.read(device.id, supportSelector, initial: UInt32(0)) else { continue }
      // Off can be restricted by hearing-protection settings. Offer only advertised modes.
      var modes: [UInt32] = []
      if support & 1 != 0 { modes.append(2) }
      if support & 2 != 0 { modes.append(3) }
      if support & 4 != 0 { modes.append(4) }
      if !modes.isEmpty { rows.append(.init(title: device.name + " · Noise control", detail: "Current mode: " + Self.halModes[current, default: "Unknown"], control: .halListeningMode(device.id, modes, current))) }
    }
    if rows.isEmpty { rows.append(.init(title: "Advanced headphone controls", detail: "Connect supported AirPods or Beats and select them as the sound output. macOS has not exposed listening controls for the current output.")) }
    if devices.isEmpty {
      rows.append(.init(title: "Conversation awareness and spatial audio", detail: "macOS has not granted access to these private controls. Listening modes remain available when advertised by the headphones."))
    }
    return rows
  }
  static let halModes: [UInt32: String] = [1: "Off", 2: "Noise cancellation", 3: "Transparency", 4: "Adaptive"]
  func setHALMode(_ mode: UInt32, device: AudioDeviceID) async throws {
    let support = try SettingsAudio.read(device, 0x6c736d73, initial: UInt32(0))
    guard (2...4).contains(mode), support & (1 << (mode - 2)) != 0 else { throw SettingsFailure(message: "This mode is not advertised by the headphones.") }
    try SettingsAudio.write(device, 0x6c73746d, value: mode)
    for _ in 0..<20 {
      if try SettingsAudio.read(device, 0x6c73746d, initial: UInt32(0)) == mode { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw SettingsFailure(message: "The headphones did not confirm the listening mode.")
  }
  func setPower(_ enabled: Bool) async throws {
    try CRSystemSettingsBridge.setBluetoothPower(enabled)
    for _ in 0..<20 {
      guard let controller = IOBluetoothHostController.default() else { throw SettingsFailure(message: "Bluetooth controller unavailable.") }
      if (controller.powerState == kBluetoothHCIPowerStateON) == enabled { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw SettingsFailure(message: "Bluetooth did not reach the requested power state.")
  }
  func setMode(_ mode: String, device: String) async throws {
    guard Self.listeningModes[mode] != nil else { throw SettingsFailure(message: "Unknown listening mode.") }
    try CRSystemSettingsBridge.setListeningMode(mode, device: device)
    for _ in 0..<20 {
      if try endpoints().first(where: { $0.id == device })?.current == mode { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw SettingsFailure(message: "macOS did not confirm the listening mode change. Check that the headphones are connected and in use.")
  }
  func setHeadTracking(_ enabled: Bool, device: String) async throws {
    try CRSystemSettingsBridge.setHeadTracking(enabled, device: device)
    for _ in 0..<20 {
      if try endpoints().first(where: { $0.id == device })?.headTracking == enabled { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw SettingsFailure(message: "macOS did not confirm the head tracking change.")
  }
  func setConversation(_ enabled: Bool, device: String) async throws {
    try CRSystemSettingsBridge.setConversationDetection(enabled, device: device)
    for _ in 0..<20 {
      if try endpoints().first(where: { $0.id == device })?.conversation == enabled { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw SettingsFailure(message: "macOS did not confirm the conversation awareness change.")
  }
}
