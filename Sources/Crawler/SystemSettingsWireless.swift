import Foundation
import CoreWLAN
import IOBluetooth

struct SettingsFailure: LocalizedError, Sendable {
  let message: String
  var errorDescription: String? { message }
}

struct SettingsNetwork: Sendable, Equatable {
  let id: UUID
  let name: String
  let signal: Int
  let security: Security
  enum Security: String, Sendable { case open = "Open", personal = "Password", enterprise = "Enterprise" }
}

/// Retains CoreWLAN scan objects on one worker; UI actions carry only an opaque scan identity.
actor SettingsWireless {
  private var scanned: [UUID: CWNetwork] = [:]
  private var networks: [SettingsNetwork] = []

  func scan() throws {
    guard let wifi = CWWiFiClient.shared().interface(), wifi.powerOn() else {
      throw SettingsFailure(message: "Turn on Wi-Fi before scanning.")
    }
    let found = try wifi.scanForNetworks(withSSID: nil)
    var unique: [String: CWNetwork] = [:]
    for network in found {
      guard let name = network.ssid, !name.isEmpty else { continue }
      let key = name + ":" + security(network).rawValue
      if let previous = unique[key], previous.rssiValue >= network.rssiValue { continue }
      unique[key] = network
    }
    guard !unique.isEmpty || found.isEmpty else {
      throw SettingsFailure(message: "macOS withheld network names. Allow Location access for Crawler, then scan again.")
    }
    scanned.removeAll()
    networks = unique.values.compactMap { network in
      guard let name = network.ssid else { return nil }
      let id = UUID()
      scanned[id] = network
      return SettingsNetwork(id: id, name: name, signal: network.rssiValue, security: security(network))
    }.sorted { $0.signal == $1.signal ? $0.name.localizedStandardCompare($1.name) == .orderedAscending : $0.signal > $1.signal }
  }
  private func security(_ network: CWNetwork) -> SettingsNetwork.Security {
    if network.supportsSecurity(.none) { return .open }
    if network.supportsSecurity(.wpaEnterprise) || network.supportsSecurity(.wpa2Enterprise) || network.supportsSecurity(.wpa3Enterprise) { return .enterprise }
    return .personal
  }
  func results() -> [SettingsNetwork] { networks }
  func connect(_ id: UUID, username: String?, password: String?) throws {
    guard let network = scanned[id], let wifi = CWWiFiClient.shared().interface() else {
      throw SettingsFailure(message: "This scan is no longer available. Scan again.")
    }
    if security(network) == .enterprise {
      try wifi.associate(toEnterpriseNetwork: network, identity: nil, username: username, password: password)
    } else { try wifi.associate(to: network, password: password) }
    // SSID may be privacy-redacted; a successful association is the framework acknowledgement.
    if let actual = wifi.ssidData(), let expected = network.ssidData, actual != expected {
      throw SettingsFailure(message: "macOS is connected to a different network. Scan and try again.")
    }
  }
  func disconnect() throws {
    guard let wifi = CWWiFiClient.shared().interface() else { throw SettingsFailure(message: "Wi-Fi is unavailable.") }
    wifi.disassociate()
  }
  func connectBluetooth(address: String, connected: Bool) throws {
    guard let device = IOBluetoothDevice(addressString: address), device.isPaired() else {
      throw SettingsFailure(message: "The paired device is no longer available.")
    }
    if device.isConnected() == connected { return }
    let result = connected ? device.openConnection() : device.closeConnection()
    guard result == kIOReturnSuccess else {
      throw SettingsFailure(message: "Bluetooth could not \(connected ? "connect" : "disconnect") the device (\(result)).")
    }
    guard device.isConnected() == connected else {
      throw SettingsFailure(message: "The device has not reached the requested connection state. Refresh and try again.")
    }
  }
}
